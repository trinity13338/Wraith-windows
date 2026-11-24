//! Windows helpers: time, processes without console windows, elevation, ports, formatting.

use std::os::windows::process::CommandExt;
use std::process::Command;

pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub struct Now { pub date: String, pub time: String }
pub fn local_time() -> Now {
    use windows_sys::Win32::System::SystemInformation::GetLocalTime;
    let mut t = unsafe { std::mem::zeroed() };
    unsafe { GetLocalTime(&mut t) };
    Now { date: format!("{:04}-{:02}-{:02}", t.wYear, t.wMonth, t.wDay), time: format!("{:02}:{:02}:{:02}", t.wHour, t.wMinute, t.wSecond) }
}
pub fn unix_now() -> i64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0) }

/// Calendar date for a unix time (UTC), enough for "до 12.12.2026".
pub fn date_of(unix: i64) -> String {
    let days = unix.div_euclid(86_400);
    let (y, m, d) = civil(days);
    format!("{d:02}.{m:02}.{y}")
}
fn civil(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468; let era = z.div_euclid(146_097); let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

pub fn plural(n: i64, one: &str, few: &str, many: &str) -> String {
    if crate::i18n::en() { return if n.abs() == 1 { one } else { many }.to_string(); }
    let n = n.abs(); let (d10, d100) = (n % 10, n % 100);
    let w = if d10 == 1 && d100 != 11 { one } else if (2..=4).contains(&d10) && !(12..=14).contains(&d100) { few } else { many };
    w.to_string()
}
pub fn bytes(v: i64) -> String {
    let v = v.max(0) as f64;
    if v < 1024.0 { crate::trf!("{} Б", "{} B", v as i64) }
    else if v < 1024.0 * 1024.0 { crate::trf!("{:.1} КБ", "{:.1} KB", v / 1024.0) }
    else if v < 1024.0 * 1024.0 * 1024.0 { crate::trf!("{:.1} МБ", "{:.1} MB", v / 1048576.0) }
    else { crate::trf!("{:.2} ГБ", "{:.2} GB", v / 1073741824.0) }
}
pub fn duration(secs: i64) -> String { format!("{:02}:{:02}:{:02}", secs / 3600, (secs / 60) % 60, secs % 60) }

pub fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes]; let _ = getrandom::getrandom(&mut buf);
    buf.iter().map(|b| format!("{b:02x}")).collect()
}
pub fn uuid4() -> String {
    let mut b = [0u8; 16]; let _ = getrandom::getrandom(&mut b);
    b[6] = (b[6] & 0x0f) | 0x40; b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}

pub fn free_port() -> u16 {
    std::net::TcpListener::bind(("127.0.0.1", 0)).and_then(|l| l.local_addr()).map(|a| a.port()).unwrap_or(0)
}
pub fn port_open(port: u16) -> bool {
    std::net::TcpStream::connect_timeout(&std::net::SocketAddr::from(([127, 0, 0, 1], port)), std::time::Duration::from_millis(150)).is_ok()
}

pub fn app_dir() -> std::path::PathBuf {
    std::env::current_exe().ok().and_then(|p| p.parent().map(|p| p.to_path_buf())).unwrap_or_else(|| ".".into())
}
/// A child process without a console window, with Xray's geo files next to the app.
pub fn hidden(exe: &std::path::Path) -> Command {
    let mut c = Command::new(exe);
    c.creation_flags(CREATE_NO_WINDOW).current_dir(app_dir()).env("XRAY_LOCATION_ASSET", app_dir()).env("xray.location.asset", app_dir());
    c
}

pub fn is_admin() -> bool { unsafe { windows_sys::Win32::UI::Shell::IsUserAnAdmin() != 0 } }

fn wide(s: &str) -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() }

