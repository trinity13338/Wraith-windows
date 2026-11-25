//! Settings and servers, encrypted for the current Windows user (DPAPI) and written atomically.

use crate::json::{Get, Obj};
use crate::model::{AppState, Profile, Subscription};
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};

const ENTROPY: &[u8] = b"Wraith Windows profile v1";
static STATE: OnceLock<Mutex<AppState>> = OnceLock::new();
static REVISION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn folder() -> PathBuf {
    if let Some(dir) = std::env::var_os("WRAITH_HOME").map(PathBuf::from).filter(|p| p.is_absolute()) { return dir; }
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
    // 0.12: sing-box is gone. Servers it ran are converted for Xray where possible, otherwise removed.
    if state.profiles.iter().any(|p| p.engine == "singbox") {
        let mut kept = vec![];
        for p in std::mem::take(&mut state.profiles) {
            if p.engine != "singbox" { kept.push(p); continue; }
            match crate::importer::parse(&p.source).ok().and_then(|r| r.profiles.into_iter().next()) {
                Some(n) => kept.push(crate::model::Profile { id: p.id, group: p.group, favorite: p.favorite, ..n }),
                None => crate::log::add(crate::trf!("«{}» удалён: Xray его не поддерживает.", "«{}» removed: Xray does not support it.", p.name)),
            }
        }
        state.profiles = kept;
        if state.selected().is_none() { state.selected = state.profiles.first().map(|p| p.id.clone()).unwrap_or_default(); }
    }
    state.settings.remove("core");
    // 0.13.1: routing is always by rules (the «everything through VPN / direct» modes are gone).
    state.set("mode", "rules".into());
    let _ = STATE.set(Mutex::new(state));
    save();
}

/// Locked state. Keep the guard short: never hold it across network or process waits.
pub fn get() -> MutexGuard<'static, AppState> { STATE.get().expect("store").lock().unwrap_or_else(|e| e.into_inner()) }
/// Changes on every save, so views can refresh cached data.
pub fn revision() -> u64 { REVISION.load(std::sync::atomic::Ordering::Relaxed) }

/// A setting when the store is loaded (the installer may run before it), else "".
pub fn try_setting(key: &str) -> String { STATE.get().map(|m| m.lock().unwrap_or_else(|e| e.into_inner()).s(key)).unwrap_or_default() }

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
        Err(e) => crate::log::add(crate::trf!("Не удалось сохранить настройки: {e}", "Could not save the settings: {e}")),
    }
}

pub(crate) fn load() -> AppState {
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
    crate::log::add(crate::trf!("Не удалось прочитать настройки: {}", "Could not read the settings: {}", crate::log::safe(why)));
    let _ = std::fs::copy(path, path.with_extension(format!("broken-{}", crate::util::unix_now())));
}

fn from_legacy(raw: &Obj) -> AppState {
    let mut state = AppState::default();
    if let Some(s) = raw.o("Settings") {
        for k in ["proxyPort", "pingUrl", "pingTimeout", "userAgent", "mtu"] { if let Some(v) = s.get(k) { state.settings.insert(k.into(), v.clone()); } }
    }
    state.set("tunnelMode", if raw.s("TunnelMode") == "TUN" { "tun" } else { "proxy" }.into());
    for sub in raw.objects("Subscriptions") {
        state.subscriptions.push(Subscription { id: sub.s_or("Id", &crate::util::random_hex(16)), name: sub.s_or("Name", crate::tr!("Подписка", "Subscription")), url: sub.s("Url"), interval_hours: sub.i("IntervalHours", 24).max(1), ..Default::default() });
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
            Err(e) => crate::log::add(crate::trf!("Сервер «{}» не перенесён: {}", "Server «{}» was not migrated: {}", old.s("Name"), crate::log::safe(e))),
        }
    }
    crate::log::add(crate::trf!("Настройки Wraith 0.10 перенесены: {} серверов, {} подписок.", "Wraith 0.10 settings migrated: {} servers, {} subscriptions.", state.profiles.len(), state.subscriptions.len()));
    state
}

/// Replace one subscription's servers in the provider's order, keeping favorites and the selection by name/host.
pub fn replace_subscription(sub: Subscription, mut fresh: Vec<Profile>) {
    update(|s| {
        let old: Vec<Profile> = s.profiles.iter().filter(|p| p.group == sub.id).cloned().collect();
        let selected_key = s.selected().filter(|p| p.group == sub.id).map(|p| p.key());
        for p in fresh.iter_mut() {
            p.group = sub.id.clone();
            // Old latencies are dropped: a refreshed subscription shows the provider's order until it is pinged again.
            if let Some(prev) = old.iter().find(|o| o.key() == p.key()) { p.id = prev.id.clone(); p.favorite = prev.favorite; }
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
    if ok == 0 { return Err(crate::tr!("DPAPI отказал (данные другого пользователя Windows?)", "DPAPI refused (data of another Windows user?)").into()); }
    let result = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec() };
    unsafe { LocalFree(out.pbData as _) };
    Ok(result)
}

/// Servers, subscriptions, rules and settings as plain JSON (contains connection keys).
pub fn backup() -> String {
    let mut v = serde_json::to_value(&*get()).unwrap_or_default();
    if let Some(s) = v.get_mut("settings").and_then(|s| s.as_object_mut()) { s.remove("hwid"); }
    if let Some(o) = v.as_object_mut() { o.insert("wraithBackup".into(), 1.into()); }
    serde_json::to_string_pretty(&v).unwrap_or_default()
}

/// Replaces everything with a backup; the installation id stays this computer's.
pub fn restore(text: &str) -> Result<(), String> {
    let v: serde_json::Value = serde_json::from_str(text.trim()).map_err(|_| crate::tr!("Это не резервная копия Wraith (JSON не читается).", "This is not a Wraith backup (unreadable JSON).").to_string())?;
    if v.get("profiles").and_then(|p| p.as_array()).is_none() || v.get("settings").and_then(|s| s.as_object()).is_none() {
        return Err(crate::tr!("Это не резервная копия Wraith.", "This is not a Wraith backup.").into());
    }
    let mut state: AppState = serde_json::from_value(v).map_err(|e| e.to_string())?;
    let hwid = get().s("hwid");
    state.set("hwid", hwid.into());
    for (k, v) in crate::model::defaults() { state.settings.entry(k).or_insert(v); }
    state.set("mode", "rules".into());
    if state.selected().is_none() { state.selected = state.profiles.first().map(|p| p.id.clone()).unwrap_or_default(); }
    *get() = state;
    save();
    Ok(())
}
