//! Settings and servers, encrypted for the current Windows user (DPAPI) and written atomically.

use crate::json::{Get, Obj};
use crate::model::{AppState, Profile, Subscription};
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};

const ENTROPY: &[u8] = b"Wraith Windows profile v1";
static STATE: OnceLock<Mutex<AppState>> = OnceLock::new();
static REVISION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn folder() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    base.join("Wraith")
}
fn file() -> PathBuf { folder().join("wraith-state.dat") }

pub fn init() {
    let _ = std::fs::create_dir_all(folder());
    let mut state = load();
    if !state.settings.contains_key("ipMode") && state.settings.get("ipv6").and_then(|v| v.as_bool()) == Some(false) { state.set("ipMode", "v4".into()); }
    // 0.11: system proxy and TUN together became the default mode.
    if state.schema < 4 { if state.settings.s("tunnelMode") == "proxy" || state.settings.s("tunnelMode").is_empty() { state.set("tunnelMode", "both".into()); } state.schema = 4; }
    if state.settings.s("hwid").is_empty() { state.set("hwid", crate::util::uuid4().into()); }
    for (k, v) in crate::model::defaults() { state.settings.entry(k).or_insert(v); }
    let _ = STATE.set(Mutex::new(state));
    save();
}

/// Locked state. Keep the guard short: never hold it across network or process waits.
pub fn get() -> MutexGuard<'static, AppState> { STATE.get().expect("store").lock().unwrap_or_else(|e| e.into_inner()) }
/// Changes on every save, so views can refresh cached data.
pub fn revision() -> u64 { REVISION.load(std::sync::atomic::Ordering::Relaxed) }

pub fn update<R>(f: impl FnOnce(&mut AppState) -> R) -> R {
    let r = { let mut s = get(); f(&mut s) };
    save(); r
}

pub fn save() {
    let json = { serde_json::to_vec(&*get()).unwrap_or_default() };
    REVISION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    match protect(&json, true) {
        Ok(data) => {
            let tmp = file().with_extension("dat.new");
            if std::fs::write(&tmp, data).is_ok() { let _ = std::fs::rename(&tmp, file()); }
        }
        Err(e) => crate::log::add(format!("Не удалось сохранить настройки: {e}")),
    }
}

fn load() -> AppState {
    let path = file();
    if path.exists() {
        match std::fs::read(&path).map_err(|e| e.to_string()).and_then(|d| protect(&d, false)) {
            Ok(json) => match serde_json::from_slice::<AppState>(&json) {
                Ok(s) => return s,
                Err(e) => backup_broken(&path, &e.to_string()),
            },
            Err(e) => backup_broken(&path, &e),
        }
        return AppState::default();
    }
    // Wraith for Windows 0.10 (C#) stored sing-box outbounds; re-import every server from its link or JSON.
    let legacy = folder().join("wraith.dat");
    if let Ok(data) = std::fs::read(&legacy) {
        if let Ok(json) = protect(&data, false) {
            if let Ok(serde_json::Value::Object(o)) = serde_json::from_slice::<serde_json::Value>(&json) { return from_legacy(&o); }
        }
    }
    AppState::default()
}

/// An unreadable file must never be replaced by an empty state on the next save. Keep a copy.
fn backup_broken(path: &std::path::Path, why: &str) {
    crate::log::add(format!("Не удалось прочитать настройки: {}", crate::log::safe(why)));
    let _ = std::fs::copy(path, path.with_extension(format!("broken-{}", crate::util::unix_now())));
}

fn from_legacy(raw: &Obj) -> AppState {
    let mut state = AppState::default();
    if let Some(s) = raw.o("Settings") {
        for k in ["proxyPort", "pingUrl", "pingTimeout", "userAgent", "mtu"] { if let Some(v) = s.get(k) { state.settings.insert(k.into(), v.clone()); } }
    }
    state.set("tunnelMode", if raw.s("TunnelMode") == "TUN" { "tun" } else { "proxy" }.into());
    for sub in raw.objects("Subscriptions") {
        state.subscriptions.push(Subscription { id: sub.s_or("Id", &crate::util::random_hex(16)), name: sub.s_or("Name", "Подписка"), url: sub.s("Url"), interval_hours: sub.i("IntervalHours", 24).max(1), ..Default::default() });
    }
    for old in raw.objects("Profiles") {
        let source = old.first(&["JsonSource", "Source"]);
        if source.is_empty() { continue; }
        match crate::importer::parse(&source) {
            Ok(result) => for mut p in result.profiles {
                p.group = old.s("Group"); p.favorite = old.b("Favorite", false);
                if old.s("Id") == raw.s("Selected") { state.selected = p.id.clone(); }
                state.profiles.push(p);
            },
            Err(e) => crate::log::add(format!("Сервер «{}» не перенесён: {}", old.s("Name"), crate::log::safe(e))),
        }
    }
    crate::log::add(format!("Настройки Wraith 0.10 перенесены: {} серверов, {} подписок.", state.profiles.len(), state.subscriptions.len()));
    state
}

/// Replace one subscription's servers, keeping favorites, latencies and the selection by name/host.
pub fn replace_subscription(sub: Subscription, mut fresh: Vec<Profile>) {
    update(|s| {
        let old: Vec<Profile> = s.profiles.iter().filter(|p| p.group == sub.id).cloned().collect();
        let selected_key = s.selected().filter(|p| p.group == sub.id).map(|p| p.key());
        for p in fresh.iter_mut() {
            p.group = sub.id.clone();
            if let Some(prev) = old.iter().find(|o| o.key() == p.key()) { p.id = prev.id.clone(); p.favorite = prev.favorite; p.latency = prev.latency; p.ping_error = prev.ping_error.clone(); }
        }
        let at = s.profiles.iter().position(|p| p.group == sub.id).unwrap_or(s.profiles.len());
        s.profiles.retain(|p| p.group != sub.id);
        let at = at.min(s.profiles.len());
        if let Some(key) = selected_key {
            s.selected = fresh.iter().find(|p| p.key() == key).or(fresh.first()).map(|p| p.id.clone()).unwrap_or_default();
        }
        s.profiles.splice(at..at, fresh);
        match s.subscriptions.iter().position(|x| x.id == sub.id) { Some(i) => s.subscriptions[i] = sub, None => s.subscriptions.push(sub) }
        if s.selected().is_none() { s.selected = s.profiles.first().map(|p| p.id.clone()).unwrap_or_default(); }
    });
}

/// DPAPI CryptProtectData / CryptUnprotectData for the current user.
fn protect(data: &[u8], encrypt: bool) -> Result<Vec<u8>, String> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{CryptProtectData, CryptUnprotectData, CRYPT_INTEGER_BLOB};
    let input = CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 };
    let entropy = CRYPT_INTEGER_BLOB { cbData: ENTROPY.len() as u32, pbData: ENTROPY.as_ptr() as *mut u8 };
    let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
    let ok = unsafe {
        if encrypt { CryptProtectData(&input, std::ptr::null(), &entropy, std::ptr::null(), std::ptr::null(), 0, &mut out) }
        else { CryptUnprotectData(&input, std::ptr::null_mut(), &entropy, std::ptr::null(), std::ptr::null(), 0, &mut out) }
    };
    if ok == 0 { return Err("DPAPI отказал (данные другого пользователя Windows?)".into()); }
    let result = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec() };
    unsafe { LocalFree(out.pbData as _) };
    Ok(result)
}
