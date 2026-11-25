//! Window handling (show/hide from the tray, dark title bar), tray icon and autostart.

pub mod app;
pub mod backdrop;
pub mod flags;
pub mod icon;
pub mod icons;
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
    // The app is dark (as on Android): the frame follows.
    set_dark_title_bar(true);
    if start_hidden && tray_available() { hide_window(); }
}
/// Dark or light window frame, following the app theme.
pub fn set_dark_title_bar(dark: bool) {
    let h = HWND.load(Ordering::Relaxed); if h == 0 { return; }
    let dark: i32 = dark as i32;
    unsafe { windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute(h as _, 20, &dark as *const i32 as *const _, 4) };
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

    /// The mark alone, light on a dark taskbar and dark on a light one, at the small-icon size of this DPI.
    pub fn icon() -> Option<tray_icon::Icon> {
        use windows_sys::Win32::UI::{HiDpi::GetSystemMetricsForDpi, WindowsAndMessaging::SM_CXSMICON};
        let dpi = unsafe { windows_sys::Win32::UI::HiDpi::GetDpiForSystem() };
        let size = (unsafe { GetSystemMetricsForDpi(SM_CXSMICON, dpi) }).clamp(16, 64) as u32;
        let rgb = if super::light_taskbar() { [0x1C, 0x1B, 0x22] } else { [0xF5, 0xF2, 0xFA] };
        tray_icon::Icon::from_rgba(super::icon::mark(size, rgb), size, size).ok()
    }

    /// Tooltip with the state and speed, and the icon again when the taskbar's theme changed.
    pub fn refresh(tray: &TrayIcon, light: &mut Option<bool>) {
        let st = crate::core::status();
        let text = match st.state {
            crate::core::State::Connected => crate::trf!("Wraith · {}\n↓ {}/с  ↑ {}/с", "Wraith · {}\n↓ {}/s  ↑ {}/s", crate::ui::widgets::display_name(&st.profile_name), crate::util::bytes(st.rx_rate), crate::util::bytes(st.tx_rate)),
            _ => crate::tr!("Wraith · не подключено", "Wraith · not connected").into(),
        };
        let _ = tray.set_tooltip(Some(text));
        let now = super::light_taskbar();
        if *light != Some(now) { *light = Some(now); let _ = tray.set_icon(icon()); }
    }

    pub fn create() -> Option<TrayIcon> {
        let menu = Menu::new();
        let open = MenuItem::with_id("open", crate::tr!("Открыть Wraith", "Open Wraith"), true, None);
        let toggle = MenuItem::with_id("toggle", crate::tr!("Подключить / отключить", "Connect / disconnect"), true, None);
        let quit = MenuItem::with_id("quit", crate::tr!("Выход", "Quit"), true, None);
        menu.append_items(&[&open, &toggle, &PredefinedMenuItem::separator(), &quit]).ok()?;
        let icon = icon()?;
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

/// The taskbar (and so the tray) is light: Settings → Personalisation → Colours → Windows mode.
pub fn light_taskbar() -> bool {
    use winreg::enums::HKEY_CURRENT_USER;
    winreg::RegKey::predef(HKEY_CURRENT_USER).open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize")
        .and_then(|k| k.get_value::<u32, _>("SystemUsesLightTheme")).map(|v| v != 0).unwrap_or(false)
}

/// The window icon in the interface's accent (the Windows accent by default).
pub fn app_icon(size: u32) -> egui::IconData {
    let [r, g, b, _] = theme::accent_color().to_srgba_unmultiplied();
    egui::IconData { rgba: icon::tile(size, [0x0B, 0x0A, 0x10], [r, g, b]), width: size, height: size }
}
