//! Main window: navigation rail, Home (status + servers), Settings, Journal; dialogs and background tasks.

use super::theme::{self, icon, semibold, Palette};
use super::widgets::{self as w, Kind};
use crate::core::{self, State};
use crate::json::{Get, Obj};
use crate::model::{Profile, Subscription, RULE_KINDS};
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
    pal: Palette, theme_key: String,
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
        let theme_key = store::get().s("theme");
        let pal = theme::palette(&theme_key);
        theme::apply(&cc.egui_ctx, &pal);
        super::set_context(cc.egui_ctx.clone());
        let tray = super::tray::create();
        let page = match std::env::args().find_map(|a| a.strip_prefix("--page=").map(String::from)).as_deref() { Some("settings") => Page::Settings, Some("logs") => Page::Logs, _ => Page::Home };
        let app = App { pal, theme_key, page, search: String::new(), modal: None, shared: Arc::new(Mutex::new(Shared::default())), ping_cancel: None, profiles: vec![], subs: vec![], revision: u64::MAX, log_cache: (u64::MAX, String::new()), start_hidden, frames: 0, _tray: tray };
        let (update_on_open, check_updates, connect_on_start) = { let s = store::get(); (s.b("updateOnOpen"), s.b("checkUpdates"), s.b("connectOnStart")) };
        if update_on_open { app.background("Обновляем подписки…", || { crate::subs::update_all(true); Ok("Подписки обновлены".into()) }); }
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
        let key = s.s("theme"); drop(s);
        if key != self.theme_key { self.theme_key = key; self.pal = theme::palette(&self.theme_key); if let Some(ctx) = super::context() { theme::apply(&ctx, &self.pal); } }
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
        let Some(p) = store::get().selected().cloned() else { self.toast("Сначала добавьте и выберите сервер"); return };
        let preferred = store::get().settings.s_or("core", "xray");
        if (preferred == "singbox") != crate::importer::for_engine(&p, &preferred).is_singbox() {
            self.toast(if preferred == "singbox" { "Этот сервер работает только на Xray — sing-box такую конфигурацию не поддерживает" } else { "Этот сервер работает только на sing-box" });
        }
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
            self.background("Загружаем подписку…", move || { let sub = crate::subs::create(&payload)?; Ok(format!("Подписка «{}» добавлена", sub.name)) });
            return;
        }
        match crate::importer::parse(&payload) {
            Ok(result) => {
                let n = result.profiles.len();
                store::update(|s| { let first = result.profiles[0].id.clone(); s.profiles.extend(result.profiles); if s.selected().is_none() { s.selected = first; } });
                self.modal = None;
                let skipped = if result.skipped.is_empty() { String::new() } else { format!(", пропущено {}", result.skipped.len()) };
                self.toast(format!("Добавлено {n} {}{skipped}", util::plural(n as i64, "сервер", "сервера", "серверов")));
            }
            Err(e) => self.toast(e),
        }
    }

    fn show_config(&mut self, p: &Profile) {
        let state = store::get().clone();
        let p = &crate::importer::for_engine(p, &state.settings.s_or("core", "xray"));
        let dns = crate::routing::system_resolvers();
        let result = if p.is_singbox() { crate::singbox::build(&state, p, &dns, 0, "••••") } else { crate::xray::build(&state, p, &dns, 0).map(|b| b.config) };
        self.modal = Some(match result {
            Ok(c) => Modal::Info { title: format!("Конфигурация · {}", if p.is_singbox() { "sing-box" } else { "Xray" }), text: serde_json::to_string_pretty(&c).unwrap_or_default(), copy: true },
            Err(e) => Modal::Info { title: "Конфигурация не собрана".into(), text: e, copy: false },
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
            for (page, glyph, label) in [(Page::Home, icon::HOME, "Главная"), (Page::Settings, icon::SETTINGS, "Настройки"), (Page::Logs, icon::JOURNAL, "Журнал")] {
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
                    ui.label(RichText::new(format!("Доступна версия {}", r.version)).font(FontId::new(14.0, semibold())));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| if w::button(ui, &p, Kind::Text, None, "Обновить").clicked() { self.modal = Some(Modal::Update { release: r.clone() }); });
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
                let (title, color) = match st.state { State::Connected => ("Подключено", p.primary), State::Connecting => ("Подключение…", p.text), State::Stopping => ("Отключение…", p.text), State::Off => ("Не подключено", p.text) };
                ui.label(RichText::new(title).font(FontId::new(24.0, semibold())).color(color));
                let selected = store::get().selected().cloned();
                match (&st.state, &selected) {
                    (State::Connected | State::Connecting, _) if !st.profile_name.is_empty() => { ui.label(RichText::new(w::display_name(&st.profile_name)).color(p.muted)); }
                    (_, Some(s)) => { ui.label(RichText::new(w::display_name(&s.name)).color(p.muted)); }
                    _ => { ui.label(RichText::new("Добавьте сервер, чтобы подключиться").color(p.muted)); }
                }
                if st.state == State::Connected {
                    ui.add_space(10.0);
                    let secs = st.since.map(|s| s.elapsed().as_secs() as i64).unwrap_or(0);
                    ui.horizontal(|ui| {
                        let total = 3.0 * 8.0 + 3.0 * 96.0; ui.add_space(((ui.available_width() - total) / 2.0).max(0.0));
                        for (glyph, text) in [(icon::CLOCK, util::duration(secs)), (icon::DOWNLOAD, format!("{}/с", util::bytes(st.rx_rate))), (icon::UPLOAD, format!("{}/с", util::bytes(st.tx_rate)))] {
                            Frame::new().fill(p.container).corner_radius(CornerRadius::same(10)).inner_margin(Margin::symmetric(8, 5)).show(ui, |ui| {
                                ui.set_min_width(80.0);
                                ui.horizontal(|ui| { ui.label(w::icon_text(glyph, 12.0, p.muted)); ui.label(RichText::new(text).size(12.5).color(p.text)); });
                            });
                        }
                    });
                    ui.label(RichText::new(format!("{} · {} · всего ↓ {} ↑ {}", st.engine, match store::get().s("tunnelMode").as_str() { "both" => "TUN + прокси", "tun" => "TUN", _ => "системный прокси" }, util::bytes(st.rx), util::bytes(st.tx))).size(12.0).color(p.muted));
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
            ui.label(RichText::new("Подключение").size(12.5).color(p.muted));
            if w::toggles(ui, &p, &mut flags, &["Системный прокси", "TUN"], ui.available_width()) {
                if !flags[0] && !flags[1] { self.toast("Нужен хотя бы один режим"); }
                else {
                    let next = match flags { [true, true] => "both", [true, false] => "proxy", _ => "tun" };
                    store::update(|s| s.set("tunnelMode", next.into()));
                    if st.state == State::Connected { self.connect_selected(); }
                }
            }
            ui.add_space(10.0);
            let mut engine = store::get().settings.s_or("core", "xray");
            ui.label(RichText::new("Ядро").size(12.5).color(p.muted));
            if w::segmented(ui, &p, &mut engine, &[("xray", "Xray"), ("singbox", "sing-box")], ui.available_width()) {
                store::update(|s| s.set("core", engine.clone().into()));
                if st.state == State::Connected { self.connect_selected(); }
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
        response.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(if on { "Отключить" } else { "Подключить" })
    }

    fn servers(&mut self, ui: &mut Ui) {
        let p = self.pal;
        let pinging = !self.shared.lock().unwrap().pinging.is_empty();
        ui.horizontal(|ui| {
            ui.label(RichText::new("Серверы").font(FontId::new(24.0, semibold())));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if w::button(ui, &p, Kind::Filled, Some(icon::ADD), "Добавить").clicked() { self.modal = Some(Modal::Add { text: String::new() }); }
                if !self.subs.is_empty() && w::icon_button(ui, &p, icon::REFRESH, "Обновить подписки", false).clicked() {
                    self.background("Обновляем подписки…", || { crate::subs::update_all(false); Ok("Подписки обновлены".into()) });
                }
                if w::icon_button(ui, &p, if pinging { icon::CLOSE } else { icon::PULSE }, if pinging { "Остановить проверку" } else { "Проверить задержку" }, pinging).clicked() { self.ping(None); }
                ui.add(egui::TextEdit::singleline(&mut self.search).hint_text("Поиск").desired_width((ui.available_width() - 8.0).clamp(120.0, 260.0)).margin(Margin::symmetric(12, 9)));
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
                ui.label(RichText::new("Пока нет серверов").font(FontId::new(18.0, semibold())));
                ui.label(RichText::new("Вставьте ссылку подписки, vless:// / vmess:// / trojan:// / ss:// / hy2:// или JSON-конфигурацию Xray и sing-box.").color(p.muted));
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    ui.add_space((ui.available_width() - 330.0).max(0.0) / 2.0);
                    if w::button(ui, &p, Kind::Filled, Some(icon::PASTE), "Вставить из буфера").clicked() { if let Some(t) = util::clipboard_text() { self.add(t); } else { self.toast("Буфер обмена пуст"); } }
                    if w::button(ui, &p, Kind::Tonal, Some(icon::ADD), "Добавить").clicked() { self.modal = Some(Modal::Add { text: String::new() }); }
                });
                ui.add_space(30.0);
            });
        });
    }

    fn group_header(&mut self, ui: &mut Ui, sub: Option<&Subscription>, count: usize) {
        let p = self.pal;
        let Some(sub) = sub else {
            ui.add_space(6.0);
            ui.label(RichText::new(format!("Мои серверы · {count}")).font(FontId::new(13.5, semibold())).color(p.primary));
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
                    let mut info = vec![format!("{count} {}", util::plural(count as i64, "сервер", "сервера", "серверов"))];
                    if total > 0 { info.push(format!("{} из {}", util::bytes(used), util::bytes(total))); } else if used > 0 { info.push(util::bytes(used)); }
                    if expire > 0 { let days = (expire - util::unix_now()) / 86400; info.push(if days < 0 { "срок истёк".into() } else { format!("до {} · {} дн.", util::date_of(expire), days) }); }
                    if sub.updated > 0 { let mins = (util::unix_now() - sub.updated) / 60; info.push(if mins < 1 { "обновлено сейчас".into() } else if mins < 60 { format!("обновлено {mins} мин назад") } else if mins < 1440 { format!("обновлено {} ч назад", mins / 60) } else { format!("обновлено {} дн. назад", mins / 1440) }); }
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
                    let menu = w::icon_button(ui, &p, icon::MORE, "Подписка", false);
                    let id = sub.id.clone(); let s2 = sub.clone();
                    let busy = self.shared.lock().unwrap().busy.is_some();
                    let pinging = { let s = self.shared.lock().unwrap(); self.profiles.iter().any(|x| x.group == id && s.pinging.contains(&x.id)) };
                    if w::icon_button(ui, &p, if pinging { icon::CLOSE } else { icon::PULSE }, if pinging { "Остановить проверку" } else { "Проверить задержку серверов подписки" }, pinging).clicked() {
                        if pinging { self.ping(None); } else { let ids = self.profiles.iter().filter(|x| x.group == id).map(|x| x.id.clone()).collect(); self.ping(Some(ids)); }
                    }
                    if w::icon_button(ui, &p, icon::REFRESH, "Обновить подписку", false).clicked() && !busy {
                        let s3 = s2.clone(); self.background("Обновляем подписку…", move || { crate::subs::update(s3)?; Ok("Подписка обновлена".into()) });
                    }
                    egui::Popup::menu(&menu).show(|ui| {
                        ui.set_min_width(220.0);
                        if ui.button("Обновить").clicked() { let s3 = s2.clone(); self.background("Обновляем подписку…", move || { crate::subs::update(s3)?; Ok("Подписка обновлена".into()) }); }
                        if ui.button("Проверить задержку").clicked() { let ids = self.profiles.iter().filter(|p| p.group == id).map(|p| p.id.clone()).collect(); self.ping(Some(ids)); }
                        if ui.button("Изменить").clicked() { self.modal = Some(Modal::Sub { sub: s2.clone() }); }
                        if ui.button("Скопировать ссылку").clicked() { ui.ctx().copy_text(s2.url.clone()); self.toast("Ссылка скопирована"); }
                        if !s2.support_url.is_empty() && ui.button("Поддержка").clicked() { util::open_url(&s2.support_url); }
                        if !s2.web_page_url.is_empty() && ui.button("Сайт провайдера").clicked() { util::open_url(&s2.web_page_url); }
                        if ui.button(RichText::new("Удалить").color(p.error)).clicked() { self.modal = Some(Modal::Confirm { title: "Удалить подписку?".into(), text: format!("«{}» и все её серверы будут удалены.", s2.name), action: Confirmed::DeleteSubscription(id.clone()) }); }
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
        ui.painter().circle_filled(badge.center(), 20.0, if live { p.primary } else if selected { p.primary_container } else { p.highest });
        ui.painter().text(badge.center(), Align2::CENTER_CENTER, w::badge_text(&pr.name), FontId::new(13.0, semibold()), if live { p.on_primary } else if selected { p.on_primary_container } else { p.text });
        // Latency chip on the right.
        let (chip_text, chip_color) = if pinging { ("…".to_string(), p.muted) } else if pr.latency > 0 { (format!("{} мс", pr.latency), if pr.latency < 300 { p.good } else if pr.latency < 800 { p.warn } else { p.error }) } else if !pr.ping_error.is_empty() { (crate::pinger::short_error(&pr.ping_error).chars().take(22).collect(), p.error) } else { (String::new(), p.muted) };
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
        let sub = truncate(ui, ui.painter().layout_no_wrap(String::new(), FontId::new(12.5, egui::FontFamily::Proportional), p.muted), &format!("{}{}", if live { "Подключено · " } else { "" }, pr.summary()), FontId::new(12.5, egui::FontFamily::Proportional), if live { p.primary } else { p.muted }, max_w);
        ui.painter().galley(egui::pos2(text_left, rect.center().y - 19.0), name, p.text);
        ui.painter().galley(egui::pos2(text_left, rect.center().y + 2.0), sub, p.muted);
        let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
        if response.double_clicked() { self.select(&pr.id, true); }
        else if response.clicked() { self.select(&pr.id, false); }
        let id = pr.id.clone(); let pr2 = pr.clone();
        response.context_menu(|ui| {
            ui.set_min_width(230.0);
            if ui.button("Подключиться").clicked() { self.select(&id, true); ui.close(); }
            if ui.button("Проверить задержку").clicked() { self.ping(Some(vec![id.clone()])); ui.close(); }
            if ui.button(if pr2.favorite { "Убрать из избранного" } else { "В избранное" }).clicked() { let i = id.clone(); store::update(|s| if let Some(x) = s.profiles.iter_mut().find(|x| x.id == i) { x.favorite = !x.favorite; }); ui.close(); }
            if ui.button("Скопировать ссылку").clicked() { ui.ctx().copy_text(pr2.source.clone()); self.toast("Скопировано"); ui.close(); }
            if ui.button("Показать конфигурацию").clicked() { self.show_config(&pr2); ui.close(); }
            if !pr2.ping_error.is_empty() && ui.button("Ошибка проверки").clicked() { self.modal = Some(Modal::Info { title: "Ошибка проверки".into(), text: pr2.ping_error.clone(), copy: true }); ui.close(); }
            if ui.button(RichText::new("Удалить").color(p.error)).clicked() { self.modal = Some(Modal::Confirm { title: "Удалить сервер?".into(), text: format!("«{}» будет удалён.", w::display_name(&pr2.name)), action: Confirmed::DeleteProfile(id.clone()) }); ui.close(); }
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
                    ui.label(RichText::new("Настройки").font(FontId::new(26.0, semibold())));
                    w::section_title(ui, &p, "Подключение");
                    w::group(ui, &p, |ui| {
                        self.pick_row(ui, "Режим", "Системный прокси — без прав администратора; TUN — весь трафик компьютера.", "tunnelMode", &[("both", "Прокси + TUN"), ("proxy", "Системный прокси"), ("tun", "Только TUN")]);
                        self.pick_row(ui, "Маршрутизация", "", "mode", &[("rules", "По правилам"), ("global", "Всё через VPN"), ("direct", "Всё напрямую")]);
                        self.bool_row(ui, "Быстрый сервер автоматически", "Для конфигураций с несколькими серверами: Xray сам выбирает самый быстрый живой сервер.", "autoSelect");
                        self.bool_row(ui, "Не трогать локальную сеть", "Роутер, принтеры и локальные адреса — напрямую.", "bypassLan");
                        self.ip_row(ui);
                        self.bool_row(ui, "Определять домены (sniffing)", "Нужно для правил по доменам при подключении по IP.", "sniff");
                        self.value_row(ui, "MTU туннеля", "Только для TUN. Обычно 1500.", "mtu", true);
                    });
                    w::section_title(ui, &p, "DNS");
                    w::group(ui, &p, |ui| {
                        self.bool_row(ui, "DNS из JSON", "Использовать DNS, который прописал провайдер в конфигурации.", "dnsFromJson");
                        self.bool_row(ui, "Свой DNS туннеля", "Иначе используется DNS системы, как без VPN.", "customDns");
                        if store::get().b("customDns") { self.value_row(ui, "Адрес DNS", "https://…/dns-query, tls://…, IP-адрес", "tunnelDns", false); }
                    });
                    w::section_title(ui, &p, "Локальный прокси");
                    w::group(ui, &p, |ui| {
                        self.value_row(ui, "Порт SOCKS5", "127.0.0.1 — для программ, которые не видят системный прокси.", "proxyPort", true);
                        self.value_row(ui, "Порт HTTP", "Этот порт прописывается в системный прокси Windows.", "httpProxyPort", true);
                        self.bool_row(ui, "Доступ из локальной сети", "Другие устройства смогут ходить через этот компьютер. Нужен пароль.", "allowLan");
                        if store::get().b("allowLan") { self.value_row(ui, "Пароль прокси", "Не короче 8 символов. Логин: wraith.", "proxyPassword", false); }
                    });
                    w::section_title(ui, &p, "Обход блокировок");
                    w::group(ui, &p, |ui| {
                        self.bool_row(ui, "Фрагментация TLS", "Делит первый пакет TLS, чтобы DPI не увидел имя сайта.", "fragment");
                        if store::get().b("fragment") {
                            self.value_row(ui, "Пакеты", "tlshello или диапазон, например 1-3", "fragmentPackets", false);
                            self.value_row(ui, "Длина фрагмента", "Байты, например 50-100", "fragmentLength", false);
                            self.value_row(ui, "Интервал", "Миллисекунды, например 10-20", "fragmentInterval", false);
                        }
                        self.bool_row(ui, "Шум UDP", "Случайные пакеты перед QUIC/UDP-соединением.", "noise");
                        if store::get().b("noise") {
                            self.pick_row(ui, "Тип шума", "", "noiseType", &[("rand", "Случайные байты"), ("array", "Байты (hex через запятую)"), ("str", "Строка"), ("hex", "Hex")]);
                            if store::get().s("noiseType") == "rand" { self.value_row(ui, "Размер", "Байты, например 10-20", "noiseRand", false); } else { self.value_row(ui, "Пакет", "", "noisePacket", false); }
                            self.value_row(ui, "Задержка", "Миллисекунды, например 10-16", "noiseDelay", false);
                        }
                        self.bool_row(ui, "Мультиплексирование", "Несколько соединений в одном. Не применяется к XTLS Vision.", "mux");
                        if store::get().b("mux") { self.value_row(ui, "Потоков", "1–1024", "muxConnections", true); }
                    });
                    w::section_title(ui, &p, "Свои правила маршрутизации");
                    self.rules(ui);
                    w::section_title(ui, &p, "Проверка задержки");
                    w::group(ui, &p, |ui| {
                        self.pick_row(ui, "Способ", "Через прокси — честная проверка: запрос идёт через сам сервер.", "pingType", &[("proxyGet", "Через прокси (GET)"), ("proxyHead", "Через прокси (HEAD)"), ("tcp", "TCP-подключение"), ("icmp", "ICMP ping")]);
                        if store::get().s("pingType").starts_with("proxy") { self.pick_row(ui, "Соединение при проверке", "Keep-alive: второй запрос идёт по уже открытому соединению — показывает задержку без рукопожатия. Два соединения: каждый запрос с нуля.", "pingMode", &[("keepalive", "Keep-alive"), ("double", "Два новых соединения")]); }
                        self.value_row(ui, "Адрес проверки", "", "pingUrl", false);
                        self.pick_row(ui, "Тайм-аут", "", "pingTimeout", &[("3000", "3 секунды"), ("5000", "5 секунд"), ("10000", "10 секунд")]);
                        self.pick_row(ui, "Сортировка серверов", "", "sort", &[("none", "Как у провайдера"), ("ping", "По задержке"), ("name", "По имени")]);
                    });
                    w::section_title(ui, &p, "Подписки");
                    w::group(ui, &p, |ui| {
                        self.bool_row(ui, "Обновлять при запуске", "Только подписки, у которых истёк интервал обновления.", "updateOnOpen");
                        self.bool_row(ui, "Отправлять HWID", "Некоторые провайдеры ограничивают число устройств по этому идентификатору.", "sendHwid");
                        self.value_row(ui, "User-Agent", "Пусто — как Happ, чтобы получать полные конфигурации.", "userAgent", false);
                    });
                    w::section_title(ui, &p, "Приложение");
                    w::group(ui, &p, |ui| {
                        self.pick_row(ui, "Тема", "", "theme", &[("system", "Как в Windows"), ("light", "Светлая"), ("dark", "Тёмная")]);
                        let mut autostart = store::get().b("autostart");
                        if w::switch_row(ui, &p, "Запускать вместе с Windows", "Wraith стартует свёрнутым в трей.", &mut autostart) { match crate::install::set_autostart(autostart) { Ok(()) => store::update(|s| s.set("autostart", autostart.into())), Err(e) => self.toast(e) } }
                        self.bool_row(ui, "Подключаться при запуске", "К выбранному серверу.", "connectOnStart");
                        self.bool_row(ui, "Сворачивать в трей при закрытии", "Крестик прячет окно, VPN продолжает работать.", "minimizeToTray");
                        self.bool_row(ui, "Проверять обновления", "Только запрос к GitHub Releases, без данных об устройстве.", "checkUpdates");
                        self.bool_row(ui, "Подробный журнал ядра", "", "debugLogs");
                    });
                    w::section_title(ui, &p, "О программе");
                    w::group(ui, &p, |ui| {
                        w::row(ui, &p, &format!("Wraith {}", env!("CARGO_PKG_VERSION")), "Xray + sing-box · открытый код, GPL-3.0", |ui| {
                            if w::button(ui, &p, Kind::Text, Some(icon::LINK), "GitHub").clicked() { util::open_url(&format!("https://github.com/{}", updater::REPO)); }
                        });
                        w::row(ui, &p, "Обновления", "Скачиваются только с GitHub и проверяются по SHA-256.", |ui| {
                            if w::button(ui, &p, Kind::Tonal, None, "Проверить").clicked() {
                                let shared = self.shared.clone();
                                self.background("Проверяем обновления…", move || { let r = updater::check()?; if r.available { let v = r.version.clone(); shared.lock().unwrap().release = Some(r); Ok(format!("Доступна версия {v}")) } else { Ok(format!("Установлена последняя версия {}", env!("CARGO_PKG_VERSION"))) } });
                            }
                        });
                        w::row(ui, &p, "Папка данных", "Настройки зашифрованы для вашей учётной записи Windows.", |ui| { if w::button(ui, &p, Kind::Text, Some(icon::FOLDER), "Открыть").clicked() { util::open_path(&store::folder()); } });
                        w::row(ui, &p, "Сбросить настройки", "Серверы и подписки останутся.", |ui| { if w::button(ui, &p, Kind::Danger, None, "Сбросить").clicked() { self.modal = Some(Modal::Confirm { title: "Сбросить настройки?".into(), text: "Все параметры вернутся к значениям по умолчанию.".into(), action: Confirmed::Reset }); } });
                    });
                    ui.add_space(30.0);
                });
            });
        });
    }

    fn ip_row(&mut self, ui: &mut Ui) {
        let p = self.pal; let mut v = crate::model::ip_mode(&store::get().settings); let mut changed = false;
        w::row(ui, &p, "IP-адреса", "Авто — IPv4 и IPv6; IPv4 — если сеть или провайдер без IPv6; IPv6 — только IPv6.", |ui| changed = w::segmented(ui, &p, &mut v, &[("auto", "Авто"), ("v4", "IPv4"), ("v6", "IPv6")], 228.0));
        if changed { store::update(|s| s.set("ipMode", v.clone().into())); self.after_change("ipMode"); }
    }
    fn bool_row(&mut self, ui: &mut Ui, title: &str, subtitle: &str, key: &str) {
        let mut v = store::get().b(key);
        if w::switch_row(ui, &self.pal, title, subtitle, &mut v) { let k = key.to_string(); store::update(|s| s.set(&k, v.into())); self.after_change(key); }
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
        let shown = if key == "proxyPassword" && !v.is_empty() { "••••••••".to_string() } else if v.is_empty() { "Не задано".into() } else { v.clone() };
        let mut open = false;
        w::row(ui, &p, title, subtitle, |ui| {
            let short: String = shown.chars().take(30).collect();
            if w::button(ui, &p, Kind::Outlined, Some(icon::EDIT), &short).clicked() { open = true; }
        });
        if open { self.modal = Some(Modal::Value { key: key.into(), title: title.into(), hint: subtitle.into(), value: v, numeric }); }
    }
    fn after_change(&mut self, key: &str) {
        let tunnel = ["tunnelMode", "mode", "autoSelect", "bypassLan", "ipMode", "sniff", "mtu", "dnsFromJson", "customDns", "tunnelDns", "proxyPort", "httpProxyPort", "allowLan", "proxyPassword", "fragment", "fragmentPackets", "fragmentLength", "fragmentInterval", "noise", "noiseType", "noisePacket", "noiseRand", "noiseDelay", "mux", "muxConnections", "debugLogs"];
        if tunnel.contains(&key) && core::status().state == State::Connected { self.toast("Переподключаемся с новыми настройками…"); self.connect_selected(); }
        self.revision = u64::MAX;
    }

    fn rules(&mut self, ui: &mut Ui) {
        let p = self.pal; let rules = store::get().rules.clone();
        w::group(ui, &p, |ui| {
            if rules.is_empty() { w::row(ui, &p, "Правил пока нет", "Например: банки и госуслуги — напрямую, реклама — блокировать, telegram.exe — через VPN.", |_| {}); }
            for (i, rule) in rules.iter().enumerate() {
                let action = match rule.s("action").as_str() { "direct" => "Напрямую", "block" => "Блокировать", _ => "Через VPN" };
                let values = crate::model::split_values(&rule.s("value"));
                let title = if rule.s("name").is_empty() { crate::model::rule_kind_label(&rule.s("kind")).to_string() } else { rule.s("name") };
                let preview = format!("{} · {}{}", crate::model::rule_kind_label(&rule.s("kind")), values.iter().take(3).cloned().collect::<Vec<_>>().join(", "), if values.len() > 3 { format!(" и ещё {}", values.len() - 3) } else { String::new() });
                let mut edit = false; let mut enabled = rule.b("enabled", true);
                w::row(ui, &p, &title, &preview, |ui| {
                    if w::switch(ui, &p, &mut enabled).changed() { store::update(|s| if let Some(r) = s.rules.get_mut(i) { r.insert("enabled".into(), enabled.into()); }); }
                    if w::icon_button(ui, &p, icon::EDIT, "Изменить", false).clicked() { edit = true; }
                    w::chip(ui, &p, action, p.secondary_container, p.on_secondary_container);
                });
                if edit { self.modal = Some(Modal::Rule { index: Some(i), rule: rule.clone() }); }
            }
            w::row(ui, &p, "", "", |ui| { if w::button(ui, &p, Kind::Tonal, Some(icon::ADD), "Добавить правило").clicked() { self.modal = Some(Modal::Rule { index: None, rule: crate::obj! {"name" => "", "kind" => "domain_suffix", "value" => "", "action" => "direct", "enabled" => true} }); } });
        });
        ui.label(RichText::new("Правила по программам (.exe) работают в режиме TUN. Правила провайдера применяются раньше ваших.").size(12.0).color(p.muted));
    }

    fn logs(&mut self, ui: &mut Ui) {
        let p = self.pal;
        ui.horizontal(|ui| {
            ui.label(RichText::new("Журнал").font(FontId::new(26.0, semibold())));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if w::button(ui, &p, Kind::Text, Some(icon::FOLDER), "Файл").clicked() { util::open_path(&crate::log::path()); }
                if w::button(ui, &p, Kind::Text, Some(icon::CLEAR), "Очистить").clicked() { crate::log::clear(); }
                if w::button(ui, &p, Kind::Tonal, Some(icon::COPY), "Копировать").clicked() { ui.ctx().copy_text(crate::log::text()); self.toast("Журнал скопирован"); }
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
                    ui.label(RichText::new("Добавить сервер или подписку").font(FontId::new(20.0, semibold())));
                    ui.label(RichText::new("Ссылка подписки (https://…), ссылки vless:// vmess:// trojan:// ss:// hy2:// tuic://, JSON Xray или sing-box.").color(p.muted));
                    ui.add_space(8.0);
                    ScrollArea::vertical().max_height(260.0).show(ui, |ui| { ui.add(egui::TextEdit::multiline(text).desired_rows(8).desired_width(f32::INFINITY).hint_text("Вставьте сюда…").font(egui::TextStyle::Monospace)); });
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        if w::button(ui, &p, Kind::Text, Some(icon::PASTE), "Из буфера").clicked() { if let Some(t) = util::clipboard_text() { *text = t; } }
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if w::button(ui, &p, Kind::Filled, None, "Добавить").clicked() && !text.trim().is_empty() { let t = text.clone(); keep = false; self.add(t); }
                            if w::button(ui, &p, Kind::Text, None, "Отмена").clicked() { keep = false; }
                        });
                    });
                }
                Modal::Info { title, text, copy } => {
                    ui.label(RichText::new(title.as_str()).font(FontId::new(20.0, semibold())));
                    ui.add_space(8.0);
                    ScrollArea::both().max_height(460.0).show(ui, |ui| { let mut t = text.as_str(); ui.add(egui::TextEdit::multiline(&mut t).font(egui::TextStyle::Monospace).desired_width(f32::INFINITY).frame(false)); });
                    ui.add_space(12.0);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if w::button(ui, &p, Kind::Filled, None, "Закрыть").clicked() { keep = false; }
                        if *copy && w::button(ui, &p, Kind::Text, Some(icon::COPY), "Копировать").clicked() { ui.ctx().copy_text(text.clone()); }
                    });
                }
                Modal::Confirm { title, text, action } => {
                    ui.label(RichText::new(title.as_str()).font(FontId::new(20.0, semibold())));
                    ui.label(RichText::new(text.as_str()).color(p.muted));
                    ui.add_space(14.0);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if w::button(ui, &p, Kind::Danger, None, "Да").clicked() {
                            keep = false;
                            match action {
                                Confirmed::DeleteProfile(id) => { let id = id.clone(); store::update(|s| { s.profiles.retain(|x| x.id != id); if s.selected == id { s.selected = s.profiles.first().map(|x| x.id.clone()).unwrap_or_default(); } }); }
                                Confirmed::DeleteSubscription(id) => { let id = id.clone(); store::update(|s| { s.subscriptions.retain(|x| x.id != id); s.profiles.retain(|x| x.group != id); if s.selected().is_none() { s.selected = s.profiles.first().map(|x| x.id.clone()).unwrap_or_default(); } }); }
                                Confirmed::Reset => { store::update(|s| { let keep = (s.s("hwid"), s.s("clientNumericId")); s.settings = crate::model::defaults(); s.set("hwid", keep.0.into()); s.set("clientNumericId", keep.1.into()); }); let _ = crate::install::set_autostart(false); }
                            }
                        }
                        if w::button(ui, &p, Kind::Text, None, "Отмена").clicked() { keep = false; }
                    });
                }
                Modal::Admin => {
                    ui.label(RichText::new("Нужны права администратора").font(FontId::new(20.0, semibold())));
                    ui.label(RichText::new("Режим TUN создаёт сетевой адаптер и меняет маршруты Windows. Перезапустите Wraith от имени администратора или подключитесь через системный прокси.").color(p.muted));
                    ui.add_space(14.0);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if w::button(ui, &p, Kind::Filled, Some(icon::SHIELD), "Перезапустить").clicked() {
                            keep = false;
                            if util::relaunch_elevated("--connect --elevated") { core::shutdown(); std::process::exit(0); } else { self.toast("Запуск от администратора отменён"); }
                        }
                        if w::button(ui, &p, Kind::Text, None, "Системный прокси").clicked() { keep = false; store::update(|s| s.set("tunnelMode", "proxy".into())); self.connect_selected(); }
                    });
                }
                Modal::Rule { index, rule } => {
                    ui.label(RichText::new(if index.is_some() { "Правило" } else { "Новое правило" }).font(FontId::new(20.0, semibold())));
                    ui.add_space(8.0);
                    let mut name = rule.s("name"); let mut kind = rule.s("kind"); let mut value = rule.s("value"); let mut action = rule.s_or("action", "direct");
                    ui.label(RichText::new("Название").size(12.5).color(p.muted)); ui.add(egui::TextEdit::singleline(&mut name).desired_width(f32::INFINITY).hint_text("Например: Банки"));
                    ui.label(RichText::new("Что сравнивать").size(12.5).color(p.muted)); w::picker(ui, "rule-kind", &mut kind, RULE_KINDS);
                    ui.label(RichText::new("Значения — по одному в строке").size(12.5).color(p.muted));
                    let hint = match kind.as_str() { "domain_suffix" => "sberbank.ru\ngosuslugi.ru", "ip_cidr" => "10.0.0.0/8", "geosite" => "category-ads-all", "geoip" => "ru", "process" => "telegram.exe", _ => "" };
                    ui.add(egui::TextEdit::multiline(&mut value).desired_rows(5).desired_width(f32::INFINITY).hint_text(hint));
                    ui.label(RichText::new("Действие").size(12.5).color(p.muted));
                    w::segmented(ui, &p, &mut action, &[("direct", "Напрямую"), ("proxy", "Через VPN"), ("block", "Блокировать")], ui.available_width());
                    rule.insert("name".into(), name.into()); rule.insert("kind".into(), kind.into()); rule.insert("value".into(), value.clone().into()); rule.insert("action".into(), action.into());
                    ui.add_space(14.0);
                    ui.horizontal(|ui| {
                        if let Some(i) = *index { if w::button(ui, &p, Kind::Danger, Some(icon::DELETE), "Удалить").clicked() { keep = false; store::update(|s| { if i < s.rules.len() { s.rules.remove(i); } }); self.after_change("mode"); } }
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if w::button(ui, &p, Kind::Filled, None, "Сохранить").clicked() && !value.trim().is_empty() {
                                keep = false; let r = rule.clone(); let i = *index;
                                store::update(|s| match i { Some(i) if i < s.rules.len() => s.rules[i] = r, _ => s.rules.push(r) });
                                self.after_change("mode");
                            }
                            if w::button(ui, &p, Kind::Text, None, "Отмена").clicked() { keep = false; }
                        });
                    });
                }
                Modal::Sub { sub } => {
                    ui.label(RichText::new("Подписка").font(FontId::new(20.0, semibold())));
                    ui.add_space(8.0);
                    ui.label(RichText::new("Название").size(12.5).color(p.muted)); ui.add(egui::TextEdit::singleline(&mut sub.name).desired_width(f32::INFINITY));
                    ui.label(RichText::new("Адрес").size(12.5).color(p.muted)); ui.add(egui::TextEdit::singleline(&mut sub.url).desired_width(f32::INFINITY));
                    let mut interval = sub.interval_hours.to_string();
                    ui.label(RichText::new("Интервал обновления, часов").size(12.5).color(p.muted)); ui.add(egui::TextEdit::singleline(&mut interval).desired_width(120.0));
                    if let Ok(h) = interval.trim().parse::<i64>() { sub.interval_hours = h.clamp(1, 168); }
                    ui.add_space(14.0);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if w::button(ui, &p, Kind::Filled, None, "Сохранить").clicked() {
                            if !sub.url.starts_with("https://") { self.toast("Адрес подписки должен начинаться с https://"); }
                            else { keep = false; let s2 = sub.clone(); store::update(|s| if let Some(x) = s.subscriptions.iter_mut().find(|x| x.id == s2.id) { x.name = s2.name.clone(); x.url = s2.url.clone(); x.interval_hours = s2.interval_hours; }); }
                        }
                        if w::button(ui, &p, Kind::Text, None, "Отмена").clicked() { keep = false; }
                    });
                }
                Modal::Update { release } => {
                    ui.label(RichText::new(format!("Wraith {}", release.version)).font(FontId::new(20.0, semibold())));
                    ui.add_space(6.0);
                    ScrollArea::vertical().max_height(260.0).show(ui, |ui| { ui.label(RichText::new(&release.notes).color(p.muted)); });
                    let progress = self.shared.lock().unwrap().download;
                    if let Some((done, total)) = progress {
                        ui.add_space(8.0);
                        ui.add(egui::ProgressBar::new(if total > 0 { done as f32 / total as f32 } else { 0.0 }).text(format!("{} из {}", util::bytes(done as i64), util::bytes(total as i64))));
                    }
                    ui.add_space(14.0);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if progress.is_none() && w::button(ui, &p, Kind::Filled, Some(icon::DOWNLOAD), "Скачать и установить").clicked() {
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
                        if w::button(ui, &p, Kind::Text, None, "Позже").clicked() { keep = false; }
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
                        if w::button(ui, &p, Kind::Filled, None, "Сохранить").clicked() || enter {
                            let v = value.trim().to_string();
                            let parsed: Result<Value, String> = if *numeric { v.parse::<i64>().map(Value::from).map_err(|_| "Введите число".into()) } else { Ok(v.clone().into()) };
                            let valid = match key.as_str() {
                                "proxyPort" | "httpProxyPort" => v.parse::<u16>().map(|n| n >= 1024).unwrap_or(false),
                                "mtu" => v.parse::<i64>().map(|n| (1280..=9000).contains(&n)).unwrap_or(false),
                                "pingUrl" => v.starts_with("https://") || v.starts_with("http://"),
                                "proxyPassword" => v.is_empty() || v.chars().count() >= 8,
                                _ => true,
                            };
                            match parsed { Ok(val) if valid => { keep = false; let k = key.clone(); store::update(|s| s.set(&k, val)); self.after_change(&k.clone()); } _ => self.toast("Некорректное значение") }
                        }
                        if w::button(ui, &p, Kind::Text, None, "Отмена").clicked() { keep = false; }
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
