//! Latency checks. "Через прокси" runs one short-lived Xray with a loopback HTTP inlet per server
//! (every server of an auto config is measured, the best one counts), so no VPN is needed.

use crate::importer::{endpoints, Endpoint, LOCAL_PROTOCOLS};
use crate::json::{Get, Obj};
use crate::model::{AppState, Profile};
use crate::util;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub type Report = Arc<dyn Fn(&str, i64, String) + Send + Sync>;

pub fn run(state: AppState, profiles: Vec<Profile>, report: Report, cancel: Arc<AtomicBool>) {
    let s = &state.settings;
    let kind = s.s_or("pingType", "proxyGet"); let timeout = s.i("pingTimeout", 5000).max(1000) as u64;
    if kind == "tcp" || kind == "icmp" {
        parallel(profiles, 16, |p| { if cancel.load(Ordering::Relaxed) { return; } match if kind == "icmp" { icmp(&p, timeout) } else { tcp(&p, timeout) } { Ok(ms) => report(&p.id, ms, String::new()), Err(e) => report(&p.id, -1, e) } });
        return;
    }
    let url = s.s_or("pingUrl", "https://www.gstatic.com/generate_204"); let head = kind == "proxyHead"; let keep_alive = s.s_or("pingMode", "keepalive") != "double";
    xray_batch(&state, profiles, &url, head, keep_alive, timeout, &report, &cancel);
}

fn parallel<T: Send + 'static>(items: Vec<T>, threads: usize, f: impl Fn(T) + Send + Sync) {
    let queue = Mutex::new(items.into_iter()); let f = &f; let queue = &queue;
    std::thread::scope(|scope| { for _ in 0..threads { scope.spawn(move || loop { let next = queue.lock().unwrap().next(); match next { Some(item) => f(item), None => break } }); } });
}

fn tcp(p: &Profile, timeout: u64) -> Result<i64, String> {
    if p.host.is_empty() || p.port < 1 { return Err(crate::tr!("Нет адреса сервера.", "No server address.").into()); }
    if ["hysteria2", "tuic", "wireguard"].contains(&p.protocol.as_str()) { return Err(crate::tr!("UDP-протокол: используйте проверку через прокси.", "UDP protocol: use the check through the proxy.").into()); }
    use std::net::ToSocketAddrs;
    let addr = (p.host.as_str(), p.port as u16).to_socket_addrs().map_err(|e| format!("DNS: {e}"))?.next().ok_or(crate::tr!("Адрес не найден", "Address not found"))?;
    let start = Instant::now();
    std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(timeout)).map_err(|_| crate::tr!("Нет ответа", "No answer").to_string())?;
    Ok(start.elapsed().as_millis().max(1) as i64)
}
fn icmp(p: &Profile, timeout: u64) -> Result<i64, String> {
    if p.host.is_empty() { return Err(crate::tr!("Нет адреса сервера.", "No server address.").into()); }
    let out = util::hidden(std::path::Path::new("ping")).args(["-n", "1", "-w", &timeout.to_string(), &p.host]).output().map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&out.stdout);
    let re = regex::Regex::new(r"[=<](\d+)\s?(ms|мс)").unwrap();
    re.captures(&text).and_then(|c| c[1].parse::<i64>().ok()).map(|v| v.max(1)).ok_or_else(|| crate::tr!("ICMP недоступен или заблокирован", "ICMP is unavailable or blocked").into())
}

struct Job { profile: Profile, endpoint: Endpoint, port: u16 }

