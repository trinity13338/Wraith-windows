//! Statistics that never leave the computer (port of the Android TrafficHistory / ExitInfo):
//! the live speed of the last minute, daily totals in traffic.json, and the exit IP with the
//! delay through the tunnel.

use crate::json::{Get, Obj};
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Samples of (download, upload) bytes per second, one per second, the last 60.
static SPEED: Mutex<VecDeque<(i64, i64)>> = Mutex::new(VecDeque::new());
/// Bytes counted since the last write, per day ("yyyy-mm-dd" → [rx, tx]).
static PENDING: Mutex<Option<HashMap<String, [i64; 2]>>> = Mutex::new(None);
static LAST_SAVE: Mutex<Option<Instant>> = Mutex::new(None);

fn file() -> std::path::PathBuf { crate::store::folder().join("traffic.json") }

/// Called by the core monitor once a second while connected.
pub fn sample(rx_rate: i64, tx_rate: i64) {
    {
        let mut s = SPEED.lock().unwrap();
        s.push_back((rx_rate.max(0), tx_rate.max(0)));
        while s.len() > 60 { s.pop_front(); }
    }
    if rx_rate > 0 || tx_rate > 0 {
        let mut p = PENDING.lock().unwrap();
        let d = p.get_or_insert_with(HashMap::new).entry(crate::util::local_time().date).or_insert([0, 0]);
        d[0] += rx_rate.max(0); d[1] += tx_rate.max(0);
    }
    let due = LAST_SAVE.lock().unwrap().map(|t| t.elapsed() > Duration::from_secs(30)).unwrap_or(true);
    if due { save(); }
}

pub fn speed() -> Vec<(i64, i64)> { SPEED.lock().unwrap().iter().copied().collect() }
/// The session ended: the chart starts empty next time and the counters are written.
pub fn session_ended() { SPEED.lock().unwrap().clear(); save(); }

pub fn save() {
    *LAST_SAVE.lock().unwrap() = Some(Instant::now());
    let pending = PENDING.lock().unwrap().take();
    let Some(pending) = pending.filter(|p| !p.is_empty()) else { return };
    let mut all = read();
    for (day, v) in pending {
        let old = all.a(&day).cloned().unwrap_or_default();
        let get = |i: usize| old.get(i).and_then(|x| x.as_i64()).unwrap_or(0);
        all.insert(day, json!([get(0) + v[0], get(1) + v[1]]));
    }
    // Three months are kept.
    let mut keys: Vec<String> = all.keys().cloned().collect(); keys.sort();
    if keys.len() > 90 { for k in &keys[..keys.len() - 90] { all.remove(k); } }
    let tmp = file().with_extension("json.new");
    if std::fs::write(&tmp, Value::Object(all).to_string()).is_ok() { let _ = std::fs::rename(&tmp, file()); }
}

pub fn read() -> Obj {
    std::fs::read_to_string(file()).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).and_then(|v| v.as_object().cloned()).unwrap_or_default()
}
pub fn clear() { PENDING.lock().unwrap().take(); let _ = std::fs::remove_file(file()); }

/// (day of month label, total bytes) for the last `n` days, oldest first (local calendar days).
pub fn last_days(history: &Obj, n: i64) -> Vec<(String, i64)> {
    let today = crate::util::local_time().date;
    let p = |r: std::ops::Range<usize>| today.get(r).and_then(|v| v.parse::<i64>().ok()).unwrap_or(1);
    let base = days_from_civil(p(0..4), p(5..7), p(8..10));
    (0..n).rev().map(|back| {
        let (y, m, d) = civil_from_days(base - back);
        let key = format!("{y:04}-{m:02}-{d:02}");
        let a = history.a(&key).cloned().unwrap_or_default();
        let total = a.first().and_then(|x| x.as_i64()).unwrap_or(0) + a.get(1).and_then(|x| x.as_i64()).unwrap_or(0);
        (format!("{d:02}"), total)
    }).collect()
}
/// Days since 1970-01-01 for a calendar date, and back (H. Hinnant's algorithms).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400); let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468; let era = z.div_euclid(146_097); let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1; let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { yoe + era * 400 + 1 } else { yoe + era * 400 }, m, d)
}

// ------------------------------------------------------------------ exit IP

#[derive(Clone, Default)]
pub struct Exit { pub ip: String, pub country: String, pub delay: i64 }

