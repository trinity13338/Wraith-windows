//! Installation into Program Files, elevated start through Task Scheduler, uninstall, and
//! unpacking release archives (used by the installer and by the updater).

use crate::util;
use std::io::Read;
use std::path::{Path, PathBuf};

pub const TASK: &str = "\\Wraith\\Wraith";
pub const TASK_AUTOSTART: &str = "\\Wraith\\Autostart";
const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\Wraith";

pub fn install_dir() -> PathBuf {
    std::env::var_os("ProgramFiles").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("C:\\Program Files")).join("Wraith")
}

/// Unpacks a release zip into `dir`. A single top folder in the archive is stripped. A file that is
/// in use (the running Wraith.exe) is renamed to *.old first; *.old files are removed on next start.
pub fn extract(zip_bytes: &[u8], dir: &Path, progress: &dyn Fn(usize, usize)) -> Result<(), String> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(zip_bytes)).map_err(|e| crate::trf!("Архив повреждён: {e}", "The archive is damaged: {e}"))?;
    let names: Vec<PathBuf> = (0..archive.len()).filter_map(|i| archive.by_index(i).ok().and_then(|f| f.enclosed_name())).collect();
    let top = names.first().and_then(|p| p.components().next()).map(|c| c.as_os_str().to_owned());
    let strip = top.is_some() && names.iter().all(|p| p.components().next().map(|c| Some(c.as_os_str().to_owned()) == top).unwrap_or(false)) && names.iter().any(|p| p.components().count() > 1);
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let total = archive.len();
    for i in 0..total {
        let mut file = archive.by_index(i).map_err(|e| e.to_string())?;
        // enclosed_name rejects absolute paths and ".." — nothing can be written outside `dir`.
        let Some(name) = file.enclosed_name() else { continue };
        let rel: PathBuf = if strip { name.components().skip(1).collect() } else { name };
        if rel.as_os_str().is_empty() { continue; }
        let target = dir.join(&rel);
        if file.is_dir() { std::fs::create_dir_all(&target).map_err(|e| e.to_string())?; continue; }
        if let Some(parent) = target.parent() { std::fs::create_dir_all(parent).map_err(|e| e.to_string())?; }
        let mut data = Vec::with_capacity(file.size() as usize); file.read_to_end(&mut data).map_err(|e| e.to_string())?;
        if std::fs::write(&target, &data).is_err() {
            let old = target.with_extension(format!("{}.old", target.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default()));
            let _ = std::fs::remove_file(&old);
            std::fs::rename(&target, &old).map_err(|e| crate::trf!("{} занят: {e}", "{} is in use: {e}", rel.display()))?;
            std::fs::write(&target, &data).map_err(|e| format!("{}: {e}", rel.display()))?;
        }
        progress(i + 1, total);
    }
    Ok(())
}

/// Leftovers of an update that replaced files in use.
pub fn clean_old_files() {
    if let Ok(entries) = std::fs::read_dir(util::app_dir()) {
        for e in entries.flatten() { if e.path().extension().map(|x| x == "old").unwrap_or(false) { let _ = std::fs::remove_file(e.path()); } }
    }
    // sing-box was removed in 0.12.0; its binary from older installs is no longer used.
    if util::installed() { let _ = std::fs::remove_file(util::app_dir().join("sing-box.exe")); }
}

