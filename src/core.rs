//! Runs xray.exe or sing-box.exe for the selected server; owns the Windows proxy setting and the traffic counters.

use crate::json::{Get, Obj};
use crate::model::Profile;
use crate::util;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State { Off, Connecting, Connected, Stopping }

#[derive(Clone)]
pub struct Status {
    pub state: State,
    pub error: String,
    pub profile_id: String,
    pub profile_name: String,
    pub engine: &'static str,
    pub tun: bool,
    pub since: Option<Instant>,
    pub rx: i64, pub tx: i64, pub rx_rate: i64, pub tx_rate: i64,
}
static STATUS: Mutex<Status> = Mutex::new(Status { state: State::Off, error: String::new(), profile_id: String::new(), profile_name: String::new(), engine: "Xray", tun: false, since: None, rx: 0, tx: 0, rx_rate: 0, tx_rate: 0 });
pub fn status() -> Status { STATUS.lock().unwrap().clone() }
fn set_state(state: State, error: Option<String>) {
    let mut s = STATUS.lock().unwrap(); s.state = state; if let Some(e) = error { s.error = e; }
}

struct Running { child: Arc<Mutex<Child>>, config: PathBuf, proxy_mode: bool, stats_port: u16, secret: String, tags: Vec<String>, singbox: bool }
static RUNNING: Mutex<Option<Running>> = Mutex::new(None);
static SESSION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static CONNECT: Mutex<()> = Mutex::new(());
/// Last profile started and how many automatic restarts it already had.
static LAST: Mutex<Option<Profile>> = Mutex::new(None);
static RETRIES: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
/// Last meaningful line the core printed (shown when it dies).
static LAST_LINE: Mutex<String> = Mutex::new(String::new());

pub fn xray_exe() -> PathBuf { util::app_dir().join("xray.exe") }
pub fn singbox_exe() -> PathBuf { util::app_dir().join("sing-box.exe") }

/// At startup: a crashed Wraith may have left the Windows proxy pointing at a dead port, or a core holding the TUN.
pub fn recover() {
    proxy::restore_leftover();
    kill_orphans();
}

pub fn connect(profile: Profile) { RETRIES.store(0, std::sync::atomic::Ordering::SeqCst); start_connect(profile); }
fn start_connect(profile: Profile) {
    *LAST.lock().unwrap() = Some(profile.clone());
    LAST_LINE.lock().unwrap().clear();
    let preferred = crate::store::get().settings.s_or("core", "xray");
    let profile = crate::importer::for_engine(&profile, &preferred);
    if (preferred == "singbox") != profile.is_singbox() { crate::log::add(format!("«{}» запускается через {}: другое ядро не поддерживает эту конфигурацию.", profile.name, if profile.is_singbox() { "sing-box" } else { "Xray" })); }
    std::thread::spawn(move || {
        let _serial = CONNECT.lock().unwrap();
        stop_running(false);
        let session = SESSION.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        {
            let mut s = STATUS.lock().unwrap();
            *s = Status { state: State::Connecting, error: String::new(), profile_id: profile.id.clone(), profile_name: profile.name.clone(), engine: if profile.is_singbox() { "sing-box" } else { "Xray" }, tun: crate::model::uses_tun(&crate::store::get().settings), since: None, rx: 0, tx: 0, rx_rate: 0, tx_rate: 0 };
        }
        match start(&profile, session) {
            Ok(()) => { let mut s = STATUS.lock().unwrap(); s.state = State::Connected; s.since = Some(Instant::now()); }
            Err(e) => { let e = crate::log::safe(e); crate::log::add(format!("Подключение не удалось: {e}")); stop_running(true); set_state(State::Off, Some(e)); }
        }
    });
}
pub fn disconnect() {
    std::thread::spawn(|| {
        let _serial = CONNECT.lock().unwrap();
        if RUNNING.lock().unwrap().is_none() { set_state(State::Off, Some(String::new())); return; }
        set_state(State::Stopping, Some(String::new()));
        stop_running(false);
        set_state(State::Off, None);
    });
}
/// Synchronous stop for application exit: the Windows proxy must never stay pointed at a dead port.
pub fn shutdown() { stop_running(true); }

