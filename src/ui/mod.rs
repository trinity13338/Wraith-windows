//! Window handling (show/hide from the tray, dark title bar), tray icon and autostart.

pub mod app;
pub mod flags;
pub mod icon;
pub mod theme;
pub mod widgets;

use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::OnceLock;

static CTX: OnceLock<egui::Context> = OnceLock::new();
static HWND: AtomicIsize = AtomicIsize::new(0);
static TRAY: AtomicBool = AtomicBool::new(false);
pub const TITLE: &str = "Wraith";

pub fn set_context(ctx: egui::Context) { let _ = CTX.set(ctx); }
pub fn context() -> Option<egui::Context> { CTX.get().cloned() }
pub fn repaint() { if let Some(c) = CTX.get() { c.request_repaint(); } }
pub fn tray_available() -> bool { TRAY.load(Ordering::Relaxed) }

fn wide(s: &str) -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() }

/// Our top-level window, found by title and process id.
pub fn find_window(pid: u32) -> isize {
    use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowExW, GetWindowThreadProcessId};
    let title = wide(TITLE); let mut after = std::ptr::null_mut();
    loop {
        let h = unsafe { FindWindowExW(std::ptr::null_mut(), after, std::ptr::null(), title.as_ptr()) };
        if h.is_null() { return 0; }
        let mut owner = 0u32; unsafe { GetWindowThreadProcessId(h, &mut owner) };
        if pid == 0 || owner == pid { return h as isize; }
        after = h;
    }
}

pub fn window_ready(start_hidden: bool) {
    let h = find_window(std::process::id()); if h == 0 { return; }
    HWND.store(h, Ordering::Relaxed);
    let dark: i32 = if theme::palette(&crate::store::get().s("theme")).dark { 1 } else { 0 };
    unsafe { windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute(h as _, 20, &dark as *const i32 as *const _, 4) };
    if start_hidden && tray_available() { hide_window(); }
}
pub fn hide_window() {
    let h = HWND.load(Ordering::Relaxed);
    if h != 0 { unsafe { windows_sys::Win32::UI::WindowsAndMessaging::ShowWindow(h as _, windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE) }; }
}
pub fn show_window() { show_hwnd(HWND.load(Ordering::Relaxed)); repaint(); }
pub fn show_hwnd(h: isize) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{IsIconic, SetForegroundWindow, ShowWindow, SW_RESTORE, SW_SHOW};
    if h == 0 { return; }
    unsafe { ShowWindow(h as _, if IsIconic(h as _) != 0 { SW_RESTORE } else { SW_SHOW }); SetForegroundWindow(h as _); }
}

/// HKCU Run entry; Wraith starts minimised to the tray.
pub fn set_autostart(on: bool) {
    use winreg::enums::{HKEY_CURRENT_USER, KEY_WRITE};
    let Ok(key) = winreg::RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(r"Software\Microsoft\Windows\CurrentVersion\Run", KEY_WRITE) else { return };
    if on { let exe = std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(); let _ = key.set_value("Wraith", &format!("\"{exe}\" --tray")); }
    else { let _ = key.delete_value("Wraith"); }
}

pub mod tray {
    use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

    pub fn create() -> Option<TrayIcon> {
        let menu = Menu::new();
        let open = MenuItem::with_id("open", crate::tr!("Открыть Wraith", "Open Wraith"), true, None);
        let toggle = MenuItem::with_id("toggle", crate::tr!("Подключить / отключить", "Connect / disconnect"), true, None);
        let quit = MenuItem::with_id("quit", crate::tr!("Выход", "Quit"), true, None);
        menu.append_items(&[&open, &toggle, &PredefinedMenuItem::separator(), &quit]).ok()?;
        let icon = tray_icon::Icon::from_rgba(super::icon::rgba(32), 32, 32).ok()?;
        let tray = TrayIconBuilder::new().with_menu(Box::new(menu)).with_tooltip("Wraith").with_icon(icon).with_menu_on_left_click(false).build().ok()?;
        // Handled right here: the window may be hidden, and a hidden window gets no frames.
        MenuEvent::set_event_handler(Some(|e: MenuEvent| match e.id.0.as_str() {
            "open" => super::show_window(),
            "toggle" => {
                let st = crate::core::status();
                if st.state == crate::core::State::Off {
                    let selected = crate::store::get().selected().cloned();
                    let tun = crate::model::uses_tun(&crate::store::get().settings);
                    match selected { Some(p) if !tun || crate::util::is_admin() => crate::core::connect(p), _ => super::show_window() }
                } else { crate::core::disconnect(); }
                super::repaint();
            }
            "quit" => { crate::core::shutdown(); std::process::exit(0); }
            _ => {}
        }));
        TrayIconEvent::set_event_handler(Some(|e: TrayIconEvent| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e { super::show_window(); }
        }));
        super::TRAY.store(true, std::sync::atomic::Ordering::Relaxed);
        Some(tray)
    }
}