/// Wraith, Xray and sing-box processes started from `dir` (except this process).
pub fn kill_in_dir(dir: &Path) {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};
    use windows_sys::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, TerminateProcess, WaitForSingleObject, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE, PROCESS_SYNCHRONIZE};
    let dir = dir.to_string_lossy().to_ascii_lowercase(); let me = std::process::id();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap.is_null() || snap as isize == -1 { return; }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed(); entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut ok = Process32FirstW(snap, &mut entry) != 0;
        while ok {
            let len = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(0);
            let name = String::from_utf16_lossy(&entry.szExeFile[..len]).to_ascii_lowercase();
            if entry.th32ProcessID != me && ["wraith.exe", "xray.exe", "sing-box.exe"].contains(&name.as_str()) {
                let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE | PROCESS_SYNCHRONIZE, 0, entry.th32ProcessID);
                if !h.is_null() {
                    let mut buf = [0u16; 1024]; let mut size = buf.len() as u32;
                    if QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut size) != 0 {
                        let path = String::from_utf16_lossy(&buf[..size as usize]).to_ascii_lowercase();
                        if path.starts_with(&dir) { TerminateProcess(h, 0); WaitForSingleObject(h, 3000); }
                    }
                    CloseHandle(h);
                }
            }
            ok = Process32NextW(snap, &mut entry) != 0;
        }
        CloseHandle(snap);
    }
}