static EXIT: Mutex<Option<Exit>> = Mutex::new(None);
static EXIT_DUE: Mutex<Option<Instant>> = Mutex::new(None);
static EXIT_RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn exit() -> Exit { EXIT.lock().unwrap().clone().unwrap_or_default() }
pub fn reset_exit() { *EXIT.lock().unwrap() = None; *EXIT_DUE.lock().unwrap() = Some(Instant::now() + Duration::from_millis(1500)); }

/// Once a minute while connected (if «Внешний IP и задержка» is on): who the sites see, and how long a request takes.
pub fn tick_exit(settings: &Obj) {
    if !settings.b("showExitIp", true) { return; }
    let due = EXIT_DUE.lock().unwrap().map(|t| Instant::now() >= t).unwrap_or(true);
    if !due || EXIT_RUNNING.swap(true, std::sync::atomic::Ordering::SeqCst) { return; }
    *EXIT_DUE.lock().unwrap() = Some(Instant::now() + Duration::from_secs(60));
    let settings = settings.clone();
    std::thread::spawn(move || {
        let info = fetch(&settings);
        if crate::core::status().state == crate::core::State::Connected { *EXIT.lock().unwrap() = Some(info); }
        EXIT_RUNNING.store(false, std::sync::atomic::Ordering::SeqCst);
        crate::ui::repaint();
    });
}

/// Wraith's own requests go through its local HTTP inlet, i.e. through the tunnel, whatever the mode.
fn agent(settings: &Obj, timeout: u64) -> Option<ureq::Agent> {
    let port = settings.i("httpProxyPort", 2082);
    let proxy = if settings.b("allowLan", false) {
        ureq::Proxy::new(format!("http://wraith:{}@127.0.0.1:{port}", settings.s("proxyPassword"))).ok()?
    } else { ureq::Proxy::new(format!("http://127.0.0.1:{port}")).ok()? };
    Some(ureq::AgentBuilder::new().proxy(proxy).timeout(Duration::from_millis(timeout)).redirects(0)
        .user_agent(&format!("Wraith/{}/Linux", env!("CARGO_PKG_VERSION"))).build())
}

pub fn fetch(settings: &Obj) -> Exit {
    let mut out = Exit { delay: -1, ..Default::default() };
    let Some(agent) = agent(settings, 6000) else { return out };
    // The IPv4 address first (1.1.1.1 is only reachable over IPv4): a server with IPv6 would otherwise show
    // its IPv6 exit. The name-based endpoint is the fallback for servers without IPv4.
    for url in ["https://1.1.1.1/cdn-cgi/trace", "https://www.cloudflare.com/cdn-cgi/trace"] {
        let Ok(r) = agent.get(url).call() else { continue };
        let Ok(text) = r.into_string() else { continue };
        for line in text.lines().take(64) {
            if let Some(v) = line.strip_prefix("ip=") { out.ip = v.trim().to_string(); }
            if let Some(v) = line.strip_prefix("loc=") { out.country = v.trim().to_ascii_lowercase(); }
        }
        if !out.ip.is_empty() { break; }
    }
    // One HTTPS request through the tunnel, timed: what a browser would feel. The better of two.
    let url = settings.s_or("pingUrl", "https://www.gstatic.com/generate_204");
    let mut best = i64::MAX;
    if let Some(agent) = self::agent(settings, 5000) {
        for _ in 0..2 {
            let start = Instant::now();
            match agent.head(&url).call() { Ok(_) | Err(ureq::Error::Status(..)) => best = best.min(start.elapsed().as_millis() as i64), Err(_) => {} }
        }
    }
    if best != i64::MAX { out.delay = best; }
    out
}

#[cfg(test)]
pub fn set_test_exit(ip: &str, country: &str, delay: i64) { *EXIT.lock().unwrap() = Some(Exit { ip: ip.into(), country: country.into(), delay }); }

#[cfg(test)]
mod calendar {
    #[test]
    fn days_round_trip_across_months_and_leap_years() {
        assert_eq!(super::days_from_civil(1970, 1, 1), 0);
        assert_eq!(super::civil_from_days(super::days_from_civil(2024, 3, 1) - 1), (2024, 2, 29));
        assert_eq!(super::civil_from_days(super::days_from_civil(2026, 1, 1) - 1), (2025, 12, 31));
        for z in -800..20_000 { let (y, m, d) = super::civil_from_days(z); assert_eq!(super::days_from_civil(y, m, d), z); }
    }
}