fn xray_batch(state: &AppState, profiles: Vec<Profile>, url: &str, head: bool, keep_alive: bool, timeout: u64, report: &Report, cancel: &Arc<AtomicBool>) {
    let mut jobs = vec![];
    for p in &profiles {
        let result = (|| -> Result<Vec<Endpoint>, String> {
            let source = crate::xray::source(p)?; let tags = crate::xray::proxy_tags(&source); let mut seen = HashSet::new();
            let list: Vec<Endpoint> = source.objects("outbounds").into_iter()
                .filter(|o| tags.contains(&o.s("tag")) || (o.s("tag").is_empty() && !LOCAL_PROTOCOLS.contains(&o.s("protocol").as_str())))
                .flat_map(|o| endpoints(o)).filter(|e| seen.insert(format!("{}{}:{}", e.protocol, e.host, e.port))).take(24).collect();
            if list.is_empty() { Err(crate::tr!("Нет сервера для проверки.", "No server to check.").into()) } else { Ok(list) }
        })();
        match result { Ok(list) => for e in list { jobs.push(Job { profile: p.clone(), endpoint: e, port: 0 }); }, Err(e) => report(&p.id, -1, e) }
    }
    // Each server of an auto config is a job; a row reports once all of its jobs finished, with the best result.
    let remaining: Mutex<HashMap<String, (usize, Vec<i64>, String)>> = Mutex::new(HashMap::new());
    for j in &jobs { remaining.lock().unwrap().entry(j.profile.id.clone()).or_insert((0, vec![], String::new())).0 += 1; }
    let done = |id: &str, ms: i64, err: String| {
        let mut map = remaining.lock().unwrap();
        let Some(entry) = map.get_mut(id) else { return };
        if ms > 0 { entry.1.push(ms); } else if entry.2.is_empty() { entry.2 = err; }
        entry.0 -= 1;
        if entry.0 == 0 {
            match entry.1.iter().min() { Some(best) => report(id, *best, String::new()), None => report(id, -1, if entry.2.is_empty() { crate::tr!("Нет ответа", "No answer").into() } else { entry.2.clone() }) }
        }
    };
    let mut iter = jobs.into_iter().peekable();
    while iter.peek().is_some() {
        if cancel.load(Ordering::Relaxed) { return; }
        let chunk: Vec<Job> = iter.by_ref().take(48).collect();
        run_xray(state, chunk, url, head, keep_alive, timeout, cancel, &done);
    }
}

fn run_xray(state: &AppState, chunk: Vec<Job>, url: &str, head: bool, keep_alive: bool, timeout: u64, cancel: &Arc<AtomicBool>, done: &(dyn Fn(&str, i64, String) + Sync)) {
    let (mut inbounds, mut outbounds, mut rules) = (vec![], vec![], vec![]); let mut dns: Option<Obj> = None; let mut valid = vec![];
    for (i, mut job) in chunk.into_iter().enumerate() {
        job.port = util::free_port();
        let r = crate::xray::source(&job.profile).and_then(|src| crate::xray::append_probe(&job.profile, &state.settings, &src, &job.endpoint, i, job.port, &mut outbounds, &mut inbounds, &mut rules));
        match r { Ok(d) => { if dns.is_none() { dns = d; } valid.push(job); } Err(e) => done(&job.profile.id, -1, crate::log::safe(e)) }
    }
    if valid.is_empty() { return; }
    let result = (|| -> Result<(), String> {
        let exe = crate::core::xray_exe(); if !exe.exists() { return Err(crate::tr!("Не найден xray.exe рядом с Wraith.exe.", "xray.exe not found next to Wraith.exe.").into()); }
        // Config through stdin: nothing on disk to swap while Xray starts.
        let bytes = serde_json::to_vec(&crate::xray::assemble_probe(inbounds, outbounds, rules, dns)).unwrap();
        let mut child = util::hidden(&exe).args(["run", "-c", "stdin:"]).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::piped()).spawn().map_err(|e| e.to_string())?;
        if let Some(mut stdin) = child.stdin.take() { use std::io::Write; stdin.write_all(&bytes).map_err(|e| e.to_string())?; }
        let guard = KillOnDrop(&mut child);
        let deadline = Instant::now() + Duration::from_secs(6);
        for job in &valid {
            while !util::port_open(job.port) {
                if let Ok(Some(_)) = guard.0.try_wait() {
                    let mut err = String::new(); if let Some(mut e) = guard.0.stderr.take() { use std::io::Read; let _ = e.read_to_string(&mut err); }
                    return Err(crate::trf!("Xray не запустил проверку: {}", "Xray did not start the check: {}", util::tail(&err, 300)));
                }
                if Instant::now() > deadline || cancel.load(Ordering::Relaxed) { return Err(crate::tr!("Xray не открыл локальный вход для проверки.", "Xray did not open the local inlet for the check.").into()); }
                std::thread::sleep(Duration::from_millis(30));
            }
        }
        let jobs: Vec<(String, u16)> = valid.iter().map(|j| (j.profile.id.clone(), j.port)).collect();
        parallel(jobs, 16, |(id, port)| {
            if cancel.load(Ordering::Relaxed) { done(&id, -1, crate::tr!("Проверка отменена", "Check cancelled").into()); return; }
            match measure(url, head, keep_alive, timeout, port) { Ok(ms) => done(&id, ms, String::new()), Err(e) => done(&id, -1, e) }
        });
        drop(guard); Ok(())
    })();
    if let Err(e) = result { for job in &valid { done(&job.profile.id, -1, e.clone()); } }
}
struct KillOnDrop<'a>(&'a mut std::process::Child);
impl Drop for KillOnDrop<'_> { fn drop(&mut self) { let _ = self.0.kill(); let _ = self.0.wait(); } }