fn schtasks(args: &[&str]) -> bool {
    util::hidden(Path::new("schtasks.exe")).args(args).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().map(|s| s.success()).unwrap_or(false)
}
fn xml_escape(s: &str) -> String { s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;") }

/// Task that starts Wraith with the user's full (administrator) rights without a UAC prompt.
pub fn create_task(name: &str, exe: &Path, args: &str, at_logon: bool) -> Result<(), String> {
    let user = format!("{}\\{}", std::env::var("USERDOMAIN").unwrap_or_default(), std::env::var("USERNAME").unwrap_or_default());
    let trigger = if at_logon { format!("<LogonTrigger><Enabled>true</Enabled><UserId>{}</UserId><Delay>PT3S</Delay></LogonTrigger>", xml_escape(&user)) } else { String::new() };
    let xml = format!(r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo><Description>Wraith VPN</Description></RegistrationInfo>
  <Triggers>{trigger}</Triggers>
  <Principals><Principal id="Author"><UserId>{user}</UserId><LogonType>InteractiveToken</LogonType><RunLevel>HighestAvailable</RunLevel></Principal></Principals>
  <Settings><MultipleInstancesPolicy>Parallel</MultipleInstancesPolicy><DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries><StopIfGoingOnBatteries>false</StopIfGoingOnBatteries><AllowHardTerminate>true</AllowHardTerminate><StartWhenAvailable>false</StartWhenAvailable><RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable><IdleSettings><StopOnIdleEnd>false</StopOnIdleEnd><RestartOnIdle>false</RestartOnIdle></IdleSettings><AllowStartOnDemand>true</AllowStartOnDemand><Enabled>true</Enabled><Hidden>false</Hidden><RunOnlyIfIdle>false</RunOnlyIfIdle><WakeToRun>false</WakeToRun><ExecutionTimeLimit>PT0S</ExecutionTimeLimit><Priority>5</Priority></Settings>
  <Actions Context="Author"><Exec><Command>"{exe}"</Command><Arguments>{args}</Arguments><WorkingDirectory>{dir}</WorkingDirectory></Exec></Actions>
</Task>"#, user = xml_escape(&user), exe = xml_escape(&exe.to_string_lossy()), args = xml_escape(args), dir = xml_escape(&exe.parent().unwrap_or(Path::new("C:\\")).to_string_lossy()));
    // Written into the protected install folder, used, and removed.
    let file = exe.with_file_name(format!("task-{}.xml", util::random_hex(4)));
    let mut bytes = vec![0xFF, 0xFE]; for u in xml.encode_utf16() { bytes.extend_from_slice(&u.to_le_bytes()); }
    std::fs::write(&file, bytes).map_err(|e| e.to_string())?;
    let ok = schtasks(&["/create", "/tn", name, "/xml", &file.to_string_lossy(), "/f"]);
    let _ = std::fs::remove_file(&file);
    if ok { Ok(()) } else { Err(crate::tr!("Не удалось создать задачу в планировщике Windows.", "Could not create the Task Scheduler task.").into()) }
}
pub fn delete_task(name: &str) { schtasks(&["/delete", "/tn", name, "/f"]); }
pub fn run_task(name: &str) -> bool { schtasks(&["/run", "/tn", name]) }

/// Autostart: a logon task for the installed app (starts elevated), a Run entry otherwise.
pub fn set_autostart(on: bool) -> Result<(), String> {
    if util::installed() && util::is_admin() {
        if on { create_task(TASK_AUTOSTART, &util::app_dir().join("Wraith.exe"), "--tray", true) } else { delete_task(TASK_AUTOSTART); Ok(()) }
    } else { crate::ui::set_autostart(on); Ok(()) }
}

fn shortcut(lnk: &Path, target: &Path) {
    let ps = format!("$s=(New-Object -ComObject WScript.Shell).CreateShortcut('{}');$s.TargetPath='{}';$s.WorkingDirectory='{}';$s.IconLocation='{},0';$s.Description='Wraith VPN';$s.Save()",
        lnk.to_string_lossy().replace('\'', "''"), target.to_string_lossy().replace('\'', "''"), target.parent().unwrap().to_string_lossy().replace('\'', "''"), target.to_string_lossy().replace('\'', "''"));
    let _ = util::hidden(Path::new("powershell.exe")).args(["-NoProfile", "-NonInteractive", "-Command", &ps]).status();
}
fn start_menu() -> PathBuf { PathBuf::from(std::env::var("ProgramData").unwrap_or_else(|_| "C:\\ProgramData".into())).join("Microsoft\\Windows\\Start Menu\\Programs\\Wraith.lnk") }
fn desktop() -> PathBuf { PathBuf::from(std::env::var("USERPROFILE").unwrap_or_default()).join("Desktop\\Wraith.lnk") }

pub struct Options { pub autostart: bool, pub desktop: bool }

/// Full installation (the setup runs elevated).
pub fn install(payload: &[u8], options: &Options, status: &dyn Fn(&str)) -> Result<PathBuf, String> {
    let dir = install_dir();
    status(crate::tr!("Останавливаем запущенный Wraith…", "Stopping the running Wraith…"));
    kill_in_dir(&dir);
    crate::core::proxy::restore_leftover();
    status(crate::tr!("Копируем файлы…", "Copying files…"));
    extract(payload, &dir, &|_, _| {})?;
    std::fs::write(dir.join("wraith.installed"), env!("CARGO_PKG_VERSION")).map_err(|e| e.to_string())?;
    let exe = dir.join("Wraith.exe");
    status(crate::tr!("Настраиваем запуск от администратора…", "Setting up the elevated start…"));
    create_task(TASK, &exe, "", false)?;
    if options.autostart { create_task(TASK_AUTOSTART, &exe, "--tray", true)?; } else { delete_task(TASK_AUTOSTART); }
    crate::ui::set_autostart(false); // a Run entry from the portable version would start a second, unelevated copy
    status(crate::tr!("Создаём ярлыки…", "Creating shortcuts…"));
    shortcut(&start_menu(), &exe);
    if options.desktop { shortcut(&desktop(), &exe); }
    status(crate::tr!("Регистрируем в «Установленных приложениях»…", "Registering in Installed apps…"));
    register_uninstall(&dir)?;
    crate::store::update(|s| s.set("autostart", options.autostart.into()));
    Ok(exe)
}

fn register_uninstall(dir: &Path) -> Result<(), String> {
    use winreg::enums::HKEY_LOCAL_MACHINE;
    let (key, _) = winreg::RegKey::predef(HKEY_LOCAL_MACHINE).create_subkey(UNINSTALL_KEY).map_err(|e| e.to_string())?;
    let exe = dir.join("Wraith.exe").to_string_lossy().into_owned();
    let size: u32 = std::fs::read_dir(dir).map(|d| d.flatten().filter_map(|e| e.metadata().ok()).map(|m| m.len()).sum::<u64>() / 1024).unwrap_or(0) as u32;
    let set = |k: &str, v: &str| key.set_value(k, &v.to_string()).map_err(|e| e.to_string());
    set("DisplayName", "Wraith")?; set("DisplayVersion", env!("CARGO_PKG_VERSION"))?; set("Publisher", "Wraith")?;
    set("DisplayIcon", &exe)?; set("InstallLocation", &dir.to_string_lossy())?;
    set("UninstallString", &format!("\"{exe}\" --uninstall"))?; set("URLInfoAbout", "https://github.com/trinity13338/Wraith-windows")?;
    key.set_value("NoModify", &1u32).map_err(|e| e.to_string())?; key.set_value("NoRepair", &1u32).map_err(|e| e.to_string())?;
    key.set_value("EstimatedSize", &size).map_err(|e| e.to_string())?;
    Ok(())
}

fn message(text: &str, question: bool) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, IDYES, MB_ICONINFORMATION, MB_ICONQUESTION, MB_OK, MB_YESNO};
    let t: Vec<u16> = text.encode_utf16().chain(Some(0)).collect(); let c: Vec<u16> = "Wraith".encode_utf16().chain(Some(0)).collect();
    unsafe { MessageBoxW(std::ptr::null_mut(), t.as_ptr(), c.as_ptr(), if question { MB_YESNO | MB_ICONQUESTION } else { MB_OK | MB_ICONINFORMATION }) == IDYES }
}

