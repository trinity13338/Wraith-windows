//! App and core journal: kept in memory for the Журнал page and appended to %LOCALAPPDATA%\Wraith\wraith.log.

use std::collections::VecDeque;
use std::io::Write;
use std::sync::Mutex;

static LINES: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());
static VERSION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
const MAX_LINES: usize = 4000;

pub fn path() -> std::path::PathBuf { crate::store::folder().join("wraith.log") }

pub fn add(text: impl AsRef<str>) {
    let text = text.as_ref().trim_end();
    if text.is_empty() { return; }
    let now = crate::util::local_time();
    let line = format!("{} {}", now.time, text);
    {
        let mut lines = LINES.lock().unwrap();
        lines.push_back(line.clone());
        while lines.len() > MAX_LINES { lines.pop_front(); }
    }
    VERSION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let file = path();
    if let Ok(meta) = std::fs::metadata(&file) {
        if meta.len() > 2 * 1024 * 1024 { let _ = std::fs::rename(&file, file.with_extension("log.1")); }
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&file) {
        let _ = writeln!(f, "{} {}", now.date, line);
    }
}

/// Changes whenever a line is added, so the UI only rebuilds the text when needed.
pub fn version() -> u64 { VERSION.load(std::sync::atomic::Ordering::Relaxed) }
pub fn text() -> String { LINES.lock().unwrap().iter().cloned().collect::<Vec<_>>().join("\n") }
pub fn clear() {
    LINES.lock().unwrap().clear();
    VERSION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let _ = std::fs::write(path(), "");
}

/// Error text without subscription URLs or UUIDs, safe to show and to log.
pub fn safe(msg: impl AsRef<str>) -> String {
    use std::sync::OnceLock;
    static URL: OnceLock<regex::Regex> = OnceLock::new();
    static ID: OnceLock<regex::Regex> = OnceLock::new();
    let url = URL.get_or_init(|| regex::Regex::new(r"[a-zA-Z][a-zA-Z0-9+.-]*://[^\s]+").unwrap());
    let id = ID.get_or_init(|| regex::Regex::new(r"[0-9a-fA-F]{8}-[0-9a-fA-F-]{27,}").unwrap());
    let m = url.replace_all(msg.as_ref(), crate::tr!("[адрес скрыт]", "[address hidden]"));
    id.replace_all(&m, "[id]").into_owned()
}
