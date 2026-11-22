//! Main window: navigation rail, Home (status + servers), Settings, Journal; dialogs and background tasks.

use super::theme::{self, icon, semibold, Palette};
use super::widgets::{self as w, Kind};
use crate::core::{self, State};
use crate::json::{Get, Obj};
use crate::model::{Profile, Subscription};
use crate::store;
use crate::subs::updater::{self, Release};
use crate::util;
use egui::{Align, Align2, Color32, CornerRadius, FontId, Frame, Layout, Margin, RichText, ScrollArea, Sense, Stroke, Ui, Vec2};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(PartialEq, Clone, Copy)]
enum Page { Home, Settings, Logs }

enum Modal {
    Add { text: String },
    Info { title: String, text: String, copy: bool },
    Confirm { title: String, text: String, action: Confirmed },
    Admin,
    Rule { index: Option<usize>, rule: Obj },
    Sub { sub: Subscription },
    Update { release: Release },
    Value { key: String, title: String, hint: String, value: String, numeric: bool },
}
enum Confirmed { DeleteProfile(String), DeleteSubscription(String), Reset }

#[derive(Default)]
struct Shared {
    busy: Option<String>,
    toast: Option<(String, Instant)>,
    pinging: HashSet<String>,
    release: Option<Release>,
    download: Option<(u64, u64)>,
}