/// `Wraith.exe --uninstall` (from «Установленные приложения»).
pub fn uninstall() {
    if !util::is_admin() { util::relaunch_elevated("--uninstall"); return; }
    if !message(crate::tr!("Удалить Wraith с этого компьютера?", "Remove Wraith from this computer?"), true) { return; }
    let dir = util::app_dir();
    crate::store::init();
    crate::core::proxy::restore_leftover();
    kill_in_dir(&dir);
    delete_task(TASK); delete_task(TASK_AUTOSTART);
    crate::ui::set_autostart(false);
    let _ = std::fs::remove_file(start_menu()); let _ = std::fs::remove_file(desktop());
    { use winreg::enums::HKEY_LOCAL_MACHINE; let _ = winreg::RegKey::predef(HKEY_LOCAL_MACHINE).delete_subkey_all(UNINSTALL_KEY); }
    if message(crate::tr!("Удалить и ваши серверы, подписки и настройки?\n\nНет — оставить их для повторной установки.", "Also remove your servers, subscriptions and settings?\n\nNo keeps them for a reinstall."), true) {
        let _ = std::fs::remove_dir_all(crate::store::folder());
    }
    // The folder (with this exe) is removed by cmd after Wraith exits.
    let cmd = format!("/c ping -n 3 127.0.0.1 >nul & rmdir /s /q \"{}\"", dir.to_string_lossy());
    { use std::os::windows::process::CommandExt; let _ = std::process::Command::new("cmd.exe").creation_flags(util::CREATE_NO_WINDOW).current_dir(std::env::temp_dir()).raw_arg(&cmd).spawn(); }
    message(crate::tr!("Wraith удалён.", "Wraith has been removed."), false);
}

/// At an unelevated start: the installed app hands over to its elevated task (no UAC prompt), any
/// other copy asks for administrator rights through UAC. Returns true when another process took over;
/// false when already elevated or the prompt was declined (Wraith then runs without TUN).
pub fn elevate_via_task() -> bool {
    if util::is_admin() || std::env::args().any(|a| a == "--no-elevate") { return false; }
    if util::installed() && run_task(TASK) { return true; }
    // Arguments such as --tray or --connect carry over to the elevated copy.
    let args: Vec<String> = std::env::args().skip(1).map(|a| if a.contains(' ') { format!("\"{a}\"") } else { a }).collect();
    util::relaunch_elevated(&format!("{} --no-elevate", args.join(" ")).trim_start())
}