fn start(profile: &Profile, session: u64) -> Result<(), String> {
    let state = crate::store::get().clone();
    let settings = &state.settings;
    let tun = crate::model::uses_tun(settings); let system_proxy = crate::model::uses_proxy(settings);
    if tun && !util::is_admin() { return Err("Для режима TUN нужны права администратора.".into()); }
    let singbox = profile.is_singbox();
    // The system's resolvers are read before the tunnel exists.
    let system_dns = crate::routing::system_resolvers();
    let stats_port = util::free_port(); let secret = util::random_hex(16);
    let (config, tags): (Obj, Vec<String>) = if singbox { (crate::singbox::build(&state, profile, &system_dns, stats_port, &secret)?, vec![]) }
        else { let b = crate::xray::build(&state, profile, &system_dns, stats_port)?; (b.config, b.proxy_tags) };
    let exe = if singbox { singbox_exe() } else { xray_exe() };
    if !exe.exists() { return Err(format!("Не найден {} рядом с Wraith.exe. Распакуйте архив целиком.", exe.file_name().unwrap().to_string_lossy())); }
    if tun && !util::app_dir().join("wintun.dll").exists() { return Err("Для TUN нужен wintun.dll рядом с Wraith.exe.".into()); }
    // Xray reads its config from stdin: no file another program could swap while the core starts
    // (Wraith may run as administrator). sing-box has no stdin option; its file lives in a folder
    // only administrators can write when Wraith is installed.
    let bytes = serde_json::to_vec(&config).unwrap();
    let path = util::runtime_dir().join("active-config.json");
    let p = path.to_string_lossy().to_string();
    let check = if singbox {
        std::fs::write(&path, &bytes).map_err(|e| e.to_string())?;
        util::run_hidden(&exe, &["check", "-c", &p], 20)
    } else { util::run_hidden_stdin(&exe, &["run", "-test", "-c", "stdin:"], &bytes, 20) };
    if let Err(out) = check { return Err(format!("{} отклонил конфигурацию: {}", if singbox { "sing-box" } else { "Xray" }, util::tail(&out, 600))); }
    crate::log::add(format!("Wraith: запуск {} · {} · «{}»", if singbox { "sing-box" } else { "Xray" }, if tun && system_proxy { "TUN + прокси" } else if tun { "TUN" } else { "системный прокси" }, profile.name));
    let mut cmd = util::hidden(&exe);
    if singbox { cmd.args(["run", "-c", &p, "-D", &util::runtime_dir().to_string_lossy()]); } else { cmd.args(["run", "-c", "stdin:"]); }
    let mut child = cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).stdin(if singbox { Stdio::null() } else { Stdio::piped() }).spawn().map_err(|e| format!("Не удалось запустить ядро: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() { use std::io::Write; stdin.write_all(&bytes).map_err(|e| e.to_string())?; }
    for stream in [child.stdout.take().map(|s| Box::new(s) as Box<dyn std::io::Read + Send>), child.stderr.take().map(|s| Box::new(s) as Box<dyn std::io::Read + Send>)].into_iter().flatten() {
        std::thread::spawn(move || { for line in BufReader::new(stream).lines().map_while(Result::ok) { if !line.contains(" accepted ") && !line.trim().is_empty() { *LAST_LINE.lock().unwrap() = line.clone(); } crate::log::add(line); } });
    }
    let child = Arc::new(Mutex::new(child));
    *RUNNING.lock().unwrap() = Some(Running { child: child.clone(), config: path, proxy_mode: system_proxy, stats_port, secret, tags, singbox });
    // Wait for the local inlet: it proves the core finished starting (TUN comes up first).
    let port = settings.i("httpProxyPort", 2082) as u16;
    let deadline = Instant::now() + Duration::from_secs(if tun { 12 } else { 6 });
    loop {
        if let Ok(Some(_)) = child.lock().unwrap().try_wait() { return Err("Ядро остановилось при запуске. Подробности — в журнале.".into()); }
        if util::port_open(port) { break; }
        if Instant::now() > deadline { return Err(format!("Ядро не открыло локальный порт {port}. Возможно, он занят другой программой.")); }
        std::thread::sleep(Duration::from_millis(80));
    }
    if system_proxy { proxy::enable(&format!("127.0.0.1:{port}"))?; }
    // Watchdog and traffic counters for this session.
    std::thread::spawn(move || monitor(session));
    crate::log::add("Wraith: подключено");
    Ok(())
}

fn monitor(session: u64) {
    loop {
        std::thread::sleep(Duration::from_secs(1));
        if SESSION.load(std::sync::atomic::Ordering::SeqCst) != session { return; }
        let (child, port, secret, tags, singbox) = {
            let guard = RUNNING.lock().unwrap();
            let Some(r) = guard.as_ref() else { return };
            (r.child.clone(), r.stats_port, r.secret.clone(), r.tags.clone(), r.singbox)
        };
        let exited = child.lock().unwrap().try_wait().ok().flatten();
        if let Some(code) = exited {
            let _serial = CONNECT.lock().unwrap();
            if SESSION.load(std::sync::atomic::Ordering::SeqCst) != session { return; }
            crate::log::add(format!("Ядро неожиданно остановилось ({code})."));
            stop_running(true);
            // A crash of the core (driver hiccup, network change) gets two automatic restarts.
            let retries = RETRIES.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let last = LAST.lock().unwrap().clone();
            if retries < 2 { if let Some(p) = last { crate::log::add("Перезапускаем ядро…"); drop(_serial); start_connect(p); return; } }
            let line = crate::util::tail(&LAST_LINE.lock().unwrap(), 200);
            set_state(State::Off, Some(if line.is_empty() { "Ядро остановилось. Подробности — в журнале.".into() } else { format!("Ядро остановилось: {}", crate::log::safe(line)) }));
            return;
        }
        if let Some((down, up)) = read_stats(port, &secret, &tags, singbox) {
            let mut s = STATUS.lock().unwrap();
            s.rx_rate = (down - s.rx).max(0); s.tx_rate = (up - s.tx).max(0); s.rx = down; s.tx = up;
        }
    }
}

fn read_stats(port: u16, secret: &str, tags: &[String], singbox: bool) -> Option<(i64, i64)> {
    let agent = ureq::AgentBuilder::new().timeout(Duration::from_millis(800)).build();
    if singbox {
        let o: Obj = agent.get(&format!("http://127.0.0.1:{port}/connections")).set("Authorization", &format!("Bearer {secret}")).call().ok()?.into_json().ok()?;
        Some((o.i("downloadTotal", 0), o.i("uploadTotal", 0)))
    } else {
        let o: Obj = agent.get(&format!("http://127.0.0.1:{port}/debug/vars")).call().ok()?.into_json().ok()?;
        let outbound = o.o("stats").and_then(|s| s.o("outbound"))?;
        let (mut d, mut u) = (0, 0);
        for t in tags { if let Some(x) = outbound.o(t) { d += x.i("downlink", 0); u += x.i("uplink", 0); } }
        Some((d, u))
    }
}

fn stop_running(quiet: bool) {
    SESSION.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let Some(r) = RUNNING.lock().unwrap().take() else { return };
    if r.proxy_mode { if let Err(e) = proxy::restore() { crate::log::add(format!("Не удалось вернуть настройки прокси Windows: {e}")); } }
    { let mut c = r.child.lock().unwrap(); let _ = c.kill(); let _ = c.wait(); }
    let _ = std::fs::remove_file(&r.config);
    if !quiet { crate::log::add("Wraith: отключено"); }
    let mut s = STATUS.lock().unwrap(); s.rx_rate = 0; s.tx_rate = 0;
}

/// A core left running by a crashed Wraith keeps the TUN and the ports busy.
fn kill_orphans() {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};
    use windows_sys::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, TerminateProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE};
    let ours: Vec<String> = [xray_exe(), singbox_exe()].iter().map(|p| p.to_string_lossy().to_ascii_lowercase()).collect();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap.is_null() || snap as isize == -1 { return; }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed(); entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut ok = Process32FirstW(snap, &mut entry) != 0;
        while ok {
            let len = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(0);
            let name = String::from_utf16_lossy(&entry.szExeFile[..len]).to_ascii_lowercase();
            if name == "xray.exe" || name == "sing-box.exe" {
                let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE, 0, entry.th32ProcessID);
                if !h.is_null() {
                    let mut buf = [0u16; 1024]; let mut size = buf.len() as u32;
                    if QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut size) != 0 {
                        let path = String::from_utf16_lossy(&buf[..size as usize]).to_ascii_lowercase();
                        if ours.contains(&path) { TerminateProcess(h, 1); crate::log::add("Остановлено ядро, оставшееся от прошлого запуска."); }
                    }
                    CloseHandle(h);
                }
            }
            ok = Process32NextW(snap, &mut entry) != 0;
        }
        CloseHandle(snap);
    }
}