/// Two requests through the local inlet; the best one counts (keep-alive reuses the connection).
pub fn measure(url: &str, head: bool, keep_alive: bool, timeout: u64, port: u16) -> Result<i64, String> {
    if keep_alive { return keepalive::measure(url, head, timeout, port); }
    let proxy = ureq::Proxy::new(format!("http://127.0.0.1:{port}")).map_err(|e| e.to_string())?;
    let agent = ureq::AgentBuilder::new().proxy(proxy).timeout(Duration::from_millis(timeout)).redirects(0).user_agent("Mozilla/5.0").max_idle_connections(if keep_alive { 4 } else { 0 }).build();
    let mut best = i64::MAX; let mut last = String::new();
    for _ in 0..2 {
        let start = Instant::now();
        let response = if head { agent.head(url).call() } else { agent.get(url).call() };
        match response {
            Ok(r) | Err(ureq::Error::Status(_, r)) => {
                if !head { let mut body = r.into_reader(); let mut buf = [0u8; 8192]; let mut total = 0; use std::io::Read; while total < 1 << 20 { match body.read(&mut buf) { Ok(0) | Err(_) => break, Ok(n) => total += n } } }
                best = best.min(start.elapsed().as_millis().max(1) as i64);
            }
            // A server that did not answer once will not answer the second time either: do not wait twice.
            Err(e) => { last = short_error(&e.to_string()); break; }
        }
    }
    if best == i64::MAX { Err(if last.is_empty() { crate::tr!("Нет ответа", "No answer").into() } else { last }) } else { Ok(best) }
}

pub fn short_error(e: &str) -> String {
    let l = e.to_lowercase();
    if l.contains("reality") { crate::tr!("Reality: проверка не пройдена", "Reality: verification failed").into() }
    else if l.contains("timed out") || l.contains("timeout") || l.contains("deadline") || l.contains("did not properly respond") || l.contains("connection attempt failed") || l.contains("10060") { crate::tr!("Нет ответа", "No answer").into() }
    else if l.contains("refused") { crate::tr!("Соединение отклонено", "Connection refused").into() }
    else if l.contains("certificate") || l.contains("x509") { crate::tr!("Ошибка сертификата TLS", "TLS certificate error").into() }
    else if l.contains("connection reset") || l.contains("closed") { crate::tr!("Соединение сброшено", "Connection reset").into() }
    else { crate::log::safe(e) }
}

/// Real keep-alive through the local HTTP inlet: one CONNECT tunnel, one TLS session, two requests on it.
/// The second request measures the server's round trip without dial and handshake (like Happ's keep-alive).
mod keepalive {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpStream;
    use std::sync::{Arc, OnceLock};
    use std::time::{Duration, Instant};

    fn tls_config() -> Arc<rustls::ClientConfig> {
        static CONFIG: OnceLock<Arc<rustls::ClientConfig>> = OnceLock::new();
        CONFIG.get_or_init(|| {
            let mut roots = rustls::RootCertStore::empty();
            roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            let config = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions().expect("tls versions").with_root_certificates(roots).with_no_client_auth();
            Arc::new(config)
        }).clone()
    }