/// ShellExecute with an optional verb ("runas" for elevation). Returns false if the user declined.
pub fn shell_execute(file: &str, args: &str, verb: Option<&str>) -> bool {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let f = wide(file); let a = wide(args); let v = verb.map(wide);
    let r = unsafe { ShellExecuteW(std::ptr::null_mut(), v.as_ref().map(|v| v.as_ptr()).unwrap_or(std::ptr::null()), f.as_ptr(), a.as_ptr(), std::ptr::null(), SW_SHOWNORMAL) };
    r as isize > 32
}
pub fn open_url(url: &str) {
    if url.starts_with("https://") || url.starts_with("tg://") || url.starts_with("http://t.me/") { shell_execute(url, "", Some("open")); }
}
pub fn open_path(p: &std::path::Path) { shell_execute(&p.to_string_lossy(), "", Some("open")); }

/// Relaunch this exe elevated with the given arguments.
pub fn relaunch_elevated(args: &str) -> bool {
    let exe = std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    shell_execute(&exe, args, Some("runas"))
}

pub fn run_hidden(exe: &std::path::Path, args: &[&str], timeout_secs: u64) -> Result<String, String> {
    let mut child = hidden(exe).args(args).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).spawn().map_err(|e| e.to_string())?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
    loop {
        if let Ok(Some(_)) = child.try_wait() { break; }
        if std::time::Instant::now() > deadline { let _ = child.kill(); return Err(crate::tr!("Проверка конфигурации не завершилась вовремя.", "The configuration check did not finish in time.").into()); }
        std::thread::sleep(std::time::Duration::from_millis(40));
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    let text = format!("{}\n{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    if out.status.success() { Ok(text) } else { Err(text) }
}

/// Last meaningful lines of a core's output (no banner).
pub fn tail(text: &str, max: usize) -> String {
    let joined = text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with("Xray ") && !l.starts_with("A unified platform")).collect::<Vec<_>>().join(" ");
    if joined.chars().count() > max { format!("…{}", joined.chars().rev().take(max).collect::<Vec<_>>().into_iter().rev().collect::<String>()) } else { joined }
}

pub fn clipboard_text() -> Option<String> {
    use windows_sys::Win32::System::DataExchange::{CloseClipboard, GetClipboardData, OpenClipboard};
    use windows_sys::Win32::System::Memory::{GlobalLock, GlobalUnlock};
    const CF_UNICODETEXT: u32 = 13;
    unsafe {
        if OpenClipboard(std::ptr::null_mut()) == 0 { return None; }
        let h = GetClipboardData(CF_UNICODETEXT);
        let result = if h.is_null() { None } else {
            let p = GlobalLock(h) as *const u16;
            if p.is_null() { None } else {
                let mut len = 0; while *p.add(len) != 0 { len += 1; }
                let s = String::from_utf16_lossy(std::slice::from_raw_parts(p, len)); GlobalUnlock(h); Some(s)
            }
        };
        CloseClipboard(); result
    }
}

/// Installed by the setup into Program Files (marker written by the installer).
pub fn installed() -> bool { app_dir().join("wraith.installed").exists() }

/// Where runtime configs are written. When the installed Wraith runs as administrator this is a
/// folder inside Program Files that ordinary programs cannot modify; otherwise the user data folder.
pub fn runtime_dir() -> std::path::PathBuf {
    let dir = if is_admin() && installed() { app_dir().join("runtime") } else { crate::store::folder() };
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Like run_hidden, with the input written to the child's stdin.
pub fn run_hidden_stdin(exe: &std::path::Path, args: &[&str], input: &[u8], timeout_secs: u64) -> Result<String, String> {
    use std::io::Write;
    let mut child = hidden(exe).args(args).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).spawn().map_err(|e| e.to_string())?;
    if let Some(mut stdin) = child.stdin.take() { stdin.write_all(input).map_err(|e| e.to_string())?; }
    let out = std::sync::Arc::new(std::sync::Mutex::new(None));
    let o2 = out.clone();
    let handle = std::thread::spawn(move || { let r = child.wait_with_output(); *o2.lock().unwrap() = Some(r); });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
    while !handle.is_finished() { if std::time::Instant::now() > deadline { return Err(crate::tr!("Проверка конфигурации не завершилась вовремя.", "The configuration check did not finish in time.").into()); } std::thread::sleep(std::time::Duration::from_millis(30)); }
    let result = out.lock().unwrap().take().ok_or(crate::tr!("Нет ответа ядра", "No answer from the core"))?.map_err(|e| e.to_string())?;
    let text = format!("{}\n{}", String::from_utf8_lossy(&result.stdout), String::from_utf8_lossy(&result.stderr));
    if result.status.success() { Ok(text) } else { Err(text) }
}