/// Windows (WinINet) proxy setting with a persisted snapshot, restored even after a crash.
pub mod proxy {
    use crate::json::Get;
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE};
    use winreg::RegKey;
    const KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Internet Settings";
    fn backup() -> std::path::PathBuf { crate::store::folder().join("proxy-backup.json") }

    pub fn enable(server: &str) -> Result<(), String> {
        let key = RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(KEY, KEY_READ | KEY_WRITE).map_err(|e| format!("Не удалось открыть настройки прокси Windows: {e}"))?;
        if !backup().exists() {
            let enabled: u32 = key.get_value("ProxyEnable").unwrap_or(0);
            let old_server: Option<String> = key.get_value("ProxyServer").ok();
            let old_override: Option<String> = key.get_value("ProxyOverride").ok();
            let snapshot = serde_json::json!({"enabled": enabled != 0, "server": old_server, "override": old_override, "ours": server});
            std::fs::write(backup(), snapshot.to_string()).map_err(|e| e.to_string())?;
        }
        key.set_value("ProxyEnable", &1u32).map_err(|e| e.to_string())?;
        key.set_value("ProxyServer", &server.to_string()).map_err(|e| e.to_string())?;
        key.set_value("ProxyOverride", &"localhost;127.*;10.*;172.16.*;172.17.*;172.18.*;172.19.*;172.2*;172.30.*;172.31.*;192.168.*;<local>".to_string()).map_err(|e| e.to_string())?;
        refresh(); Ok(())
    }
    pub fn restore() -> Result<(), String> {
        let path = backup(); if !path.exists() { return Ok(()); }
        let snapshot: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        let key = RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(KEY, KEY_READ | KEY_WRITE).map_err(|e| e.to_string())?;
        // Only undo our own change: if the user switched to another proxy meanwhile, leave it.
        let current: Option<String> = key.get_value("ProxyServer").ok();
        if current.is_none() || current.as_deref() == Some(snapshot.s("ours").as_str()) {
            let _ = key.set_value("ProxyEnable", &(if snapshot.b("enabled", false) { 1u32 } else { 0u32 }));
            match snapshot.get("server").and_then(|v| v.as_str()) { Some(s) => { let _ = key.set_value("ProxyServer", &s.to_string()); } None => { let _ = key.delete_value("ProxyServer"); } }
            match snapshot.get("override").and_then(|v| v.as_str()) { Some(s) => { let _ = key.set_value("ProxyOverride", &s.to_string()); } None => { let _ = key.delete_value("ProxyOverride"); } }
        }
        let _ = std::fs::remove_file(path); refresh(); Ok(())
    }
    pub fn restore_leftover() {
        if backup().exists() { match restore() { Ok(()) => crate::log::add("Настройки прокси Windows восстановлены после прошлого запуска."), Err(e) => crate::log::add(format!("Не удалось восстановить прокси Windows: {e}")) } }
    }
    fn refresh() {
        use windows_sys::Win32::Networking::WinInet::{InternetSetOptionW, INTERNET_OPTION_REFRESH, INTERNET_OPTION_SETTINGS_CHANGED};
        unsafe { InternetSetOptionW(std::ptr::null(), INTERNET_OPTION_SETTINGS_CHANGED, std::ptr::null(), 0); InternetSetOptionW(std::ptr::null(), INTERNET_OPTION_REFRESH, std::ptr::null(), 0); }
    }
}