pub struct App {
    pal: Palette, theme_key: String, theme_checked: Instant,
    page: Page, search: String,
    modal: Option<Modal>,
    shared: Arc<Mutex<Shared>>,
    ping_cancel: Option<Arc<AtomicBool>>,
    profiles: Vec<Profile>, subs: Vec<Subscription>, revision: u64,
    log_cache: (u64, String),
    start_hidden: bool, frames: u64,
    _tray: Option<tray_icon::TrayIcon>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext, start_hidden: bool, connect: bool) -> Self {
        theme::install_fonts(&cc.egui_ctx);
        let (theme, accent) = { let s = store::get(); (s.s("theme"), s.b("windowsAccent")) };
        let theme_key = theme::signature(&theme, accent);
        let pal = theme::palette(&theme, accent);
        theme::apply(&cc.egui_ctx, &pal);
        super::set_context(cc.egui_ctx.clone());
        let tray = super::tray::create();
        let page = match std::env::args().find_map(|a| a.strip_prefix("--page=").map(String::from)).as_deref() { Some("settings") => Page::Settings, Some("logs") => Page::Logs, _ => Page::Home };
        let app = App { pal, theme_key, theme_checked: Instant::now(), page, search: String::new(), modal: None, shared: Arc::new(Mutex::new(Shared::default())), ping_cancel: None, profiles: vec![], subs: vec![], revision: u64::MAX, log_cache: (u64::MAX, String::new()), start_hidden, frames: 0, _tray: tray };
        let (update_on_open, check_updates, connect_on_start) = { let s = store::get(); (s.b("updateOnOpen"), s.b("checkUpdates"), s.b("connectOnStart")) };
        if update_on_open { app.background(crate::tr!("Обновляем подписки…", "Updating subscriptions…"), || { crate::subs::update_all(true); Ok(crate::tr!("Подписки обновлены", "Subscriptions updated").into()) }); }
        if check_updates { let shared = app.shared.clone(); std::thread::spawn(move || if let Ok(r) = updater::check() { if r.available { shared.lock().unwrap().release = Some(r); super::repaint(); } }); }
        if connect || connect_on_start { let selected = store::get().selected().cloned(); if let Some(p) = selected { core::connect(p); } }
        app
    }

    fn background(&self, label: &str, job: impl FnOnce() -> Result<String, String> + Send + 'static) {
        let shared = self.shared.clone();
        shared.lock().unwrap().busy = Some(label.into());
        std::thread::spawn(move || {
            let result = job();
            let mut s = shared.lock().unwrap(); s.busy = None;
            s.toast = Some((match result { Ok(m) => m, Err(e) => crate::log::safe(e) }, Instant::now()));
            drop(s); super::repaint();
        });
    }
    fn toast(&self, text: impl Into<String>) { self.shared.lock().unwrap().toast = Some((text.into(), Instant::now())); }

    fn refresh_cache(&mut self) {
        let rev = store::revision();
        if rev == self.revision { return; }
        self.revision = rev;
        let s = store::get(); self.profiles = s.profiles.clone(); self.subs = s.subscriptions.clone();
        drop(s);
        self.refresh_theme();
    }
    /// Rebuilds the colours when the theme setting, the Windows light/dark mode or the accent changes.
    fn refresh_theme(&mut self) {
        let (theme, accent) = { let s = store::get(); (s.s("theme"), s.b("windowsAccent")) };
        let key = theme::signature(&theme, accent);
        if key == self.theme_key { return; }
        self.theme_key = key; self.pal = theme::palette(&theme, accent);
        if let Some(ctx) = super::context() { theme::apply(&ctx, &self.pal); }
        super::set_dark_title_bar(self.pal.dark);
    }

    // ------------------------------------------------------------------ actions

    fn toggle(&mut self) {
        let st = core::status();
        match st.state {
            State::Connected | State::Connecting => core::disconnect(),
            State::Stopping => {}
            State::Off => self.connect_selected(),
        }
    }
    fn connect_selected(&mut self) {
        let Some(p) = store::get().selected().cloned() else { self.toast(crate::tr!("Сначала добавьте и выберите сервер", "Add and select a server first")); return };
        if crate::model::uses_tun(&store::get().settings) && !util::is_admin() { self.modal = Some(Modal::Admin); return; }
        core::connect(p);
    }
    fn select(&mut self, id: &str, connect_now: bool) {
        store::update(|s| s.selected = id.to_string());
        let st = core::status();
        if connect_now || (st.state != State::Off && st.profile_id != id) { self.connect_selected(); }
    }

    fn ping(&mut self, ids: Option<Vec<String>>) {
        if let Some(c) = self.ping_cancel.take() { c.store(true, Ordering::Relaxed); self.shared.lock().unwrap().pinging.clear(); if ids.is_none() { return; } }
        let state = store::get().clone();
        let targets: Vec<Profile> = match &ids { Some(ids) => state.profiles.iter().filter(|p| ids.contains(&p.id)).cloned().collect(), None => self.visible_profiles() };
        if targets.is_empty() { return; }
        let cancel = Arc::new(AtomicBool::new(false)); self.ping_cancel = Some(cancel.clone());
        { let mut s = self.shared.lock().unwrap(); for p in &targets { s.pinging.insert(p.id.clone()); } }
        // A new check of several servers starts from the provider's order: old results are dropped.
        if targets.len() > 1 { store::update(|s| for p in s.profiles.iter_mut().filter(|p| targets.iter().any(|t| t.id == p.id)) { p.latency = 0; p.ping_error.clear(); }); }
        let shared = self.shared.clone();
        let report: crate::pinger::Report = Arc::new(move |id: &str, ms: i64, err: String| {
            { let mut s = store::get(); if let Some(p) = s.profiles.iter_mut().find(|p| p.id == id) { p.latency = ms; p.ping_error = if ms > 0 { String::new() } else { err }; } }
            shared.lock().unwrap().pinging.remove(id); super::repaint();
        });
        let shared = self.shared.clone();
        std::thread::spawn(move || {
            crate::pinger::run(state, targets, report, cancel);
            shared.lock().unwrap().pinging.clear(); store::save(); super::repaint();
        });
    }

    fn add(&mut self, text: String) {
        let payload = match crate::importer::unwrap(&text) { Ok(p) => p, Err(e) => { self.toast(e); return } };
        if crate::importer::is_subscription_url(&payload) {
            self.modal = None;
            self.background(crate::tr!("Загружаем подписку…", "Loading the subscription…"), move || { let sub = crate::subs::create(&payload)?; Ok(crate::trf!("Подписка «{}» добавлена", "Subscription «{}» added", sub.name)) });
            return;
        }
        match crate::importer::parse(&payload) {
            Ok(result) => {
                let n = result.profiles.len();
                store::update(|s| { let first = result.profiles[0].id.clone(); s.profiles.extend(result.profiles); if s.selected().is_none() { s.selected = first; } });
                self.modal = None;
                let skipped = if result.skipped.is_empty() { String::new() } else { crate::trf!(", пропущено {}", ", skipped {}", result.skipped.len()) };
                self.toast(crate::trf!("Добавлено {n} {}{skipped}", "Added {n} {}{skipped}", util::plural(n as i64, crate::tr!("сервер", "server"), crate::tr!("сервера", "servers"), crate::tr!("серверов", "servers"))));
            }
            Err(e) => self.toast(e),
        }
    }

    /// The server's own configuration as the provider sent it (a link as its Xray outbound), without
    /// the inbounds, DNS, routing and other parts Wraith adds when it starts the core.
    fn show_config(&mut self, p: &Profile) {
        self.modal = Some(match crate::xray::source(p) {
            Ok(c) => Modal::Info { title: crate::tr!("Конфигурация сервера", "Server configuration").into(), text: serde_json::to_string_pretty(&c).unwrap_or_default(), copy: true },
            Err(e) => Modal::Info { title: crate::tr!("Конфигурация не собрана", "The configuration could not be built").into(), text: e, copy: false },
        });
    }

    fn visible_profiles(&self) -> Vec<Profile> {
        let q = self.search.trim().to_lowercase();
        self.profiles.iter().filter(|p| q.is_empty() || p.name.to_lowercase().contains(&q) || p.host.to_lowercase().contains(&q) || p.description.to_lowercase().contains(&q)).cloned().collect()
    }

    // ------------------------------------------------------------------ frame

    fn rail(&mut self, ui: &mut Ui) {
        let p = self.pal;
        ui.add_space(18.0);
        ui.vertical_centered(|ui| {
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(44.0), Sense::hover());
            ui.painter().rect_filled(rect, CornerRadius::same(14), p.primary_container);
            ui.painter().text(rect.center(), Align2::CENTER_CENTER, "W", FontId::new(22.0, semibold()), p.on_primary_container);
            ui.add_space(26.0);
            for (page, glyph, label) in [(Page::Home, icon::HOME, crate::tr!("Главная", "Home")), (Page::Settings, icon::SETTINGS, crate::tr!("Настройки", "Settings")), (Page::Logs, icon::JOURNAL, crate::tr!("Журнал", "Log"))] {
                let selected = self.page == page;
                let (rect, response) = ui.allocate_exact_size(Vec2::new(80.0, 62.0), Sense::click());
                let pill = egui::Rect::from_center_size(egui::pos2(rect.center().x, rect.top() + 18.0), Vec2::new(56.0, 32.0));
                if selected { ui.painter().rect_filled(pill, CornerRadius::same(16), p.secondary_container); }
                else if response.hovered() { ui.painter().rect_filled(pill, CornerRadius::same(16), p.text.gamma_multiply(0.07)); }
                ui.painter().text(pill.center(), Align2::CENTER_CENTER, glyph, FontId::new(18.0, theme::icons()), if selected { p.on_secondary_container } else { p.muted });
                ui.painter().text(egui::pos2(rect.center().x, rect.top() + 48.0), Align2::CENTER_CENTER, label, FontId::new(12.0, if selected { semibold() } else { egui::FontFamily::Proportional }), if selected { p.text } else { p.muted });
                if response.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() { self.page = page; }
                ui.add_space(4.0);
            }
        });
    }

    fn home(&mut self, ui: &mut Ui) {
        let p = self.pal;
        let wide = ui.available_width() > 860.0;
        if wide {
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| { ui.set_width(330.0); self.hero(ui); });
                ui.add_space(20.0);
                ui.vertical(|ui| self.servers(ui));
            });
        } else {
            ScrollArea::vertical().id_salt("narrow").show(ui, |ui| { self.hero(ui); ui.add_space(16.0); self.servers(ui); });
        }
        let _ = p;
    }

    fn hero(&mut self, ui: &mut Ui) {
        let p = self.pal; let st = core::status();
        let release = self.shared.lock().unwrap().release.clone();
        if let Some(r) = release {
            Frame::new().fill(p.tertiary_container).corner_radius(CornerRadius::same(20)).inner_margin(Margin::same(14)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(w::icon_text(icon::UPDATE, 18.0, p.text));
                    ui.label(RichText::new(crate::trf!("Доступна версия {}", "Version {} is available", r.version)).font(FontId::new(14.0, semibold())));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| if w::button(ui, &p, Kind::Text, None, crate::tr!("Обновить", "Update")).clicked() { self.modal = Some(Modal::Update { release: r.clone() }); });
                });
            });
            ui.add_space(12.0);
        }
        w::card(&p, p.low).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.vertical_centered(|ui| {
                ui.add_space(6.0);
                if self.orb(ui, &st).clicked() { self.toggle(); }
                ui.add_space(14.0);
                let (title, color) = match st.state { State::Connected => (crate::tr!("Подключено", "Connected"), p.primary), State::Connecting => (crate::tr!("Подключение…", "Connecting…"), p.text), State::Stopping => (crate::tr!("Отключение…", "Disconnecting…"), p.text), State::Off => (crate::tr!("Не подключено", "Not connected"), p.text) };
                ui.label(RichText::new(title).font(FontId::new(24.0, semibold())).color(color));
                let selected = store::get().selected().cloned();
                match (&st.state, &selected) {
                    (State::Connected | State::Connecting, _) if !st.profile_name.is_empty() => { ui.label(RichText::new(w::display_name(&st.profile_name)).color(p.muted)); }
                    (_, Some(s)) => { ui.label(RichText::new(w::display_name(&s.name)).color(p.muted)); }
                    _ => { ui.label(RichText::new(crate::tr!("Добавьте сервер, чтобы подключиться", "Add a server to connect")).color(p.muted)); }
                }
                if st.state == State::Connected {
                    ui.add_space(10.0);
                    let secs = st.since.map(|s| s.elapsed().as_secs() as i64).unwrap_or(0);
                    ui.horizontal(|ui| {
                        let total = 3.0 * 8.0 + 3.0 * 96.0; ui.add_space(((ui.available_width() - total) / 2.0).max(0.0));
                        for (glyph, text) in [(icon::CLOCK, util::duration(secs)), (icon::DOWNLOAD, crate::trf!("{}/с", "{}/s", util::bytes(st.rx_rate))), (icon::UPLOAD, crate::trf!("{}/с", "{}/s", util::bytes(st.tx_rate)))] {
                            Frame::new().fill(p.container).corner_radius(CornerRadius::same(10)).inner_margin(Margin::symmetric(8, 5)).show(ui, |ui| {
                                ui.set_min_width(80.0);
                                ui.horizontal(|ui| { ui.label(w::icon_text(glyph, 12.0, p.muted)); ui.label(RichText::new(text).size(12.5).color(p.text)); });
                            });
                        }
                    });
                    ui.label(RichText::new(crate::trf!("{} · {} · всего ↓ {} ↑ {}", "{} · {} · total ↓ {} ↑ {}", st.engine, match store::get().s("tunnelMode").as_str() { "both" => crate::tr!("TUN + прокси", "TUN + proxy"), "tun" => "TUN", _ => crate::tr!("системный прокси", "system proxy") }, util::bytes(st.rx), util::bytes(st.tx))).size(12.0).color(p.muted));
                }
                if st.state == State::Off && !st.error.is_empty() {
                    ui.add_space(10.0);
                    Frame::new().fill(p.error_container).corner_radius(CornerRadius::same(14)).inner_margin(Margin::same(10)).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.label(RichText::new(&st.error).size(13.0).color(p.on_error_container));
                    });
                }
                ui.add_space(16.0);
            });
            // System proxy and TUN are independent; at least one stays on.
            let mode = store::get().s("tunnelMode");
            let mut flags = [mode != "tun", mode == "tun" || mode == "both"];
            ui.label(RichText::new(crate::tr!("Подключение", "Connection")).size(12.5).color(p.muted));
            if w::toggles(ui, &p, &mut flags, &[crate::tr!("Системный прокси", "System proxy"), "TUN"], ui.available_width()) {
                if !flags[0] && !flags[1] { self.toast(crate::tr!("Нужен хотя бы один режим", "At least one mode is needed")); }
                else {
                    let next = match flags { [true, true] => "both", [true, false] => "proxy", _ => "tun" };
                    store::update(|s| s.set("tunnelMode", next.into()));
                    if st.state == State::Connected { self.connect_selected(); }
                }
            }
        });
    }

    /// Big round connect button with the state ring.
    fn orb(&self, ui: &mut Ui, st: &core::Status) -> egui::Response {
        let p = self.pal;
        let (rect, response) = ui.allocate_exact_size(Vec2::splat(168.0), Sense::click());
        let c = rect.center(); let painter = ui.painter();
        let t = ui.ctx().input(|i| i.time) as f32;
        let on = st.state == State::Connected; let busy = matches!(st.state, State::Connecting | State::Stopping);
        let hover = if response.hovered() { 1.0 } else { 0.0 };
        painter.circle_filled(c, 82.0, if on { p.primary.gamma_multiply(0.12) } else { p.text.gamma_multiply(0.035) });
        if busy {
            let start = t * 4.0;
            let points: Vec<egui::Pos2> = (0..=40).map(|i| { let a = start + i as f32 / 40.0 * 1.6 * std::f32::consts::PI; egui::pos2(c.x + 76.0 * a.cos(), c.y + 76.0 * a.sin()) }).collect();
            painter.add(egui::Shape::line(points, Stroke::new(4.0, p.primary)));
            ui.ctx().request_repaint();
        } else {
            painter.circle_stroke(c, 76.0, Stroke::new(2.0, if on { p.primary } else { p.outline_variant }));
        }
        let fill = if on { p.primary } else { p.highest };
        painter.circle_filled(c, 62.0 + hover * 2.0, fill);
        painter.text(c, Align2::CENTER_CENTER, icon::POWER, FontId::new(44.0, theme::icons()), if on { p.on_primary } else { p.text });
        response.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(if on { crate::tr!("Отключить", "Disconnect") } else { crate::tr!("Подключить", "Connect") })
    }

    fn servers(&mut self, ui: &mut Ui) {
        let p = self.pal;
        let pinging = !self.shared.lock().unwrap().pinging.is_empty();
        ui.horizontal(|ui| {
            ui.label(RichText::new(crate::tr!("Серверы", "Servers")).font(FontId::new(24.0, semibold())));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if w::button(ui, &p, Kind::Filled, Some(icon::ADD), crate::tr!("Добавить", "Add")).clicked() { self.modal = Some(Modal::Add { text: String::new() }); }
                if !self.subs.is_empty() && w::icon_button(ui, &p, icon::REFRESH, crate::tr!("Обновить подписки", "Update subscriptions"), false).clicked() {
                    self.background(crate::tr!("Обновляем подписки…", "Updating subscriptions…"), || { crate::subs::update_all(false); Ok(crate::tr!("Подписки обновлены", "Subscriptions updated").into()) });
                }
                if w::icon_button(ui, &p, if pinging { icon::CLOSE } else { icon::PULSE }, if pinging { crate::tr!("Остановить проверку", "Stop checking") } else { crate::tr!("Проверить задержку", "Check latency") }, pinging).clicked() { self.ping(None); }
                ui.add(egui::TextEdit::singleline(&mut self.search).hint_text(crate::tr!("Поиск", "Search")).desired_width((ui.available_width() - 8.0).clamp(120.0, 260.0)).margin(Margin::symmetric(12, 9)));
            });
        });
        ui.add_space(8.0);
        if self.profiles.is_empty() { self.empty(ui); return; }
        let pinging_ids = self.shared.lock().unwrap().pinging.clone();
        let sort = store::get().s("sort");
        let selected = store::get().selected.clone();
        let visible = self.visible_profiles();
        let mut groups: Vec<(Option<Subscription>, Vec<Profile>)> = vec![];
        for sub in &self.subs { groups.push((Some(sub.clone()), visible.iter().filter(|p| p.group == sub.id).cloned().collect())); }
        let manual: Vec<Profile> = visible.iter().filter(|p| p.group.is_empty() || !self.subs.iter().any(|s| s.id == p.group)).cloned().collect();
        if !manual.is_empty() || self.subs.is_empty() { groups.push((None, manual)); }
        ScrollArea::vertical().id_salt("servers").auto_shrink([false, false]).show(ui, |ui| {
            for (sub, mut list) in groups {
                match sort.as_str() {
                    "ping" => list.sort_by_key(|p| if p.latency > 0 { p.latency } else { i64::MAX }),
                    "name" => list.sort_by_key(|p| w::display_name(&p.name).to_lowercase()),
                    _ => {}
                }
                list.sort_by_key(|p| !p.favorite);
                let collapsed = sub.as_ref().map(|s| s.collapsed).unwrap_or(false);
                self.group_header(ui, sub.as_ref(), list.len());
                if collapsed && self.search.is_empty() { ui.add_space(6.0); continue; }
                for profile in &list { self.server_row(ui, profile, profile.id == selected, pinging_ids.contains(&profile.id)); }
                ui.add_space(10.0);
            }
            ui.add_space(20.0);
        });
    }

    fn empty(&mut self, ui: &mut Ui) {
        let p = self.pal;
        w::card(&p, p.low).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.vertical_centered(|ui| {
                ui.add_space(30.0);
                ui.label(w::icon_text(icon::SERVER, 42.0, p.primary));
                ui.add_space(10.0);
                ui.label(RichText::new(crate::tr!("Пока нет серверов", "No servers yet")).font(FontId::new(18.0, semibold())));
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    ui.add_space((ui.available_width() - 330.0).max(0.0) / 2.0);
                    if w::button(ui, &p, Kind::Filled, Some(icon::PASTE), crate::tr!("Вставить из буфера", "Paste from clipboard")).clicked() { if let Some(t) = util::clipboard_text() { self.add(t); } else { self.toast(crate::tr!("Буфер обмена пуст", "The clipboard is empty")); } }
                    if w::button(ui, &p, Kind::Tonal, Some(icon::ADD), crate::tr!("Добавить", "Add")).clicked() { self.modal = Some(Modal::Add { text: String::new() }); }
                });
                ui.add_space(30.0);
            });
        });
    }

    fn group_header(&mut self, ui: &mut Ui, sub: Option<&Subscription>, count: usize) {
        let p = self.pal;
        let Some(sub) = sub else {
            ui.add_space(6.0);
            ui.label(RichText::new(crate::trf!("Мои серверы · {count}", "My servers · {count}")).font(FontId::new(13.5, semibold())).color(p.primary));
            ui.add_space(4.0); return;
        };
        Frame::new().fill(p.low).corner_radius(CornerRadius::same(18)).inner_margin(Margin { left: 14, right: 8, top: 10, bottom: 10 }).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let chevron = if sub.collapsed { icon::RIGHT } else { icon::DOWN };
                if w::icon_button(ui, &p, chevron, "", false).clicked() { let id = sub.id.clone(); store::update(|s| if let Some(x) = s.subscriptions.iter_mut().find(|x| x.id == id) { x.collapsed = !x.collapsed; }); }
                // The text column takes all the width the three buttons leave, so the usage bar spans the card.
                let column = (ui.available_width() - 3.0 * 40.0 - 3.0 * 8.0 - 12.0).max(160.0);
                ui.vertical(|ui| {
                    ui.set_width(column);
                    ui.label(RichText::new(&sub.name).font(FontId::new(15.0, semibold())));
                    let (used, total, expire) = sub.usage();
                    let mut info = vec![format!("{count} {}", util::plural(count as i64, crate::tr!("сервер", "server"), crate::tr!("сервера", "servers"), crate::tr!("серверов", "servers")))];
                    if total > 0 { info.push(crate::trf!("{} из {}", "{} of {}", util::bytes(used), util::bytes(total))); } else if used > 0 { info.push(util::bytes(used)); }
                    if expire > 0 { let days = (expire - util::unix_now()) / 86400; info.push(if days < 0 { crate::tr!("срок истёк", "expired").into() } else { crate::trf!("до {} · {} дн.", "until {} · {} d", util::date_of(expire), days) }); }
                    if sub.updated > 0 { let mins = (util::unix_now() - sub.updated) / 60; info.push(if mins < 1 { crate::tr!("обновлено сейчас", "updated just now").into() } else if mins < 60 { crate::trf!("обновлено {mins} мин назад", "updated {mins} min ago") } else if mins < 1440 { crate::trf!("обновлено {} ч назад", "updated {} h ago", mins / 60) } else { crate::trf!("обновлено {} дн. назад", "updated {} d ago", mins / 1440) }); }
                    ui.label(RichText::new(info.join(" · ")).size(12.5).color(p.muted));
                    if total > 0 {
                        ui.add_space(2.0);
                        let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 6.0), Sense::hover());
                        ui.painter().rect_filled(rect, CornerRadius::same(3), p.highest);
                        let frac = (used as f32 / total as f32).clamp(0.0, 1.0);
                        ui.painter().rect_filled(egui::Rect::from_min_size(rect.min, Vec2::new((rect.width() * frac).max(6.0), 6.0)), CornerRadius::same(3), if frac > 0.9 { p.error } else { p.primary });
                        ui.add_space(2.0);
                    }
                    for text in [&sub.announce, &sub.info_text] { if !text.is_empty() { ui.label(RichText::new(text.as_str()).size(12.5).color(p.text)); } }
                    if !sub.error.is_empty() { ui.label(RichText::new(&sub.error).size(12.5).color(p.error)); }
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let menu = w::icon_button(ui, &p, icon::MORE, crate::tr!("Подписка", "Subscription"), false);
                    let id = sub.id.clone(); let s2 = sub.clone();
                    let busy = self.shared.lock().unwrap().busy.is_some();
                    let pinging = { let s = self.shared.lock().unwrap(); self.profiles.iter().any(|x| x.group == id && s.pinging.contains(&x.id)) };
                    if w::icon_button(ui, &p, if pinging { icon::CLOSE } else { icon::PULSE }, if pinging { crate::tr!("Остановить проверку", "Stop checking") } else { crate::tr!("Проверить задержку серверов подписки", "Check the subscription's servers") }, pinging).clicked() {
                        if pinging { self.ping(None); } else { let ids = self.profiles.iter().filter(|x| x.group == id).map(|x| x.id.clone()).collect(); self.ping(Some(ids)); }
                    }
                    if w::icon_button(ui, &p, icon::REFRESH, crate::tr!("Обновить подписку", "Update subscription"), false).clicked() && !busy {
                        let s3 = s2.clone(); self.background(crate::tr!("Обновляем подписку…", "Updating the subscription…"), move || { crate::subs::update(s3)?; Ok(crate::tr!("Подписка обновлена", "Subscription updated").into()) });
                    }
                    egui::Popup::menu(&menu).show(|ui| {
                        ui.set_min_width(220.0);
                        if ui.button(crate::tr!("Обновить", "Update")).clicked() { let s3 = s2.clone(); self.background(crate::tr!("Обновляем подписку…", "Updating the subscription…"), move || { crate::subs::update(s3)?; Ok(crate::tr!("Подписка обновлена", "Subscription updated").into()) }); }
                        if ui.button(crate::tr!("Проверить задержку", "Check latency")).clicked() { let ids = self.profiles.iter().filter(|p| p.group == id).map(|p| p.id.clone()).collect(); self.ping(Some(ids)); }
                        if ui.button(crate::tr!("Изменить", "Edit")).clicked() { self.modal = Some(Modal::Sub { sub: s2.clone() }); }
                        if ui.button(crate::tr!("Скопировать ссылку", "Copy link")).clicked() { ui.ctx().copy_text(s2.url.clone()); self.toast(crate::tr!("Ссылка скопирована", "Link copied")); }
                        if !s2.support_url.is_empty() && ui.button(crate::tr!("Поддержка", "Support")).clicked() { util::open_url(&s2.support_url); }
                        if !s2.web_page_url.is_empty() && ui.button(crate::tr!("Сайт провайдера", "Provider website")).clicked() { util::open_url(&s2.web_page_url); }
                        if ui.button(RichText::new(crate::tr!("Удалить", "Delete")).color(p.error)).clicked() { self.modal = Some(Modal::Confirm { title: crate::tr!("Удалить подписку?", "Delete the subscription?").into(), text: crate::trf!("«{}» и все её серверы будут удалены.", "«{}» and all its servers will be deleted.", s2.name), action: Confirmed::DeleteSubscription(id.clone()) }); }
                    });
                });
            });
        });
        ui.add_space(6.0);
    }

    fn server_row(&mut self, ui: &mut Ui, pr: &Profile, selected: bool, pinging: bool) {
        let p = self.pal;
        let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 62.0), Sense::click());
        let st = core::status();
        let live = st.profile_id == pr.id && st.state == State::Connected;
        let fill = if selected { p.secondary_container } else if response.hovered() { p.high } else { Color32::TRANSPARENT };
        ui.painter().rect_filled(rect, CornerRadius::same(16), fill);
        let badge = egui::Rect::from_center_size(egui::pos2(rect.left() + 34.0, rect.center().y), Vec2::splat(40.0));
        // A flag in the name fills the badge; the connected server gets a primary ring around it.
        let ring = if live { Stroke::new(2.5, p.primary) } else if selected { Stroke::new(2.0, p.primary_container) } else { Stroke::new(1.0, p.outline_variant) };
        let flag = super::flags::flag_code(&pr.name).is_some_and(|code| super::flags::paint(ui.painter(), &code, badge.center(), 20.0, Some(ring)));
        if !flag {
            ui.painter().circle_filled(badge.center(), 20.0, if live { p.primary } else if selected { p.primary_container } else { p.highest });
            ui.painter().text(badge.center(), Align2::CENTER_CENTER, w::badge_text(&pr.name), FontId::new(13.0, semibold()), if live { p.on_primary } else if selected { p.on_primary_container } else { p.text });
        }
        // Latency chip on the right.
        let (chip_text, chip_color) = if pinging { ("…".to_string(), p.muted) } else if pr.latency > 0 { (crate::trf!("{} мс", "{} ms", pr.latency), if pr.latency < 300 { p.good } else if pr.latency < 800 { p.warn } else { p.error }) } else if !pr.ping_error.is_empty() { (crate::pinger::short_error(&pr.ping_error).chars().take(22).collect(), p.error) } else { (String::new(), p.muted) };
        let mut right = rect.right() - 14.0;
        if !chip_text.is_empty() {
            let g = ui.painter().layout_no_wrap(chip_text, FontId::new(12.5, semibold()), chip_color);
            let chip = egui::Rect::from_min_size(egui::pos2(right - g.size().x - 18.0, rect.center().y - 13.0), Vec2::new(g.size().x + 18.0, 26.0));
            ui.painter().rect_filled(chip, CornerRadius::same(8), chip_color.gamma_multiply(0.14));
            ui.painter().galley(chip.center() - g.size() / 2.0, g, chip_color);
            right = chip.left() - 8.0;
        }
        if pr.favorite { ui.painter().text(egui::pos2(right - 8.0, rect.center().y), Align2::CENTER_CENTER, icon::STAR, FontId::new(14.0, theme::icons()), p.warn); right -= 22.0; }
        let text_left = badge.right() + 14.0; let max_w = (right - text_left - 8.0).max(40.0);
        let name = ui.painter().layout(w::display_name(&pr.name), FontId::new(14.5, semibold()), if selected { p.on_secondary_container } else { p.text }, max_w);
        let name = truncate(ui, name, &w::display_name(&pr.name), FontId::new(14.5, semibold()), if selected { p.on_secondary_container } else { p.text }, max_w);
        let sub = truncate(ui, ui.painter().layout_no_wrap(String::new(), FontId::new(12.5, egui::FontFamily::Proportional), p.muted), &format!("{}{}", if live { crate::tr!("Подключено · ", "Connected · ") } else { "" }, pr.summary()), FontId::new(12.5, egui::FontFamily::Proportional), if live { p.primary } else { p.muted }, max_w);
        ui.painter().galley(egui::pos2(text_left, rect.center().y - 19.0), name, p.text);
        ui.painter().galley(egui::pos2(text_left, rect.center().y + 2.0), sub, p.muted);
        let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
        if response.double_clicked() { self.select(&pr.id, true); }
        else if response.clicked() { self.select(&pr.id, false); }
        let id = pr.id.clone(); let pr2 = pr.clone();
        response.context_menu(|ui| {
            ui.set_min_width(230.0);
            if ui.button(crate::tr!("Подключиться", "Connect")).clicked() { self.select(&id, true); ui.close(); }
            if ui.button(crate::tr!("Проверить задержку", "Check latency")).clicked() { self.ping(Some(vec![id.clone()])); ui.close(); }
            if ui.button(if pr2.favorite { crate::tr!("Убрать из избранного", "Remove from favourites") } else { crate::tr!("В избранное", "Add to favourites") }).clicked() { let i = id.clone(); store::update(|s| if let Some(x) = s.profiles.iter_mut().find(|x| x.id == i) { x.favorite = !x.favorite; }); ui.close(); }
            if ui.button(crate::tr!("Скопировать ссылку", "Copy link")).clicked() { ui.ctx().copy_text(pr2.source.clone()); self.toast(crate::tr!("Скопировано", "Copied")); ui.close(); }
            if ui.button(crate::tr!("Показать конфигурацию", "Show configuration")).clicked() { self.show_config(&pr2); ui.close(); }
            if !pr2.ping_error.is_empty() && ui.button(crate::tr!("Ошибка проверки", "Check error")).clicked() { self.modal = Some(Modal::Info { title: crate::tr!("Ошибка проверки", "Check error").into(), text: pr2.ping_error.clone(), copy: true }); ui.close(); }
            if ui.button(RichText::new(crate::tr!("Удалить", "Delete")).color(p.error)).clicked() { self.modal = Some(Modal::Confirm { title: crate::tr!("Удалить сервер?", "Delete the server?").into(), text: crate::trf!("«{}» будет удалён.", "«{}» will be deleted.", w::display_name(&pr2.name)), action: Confirmed::DeleteProfile(id.clone()) }); ui.close(); }
        });
        ui.add_space(2.0);
    }

    // ------------------------------------------------------------------ settings

    fn settings(&mut self, ui: &mut Ui) {
        let p = self.pal;
        ScrollArea::vertical().id_salt("settings").auto_shrink([false, false]).show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.set_max_width(760.0);
                ui.with_layout(Layout::top_down(Align::Min), |ui| {
                    ui.label(RichText::new(crate::tr!("Настройки", "Settings")).font(FontId::new(26.0, semibold())));
                    w::section_title(ui, &p, crate::tr!("Подключение", "Connection"));
                    w::group(ui, &p, |ui| {
                        self.pick_row(ui, crate::tr!("Режим", "Mode"), "", "tunnelMode", &[("both", crate::tr!("Прокси + TUN", "Proxy + TUN")), ("proxy", crate::tr!("Системный прокси", "System proxy")), ("tun", crate::tr!("Только TUN", "TUN only"))]);
                        self.pick_row(ui, crate::tr!("Маршрутизация", "Routing"), "", "mode", &[("rules", crate::tr!("По правилам", "By rules")), ("global", crate::tr!("Всё через VPN", "Everything through VPN")), ("direct", crate::tr!("Всё напрямую", "Everything direct"))]);
                        self.bool_row(ui, crate::tr!("Быстрый сервер автоматически", "Fastest server automatically"), "", "autoSelect");
                        self.bool_row(ui, crate::tr!("Не трогать локальную сеть", "Keep the local network direct"), "", "bypassLan");
                        self.ip_row(ui);
                        self.bool_row(ui, crate::tr!("Определять домены (sniffing)", "Detect domains (sniffing)"), "", "sniff");
                        self.value_row(ui, crate::tr!("MTU туннеля", "Tunnel MTU"), "", "mtu", true);
                    });
                    w::section_title(ui, &p, "DNS");
                    w::group(ui, &p, |ui| {
                        self.bool_row(ui, crate::tr!("DNS из JSON", "DNS from JSON"), "", "dnsFromJson");
                        self.bool_row(ui, crate::tr!("Свой DNS туннеля", "Own tunnel DNS"), "", "customDns");
                        if store::get().b("customDns") { self.value_row(ui, crate::tr!("Адрес DNS", "DNS address"), "", "tunnelDns", false); }
                    });
                    w::section_title(ui, &p, crate::tr!("Локальный прокси", "Local proxy"));
                    w::group(ui, &p, |ui| {
                        self.value_row(ui, crate::tr!("Порт SOCKS5", "SOCKS5 port"), "", "proxyPort", true);
                        self.value_row(ui, crate::tr!("Порт HTTP", "HTTP port"), "", "httpProxyPort", true);
                        self.bool_row(ui, crate::tr!("Доступ из локальной сети", "Access from the local network"), "", "allowLan");
                        if store::get().b("allowLan") { self.value_row(ui, crate::tr!("Пароль прокси", "Proxy password"), "", "proxyPassword", false); }
                    });
                    w::section_title(ui, &p, crate::tr!("Обход блокировок", "Censorship bypass"));
                    w::group(ui, &p, |ui| {
                        self.bool_row(ui, crate::tr!("Фрагментация TLS", "TLS fragmentation"), "", "fragment");
                        if store::get().b("fragment") {
                            self.value_row(ui, crate::tr!("Пакеты", "Packets"), "", "fragmentPackets", false);
                            self.value_row(ui, crate::tr!("Длина фрагмента", "Fragment length"), "", "fragmentLength", false);
                            self.value_row(ui, crate::tr!("Интервал", "Interval"), "", "fragmentInterval", false);
                        }
                        self.bool_row(ui, crate::tr!("Шум UDP", "UDP noise"), "", "noise");
                        if store::get().b("noise") {
                            self.pick_row(ui, crate::tr!("Тип шума", "Noise type"), "", "noiseType", &[("rand", crate::tr!("Случайные байты", "Random bytes")), ("array", crate::tr!("Байты (hex через запятую)", "Bytes (hex, comma separated)")), ("str", crate::tr!("Строка", "String")), ("hex", "Hex")]);
                            if store::get().s("noiseType") == "rand" { self.value_row(ui, crate::tr!("Размер", "Size"), "", "noiseRand", false); } else { self.value_row(ui, crate::tr!("Пакет", "Packet"), "", "noisePacket", false); }
                            self.value_row(ui, crate::tr!("Задержка", "Delay"), "", "noiseDelay", false);
                        }
                        self.bool_row(ui, crate::tr!("Мультиплексирование", "Multiplexing"), "", "mux");
                        if store::get().b("mux") { self.value_row(ui, crate::tr!("Потоков", "Streams"), "", "muxConnections", true); }
                    });
                    w::section_title(ui, &p, crate::tr!("Свои правила маршрутизации", "Own routing rules"));
                    self.rules(ui);
                    w::section_title(ui, &p, crate::tr!("Проверка задержки", "Latency check"));
                    w::group(ui, &p, |ui| {
                        self.pick_row(ui, crate::tr!("Способ", "Method"), "", "pingType", &[("proxyGet", crate::tr!("Через прокси (GET)", "Through the proxy (GET)")), ("proxyHead", crate::tr!("Через прокси (HEAD)", "Through the proxy (HEAD)")), ("tcp", crate::tr!("TCP-подключение", "TCP connection")), ("icmp", "ICMP ping")]);
                        if store::get().s("pingType").starts_with("proxy") { self.pick_row(ui, crate::tr!("Соединение при проверке", "Connection during the check"), "", "pingMode", &[("keepalive", "Keep-alive"), ("double", crate::tr!("Два новых соединения", "Two new connections"))]); }
                        self.value_row(ui, crate::tr!("Адрес проверки", "Check address"), "", "pingUrl", false);
                        self.pick_row(ui, crate::tr!("Тайм-аут", "Timeout"), "", "pingTimeout", &[("3000", crate::tr!("3 секунды", "3 seconds")), ("5000", crate::tr!("5 секунд", "5 seconds")), ("10000", crate::tr!("10 секунд", "10 seconds"))]);
                        self.pick_row(ui, crate::tr!("Сортировка серверов", "Server order"), "", "sort", &[("none", crate::tr!("Как у провайдера", "As the provider sent them")), ("ping", crate::tr!("По задержке", "By latency")), ("name", crate::tr!("По имени", "By name"))]);
                    });
                    w::section_title(ui, &p, crate::tr!("Подписки", "Subscriptions"));
                    w::group(ui, &p, |ui| {
                        self.bool_row(ui, crate::tr!("Обновлять при запуске", "Update on start"), "", "updateOnOpen");
                        self.bool_row(ui, crate::tr!("Отправлять HWID", "Send HWID"), "", "sendHwid");
                        self.value_row(ui, "User-Agent", "", "userAgent", false);
                    });
                    w::section_title(ui, &p, crate::tr!("Приложение", "App"));
                    w::group(ui, &p, |ui| {
                        self.pick_row(ui, crate::tr!("Тема", "Theme"), "", "theme", &[("system", crate::tr!("Как в Windows", "As in Windows")), ("light", crate::tr!("Светлая", "Light")), ("dark", crate::tr!("Тёмная", "Dark"))]);
                        self.bool_row(ui, crate::tr!("Цвет акцента Windows", "Windows accent colour"), "", "windowsAccent");
                        let mut autostart = store::get().b("autostart");
                        if w::switch_row(ui, &p, crate::tr!("Запускать вместе с Windows", "Start with Windows"), "", &mut autostart) { match crate::install::set_autostart(autostart) { Ok(()) => store::update(|s| s.set("autostart", autostart.into())), Err(e) => self.toast(e) } }
                        self.bool_row(ui, crate::tr!("Подключаться при запуске", "Connect on start"), "", "connectOnStart");
                        self.bool_row(ui, crate::tr!("Сворачивать в трей при закрытии", "Minimise to the tray on close"), "", "minimizeToTray");
                        self.bool_row(ui, crate::tr!("Проверять обновления", "Check for updates"), "", "checkUpdates");
                        self.bool_row(ui, crate::tr!("Подробный журнал ядра", "Detailed core log"), "", "debugLogs");
                    });
                    w::section_title(ui, &p, crate::tr!("О программе", "About"));
                    w::group(ui, &p, |ui| {
                        w::row(ui, &p, &format!("Wraith {}", env!("CARGO_PKG_VERSION")), "Xray", |ui| {
                            if w::button(ui, &p, Kind::Text, Some(icon::LINK), "GitHub").clicked() { util::open_url(&format!("https://github.com/{}", updater::REPO)); }
                        });
                        w::row(ui, &p, crate::tr!("Обновления", "Updates"), "", |ui| {
                            if w::button(ui, &p, Kind::Tonal, None, crate::tr!("Проверить", "Check")).clicked() {
                                let shared = self.shared.clone();
                                self.background(crate::tr!("Проверяем обновления…", "Checking for updates…"), move || { let r = updater::check()?; if r.available { let v = r.version.clone(); shared.lock().unwrap().release = Some(r); Ok(crate::trf!("Доступна версия {v}", "Version {v} is available")) } else { Ok(crate::trf!("Установлена последняя версия {}", "The latest version {} is installed", env!("CARGO_PKG_VERSION"))) } });
                            }
                        });
                        w::row(ui, &p, crate::tr!("Папка данных", "Data folder"), "", |ui| { if w::button(ui, &p, Kind::Text, Some(icon::FOLDER), crate::tr!("Открыть", "Open")).clicked() { util::open_path(&store::folder()); } });
                        w::row(ui, &p, crate::tr!("Сбросить настройки", "Reset settings"), "", |ui| { if w::button(ui, &p, Kind::Danger, None, crate::tr!("Сбросить", "Reset")).clicked() { self.modal = Some(Modal::Confirm { title: crate::tr!("Сбросить настройки?", "Reset the settings?").into(), text: crate::tr!("Все параметры вернутся к значениям по умолчанию.", "All options return to their defaults.").into(), action: Confirmed::Reset }); } });
                    });
                    ui.add_space(30.0);
                });
            });
        });
    }

    fn ip_row(&mut self, ui: &mut Ui) {
        let p = self.pal; let mut v = crate::model::ip_mode(&store::get().settings); let mut changed = false;
        w::row(ui, &p, crate::tr!("IP-адреса", "IP addresses"), "", |ui| changed = w::segmented(ui, &p, &mut v, &[("auto", crate::tr!("Авто", "Auto")), ("v4", "IPv4"), ("v6", "IPv6")], 228.0));
        if changed { store::update(|s| s.set("ipMode", v.clone().into())); self.after_change("ipMode"); }
    }
    fn bool_row(&mut self, ui: &mut Ui, title: &str, subtitle: &str, key: &str) {
        let mut v = store::get().b(key);
        if w::switch_row(ui, &self.pal, title, subtitle, &mut v) {
            let k = key.to_string();
            store::update(|s| {
                s.set(&k, v.into());
                // Provider DNS and own DNS are alternatives: turning one on turns the other off.
                if v && k == "dnsFromJson" { s.set("customDns", false.into()); }
                if v && k == "customDns" { s.set("dnsFromJson", false.into()); }
            });
            self.after_change(key);
        }
    }
    fn pick_row(&mut self, ui: &mut Ui, title: &str, subtitle: &str, key: &str, options: &[(&str, &str)]) {
        let mut v = store::get().s(key); let mut changed = false;
        w::row(ui, &self.pal, title, subtitle, |ui| changed = w::picker(ui, key, &mut v, options));
        if changed {
            let value: Value = if let Ok(n) = v.parse::<i64>() { n.into() } else { v.clone().into() };
            let k = key.to_string(); store::update(|s| s.set(&k, value)); self.after_change(key);
        }
    }
    fn value_row(&mut self, ui: &mut Ui, title: &str, subtitle: &str, key: &str, numeric: bool) {
        let p = self.pal; let v = store::get().s(key);
        let shown = if key == "proxyPassword" && !v.is_empty() { "••••••••".to_string() } else if v.is_empty() { crate::tr!("Не задано", "Not set").into() } else { v.clone() };
        let mut open = false;
        w::row(ui, &p, title, subtitle, |ui| {
            let short: String = shown.chars().take(30).collect();
            if w::button(ui, &p, Kind::Outlined, Some(icon::EDIT), &short).clicked() { open = true; }
        });
        if open { self.modal = Some(Modal::Value { key: key.into(), title: title.into(), hint: subtitle.into(), value: v, numeric }); }
    }
    fn after_change(&mut self, key: &str) {
        let tunnel = ["tunnelMode", "mode", "autoSelect", "bypassLan", "ipMode", "sniff", "mtu", "dnsFromJson", "customDns", "tunnelDns", "proxyPort", "httpProxyPort", "allowLan", "proxyPassword", "fragment", "fragmentPackets", "fragmentLength", "fragmentInterval", "noise", "noiseType", "noisePacket", "noiseRand", "noiseDelay", "mux", "muxConnections", "debugLogs"];
        if tunnel.contains(&key) && core::status().state == State::Connected { self.toast(crate::tr!("Переподключаемся с новыми настройками…", "Reconnecting with the new settings…")); self.connect_selected(); }
        self.revision = u64::MAX;
    }

    fn rules(&mut self, ui: &mut Ui) {
        let p = self.pal; let rules = store::get().rules.clone();
        w::group(ui, &p, |ui| {
            if rules.is_empty() { w::row(ui, &p, crate::tr!("Правил пока нет", "No rules yet"), "", |_| {}); }
            for (i, rule) in rules.iter().enumerate() {
                let action = match rule.s("action").as_str() { "direct" => crate::tr!("Напрямую", "Direct"), "block" => crate::tr!("Блокировать", "Block"), _ => crate::tr!("Через VPN", "Through VPN") };
                let values = crate::model::split_values(&rule.s("value"));
                let title = if rule.s("name").is_empty() { crate::model::rule_kind_label(&rule.s("kind")).to_string() } else { rule.s("name") };
                let preview = format!("{} · {}{}", crate::model::rule_kind_label(&rule.s("kind")), values.iter().take(3).cloned().collect::<Vec<_>>().join(", "), if values.len() > 3 { crate::trf!(" и ещё {}", " and {} more", values.len() - 3) } else { String::new() });
                let mut edit = false; let mut enabled = rule.b("enabled", true);
                w::row(ui, &p, &title, &preview, |ui| {
                    if w::switch(ui, &p, &mut enabled).changed() { store::update(|s| if let Some(r) = s.rules.get_mut(i) { r.insert("enabled".into(), enabled.into()); }); }
                    if w::icon_button(ui, &p, icon::EDIT, crate::tr!("Изменить", "Edit"), false).clicked() { edit = true; }
                    w::chip(ui, &p, action, p.secondary_container, p.on_secondary_container);
                });
                if edit { self.modal = Some(Modal::Rule { index: Some(i), rule: rule.clone() }); }
            }
            w::row(ui, &p, "", "", |ui| { if w::button(ui, &p, Kind::Tonal, Some(icon::ADD), crate::tr!("Добавить правило", "Add rule")).clicked() { self.modal = Some(Modal::Rule { index: None, rule: crate::obj! {"name" => "", "kind" => "domain_suffix", "value" => "", "action" => "direct", "enabled" => true} }); } });
        });
    }

    fn logs(&mut self, ui: &mut Ui) {
        let p = self.pal;
        ui.horizontal(|ui| {
            ui.label(RichText::new(crate::tr!("Журнал", "Log")).font(FontId::new(26.0, semibold())));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if w::button(ui, &p, Kind::Text, Some(icon::FOLDER), crate::tr!("Файл", "File")).clicked() { util::open_path(&crate::log::path()); }
                if w::button(ui, &p, Kind::Text, Some(icon::CLEAR), crate::tr!("Очистить", "Clear")).clicked() { crate::log::clear(); }
                if w::button(ui, &p, Kind::Tonal, Some(icon::COPY), crate::tr!("Копировать", "Copy")).clicked() { ui.ctx().copy_text(crate::log::text()); self.toast(crate::tr!("Журнал скопирован", "Log copied")); }
            });
        });
        ui.add_space(8.0);
        let version = crate::log::version();
        if self.log_cache.0 != version { self.log_cache = (version, crate::log::text()); }
        w::card(&p, p.lowest).inner_margin(Margin::same(14)).show(ui, |ui| {
            ScrollArea::both().id_salt("log").auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
                let mut text = self.log_cache.1.as_str();
                ui.add(egui::TextEdit::multiline(&mut text).font(egui::TextStyle::Monospace).desired_width(f32::INFINITY).frame(false));
            });
        });
    }

    // ------------------------------------------------------------------ dialogs

    fn modals(&mut self, ctx: &egui::Context) {
        let Some(modal) = self.modal.take() else { return };
        let p = self.pal;
        let mut keep = true; let mut next: Option<Modal> = None;
        let mut modal = modal;
        let response = egui::Modal::new(egui::Id::new("modal")).frame(Frame::new().fill(p.container).corner_radius(CornerRadius::same(28)).inner_margin(Margin::same(24))).show(ctx, |ui| {
            ui.set_width(560.0);
            match &mut modal {
                Modal::Add { text } => {
                    ui.label(RichText::new(crate::tr!("Добавить сервер или подписку", "Add a server or subscription")).font(FontId::new(20.0, semibold())));
                    ui.add_space(8.0);
                    ScrollArea::vertical().max_height(260.0).show(ui, |ui| { ui.add(egui::TextEdit::multiline(text).desired_rows(8).desired_width(f32::INFINITY).hint_text(crate::tr!("Вставьте сюда…", "Paste here…")).font(egui::TextStyle::Monospace)); });
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        if w::button(ui, &p, Kind::Text, Some(icon::PASTE), crate::tr!("Из буфера", "From clipboard")).clicked() { if let Some(t) = util::clipboard_text() { *text = t; } }
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if w::button(ui, &p, Kind::Filled, None, crate::tr!("Добавить", "Add")).clicked() && !text.trim().is_empty() { let t = text.clone(); keep = false; self.add(t); }
                            if w::button(ui, &p, Kind::Text, None, crate::tr!("Отмена", "Cancel")).clicked() { keep = false; }
                        });
                    });
                }
                Modal::Info { title, text, copy } => {
                    ui.label(RichText::new(title.as_str()).font(FontId::new(20.0, semibold())));
                    ui.add_space(8.0);
                    ScrollArea::both().max_height(460.0).show(ui, |ui| { let mut t = text.as_str(); ui.add(egui::TextEdit::multiline(&mut t).font(egui::TextStyle::Monospace).desired_width(f32::INFINITY).frame(false)); });
                    ui.add_space(12.0);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if w::button(ui, &p, Kind::Filled, None, crate::tr!("Закрыть", "Close")).clicked() { keep = false; }
                        if *copy && w::button(ui, &p, Kind::Text, Some(icon::COPY), crate::tr!("Копировать", "Copy")).clicked() { ui.ctx().copy_text(text.clone()); }
                    });
                }
                Modal::Confirm { title, text, action } => {
                    ui.label(RichText::new(title.as_str()).font(FontId::new(20.0, semibold())));
                    ui.label(RichText::new(text.as_str()).color(p.muted));
                    ui.add_space(14.0);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if w::button(ui, &p, Kind::Danger, None, crate::tr!("Да", "Yes")).clicked() {
                            keep = false;
                            match action {
                                Confirmed::DeleteProfile(id) => { let id = id.clone(); store::update(|s| { s.profiles.retain(|x| x.id != id); if s.selected == id { s.selected = s.profiles.first().map(|x| x.id.clone()).unwrap_or_default(); } }); }
                                Confirmed::DeleteSubscription(id) => { let id = id.clone(); store::update(|s| { s.subscriptions.retain(|x| x.id != id); s.profiles.retain(|x| x.group != id); if s.selected().is_none() { s.selected = s.profiles.first().map(|x| x.id.clone()).unwrap_or_default(); } }); }
                                Confirmed::Reset => { store::update(|s| { let keep = (s.s("hwid"), s.s("clientNumericId")); s.settings = crate::model::defaults(); s.set("hwid", keep.0.into()); s.set("clientNumericId", keep.1.into()); }); let _ = crate::install::set_autostart(false); }
                            }
                        }
                        if w::button(ui, &p, Kind::Text, None, crate::tr!("Отмена", "Cancel")).clicked() { keep = false; }
                    });
                }
                Modal::Admin => {
                    ui.label(RichText::new(crate::tr!("Нужны права администратора", "Administrator rights needed")).font(FontId::new(20.0, semibold())));
                    ui.label(RichText::new(crate::tr!("Режим TUN создаёт сетевой адаптер и меняет маршруты Windows. Перезапустите Wraith от имени администратора или подключитесь через системный прокси.", "TUN mode creates a network adapter and changes Windows routes. Restart Wraith as administrator or connect through the system proxy.")).color(p.muted));
                    ui.add_space(14.0);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if w::button(ui, &p, Kind::Filled, Some(icon::SHIELD), crate::tr!("Перезапустить", "Restart")).clicked() {
                            keep = false;
                            if util::relaunch_elevated("--connect --elevated") { core::shutdown(); std::process::exit(0); } else { self.toast(crate::tr!("Запуск от администратора отменён", "Start as administrator was cancelled")); }
                        }
                        if w::button(ui, &p, Kind::Text, None, crate::tr!("Системный прокси", "System proxy")).clicked() { keep = false; store::update(|s| s.set("tunnelMode", "proxy".into())); self.connect_selected(); }
                    });
                }
                Modal::Rule { index, rule } => {
                    ui.label(RichText::new(if index.is_some() { crate::tr!("Правило", "Rule") } else { crate::tr!("Новое правило", "New rule") }).font(FontId::new(20.0, semibold())));
                    ui.add_space(8.0);
                    let mut name = rule.s("name"); let mut kind = rule.s("kind"); let mut value = rule.s("value"); let mut action = rule.s_or("action", "direct");
                    ui.label(RichText::new(crate::tr!("Название", "Name")).size(12.5).color(p.muted)); ui.add(egui::TextEdit::singleline(&mut name).desired_width(f32::INFINITY).hint_text(crate::tr!("Например: Банки", "For example: Banks")));
                    ui.label(RichText::new(crate::tr!("Что сравнивать", "Match by")).size(12.5).color(p.muted)); w::picker(ui, "rule-kind", &mut kind, &crate::model::rule_kinds());
                    ui.label(RichText::new(crate::tr!("Значения — по одному в строке", "Values, one per line")).size(12.5).color(p.muted));
                    let hint = match kind.as_str() { "domain_suffix" => "sberbank.ru\ngosuslugi.ru", "ip_cidr" => "10.0.0.0/8", "geosite" => "category-ads-all", "geoip" => "ru", "process" => "telegram.exe", _ => "" };
                    ui.add(egui::TextEdit::multiline(&mut value).desired_rows(5).desired_width(f32::INFINITY).hint_text(hint));
                    ui.label(RichText::new(crate::tr!("Действие", "Action")).size(12.5).color(p.muted));
                    w::segmented(ui, &p, &mut action, &[("direct", crate::tr!("Напрямую", "Direct")), ("proxy", crate::tr!("Через VPN", "Through VPN")), ("block", crate::tr!("Блокировать", "Block"))], ui.available_width());
                    rule.insert("name".into(), name.into()); rule.insert("kind".into(), kind.into()); rule.insert("value".into(), value.clone().into()); rule.insert("action".into(), action.into());
                    ui.add_space(14.0);
                    ui.horizontal(|ui| {
                        if let Some(i) = *index { if w::button(ui, &p, Kind::Danger, Some(icon::DELETE), crate::tr!("Удалить", "Delete")).clicked() { keep = false; store::update(|s| { if i < s.rules.len() { s.rules.remove(i); } }); self.after_change("mode"); } }
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if w::button(ui, &p, Kind::Filled, None, crate::tr!("Сохранить", "Save")).clicked() && !value.trim().is_empty() {
                                keep = false; let r = rule.clone(); let i = *index;
                                store::update(|s| match i { Some(i) if i < s.rules.len() => s.rules[i] = r, _ => s.rules.push(r) });
                                self.after_change("mode");
                            }
                            if w::button(ui, &p, Kind::Text, None, crate::tr!("Отмена", "Cancel")).clicked() { keep = false; }
                        });
                    });
                }
                Modal::Sub { sub } => {
                    ui.label(RichText::new(crate::tr!("Подписка", "Subscription")).font(FontId::new(20.0, semibold())));
                    ui.add_space(8.0);
                    ui.label(RichText::new(crate::tr!("Название", "Name")).size(12.5).color(p.muted)); ui.add(egui::TextEdit::singleline(&mut sub.name).desired_width(f32::INFINITY));
                    ui.label(RichText::new(crate::tr!("Адрес", "Address")).size(12.5).color(p.muted)); ui.add(egui::TextEdit::singleline(&mut sub.url).desired_width(f32::INFINITY));
                    let mut interval = sub.interval_hours.to_string();
                    ui.label(RichText::new(crate::tr!("Интервал обновления, часов", "Update interval, hours")).size(12.5).color(p.muted)); ui.add(egui::TextEdit::singleline(&mut interval).desired_width(120.0));
                    if let Ok(h) = interval.trim().parse::<i64>() { sub.interval_hours = h.clamp(1, 168); }
                    ui.add_space(14.0);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if w::button(ui, &p, Kind::Filled, None, crate::tr!("Сохранить", "Save")).clicked() {
                            if !sub.url.starts_with("https://") { self.toast(crate::tr!("Адрес подписки должен начинаться с https://", "The subscription address must start with https://")); }
                            else { keep = false; let s2 = sub.clone(); store::update(|s| if let Some(x) = s.subscriptions.iter_mut().find(|x| x.id == s2.id) { x.name = s2.name.clone(); x.url = s2.url.clone(); x.interval_hours = s2.interval_hours; }); }
                        }
                        if w::button(ui, &p, Kind::Text, None, crate::tr!("Отмена", "Cancel")).clicked() { keep = false; }
                    });
                }
                Modal::Update { release } => {
                    ui.label(RichText::new(format!("Wraith {}", release.version)).font(FontId::new(20.0, semibold())));
                    ui.add_space(6.0);
                    ScrollArea::vertical().max_height(260.0).show(ui, |ui| { ui.label(RichText::new(&release.notes).color(p.muted)); });
                    let progress = self.shared.lock().unwrap().download;
                    if let Some((done, total)) = progress {
                        ui.add_space(8.0);
                        ui.add(egui::ProgressBar::new(if total > 0 { done as f32 / total as f32 } else { 0.0 }).text(crate::trf!("{} из {}", "{} of {}", util::bytes(done as i64), util::bytes(total as i64))));
                    }
                    ui.add_space(14.0);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if progress.is_none() && w::button(ui, &p, Kind::Filled, Some(icon::DOWNLOAD), crate::tr!("Скачать и установить", "Download and install")).clicked() {
                            let r = release.clone(); let shared = self.shared.clone();
                            shared.lock().unwrap().download = Some((0, r.size.max(0) as u64));
                            std::thread::spawn(move || {
                                let sh = shared.clone();
                                let result = updater::download(&r, move |d, t| { sh.lock().unwrap().download = Some((d, t)); super::repaint(); });
                                match result.and_then(|zip| updater::install_and_restart(&zip)) {
                                    Ok(()) => { core::shutdown(); std::process::exit(0); }
                                    Err(e) => { let mut s = shared.lock().unwrap(); s.download = None; s.toast = Some((e, Instant::now())); super::repaint(); }
                                }
                            });
                        }
                        if w::button(ui, &p, Kind::Text, None, crate::tr!("Позже", "Later")).clicked() { keep = false; }
                    });
                }
                Modal::Value { key, title, hint, value, numeric } => {
                    ui.label(RichText::new(title.as_str()).font(FontId::new(20.0, semibold())));
                    if !hint.is_empty() { ui.label(RichText::new(hint.as_str()).color(p.muted)); }
                    ui.add_space(8.0);
                    let edit = ui.add(egui::TextEdit::singleline(value).desired_width(f32::INFINITY).password(key == "proxyPassword").margin(Margin::symmetric(12, 10)));
                    if !edit.has_focus() && ui.memory(|m| m.focused().is_none()) { edit.request_focus(); }
                    let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    ui.add_space(14.0);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if w::button(ui, &p, Kind::Filled, None, crate::tr!("Сохранить", "Save")).clicked() || enter {
                            let v = value.trim().to_string();
                            let parsed: Result<Value, String> = if *numeric { v.parse::<i64>().map(Value::from).map_err(|_| crate::tr!("Введите число", "Enter a number").into()) } else { Ok(v.clone().into()) };
                            let valid = match key.as_str() {
                                "proxyPort" | "httpProxyPort" => v.parse::<u16>().map(|n| n >= 1024).unwrap_or(false),
                                "mtu" => v.parse::<i64>().map(|n| (1280..=9000).contains(&n)).unwrap_or(false),
                                "pingUrl" => v.starts_with("https://") || v.starts_with("http://"),
                                "proxyPassword" => v.is_empty() || v.chars().count() >= 8,
                                _ => true,
                            };
                            match parsed { Ok(val) if valid => { keep = false; let k = key.clone(); store::update(|s| s.set(&k, val)); self.after_change(&k.clone()); } _ => self.toast(crate::tr!("Некорректное значение", "Invalid value")) }
                        }
                        if w::button(ui, &p, Kind::Text, None, crate::tr!("Отмена", "Cancel")).clicked() { keep = false; }
                    });
                }
            }
        });
        if response.should_close() && !matches!(modal, Modal::Update { .. }) { keep = false; }
        if keep && self.modal.is_none() { self.modal = Some(modal); }
        if let Some(n) = next.take() { self.modal = Some(n); }
    }

    fn snackbar(&mut self, ctx: &egui::Context) {
        let p = self.pal;
        let (busy, toast) = { let s = self.shared.lock().unwrap(); (s.busy.clone(), s.toast.clone()) };
        let text = match (&busy, &toast) {
            (Some(b), _) => Some(b.clone()),
            (None, Some((t, at))) if at.elapsed() < Duration::from_secs(4) => Some(t.clone()),
            (None, Some(_)) => { self.shared.lock().unwrap().toast = None; None }
            _ => None,
        };
        let Some(text) = text else { return };
        egui::Area::new(egui::Id::new("snackbar")).anchor(Align2::CENTER_BOTTOM, [44.0, -24.0]).order(egui::Order::Foreground).show(ctx, |ui| {
            Frame::new().fill(if p.dark { theme::LIGHT.highest } else { theme::DARK.highest }).corner_radius(CornerRadius::same(12)).inner_margin(Margin::symmetric(18, 12)).shadow(ctx.style().visuals.popup_shadow).show(ui, |ui| {
                ui.set_max_width(520.0);
                ui.horizontal(|ui| {
                    if busy.is_some() { ui.add(egui::Spinner::new().size(16.0).color(if p.dark { theme::LIGHT.primary } else { theme::DARK.primary })); }
                    ui.label(RichText::new(text).color(if p.dark { theme::LIGHT.text } else { theme::DARK.text }));
                });
            });
        });
        ctx.request_repaint_after(Duration::from_millis(500));
    }
}