    pub fn measure(url: &str, head: bool, timeout: u64, port: u16) -> Result<i64, String> {
        let u = url::Url::parse(url).map_err(|_| crate::tr!("Некорректный адрес проверки", "Invalid check address").to_string())?;
        let host = u.host_str().ok_or(crate::tr!("Нет хоста в адресе проверки", "No host in the check address"))?.to_string();
        let https = u.scheme() == "https";
        let target_port = u.port_or_known_default().unwrap_or(if https { 443 } else { 80 });
        let path = match u.query() { Some(q) => format!("{}?{q}", u.path()), None => u.path().to_string() };
        let deadline = Duration::from_millis(timeout);
        let mut tcp = TcpStream::connect_timeout(&std::net::SocketAddr::from(([127, 0, 0, 1], port)), deadline).map_err(|e| crate::pinger::short_error(&e.to_string()))?;
        tcp.set_read_timeout(Some(deadline)).ok(); tcp.set_write_timeout(Some(deadline)).ok(); tcp.set_nodelay(true).ok();
        let method = if head { "HEAD" } else { "GET" };
        let mut samples = vec![];
        if https {
            // CONNECT through Wraith's inlet: the proxy server is dialled once, here.
            write!(tcp, "CONNECT {host}:{target_port} HTTP/1.1\r\nHost: {host}:{target_port}\r\n\r\n").map_err(|e| e.to_string())?;
            let mut reader = BufReader::new(tcp.try_clone().map_err(|e| e.to_string())?);
            let status = read_head(&mut reader)?;
            if !(200..300).contains(&status.0) { return Err(crate::trf!("Прокси ответил {}", "The proxy answered {}", status.0)); }
            let name = rustls::pki_types::ServerName::try_from(host.clone()).map_err(|_| crate::tr!("Некорректное имя хоста", "Invalid host name").to_string())?;
            let conn = rustls::ClientConnection::new(tls_config(), name).map_err(|e| e.to_string())?;
            let mut tls = rustls::StreamOwned::new(conn, tcp);
            for i in 0..2 {
                let start = Instant::now();
                write!(tls, "{method} {path} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: Mozilla/5.0\r\nConnection: keep-alive\r\n\r\n").map_err(|e| crate::pinger::short_error(&e.to_string()))?;
                tls.flush().map_err(|e| e.to_string())?;
                let mut r = BufReader::new(&mut tls);
                let (_, close) = read_response(&mut r, head)?;
                samples.push(start.elapsed().as_millis().max(1) as i64);
                if close && i == 0 { break; }
            }
        } else {
            for i in 0..2 {
                let start = Instant::now();
                write!(tcp, "{method} {url} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: Mozilla/5.0\r\nProxy-Connection: keep-alive\r\nConnection: keep-alive\r\n\r\n").map_err(|e| e.to_string())?;
                let mut r = BufReader::new(&mut tcp);
                let (_, close) = read_response(&mut r, head)?;
                samples.push(start.elapsed().as_millis().max(1) as i64);
                if close && i == 0 { break; }
            }
        }
        samples.into_iter().min().ok_or_else(|| crate::tr!("Нет ответа", "No answer").to_string())
    }

    /// Status line and headers → (status, content-length, chunked, close)
    fn read_head(r: &mut impl BufRead) -> Result<(u16, Option<usize>, bool, bool), String> {
        let mut line = String::new();
        r.read_line(&mut line).map_err(|e| crate::pinger::short_error(&e.to_string()))?;
        if line.is_empty() { return Err(crate::tr!("Соединение закрыто без ответа", "Connection closed without an answer").into()); }
        let status: u16 = line.split_whitespace().nth(1).and_then(|s| s.parse().ok()).ok_or(crate::tr!("Сервер не вернул HTTP-ответ", "The server did not return an HTTP answer"))?;
        let (mut length, mut chunked, mut close) = (None, false, false);
        loop {
            let mut h = String::new();
            if r.read_line(&mut h).map_err(|e| e.to_string())? == 0 { break; }
            let h = h.trim_end(); if h.is_empty() { break; }
            if let Some((k, v)) = h.split_once(':') {
                let (k, v) = (k.trim().to_ascii_lowercase(), v.trim().to_ascii_lowercase());
                if k == "content-length" { length = v.parse().ok(); }
                if k == "transfer-encoding" && v.contains("chunked") { chunked = true; }
                if k == "connection" && v == "close" { close = true; }
            }
        }
        Ok((status, length, chunked, close))
    }
    fn read_response(r: &mut impl BufRead, head: bool) -> Result<(u16, bool), String> {
        let (status, length, chunked, close) = read_head(r)?;
        if head || status == 204 || status == 304 || (100..200).contains(&status) { return Ok((status, close)); }
        let mut sink = vec![0u8; 8192];
        if chunked {
            loop {
                let mut size = String::new(); r.read_line(&mut size).map_err(|e| e.to_string())?;
                let n = usize::from_str_radix(size.trim().split(';').next().unwrap_or("0"), 16).unwrap_or(0);
                if n == 0 { let mut end = String::new(); let _ = r.read_line(&mut end); break; }
                let mut left = n + 2; while left > 0 { let k = r.read(&mut sink[..left.min(8192)]).map_err(|e| e.to_string())?; if k == 0 { break; } left -= k; }
            }
        } else if let Some(mut left) = length {
            left = left.min(1 << 20);
            while left > 0 { let k = r.read(&mut sink[..left.min(8192)]).map_err(|e| e.to_string())?; if k == 0 { break; } left -= k; }
        } else { return Ok((status, true)); }
        Ok((status, close))
    }
}