/// Local time parts (year, month, day, hour, minute) of a unix time, in the Windows time zone.
fn local_parts(unix: i64) -> (u16, u16, u16, u16, u16) {
    use windows_sys::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows_sys::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
    let ticks = ((unix + 11_644_473_600) as u64) * 10_000_000;
    let ft = FILETIME { dwLowDateTime: ticks as u32, dwHighDateTime: (ticks >> 32) as u32 };
    let mut utc: SYSTEMTIME = unsafe { std::mem::zeroed() }; let mut local: SYSTEMTIME = unsafe { std::mem::zeroed() };
    unsafe { FileTimeToSystemTime(&ft, &mut utc); SystemTimeToTzSpecificLocalTime(std::ptr::null(), &utc, &mut local); }
    (local.wYear, local.wMonth, local.wDay, local.wHour, local.wMinute)
}
/// Local "dd.MM, HH:mm" for a unix time.
pub fn local_short(unix: i64) -> String { let (_, m, d, h, mi) = local_parts(unix); format!("{d:02}.{m:02}, {h:02}:{mi:02}") }
/// The local hour (0–23), for the greeting.
pub fn local_hour() -> i32 {
    use windows_sys::Win32::System::SystemInformation::GetLocalTime;
    let mut t = unsafe { std::mem::zeroed() };
    unsafe { GetLocalTime(&mut t) };
    t.wHour as i32
}

/// The Windows Open / Save As dialog. None when cancelled. Blocks: call it from a background thread.
pub fn file_dialog(save: bool, title: &str, name: &str, filter: &[&str]) -> Result<Option<std::path::PathBuf>, String> {
    use windows_sys::Win32::UI::Controls::Dialogs::*;
    let patterns = filter.join(";");
    let filter_w: Vec<u16> = format!("{title} ({patterns})\0{patterns}\0\0").encode_utf16().collect();
    let title_w = wide(title);
    let mut buf = vec![0u16; 4096];
    for (i, c) in name.encode_utf16().take(4000).enumerate() { buf[i] = c; }
    let ext = filter.first().map(|f| f.trim_start_matches("*.").to_string()).unwrap_or_default();
    let ext_w = wide(&ext);
    let mut ofn: OPENFILENAMEW = unsafe { std::mem::zeroed() };
    ofn.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
    ofn.lpstrFilter = filter_w.as_ptr(); ofn.nFilterIndex = 1;
    ofn.lpstrFile = buf.as_mut_ptr(); ofn.nMaxFile = buf.len() as u32;
    ofn.lpstrTitle = title_w.as_ptr(); ofn.lpstrDefExt = if ext.is_empty() { std::ptr::null() } else { ext_w.as_ptr() };
    ofn.Flags = OFN_EXPLORER | OFN_NOCHANGEDIR | OFN_PATHMUSTEXIST | if save { OFN_OVERWRITEPROMPT } else { OFN_FILEMUSTEXIST };
    let ok = unsafe { if save { GetSaveFileNameW(&mut ofn) } else { GetOpenFileNameW(&mut ofn) } };
    if ok == 0 { return Ok(None); }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Ok(Some(std::path::PathBuf::from(String::from_utf16_lossy(&buf[..len]))))
}