fn truncate(ui: &Ui, _probe: Arc<egui::Galley>, text: &str, font: FontId, color: Color32, max_w: f32) -> Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_string(), font, color);
    job.wrap = egui::text::TextWrapping { max_width: max_w, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    ui.painter().layout_job(job)
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.frames += 1;
        // Windows sends no repaint when its colours change: look every two seconds.
        if self.theme_checked.elapsed() >= std::time::Duration::from_secs(2) { self.theme_checked = Instant::now(); self.refresh_theme(); }
        ctx.request_repaint_after(std::time::Duration::from_secs(2));
        if self.frames == 2 { super::window_ready(self.start_hidden); }
        self.refresh_cache();
        if ctx.input(|i| i.viewport().close_requested()) {
            if store::get().b("minimizeToTray") && super::tray_available() { ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose); super::hide_window(); }
            else { core::shutdown(); }
        }
        let p = self.pal;
        egui::SidePanel::left("rail").exact_width(96.0).resizable(false).show_separator_line(false).frame(Frame::new().fill(p.surface).inner_margin(Margin::symmetric(8, 0))).show(ctx, |ui| self.rail(ui));
        egui::CentralPanel::default().frame(Frame::new().fill(p.surface).inner_margin(Margin { left: 12, right: 28, top: 22, bottom: 0 })).show(ctx, |ui| {
            match self.page { Page::Home => self.home(ui), Page::Settings => self.settings(ui), Page::Logs => self.logs(ui) }
        });
        self.modals(ctx);
        self.snackbar(ctx);
        ctx.request_repaint_after(Duration::from_millis(if core::status().state == State::Connected { 1000 } else { 2000 }));
    }
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) { core::shutdown(); }
}

#[allow(dead_code)]
fn _json(_: Value) -> Value { json!(null) }
