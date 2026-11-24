//! Main window, laid out like Wraith for Android 0.13: a backdrop (photo or live gradient), the Home,
//! Servers and Statistics tabs on a floating bar, Settings as its own stack of pages, and dialogs.

use super::backdrop;
use super::icons::{self, Ic};
use super::theme::{self, alpha, Palette};
use super::widgets::{self as w, font, font_m, Trailing};
use crate::core::{self, State};
use crate::json::{Get, Obj};
use crate::model::{Profile, Subscription};
use crate::store;
use crate::subs::updater::{self, Release};
use crate::util;
use egui::{pos2, vec2, Align, Align2, Color32, CornerRadius, Frame, Layout, Margin, Rect, RichText, ScrollArea, Sense, Stroke, Ui};
use serde_json::Value;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(PartialEq, Clone, Copy)]
enum Tab { Home, Servers, Stats }
#[derive(PartialEq, Clone, Copy)]
enum Page { Routing, Tunnel, Subscriptions, Ping, Appearance, Logs }

enum Modal {
    Import { text: String },
    Info { title: String, text: String, copy: bool },
    Confirm { title: String, text: String, action: String, danger: bool, run: Confirmed },
    Admin,
    Rule { index: Option<usize>, rule: Obj },
    Input { title: String, value: String, hint: String, multiline: bool, password: bool, target: Target, error: String },
    Choice { title: String, labels: Vec<String>, values: Vec<String>, selected: usize, key: String },
    Sheet { title: String, subtitle: String, actions: Vec<(String, Ic, bool, Act)> },
    Editor { id: String, title: String, value: String, error: String },
    Update { release: Release },
}
#[derive(Clone)]
enum Confirmed { DeleteProfile(String), DeleteSubscription(String), Reset, ClearHistory, NewProxyPassword, ImportRules(Vec<Obj>), Restore, Export }
#[derive(Clone)]
enum Target { Setting(String), Number(String, i64, i64), SubName(String), SubUrl(String), SubInterval(String), HappRouting }
#[derive(Clone)]
enum Act {
    Ping(String), Favorite(String), CopyLink(String), ShowConfig(String), EditConfig(String), Diagnose(String), Connect(String), DeleteProfile(String),
    SubUpdate(String), SubPing(String), SubRename(String), SubUrl(String), SubInterval(String), SubCopy(String), Open(String), DeleteSub(String),
    RuleEdit(usize), RuleToggle(usize), RuleDelete(usize),
}

#[derive(Default)]
struct Shared {
    busy: Option<String>,
    toast: Option<(String, Instant)>,
    pinging: HashSet<String>,
    ping_cancel: Option<Arc<AtomicBool>>,
    release: Option<Release>,
    download: Option<(u64, u64)>,
}

pub struct App {
    pal: Palette,
    tab: Tab, in_settings: bool, pages: Vec<Page>,
    search: String,
    modal: Option<Modal>,
    shared: Arc<Mutex<Shared>>,
    profiles: Vec<Profile>, subs: Vec<Subscription>, revision: u64,
    manual_collapsed: bool,
    log_cache: (u64, String),
    history: (Obj, Option<Instant>),
    slider: Option<(f32, f32)>,
    start_hidden: bool, frames: u64,
    _tray: Option<tray_icon::TrayIcon>,
}

/// Busy label, toasts and running checks outlive a window: it is closed into the tray and reopened.
fn shared() -> Arc<Mutex<Shared>> {
    static SHARED: std::sync::OnceLock<Arc<Mutex<Shared>>> = std::sync::OnceLock::new();
    SHARED.get_or_init(|| Arc::new(Mutex::new(Shared::default()))).clone()
}
fn run_background(shared: Arc<Mutex<Shared>>, label: &str, job: impl FnOnce() -> Result<String, String> + Send + 'static) {
    { let mut s = shared.lock().unwrap(); if s.busy.is_some() { s.toast = Some((crate::tr!("Дождитесь текущей операции", "Wait for the current operation").into(), Instant::now())); return; } s.busy = Some(label.into()); }
    std::thread::spawn(move || {
        let result = job();
        let mut s = shared.lock().unwrap(); s.busy = None;
        match result { Ok(m) if m.is_empty() => {}, Ok(m) => s.toast = Some((m, Instant::now())), Err(e) => s.toast = Some((crate::log::safe(e), Instant::now())) }
        drop(s); super::repaint();
    });
}

/// Latency checks for these servers (None: stop the running check).
fn start_ping(targets: Option<Vec<Profile>>) {
    let shared = shared();
    if let Some(c) = shared.lock().unwrap().ping_cancel.take() { c.store(true, Ordering::Relaxed); }
    shared.lock().unwrap().pinging.clear();
    let Some(targets) = targets.filter(|t| !t.is_empty()) else { super::repaint(); return };
    let state = store::get().clone();
    let cancel = Arc::new(AtomicBool::new(false));
    { let mut s = shared.lock().unwrap(); s.ping_cancel = Some(cancel.clone()); for p in &targets { s.pinging.insert(p.id.clone()); } }
    // A new check of several servers starts from the provider's order: old results are dropped.
    if targets.len() > 1 { store::update(|s| for p in s.profiles.iter_mut().filter(|p| targets.iter().any(|t| t.id == p.id)) { p.latency = 0; p.ping_error.clear(); }); }
    let sh = shared.clone();
    let report: crate::pinger::Report = Arc::new(move |id: &str, ms: i64, err: String| {
        { let mut s = store::get(); if let Some(p) = s.profiles.iter_mut().find(|p| p.id == id) { p.latency = ms; p.ping_error = if ms > 0 { String::new() } else { err }; } }
        sh.lock().unwrap().pinging.remove(id); super::repaint();
    });
    std::thread::spawn(move || {
        crate::pinger::run(state, targets, report, cancel.clone());
        let mut s = shared.lock().unwrap();
        if s.ping_cancel.as_ref().map(|c| Arc::ptr_eq(c, &cancel)).unwrap_or(false) { s.pinging.clear(); s.ping_cancel = None; }
        drop(s); store::save(); super::repaint();
    });
}

/// Startup jobs, once per run.
pub fn startup(connect: bool) {
    backdrop::load();
    let (update_on_open, check_updates, connect_on_start, ping_on_open) = { let s = store::get(); (s.b("updateOnOpen"), s.b("checkUpdates"), s.b("connectOnStart"), s.b("pingOnOpen")) };
    if update_on_open || ping_on_open {
        std::thread::spawn(move || {
            if update_on_open { crate::subs::update_all(true); super::repaint(); }
            if ping_on_open { let all = store::get().profiles.clone(); start_ping(Some(all)); }
        });
    }
    // Subscriptions whose interval has passed are refreshed in the background, checked once an hour.
    std::thread::spawn(|| loop {
        std::thread::sleep(Duration::from_secs(3600));
        let due = { let s = store::get(); s.b("autoUpdate") && !s.subscriptions.is_empty() };
        if due { crate::subs::update_all(true); super::repaint(); }
    });
    if check_updates { let shared = shared(); std::thread::spawn(move || if let Ok(r) = updater::check() { if r.available { shared.lock().unwrap().release = Some(r); super::repaint(); } }); }
    if connect || connect_on_start { let selected = store::get().selected().cloned(); if let Some(p) = selected { core::connect(p); } }
}

pub fn greeting() -> &'static str {
    match util::local_hour() {
        5..=11 => crate::tr!("Доброе утро", "Good morning"), 12..=17 => crate::tr!("Добрый день", "Good afternoon"),
        18..=22 => crate::tr!("Добрый вечер", "Good evening"), _ => crate::tr!("Доброй ночи", "Good night"),
    }
}
fn servers_count(n: usize) -> String { format!("{n} {}", util::plural(n as i64, crate::tr!("сервер", "server"), crate::tr!("сервера", "servers"), crate::tr!("серверов", "servers"))) }
fn routing_label(mode: &str) -> &'static str { match mode { "global" => crate::tr!("Всё через VPN", "Everything through VPN"), "direct" => crate::tr!("Напрямую", "Direct"), _ => crate::tr!("По правилам", "By rules") } }
fn mode_label(mode: &str) -> &'static str { match mode { "tun" => crate::tr!("Только TUN", "TUN only"), "proxy" => crate::tr!("Системный прокси", "System proxy"), _ => crate::tr!("Системный прокси + TUN", "System proxy + TUN") } }
fn ping_label(t: &str) -> &'static str { match t { "proxyHead" => "Proxy HEAD", "tcp" => "TCP", "icmp" => "ICMP", _ => "Proxy GET" } }
fn rate(v: i64) -> String { crate::trf!("{}/с", "{}/s", util::bytes(v)) }

impl App {
    pub fn new(cc: &eframe::CreationContext, start_hidden: bool, connect: bool) -> Self {
        let mut app = Self::with_context(&cc.egui_ctx, start_hidden);
        // The tray lives with the window's event loop; its menu works while the window is hidden.
        app._tray = super::tray::create();
        startup(connect);
        app
    }
    pub fn with_context(ctx: &egui::Context, start_hidden: bool) -> Self {
        theme::install_fonts(ctx);
        let pal = current_palette();
        theme::apply(ctx, &pal);
        super::set_context(ctx.clone());
        backdrop::load();
        let mut app = App { pal, tab: Tab::Home, in_settings: false, pages: vec![], search: String::new(), modal: None, shared: shared(),
            profiles: vec![], subs: vec![], revision: u64::MAX, manual_collapsed: false, log_cache: (u64::MAX, String::new()), history: (Obj::new(), None), slider: None, start_hidden, frames: 0, _tray: None };
        match std::env::args().find_map(|a| a.strip_prefix("--page=").map(String::from)).as_deref() {
            Some("servers") => app.tab = Tab::Servers, Some("stats") => app.tab = Tab::Stats,
            Some("settings") => app.in_settings = true,
            Some(p) if p.starts_with("settings/") => { app.in_settings = true; app.pages.push(match &p[9..] { "routing" => Page::Routing, "tunnel" => Page::Tunnel, "subscriptions" => Page::Subscriptions, "ping" => Page::Ping, "logs" => Page::Logs, _ => Page::Appearance }); }
            _ => {}
        }
        app
    }

    fn background(&self, label: &str, job: impl FnOnce() -> Result<String, String> + Send + 'static) { run_background(self.shared.clone(), label, job); }
    fn toast(&self, text: impl Into<String>) { self.shared.lock().unwrap().toast = Some((text.into(), Instant::now())); }

    fn refresh_cache(&mut self, ctx: &egui::Context) {
        let pal = current_palette();
        if pal != self.pal { self.pal = pal; theme::apply(ctx, &pal); }
        let rev = store::revision();
        if rev == self.revision { return; }
        self.revision = rev;
        let s = store::get(); self.profiles = s.profiles.clone(); self.subs = s.subscriptions.clone();
    }

    fn back(&mut self) -> bool {
        if self.pages.pop().is_some() { return true; }
        if self.in_settings { self.in_settings = false; return true; }
        if self.tab != Tab::Home { self.tab = Tab::Home; return true; }
        false
    }

    // ------------------------------------------------------------------ actions

    fn toggle(&mut self) {
        if !self.shared.lock().unwrap().pinging.is_empty() { start_ping(None); return; }
        match core::status().state {
            State::Connected | State::Connecting => core::disconnect(),
            State::Stopping => {}
            State::Off => { if store::get().selected().is_none() { self.modal = Some(Modal::Import { text: String::new() }); } else { self.connect_selected(); } }
        }
    }
    fn connect_selected(&mut self) {
        let Some(p) = store::get().selected().cloned() else { self.toast(crate::tr!("Сначала добавьте и выберите сервер", "Add and select a server first")); return };
        if crate::model::uses_tun(&store::get().settings) && !util::is_admin() { self.modal = Some(Modal::Admin); return; }
        core::connect(p);
    }
    fn select(&mut self, id: &str, connect_now: bool) {
        let same = store::get().selected == id;
        if !same { store::update(|s| s.selected = id.to_string()); }
        let st = core::status();
        if connect_now && st.state == State::Off { self.connect_selected(); }
        else if !same && matches!(st.state, State::Connected | State::Connecting) { self.connect_selected(); }
    }
    fn ping_ids(&self, ids: Vec<String>) { let t: Vec<Profile> = self.profiles.iter().filter(|p| ids.contains(&p.id)).cloned().collect(); start_ping(Some(t)); }

    fn import(&mut self, text: String) {
        let text = text.trim().to_string();
        if text.starts_with("happ://routing/") { self.import_routing(&text); return; }
        let payload = match crate::importer::unwrap(&text) { Ok(p) => p, Err(e) => { self.toast(e); return } };
        if payload.starts_with("http://") { self.toast(crate::tr!("Для подписок требуется HTTPS.", "Subscriptions require HTTPS.")); return; }
        self.tab = Tab::Servers; self.in_settings = false; self.pages.clear();
        if crate::importer::is_subscription_url(&payload) {
            self.background(crate::tr!("Загружаем подписку…", "Loading the subscription…"), move || { let sub = crate::subs::create(&payload)?; Ok(crate::trf!("Подписка «{}» добавлена", "Subscription «{}» added", sub.name)) });
            return;
        }
        match crate::importer::parse(&payload) {
            Ok(result) => {
                let n = result.profiles.len();
                store::update(|s| { let first = result.profiles[0].id.clone(); s.profiles.extend(result.profiles); if s.selected().is_none() { s.selected = first; } });
                let skipped = if result.skipped.is_empty() { String::new() } else { crate::trf!(". Пропущено: {}", ". Skipped: {}", result.skipped.len()) };
                self.toast(crate::trf!("Добавлено серверов: {n}{skipped}", "Servers added: {n}{skipped}"));
            }
            Err(e) => self.toast(e),
        }
    }
    fn import_routing(&mut self, text: &str) {
        let result = crate::routing::decode(text).and_then(|json| {
            let mut rules = vec![]; crate::routing::append(&mut rules, &json);
            if rules.is_empty() { Err(crate::tr!("В профиле нет правил доменов или IP.", "The profile has no domain or IP rules.").to_string()) } else { Ok(rules) }
        });
        match result {
            Ok(rules) => {
                let entries: usize = rules.iter().map(|r| crate::model::split_values(&r.s("value")).len()).sum();
                self.modal = Some(Modal::Confirm { title: crate::tr!("Импортировать правила?", "Import the rules?").into(), text: crate::trf!("Правил: {}, записей: {entries}.", "Rules: {}, entries: {entries}.", rules.len()), action: crate::tr!("Импорт", "Import").into(), danger: false, run: Confirmed::ImportRules(rules) });
            }
            Err(e) => self.toast(e),
        }
    }

    fn set(&mut self, key: &str, value: Value) {
        let k = key.to_string(); let on = value.as_bool() == Some(true);
        store::update(|s| {
            s.set(&k, value);
            // Provider DNS and own DNS are alternatives: turning one on turns the other off.
            if on && k == "dnsFromJson" { s.set("customDns", false.into()); }
            if on && k == "customDns" { s.set("dnsFromJson", false.into()); }
            // A LAN-reachable proxy always has a password.
            if on && k == "allowLan" && s.s("proxyPassword").chars().count() < 8 { s.set("proxyPassword", util::random_hex(10).into()); }
        });
        self.changed(key);
    }
    fn toggle_setting(&mut self, key: &str) { let v = store::get().b(key); self.set(key, (!v).into()); }
    /// Settings the core reads reconnect a running tunnel.
    fn changed(&mut self, key: &str) {
        const TUNNEL: &[&str] = &["tunnelMode", "mode", "autoSelect", "bypassLan", "ipMode", "sniff", "mtu", "dnsFromJson", "customDns", "tunnelDns", "proxyPort", "httpProxyPort", "allowLan", "proxyPassword",
            "fragment", "fragmentPackets", "fragmentLength", "fragmentInterval", "fragmentMaxSplit", "noise", "noiseType", "noisePacket", "noiseRand", "noiseDelay", "mux", "muxConnections", "debugLogs", "quic", "rules"];
        if TUNNEL.contains(&key) && core::status().state == State::Connected { self.toast(crate::tr!("Переподключаемся с новыми настройками…", "Reconnecting with the new settings…")); self.connect_selected(); }
        self.revision = u64::MAX;
    }

    fn input(&mut self, title: &str, value: String, target: Target) {
        let (multiline, password) = (matches!(target, Target::HappRouting), matches!(&target, Target::Setting(k) if k == "proxyPassword"));
        self.modal = Some(Modal::Input { title: title.into(), value, hint: String::new(), multiline, password, target, error: String::new() });
    }
    fn number(&mut self, title: &str, key: &str, min: i64, max: i64) {
        let v = store::get().settings.i(key, 0);
        self.modal = Some(Modal::Input { title: format!("{title} ({min}–{max})"), value: v.to_string(), hint: String::new(), multiline: false, password: false, target: Target::Number(key.into(), min, max), error: String::new() });
    }
    fn choice(&mut self, title: &str, labels: &[&str], values: &[&str], key: &str) {
        let cur = store::get().s(key);
        self.modal = Some(Modal::Choice { title: title.into(), labels: labels.iter().map(|s| s.to_string()).collect(), values: values.iter().map(|s| s.to_string()).collect(), selected: values.iter().position(|v| *v == cur).unwrap_or(0), key: key.into() });
    }

    fn save_input(&mut self, target: &Target, value: &str) -> Result<(), String> {
        let v = value.trim();
        match target {
            Target::Number(key, min, max) => {
                let n: i64 = v.parse().map_err(|_| crate::tr!("Введите целое число", "Enter a whole number").to_string())?;
                if n < *min || n > *max { return Err(crate::trf!("Допустимо {min}–{max}", "Allowed {min}–{max}")); }
                self.set(key, n.into());
            }
            Target::Setting(key) => {
                match key.as_str() {
                    "pingUrl" => if !v.starts_with("https://") || v.len() < 12 { return Err(crate::tr!("Нужен HTTPS-адрес", "An HTTPS address is required").into()); },
                    "proxyPassword" => if v.chars().count() < 8 { return Err(crate::tr!("Не короче 8 символов", "At least 8 characters").into()); },
                    "tunnelDns" => if v.is_empty() { return Err(crate::tr!("Введите адрес DNS", "Enter a DNS address").into()); },
                    k if k.starts_with("fragment") || k.starts_with("noise") => {
                        let mut probe = store::get().settings.clone(); probe.insert(k.into(), v.into());
                        if k.starts_with("noise") { crate::xray::noise(&probe)?; } else { crate::xray::fragment(&probe)?; }
                    }
                    _ => {}
                }
                let v: String = if key == "greetName" { v.chars().take(24).collect() } else { v.to_string() };
                self.set(key, v.into());
            }
            Target::SubName(id) => { if v.is_empty() { return Err(crate::tr!("Введите название", "Enter a name").into()); } let id = id.clone(); store::update(|s| if let Some(x) = s.subscriptions.iter_mut().find(|x| x.id == id) { x.name = v.to_string(); }); }
            Target::SubUrl(id) => { if !v.starts_with("https://") { return Err(crate::tr!("Нужен HTTPS-адрес", "An HTTPS address is required").into()); } let id = id.clone(); store::update(|s| if let Some(x) = s.subscriptions.iter_mut().find(|x| x.id == id) { x.url = v.to_string(); }); }
            Target::SubInterval(id) => { let h: i64 = v.parse().map_err(|_| crate::tr!("Введите целое число", "Enter a whole number").to_string())?; if !(1..=168).contains(&h) { return Err(crate::tr!("Допустимо 1–168", "Allowed 1–168").into()); } let id = id.clone(); store::update(|s| if let Some(x) = s.subscriptions.iter_mut().find(|x| x.id == id) { x.interval_hours = h; }); }
            Target::HappRouting => { let v = v.to_string(); self.modal = None; self.import_routing(&v); return Ok(()); }
        }
        Ok(())
    }

    fn server_sheet(&mut self, pr: &Profile) {
        let id = pr.id.clone();
        let mut actions = vec![
            (crate::tr!("Подключиться", "Connect").to_string(), Ic::Power, false, Act::Connect(id.clone())),
            (crate::tr!("Проверить задержку", "Check latency").into(), Ic::Pulse, false, Act::Ping(id.clone())),
            (if pr.favorite { crate::tr!("Убрать из избранного", "Remove from favourites") } else { crate::tr!("В избранное", "Add to favourites") }.into(), Ic::Star, false, Act::Favorite(id.clone())),
            (crate::tr!("Скопировать ссылку", "Copy link").into(), Ic::Copy, false, Act::CopyLink(id.clone())),
            (crate::tr!("Показать конфигурацию", "Show configuration").into(), Ic::Code, false, Act::ShowConfig(id.clone())),
            (crate::tr!("Изменить конфигурацию", "Edit configuration").into(), Ic::Edit, false, Act::EditConfig(id.clone())),
            (crate::tr!("Диагностика", "Diagnostics").into(), Ic::Info, false, Act::Diagnose(id.clone())),
        ];
        actions.push((crate::tr!("Удалить", "Delete").into(), Ic::Delete, true, Act::DeleteProfile(id)));
        self.modal = Some(Modal::Sheet { title: w::display_name(&pr.name), subtitle: pr.summary(), actions });
    }
    fn sub_sheet(&mut self, sub: &Subscription) {
        let id = sub.id.clone();
        let mut actions = vec![
            (crate::tr!("Обновить", "Update").to_string(), Ic::Refresh, false, Act::SubUpdate(id.clone())),
            (crate::tr!("Проверить задержку", "Check latency").into(), Ic::Pulse, false, Act::SubPing(id.clone())),
            (crate::tr!("Переименовать", "Rename").into(), Ic::Edit, false, Act::SubRename(id.clone())),
            (crate::tr!("Изменить адрес", "Change address").into(), Ic::Link, false, Act::SubUrl(id.clone())),
            (crate::tr!("Интервал обновления", "Update interval").into(), Ic::Clock, false, Act::SubInterval(id.clone())),
            (crate::tr!("Скопировать ссылку", "Copy link").into(), Ic::Copy, false, Act::SubCopy(id.clone())),
        ];
        let support = if sub.support_url.is_empty() { sub.web_page_url.clone() } else { sub.support_url.clone() };
        if !support.is_empty() { actions.push((crate::tr!("Поддержка провайдера", "Provider support").into(), Ic::Info, false, Act::Open(support))); }
        actions.push((crate::tr!("Удалить", "Delete").into(), Ic::Delete, true, Act::DeleteSub(id)));
        let host = sub.url.split("://").nth(1).unwrap_or("").split('/').next().unwrap_or("").to_string();
        self.modal = Some(Modal::Sheet { title: sub.name.clone(), subtitle: host, actions });
    }

    fn run_act(&mut self, ctx: &egui::Context, act: Act) {
        let profile = |id: &str| store::get().profiles.iter().find(|p| p.id == id).cloned();
        let sub = |id: &str| store::get().subscriptions.iter().find(|s| s.id == id).cloned();
        match act {
            Act::Connect(id) => self.select(&id, true),
            Act::Ping(id) => self.ping_ids(vec![id]),
            Act::Favorite(id) => store::update(|s| if let Some(x) = s.profiles.iter_mut().find(|x| x.id == id) { x.favorite = !x.favorite; }),
            Act::CopyLink(id) => if let Some(p) = profile(&id) { ctx.copy_text(p.source); self.toast(crate::tr!("Скопировано", "Copied")); },
            // The server's own configuration as the provider sent it, without the parts Wraith adds.
            Act::ShowConfig(id) => if let Some(p) = profile(&id) {
                self.modal = Some(match crate::xray::source(&p) {
                    Ok(c) => Modal::Info { title: crate::tr!("Конфигурация сервера", "Server configuration").into(), text: serde_json::to_string_pretty(&c).unwrap_or_default(), copy: true },
                    Err(e) => Modal::Info { title: crate::tr!("Конфигурация не собрана", "The configuration could not be built").into(), text: e, copy: false },
                });
            },
            Act::EditConfig(id) => if let Some(p) = profile(&id) {
                let raw = p.source.trim().to_string();
                let value = if raw.starts_with('{') || raw.starts_with('[') { serde_json::from_str::<Value>(&raw).ok().and_then(|v| serde_json::to_string_pretty(&v).ok()).unwrap_or(raw) } else { raw };
                self.modal = Some(Modal::Editor { id, title: crate::trf!("Конфигурация · {}", "Configuration · {}", w::display_name(&p.name)), value, error: String::new() });
            },
            Act::Diagnose(id) => if let Some(p) = profile(&id) {
                self.background(crate::tr!("Проверяем конфигурацию…", "Checking the configuration…"), move || {
                    let text = diagnose(&p);
                    show_info(crate::tr!("Диагностика", "Diagnostics"), text);
                    Ok(String::new())
                });
            },
            Act::DeleteProfile(id) => if let Some(p) = profile(&id) {
                let back = if p.group.is_empty() { "" } else { crate::tr!(" вернётся при следующем обновлении подписки.", " comes back on the next subscription update.") };
                self.modal = Some(Modal::Confirm { title: crate::tr!("Удалить сервер?", "Delete the server?").into(), text: format!("«{}»{back}", w::display_name(&p.name)), action: crate::tr!("Удалить", "Delete").into(), danger: true, run: Confirmed::DeleteProfile(id) });
            },
            Act::SubUpdate(id) => if let Some(s) = sub(&id) { let name = s.name.clone(); self.background(&crate::trf!("Обновляем «{name}»…", "Updating «{name}»…"), move || { crate::subs::update(s)?; Ok(crate::tr!("Подписка обновлена", "Subscription updated").into()) }); },
            Act::SubPing(id) => { let ids = self.profiles.iter().filter(|p| p.group == id).map(|p| p.id.clone()).collect(); self.ping_ids(ids); }
            Act::SubRename(id) => if let Some(s) = sub(&id) { self.input(crate::tr!("Название", "Name"), s.name, Target::SubName(id)); },
            Act::SubUrl(id) => if let Some(s) = sub(&id) { self.input(crate::tr!("HTTPS-адрес подписки", "Subscription HTTPS address"), s.url, Target::SubUrl(id)); },
            Act::SubInterval(id) => if let Some(s) = sub(&id) { self.input(crate::tr!("Интервал обновления, часов (1–168)", "Update interval, hours (1–168)"), s.interval_hours.to_string(), Target::SubInterval(id)); },
            Act::SubCopy(id) => if let Some(s) = sub(&id) { ctx.copy_text(s.url); self.toast(crate::tr!("Ссылка скопирована", "Link copied")); },
            Act::Open(url) => util::open_url(&url),
            Act::DeleteSub(id) => if let Some(s) = sub(&id) { self.modal = Some(Modal::Confirm { title: crate::tr!("Удалить подписку?", "Delete the subscription?").into(), text: crate::trf!("«{}» и все её серверы будут удалены.", "«{}» and all its servers will be removed.", s.name), action: crate::tr!("Удалить", "Delete").into(), danger: true, run: Confirmed::DeleteSubscription(id) }); },
            Act::RuleEdit(i) => { let r = store::get().rules.get(i).cloned(); if let Some(rule) = r { self.modal = Some(Modal::Rule { index: Some(i), rule }); } }
            Act::RuleToggle(i) => { store::update(|s| if let Some(r) = s.rules.get_mut(i) { let on = r.b("enabled", true); r.insert("enabled".into(), (!on).into()); }); self.changed("rules"); }
            Act::RuleDelete(i) => { store::update(|s| if i < s.rules.len() { s.rules.remove(i); }); self.changed("rules"); }
        }
    }

    fn confirmed(&mut self, ctx: &egui::Context, run: Confirmed) {
        match run {
            Confirmed::DeleteProfile(id) => store::update(|s| { s.profiles.retain(|x| x.id != id); if s.selected == id { s.selected = s.profiles.first().map(|x| x.id.clone()).unwrap_or_default(); } }),
            Confirmed::DeleteSubscription(id) => {
                let was_selected = store::get().selected().map(|p| p.group == id).unwrap_or(false);
                store::update(|s| { s.subscriptions.retain(|x| x.id != id); s.profiles.retain(|x| x.group != id); if s.selected().is_none() { s.selected = s.profiles.first().map(|x| x.id.clone()).unwrap_or_default(); } });
                if was_selected && core::status().state != State::Off { core::disconnect(); }
            }
            Confirmed::Reset => { store::update(|s| { let keep = (s.s("hwid"), s.s("clientNumericId")); s.settings = crate::model::defaults(); s.set("hwid", keep.0.into()); s.set("clientNumericId", keep.1.into()); }); let _ = crate::install::set_autostart(false); self.toast(crate::tr!("Настройки сброшены", "Settings reset")); }
            Confirmed::ClearHistory => { crate::stats::clear(); self.history = (Obj::new(), None); }
            Confirmed::NewProxyPassword => { self.set("proxyPassword", util::random_hex(10).into()); }
            Confirmed::ImportRules(rules) => { store::update(|s| for mut r in rules { r.remove("provider"); s.rules.push(r); }); self.in_settings = true; self.pages = vec![Page::Routing]; self.changed("rules"); }
            Confirmed::Export => {
                self.background(crate::tr!("Сохраняем…", "Saving…"), || {
                    let Some(path) = util::file_dialog(true, crate::tr!("Резервная копия Wraith", "Wraith backup"), "wraith-backup.json", &["*.json"])? else { return Ok(String::new()) };
                    write_private(&path, store::backup().as_bytes()).map_err(|e| e.to_string())?;
                    Ok(crate::tr!("Сохранено", "Saved").into())
                });
            }
            Confirmed::Restore => {
                self.background(crate::tr!("Читаем файл…", "Reading the file…"), || {
                    let Some(path) = util::file_dialog(false, crate::tr!("Резервная копия Wraith", "Wraith backup"), "", &["*.json"])? else { return Ok(String::new()) };
                    let meta = std::fs::metadata(&path).map_err(|e| e.to_string())?;
                    if meta.len() > 32 * 1024 * 1024 { return Err(crate::tr!("Файл слишком большой", "The file is too large").into()); }
                    let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
                    if core::status().state != State::Off { core::shutdown(); }
                    store::restore(&text)?;
                    Ok(crate::tr!("Резервная копия восстановлена", "Backup restored").into())
                });
            }
        }
        let _ = ctx;
    }

    // ------------------------------------------------------------------ layout helpers

    /// A vertically scrolling page whose content sits in a centred column.
    fn column(&mut self, ui: &mut Ui, id: &str, max: f32, add: impl FnOnce(&mut Self, &mut Ui)) {
        ScrollArea::vertical().id_salt(id).auto_shrink([false, false]).show(ui, |ui| {
            let total = ui.available_width();
            let col = (total - 40.0).min(max).max(280.0);
            ui.horizontal_top(|ui| {
                ui.add_space(((total - col) / 2.0).max(0.0));
                ui.vertical(|ui| { ui.set_width(col); ui.add_space(18.0); add(self, ui); ui.add_space(130.0); });
            });
        });
    }

    fn page_header(&mut self, ui: &mut Ui, title: &str, subtitle: &str, actions: impl FnOnce(&mut Self, &mut Ui)) {
        let p = self.pal;
        ui.horizontal(|ui| {
            if w::icon_btn(ui, &p, Ic::Back, crate::tr!("Назад", "Back"), 44.0, false).clicked() { self.back(); }
            ui.add_space(8.0);
            ui.vertical(|ui| {
                ui.label(RichText::new(title).font(font(30.0)).color(p.text));
                if !subtitle.is_empty() { ui.label(RichText::new(subtitle).font(font(12.5)).color(p.muted)); }
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| actions(self, ui));
        });
    }

    // ------------------------------------------------------------------ home

    fn home(&mut self, ui: &mut Ui) {
        let p = self.pal;
        let st = core::status();
        let selected = store::get().selected().cloned();
        let sub = selected.as_ref().filter(|s| !s.group.is_empty()).and_then(|s| self.subs.iter().find(|x| x.id == s.group).cloned());
        let name = store::get().s("greetName").trim().to_string();
        let avail_h = ui.available_height();
        self.column(ui, "home", 620.0, |app, ui| {
            ui.horizontal(|ui| {
                let g = w::spaced(ui, "WRAITH", font_m(15.0), alpha(p.text, 0.9), 4.0);
                let (r, _) = ui.allocate_exact_size(g.size(), Sense::hover());
                ui.painter().galley(r.min, g, p.text);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if w::icon_btn(ui, &p, Ic::Plus, crate::tr!("Добавить", "Add"), 44.0, false).clicked() { app.modal = Some(Modal::Import { text: String::new() }); }
                    ui.add_space(2.0);
                    if w::icon_btn(ui, &p, Ic::Refresh, crate::tr!("Обновить подписки", "Update subscriptions"), 44.0, false).clicked() {
                        app.background(crate::tr!("Обновляем подписки…", "Updating subscriptions…"), || {
                            crate::subs::update_all(false);
                            let failed = store::get().subscriptions.iter().filter(|s| !s.error.is_empty()).count();
                            Ok(if failed == 0 { crate::tr!("Подписки обновлены", "Subscriptions updated").into() } else { crate::trf!("Не обновлено: {failed}", "Not updated: {failed}") })
                        });
                    }
                });
            });
            // The block sits low, as on the phone; on a short window it moves up.
            ui.add_space(((avail_h - 600.0) * 0.6).clamp(24.0, 150.0));
            let label = sub.as_ref().map(|s| s.name.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| "Wraith · Xray".into());
            ui.label(RichText::new(label).font(font_m(16.0)).color(p.accent));
            ui.add_space(2.0);
            let text = if name.is_empty() { greeting().to_string() } else { format!("{},\n{name}", greeting()) };
            ui.add(egui::Label::new(RichText::new(text).font(font(46.0)).color(p.text)).wrap());
            ui.add_space(18.0);
            if st.state == State::Connected {
                let exit = crate::stats::exit();
                let secs = st.since.map(|s| s.elapsed().as_secs() as i64).unwrap_or(0);
                let width = ui.available_width();
                ui.horizontal(|ui| {
                    let u = (width - 8.0) / 2.7;
                    w::chip(ui, &p, &util::duration(secs), Some(Ic::Clock), p.text, u);
                    w::chip(ui, &p, if exit.ip.is_empty() { "—" } else { &exit.ip }, Some(Ic::Globe), p.text, u * 1.7);
                });
                ui.horizontal(|ui| {
                    let u = (width - 16.0) / 3.0;
                    let delay = if exit.delay > 0 { crate::trf!("{} мс", "{} ms", exit.delay) } else { crate::tr!("— мс", "— ms").into() };
                    w::chip(ui, &p, &delay, None, if exit.delay > 0 { w::latency_color(&p, exit.delay) } else { p.muted }, u * 0.7);
                    w::chip(ui, &p, &format!("↓ {}", rate(st.rx_rate)), None, p.text, u * 1.15);
                    w::chip(ui, &p, &format!("↑ {}", rate(st.tx_rate)), None, p.text, u * 1.15);
                });
            }
            if st.state == State::Off && !st.error.is_empty() {
                ui.add_space(4.0);
                let g = w::wrapped(ui, &st.error, font(12.5), p.bad, ui.available_width(), 3);
                let (r, _) = ui.allocate_exact_size(g.size(), Sense::hover()); ui.painter().galley(r.min, g, p.bad);
            }
            let release = app.shared.lock().unwrap().release.clone();
            if let Some(r) = release {
                ui.add_space(10.0);
                let resp = w::glass(&p, 20).inner_margin(Margin::symmetric(16, 12)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        let (ir, _) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::hover()); icons::paint(ui.painter(), ir, Ic::Sparkle, p.accent);
                        ui.label(RichText::new(crate::trf!("Доступна версия {}", "Version {} is available", r.version)).font(font_m(14.5)).color(p.text));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| ui.label(RichText::new(crate::tr!("Обновить", "Update")).font(font_m(14.5)).color(p.accent)));
                    });
                }).response.interact(Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
                if resp.clicked() { app.modal = Some(Modal::Update { release: r }); }
            }
            ui.add_space(16.0);
            app.server_card(ui, selected.as_ref(), sub.as_ref().map(|s| s.name.as_str()).unwrap_or(""));
            ui.add_space(14.0);
            if w::connect_button(ui, &p, st.state, st.rx_rate, st.tx_rate).clicked() && st.state != State::Stopping { app.toggle(); }
            ui.add_space(10.0);
            let mode = store::get().s("tunnelMode");
            let caption = match st.state {
                State::Connected => crate::tr!("Трафик идёт через туннель · нажмите, чтобы отключить", "Traffic goes through the tunnel · click to disconnect").to_string(),
                State::Connecting => crate::tr!("Подключаемся…", "Connecting…").into(),
                State::Stopping => crate::tr!("Отключаемся…", "Disconnecting…").into(),
                State::Off => mode_label(&mode).to_string(),
            };
            ui.vertical_centered(|ui| ui.label(RichText::new(caption).font(font(12.5)).color(p.muted)));
        });
    }

    fn server_card(&mut self, ui: &mut Ui, selected: Option<&Profile>, sub_name: &str) {
        let p = self.pal;
        let resp = w::glass(&p, 26).inner_margin(Margin::same(14)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 52.0), Sense::hover());
            let badge = Rect::from_min_size(rect.min, vec2(52.0, 52.0));
            match selected {
                None => icons::at(ui.painter(), badge.center(), 26.0, Ic::Plus, p.accent),
                Some(s) => w::flag_badge(ui, &p, badge, &s.name, false, false),
            }
            let x = badge.right() + 14.0; let max_w = rect.right() - 30.0 - x;
            let title = selected.map(|s| w::display_name(&s.name)).unwrap_or_else(|| crate::tr!("Сервер не выбран", "No server selected").into());
            let detail = match selected {
                Some(s) => format!("{}{}", s.summary(), if s.latency > 0 { crate::trf!(" · {} мс", " · {} ms", s.latency) } else { String::new() }),
                None => crate::tr!("Нажмите, чтобы добавить", "Click to add").into(),
            };
            let mut lines = vec![];
            if !sub_name.is_empty() { lines.push(w::line(ui, sub_name, font(12.0), p.muted, max_w - 18.0)); }
            lines.push(w::line(ui, &title, font_m(21.0), p.text, max_w));
            lines.push(w::line(ui, &detail, font(12.0), p.muted, max_w));
            let h: f32 = lines.iter().map(|g| g.size().y).sum();
            let mut y = rect.center().y - h / 2.0;
            for (i, g) in lines.into_iter().enumerate() {
                let (gw, gh) = (g.size().x, g.size().y);
                ui.painter().galley(pos2(x, y), g, p.text);
                // "Provider  →": which subscription the server comes from.
                if i == 0 && !sub_name.is_empty() { icons::at(ui.painter(), pos2(x + gw + 10.0, y + gh / 2.0), 12.0, Ic::Forward, p.muted); }
                y += gh;
            }
            icons::at(ui.painter(), pos2(rect.right() - 10.0, rect.center().y), 20.0, Ic::Chevron, p.faint);
        }).response.interact(Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
        if resp.hovered() { ui.painter().rect_filled(resp.rect, CornerRadius::same(26), alpha(p.text, 0.03)); }
        if resp.clicked() { if selected.is_none() { self.modal = Some(Modal::Import { text: String::new() }); } else { self.tab = Tab::Servers; } }
    }

    // ------------------------------------------------------------------ servers

    fn servers(&mut self, ui: &mut Ui) {
        let p = self.pal;
        let pinging = self.shared.lock().unwrap().pinging.clone();
        let running = !pinging.is_empty();
        let sort = store::get().s("sort");
        let selected = store::get().selected.clone();
        let q = self.search.trim().to_lowercase();
        let matches = |x: &Profile| q.is_empty() || format!("{} {} {}", x.name, x.host, x.protocol).to_lowercase().contains(&q);
        let order = |list: &mut Vec<Profile>| {
            match sort.as_str() {
                "ping" => list.sort_by_key(|p| if p.latency > 0 { p.latency } else { i64::MAX }),
                "name" => list.sort_by_key(|p| w::display_name(&p.name).to_lowercase()),
                _ => {}
            }
            list.sort_by_key(|p| !p.favorite);
        };
        let mut groups: Vec<(Option<Subscription>, Vec<Profile>)> = vec![];
        for sub in &self.subs { let mut l: Vec<Profile> = self.profiles.iter().filter(|x| x.group == sub.id && matches(x)).cloned().collect(); order(&mut l); groups.push((Some(sub.clone()), l)); }
        let known: HashSet<&str> = self.subs.iter().map(|s| s.id.as_str()).collect();
        let mut manual: Vec<Profile> = self.profiles.iter().filter(|x| !known.contains(x.group.as_str()) && matches(x)).cloned().collect(); order(&mut manual);
        if !manual.is_empty() || (q.is_empty() && self.profiles.iter().any(|x| !known.contains(x.group.as_str()))) { groups.push((None, manual)); }
        if !q.is_empty() { groups.retain(|g| !g.1.is_empty()); }
        let total = self.profiles.len();
        self.column(ui, "servers", 680.0, |app, ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(RichText::new(crate::tr!("Серверы", "Servers")).font(font(30.0)).color(p.text));
                    ui.label(RichText::new(servers_count(total)).font(font(14.0)).color(p.muted));
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if w::icon_btn(ui, &p, Ic::Plus, crate::tr!("Добавить", "Add"), 44.0, false).clicked() { app.modal = Some(Modal::Import { text: String::new() }); }
                    ui.add_space(2.0);
                    if w::icon_btn(ui, &p, if running { Ic::Close } else { Ic::Pulse }, if running { crate::tr!("Остановить проверку", "Stop") } else { crate::tr!("Пинг всех", "Ping all") }, 44.0, running).clicked() {
                        if running { start_ping(None); } else { start_ping(Some(app.profiles.clone())); }
                    }
                });
            });
            if total > 0 { ui.add_space(14.0); w::search_field(ui, &p, &mut app.search, crate::tr!("Поиск по названию или адресу", "Search by name or address")); }
            ui.add_space(6.0);
            if total == 0 { app.empty_servers(ui); return; }
            if groups.is_empty() { ui.add_space(30.0); ui.vertical_centered(|ui| ui.label(RichText::new(crate::tr!("Ничего не найдено", "Nothing found")).color(p.muted))); }
            for (sub, list) in groups {
                let collapsed = q.is_empty() && sub.as_ref().map(|s| s.collapsed).unwrap_or(app.manual_collapsed);
                app.group_header(ui, sub.as_ref(), list.len(), collapsed, &pinging);
                if collapsed { continue; }
                ui.add_space(4.0);
                for pr in &list { app.server_row(ui, pr, pr.id == selected, pinging.contains(&pr.id)); }
            }
        });
    }

    fn empty_servers(&mut self, ui: &mut Ui) {
        let p = self.pal;
        ui.add_space(10.0);
        w::glass(&p, 28).inner_margin(Margin::same(24)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.vertical_centered(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(64.0, 64.0), Sense::hover());
                ui.painter().rect_filled(r, CornerRadius::same(22), p.accent_soft); icons::at(ui.painter(), r.center(), 30.0, Ic::Globe, p.accent);
                ui.add_space(12.0);
                ui.label(RichText::new(crate::tr!("Пока нет серверов", "No servers yet")).font(font_m(21.0)).color(p.text));
                ui.label(RichText::new(crate::tr!("Добавьте ссылку на сервер, HTTPS-подписку или JSON-конфигурацию Xray.", "Add a server link, an HTTPS subscription or an Xray JSON configuration.")).font(font(14.0)).color(p.muted));
                ui.add_space(14.0);
                let width = ui.available_width(); let each = ((width - 10.0) / 2.0).min(220.0);
                ui.horizontal(|ui| {
                    ui.add_space((width - each * 2.0 - 10.0) / 2.0);
                    if w::pill(ui, &p, crate::tr!("Добавить", "Add"), Some(Ic::Plus), true, each).clicked() { self.modal = Some(Modal::Import { text: String::new() }); }
                    ui.add_space(2.0);
                    if w::pill(ui, &p, crate::tr!("Из буфера", "Paste"), Some(Ic::Paste), false, each).clicked() {
                        match util::clipboard_text() { Some(t) => self.import(t), None => self.toast(crate::tr!("Буфер обмена пуст", "The clipboard is empty")) }
                    }
                });
            });
        });
    }

    fn group_header(&mut self, ui: &mut Ui, sub: Option<&Subscription>, count: usize, collapsed: bool, pinging: &HashSet<String>) {
        let p = self.pal;
        ui.add_space(16.0);
        let title = sub.map(|s| s.name.clone()).unwrap_or_else(|| crate::tr!("Мои серверы", "My servers").into());
        let count_text = servers_count(count) + if collapsed { crate::tr!(" · свёрнуто", " · collapsed") } else { "" };
        let (line, line_color) = match sub {
            None => (count_text, p.muted),
            Some(s) if !s.error.is_empty() => (s.error.clone(), p.bad),
            Some(s) if s.updated > 0 => (crate::trf!("Обновлено {} · {count_text}", "Updated {} · {count_text}", util::local_short(s.updated)), p.muted),
            Some(_) => (count_text, p.muted),
        };
        let group_id = sub.map(|s| s.id.clone()).unwrap_or_default();
        let busy = sub.is_some() && self.profiles.iter().any(|x| x.group == group_id && pinging.contains(&x.id));
        ui.horizontal(|ui| {
            let buttons = if sub.is_some() { 3.0 } else { 1.0 } * 46.0;
            let text_w = ui.available_width() - buttons - 8.0;
            let tg = w::line(ui, &title, font_m(21.0), p.text, text_w - 8.0);
            let lg = w::wrapped(ui, &line, font(12.0), line_color, text_w - 8.0, 2);
            let h = tg.size().y + lg.size().y + 8.0;
            let (r, resp) = ui.allocate_exact_size(vec2(text_w, h), Sense::click());
            let th = tg.size().y;
            ui.painter().galley(pos2(r.left() + 6.0, r.top() + 4.0), tg, p.text);
            ui.painter().galley(pos2(r.left() + 6.0, r.top() + 4.0 + th), lg, line_color);
            if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                match sub { Some(s) => { let id = s.id.clone(); store::update(|st| if let Some(x) = st.subscriptions.iter_mut().find(|x| x.id == id) { x.collapsed = !x.collapsed; }); } None => self.manual_collapsed = !self.manual_collapsed }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if let Some(s) = sub {
                    if w::icon_btn(ui, &p, Ic::More, crate::tr!("Действия", "Actions"), 38.0, false).clicked() { self.sub_sheet(s); }
                }
                if w::icon_btn(ui, &p, if busy { Ic::Close } else { Ic::Pulse }, crate::tr!("Пинг группы", "Ping group"), 38.0, busy).clicked() {
                    if busy { start_ping(None); } else { let ids = self.profiles.iter().filter(|x| if sub.is_some() { x.group == group_id } else { !self.subs.iter().any(|s| s.id == x.group) }).map(|x| x.id.clone()).collect(); self.ping_ids(ids); }
                }
                if let Some(s) = sub {
                    if w::icon_btn(ui, &p, Ic::Refresh, crate::tr!("Обновить", "Update"), 38.0, false).clicked() { self.run_act(ui.ctx(), Act::SubUpdate(s.id.clone())); }
                }
            });
        });
        let Some(s) = sub else { return };
        let (used, total, expire) = s.usage();
        if !s.user_info.is_empty() {
            let mut text = format!("{} / {}", util::bytes(used), if total > 0 { util::bytes(total) } else { "∞".into() });
            if expire > 0 { text += &crate::trf!(" · до {}", " · until {}", util::date_of(expire)); }
            ui.horizontal(|ui| { ui.add_space(6.0); ui.label(RichText::new(text).font(font(12.0)).color(p.muted)); });
            if total > 0 {
                let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 6.0), Sense::hover());
                let r = r.shrink2(vec2(6.0, 0.0));
                ui.painter().rect_filled(r, CornerRadius::same(3), p.glass_high);
                let frac = (used as f32 / total as f32).clamp(0.02, 1.0);
                ui.painter().rect_filled(Rect::from_min_size(r.min, vec2(r.width() * frac, 6.0)), CornerRadius::same(3), if used >= total { p.bad } else { p.accent });
            }
        }
        // Provider announcement, and the expiry notice three days before the end.
        let mut message = [s.announce.trim(), s.info_text.trim()].iter().filter(|t| !t.is_empty()).cloned().collect::<Vec<_>>().join("\n");
        let mut color = p.accent; let mut button: Option<(String, String)> = None;
        let days = if expire > 0 { (expire - util::unix_now()).div_euclid(86_400) } else { i64::MAX };
        if expire > 0 && days <= 3 {
            message = if days < 0 { crate::tr!("Подписка истекла", "Subscription expired").into() } else if days == 0 { crate::tr!("Подписка истекает сегодня", "Subscription expires today").into() } else { crate::trf!("Подписка истекает через {days} дн.", "Subscription expires in {days} d.") };
            color = p.bad;
            let link = if s.web_page_url.is_empty() { s.support_url.clone() } else { s.web_page_url.clone() };
            if !link.is_empty() { button = Some((crate::tr!("Продлить", "Renew").into(), link)); }
        } else if !s.support_url.is_empty() && !message.is_empty() { button = Some((crate::tr!("Поддержка", "Support").into(), s.support_url.clone())); }
        if !message.is_empty() && message != "0" {
            ui.add_space(8.0);
            Frame::new().fill(alpha(color, 0.14)).stroke(Stroke::new(1.0_f32, alpha(color, 0.3))).corner_radius(CornerRadius::same(18)).inner_margin(Margin::same(14)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let (ir, _) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover()); icons::paint(ui.painter(), ir, Ic::Bell, color);
                    ui.add_space(4.0);
                    let bw = if button.is_some() { 100.0 } else { 0.0 };
                    let g = w::wrapped(ui, &message, font(12.5), p.text, ui.available_width() - bw, 4);
                    let (r, _) = ui.allocate_exact_size(g.size(), Sense::hover()); ui.painter().galley(r.min, g, p.text);
                    if let Some((label, link)) = &button {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.add(egui::Label::new(RichText::new(label.chars().take(25).collect::<String>()).font(font_m(14.5)).color(color)).sense(Sense::click())).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() { util::open_url(link); }
                        });
                    }
                });
            });
        }
    }

    fn server_row(&mut self, ui: &mut Ui, pr: &Profile, selected: bool, pinging: bool) {
        let p = self.pal;
        let st = core::status();
        let live = st.profile_id == pr.id && st.state == State::Connected;
        let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 66.0), Sense::click());
        let t = ui.ctx().animate_bool_with_time(response.id.with("sel"), selected, 0.2);
        let fill = theme::lerp(p.glass, p.accent_soft, t);
        let edge = theme::lerp(p.stroke, alpha(p.accent, 0.6), t);
        ui.painter().rect(rect, CornerRadius::same(22), fill, Stroke::new(1.0_f32, edge), egui::StrokeKind::Inside);
        if response.hovered() { ui.painter().rect_filled(rect, CornerRadius::same(22), alpha(p.text, 0.03)); }
        let badge = Rect::from_min_size(pos2(rect.left() + 12.0, rect.center().y - 21.0), vec2(42.0, 42.0));
        w::flag_badge(ui, &p, badge, &pr.name, selected, pr.favorite);
        // More button, then the latency chip, right to left.
        let more = Rect::from_center_size(pos2(rect.right() - 24.0, rect.center().y), vec2(40.0, 40.0));
        let more_resp = ui.interact(more, response.id.with("more"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(crate::tr!("Действия", "Actions"));
        if more_resp.hovered() { ui.painter().circle_filled(more.center(), 18.0, alpha(p.text, 0.06)); }
        icons::at(ui.painter(), more.center(), 20.0, Ic::More, p.muted);
        let mut right = more.left() - 4.0;
        if pinging {
            let c = pos2(right - 12.0, rect.center().y);
            let time = ui.ctx().input(|i| i.time) as f32;
            let pts: Vec<egui::Pos2> = (0..=24).map(|i| { let a = time * 5.0 + i as f32 / 24.0 * 4.4; pos2(c.x + 8.0 * a.cos(), c.y + 8.0 * a.sin()) }).collect();
            ui.painter().add(egui::Shape::line(pts, Stroke::new(2.0_f32, p.accent)));
            ui.ctx().request_repaint_after(Duration::from_millis(30));
            right -= 30.0;
        } else if pr.latency > 0 || !pr.ping_error.is_empty() {
            let c = w::latency_color(&p, if pr.latency > 0 { pr.latency } else { -1 });
            let text = if pr.latency > 0 { crate::trf!("{} мс", "{} ms", pr.latency) } else { crate::pinger::short_error(&pr.ping_error).chars().take(18).collect() };
            let g = ui.painter().layout_no_wrap(text, font_m(13.0), c);
            let chip = Rect::from_min_size(pos2(right - g.size().x - 20.0, rect.center().y - 13.0), vec2(g.size().x + 20.0, 26.0));
            ui.painter().rect_filled(chip, CornerRadius::same(13), alpha(c, 0.14));
            ui.painter().galley(chip.center() - g.size() / 2.0, g, c);
            right = chip.left() - 8.0;
        }
        let x = badge.right() + 12.0; let max_w = right - x;
        let name = w::line(ui, &w::display_name(&pr.name), font_m(16.0), p.text, max_w);
        let detail = format!("{}{}", if live { crate::tr!("Подключено · ", "Connected · ") } else { "" }, pr.summary());
        let sub = w::line(ui, &detail, font(12.0), if live { p.accent } else { p.muted }, max_w);
        let nh = name.size().y; let total = nh + sub.size().y;
        ui.painter().galley(pos2(x, rect.center().y - total / 2.0), name, p.text);
        ui.painter().galley(pos2(x, rect.center().y - total / 2.0 + nh), sub, p.muted);
        let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
        if more_resp.clicked() || response.secondary_clicked() { self.server_sheet(pr); }
        else if response.double_clicked() { self.select(&pr.id, true); }
        else if response.clicked() { self.select(&pr.id, false); }
        ui.add_space(8.0);
    }

    // ------------------------------------------------------------------ statistics

    fn stats(&mut self, ui: &mut Ui) {
        let p = self.pal;
        let st = core::status();
        let on = st.state == State::Connected;
        if self.history.1.map(|t| t.elapsed() > Duration::from_secs(10)).unwrap_or(true) { crate::stats::save(); self.history = (crate::stats::read(), Some(Instant::now())); }
        let speed = crate::stats::speed();
        let exit = crate::stats::exit();
        self.column(ui, "stats", 680.0, |app, ui| {
            ui.label(RichText::new(crate::tr!("Статистика", "Statistics")).font(font(30.0)).color(p.text));
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover()); icons::paint(ui.painter(), r, Ic::Lock, p.good);
                ui.label(RichText::new(crate::tr!("Считается только на этом компьютере и никуда не отправляется", "Counted on this computer only, never sent anywhere")).font(font(12.5)).color(p.muted));
            });
            ui.add_space(12.0);
            w::glass(&p, 28).inner_margin(Margin::same(20)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(RichText::new(crate::tr!("Загрузка", "Download")).font(font_m(13.0)).color(p.muted));
                        ui.label(RichText::new(if on { rate(st.rx_rate) } else { "—".into() }).font(font_m(30.0)).color(p.accent));
                    });
                    ui.with_layout(Layout::right_to_left(Align::Max), |ui| ui.vertical(|ui| {
                        ui.with_layout(Layout::top_down(Align::Max), |ui| {
                            ui.label(RichText::new(crate::tr!("Отдача", "Upload")).font(font_m(13.0)).color(p.muted));
                            ui.label(RichText::new(if on { rate(st.tx_rate) } else { "—".into() }).font(font_m(22.0)).color(p.text));
                        });
                    }));
                });
                ui.add_space(10.0);
                let r = w::speed_chart(ui, &p, &speed, 150.0);
                if !on { ui.painter().text(r.center(), Align2::CENTER_CENTER, crate::tr!("Подключитесь, чтобы увидеть график", "Connect to see the chart"), font(12.5), p.faint); }
            });
            w::section_title(ui, &p, crate::tr!("Эта сессия", "This session"));
            w::group(ui, &p, |ui| {
                let server = if on { w::display_name(&st.profile_name) } else { crate::tr!("не подключено", "not connected").into() };
                w::setting_row(ui, &p, crate::tr!("Сервер", "Server"), "", Some(Ic::Globe), false, Trailing::Text(&server), false);
                w::divider(ui, &p);
                let dur = if on { util::duration(st.since.map(|s| s.elapsed().as_secs() as i64).unwrap_or(0)) } else { "00:00:00".into() };
                w::setting_row(ui, &p, crate::tr!("Время подключения", "Connected for"), "", Some(Ic::Clock), false, Trailing::Text(&dur), false);
                w::divider(ui, &p);
                w::setting_row(ui, &p, crate::tr!("Получено", "Received"), "", Some(Ic::Down), false, Trailing::Text(&util::bytes(st.rx)), false);
                w::divider(ui, &p);
                w::setting_row(ui, &p, crate::tr!("Отправлено", "Sent"), "", Some(Ic::Up), false, Trailing::Text(&util::bytes(st.tx)), false);
                if on && !exit.ip.is_empty() {
                    w::divider(ui, &p);
                    w::setting_row(ui, &p, crate::tr!("Внешний IP", "Exit IP"), &exit.country.to_uppercase(), Some(Ic::Shield), false, Trailing::Text(&exit.ip), false);
                }
                if on && exit.delay > 0 {
                    w::divider(ui, &p);
                    w::setting_row(ui, &p, crate::tr!("Задержка через туннель", "Delay through the tunnel"), "", Some(Ic::Pulse), false, Trailing::Text(&crate::trf!("{} мс", "{} ms", exit.delay)), false);
                }
                if on {
                    w::divider(ui, &p);
                    w::setting_row(ui, &p, crate::tr!("Режим", "Mode"), "", Some(Ic::Monitor), false, Trailing::Text(mode_label(&store::get().s("tunnelMode"))), false);
                }
            });
            w::section_title(ui, &p, crate::tr!("Последние 14 дней", "Last 14 days"));
            let days = crate::stats::last_days(&app.history.0, 14);
            w::glass(&p, 28).inner_margin(Margin::same(20)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                let today = days.last().map(|d| d.1).unwrap_or(0);
                let week: i64 = days.iter().rev().take(7).map(|d| d.1).sum();
                let all: i64 = days.iter().map(|d| d.1).sum();
                ui.columns(3, |c| {
                    for (i, (label, v)) in [(crate::tr!("Сегодня", "Today"), today), (crate::tr!("7 дней", "7 days"), week), (crate::tr!("14 дней", "14 days"), all)].iter().enumerate() {
                        c[i].label(RichText::new(*label).font(font_m(13.0)).color(p.muted));
                        c[i].label(RichText::new(util::bytes(*v)).font(font_m(17.0)).color(p.text));
                    }
                });
                ui.add_space(14.0);
                w::day_bars(ui, &p, &days, 110.0);
            });
            w::section_title(ui, &p, crate::tr!("Подробнее", "More"));
            w::group(ui, &p, |ui| {
                if w::setting_row(ui, &p, crate::tr!("Журнал соединений", "Connection log"), crate::tr!("Какой адрес через какой выход ушёл", "Which address left through which outbound"), Some(Ic::Route), false, Trailing::Chevron, true).clicked() {
                    let text = crate::log::text();
                    let lines: Vec<&str> = text.lines().filter(|l| l.contains(" accepted ")).collect();
                    let tail = lines[lines.len().saturating_sub(80)..].join("\n");
                    app.modal = Some(Modal::Info { title: crate::tr!("Соединения · Xray", "Connections · Xray").into(), text: if tail.trim().is_empty() { crate::tr!("Пока пусто. Откройте сайт и вернитесь сюда.", "Empty so far. Open a website and come back.").into() } else { tail }, copy: true });
                }
                w::divider(ui, &p);
                if w::setting_row(ui, &p, crate::tr!("Очистить историю трафика", "Clear traffic history"), "", Some(Ic::Delete), true, Trailing::None, true).clicked() {
                    app.modal = Some(Modal::Confirm { title: crate::tr!("Очистить историю?", "Clear the history?").into(), text: crate::tr!("Счётчики по дням будут удалены.", "Daily counters will be deleted.").into(), action: crate::tr!("Очистить", "Clear").into(), danger: true, run: Confirmed::ClearHistory });
                }
            });
        });
    }

    // ------------------------------------------------------------------ settings

    fn settings(&mut self, ui: &mut Ui) {
        match self.pages.last().copied() {
            Some(Page::Routing) => self.routing_page(ui),
            Some(Page::Tunnel) => self.tunnel_page(ui),
            Some(Page::Subscriptions) => self.subscriptions_page(ui),
            Some(Page::Ping) => self.ping_page(ui),
            Some(Page::Appearance) => self.appearance_page(ui),
            Some(Page::Logs) => self.logs_page(ui),
            None => self.settings_root(ui),
        }
    }

    fn settings_root(&mut self, ui: &mut Ui) {
        let p = self.pal;
        let s = store::get().settings.clone();
        let rules = store::get().rules.len();
        self.column(ui, "settings", 680.0, |app, ui| {
            ui.label(RichText::new(crate::tr!("Настройки", "Settings")).font(font(30.0)).color(p.text));
            ui.label(RichText::new(format!("Wraith {} · Xray", env!("CARGO_PKG_VERSION"))).font(font(12.5)).color(p.muted));
            w::section_title(ui, &p, crate::tr!("Подключение", "Connection"));
            w::group(ui, &p, |ui| {
                if w::setting_row(ui, &p, crate::tr!("Маршрутизация", "Routing"), &crate::trf!("{} · своих правил: {rules}", "{} · custom rules: {rules}", routing_label(&s.s("mode"))), Some(Ic::Route), false, Trailing::Chevron, true).clicked() { app.pages.push(Page::Routing); }
                w::divider(ui, &p);
                if w::setting_row(ui, &p, crate::tr!("Туннель и обход блокировок", "Tunnel and DPI bypass"), crate::tr!("DPI, DNS, MTU, локальный прокси", "DPI, DNS, MTU, local proxy"), Some(Ic::Shield), false, Trailing::Chevron, true).clicked() { app.pages.push(Page::Tunnel); }
                w::divider(ui, &p);
                if w::setting_row(ui, &p, crate::tr!("Как подключаться", "How to connect"), mode_label(&s.s("tunnelMode")), Some(Ic::Monitor), false, Trailing::Chevron, true).clicked() {
                    app.choice(crate::tr!("Режим подключения", "Connection mode"), &[crate::tr!("Системный прокси + TUN · весь трафик", "System proxy + TUN · all traffic"), crate::tr!("Только системный прокси · без прав администратора", "System proxy only · no administrator rights"), crate::tr!("Только TUN", "TUN only")], &["both", "proxy", "tun"], "tunnelMode");
                }
                w::divider(ui, &p);
                if w::switch_row(ui, &p, crate::tr!("Подключаться при запуске", "Connect on start"), crate::tr!("Когда открывается Wraith", "When Wraith opens"), Some(Ic::Power), s.b("connectOnStart", false)) { app.toggle_setting("connectOnStart"); }
                w::divider(ui, &p);
                if w::switch_row(ui, &p, crate::tr!("Быстрый сервер автоматически", "Fastest server automatically"), crate::tr!("Выбирать самый быстрый сервер профиля, проверка раз в 3 минуты", "Pick the fastest server of the profile, checked every 3 minutes"), Some(Ic::Bolt), s.b("autoSelect", false)) { app.toggle_setting("autoSelect"); }
            });
            w::section_title(ui, &p, crate::tr!("Серверы", "Servers"));
            w::group(ui, &p, |ui| {
                if w::setting_row(ui, &p, crate::tr!("Подписки", "Subscriptions"), crate::tr!("Обновление, сортировка, данные для провайдера", "Updates, sorting, data for the provider"), Some(Ic::Refresh), false, Trailing::Chevron, true).clicked() { app.pages.push(Page::Subscriptions); }
                w::divider(ui, &p);
                if w::setting_row(ui, &p, crate::tr!("Проверка задержки", "Latency check"), ping_label(&s.s("pingType")), Some(Ic::Pulse), false, Trailing::Chevron, true).clicked() { app.pages.push(Page::Ping); }
            });
            w::section_title(ui, &p, crate::tr!("Оформление", "Appearance"));
            w::group(ui, &p, |ui| {
                let sub = if backdrop::has_photo() { crate::tr!("Своё фото", "Own photo") } else { crate::tr!("Живой градиент", "Live gradient") };
                if w::setting_row(ui, &p, crate::tr!("Фон, цвета и приветствие", "Background, colours and greeting"), sub, Some(Ic::Palette), false, Trailing::Chevron, true).clicked() { app.pages.push(Page::Appearance); }
            });
            w::section_title(ui, &p, crate::tr!("Приложение", "App"));
            w::group(ui, &p, |ui| {
                if w::switch_row(ui, &p, crate::tr!("Запускать вместе с Windows", "Start with Windows"), crate::tr!("Свёрнутым в трей", "Minimised to the tray"), Some(Ic::Sparkle), s.b("autostart", false)) {
                    let next = !s.b("autostart", false);
                    match crate::install::set_autostart(next) { Ok(()) => store::update(|st| st.set("autostart", next.into())), Err(e) => app.toast(e) }
                }
                w::divider(ui, &p);
                if w::switch_row(ui, &p, crate::tr!("Сворачивать в трей при закрытии", "Minimise to the tray on close"), if super::tray_available() { "" } else { crate::tr!("Значок в трее сейчас недоступен", "The tray icon is not available now") }, Some(Ic::Down), s.b("minimizeToTray", true)) { app.toggle_setting("minimizeToTray"); }
                w::divider(ui, &p);
                if w::switch_row(ui, &p, crate::tr!("Проверять обновления", "Check for updates"), crate::tr!("GitHub Releases, в запросе только версия", "GitHub Releases, the request carries only the version"), Some(Ic::Download), s.b("checkUpdates", true)) { app.toggle_setting("checkUpdates"); }
                w::divider(ui, &p);
                if w::switch_row(ui, &p, crate::tr!("Подробный журнал ядра", "Detailed core log"), "", Some(Ic::Log), s.b("debugLogs", false)) { app.toggle_setting("debugLogs"); }
            });
            w::section_title(ui, &p, crate::tr!("Данные", "Data"));
            w::group(ui, &p, |ui| {
                if w::setting_row(ui, &p, crate::tr!("Экспорт резервной копии", "Export a backup"), crate::tr!("Содержит ключи подключений — храните в закрытом месте", "Contains connection keys — keep it private"), Some(Ic::Download), false, Trailing::Chevron, true).clicked() {
                    app.modal = Some(Modal::Confirm { title: crate::tr!("Экспорт содержит ключи", "The export contains keys").into(), text: crate::tr!("Резервная копия — обычный JSON-файл с вашими серверами. Не отправляйте его другим.", "The backup is a plain JSON file with your servers. Do not send it to anyone.").into(), action: crate::tr!("Экспорт", "Export").into(), danger: false, run: Confirmed::Export });
                }
                w::divider(ui, &p);
                if w::setting_row(ui, &p, crate::tr!("Восстановить из файла", "Restore from a file"), crate::tr!("Заменит текущие серверы и настройки", "Replaces current servers and settings"), Some(Ic::Layers), false, Trailing::Chevron, true).clicked() {
                    app.modal = Some(Modal::Confirm { title: crate::tr!("Заменить текущие данные?", "Replace current data?").into(), text: crate::tr!("Серверы, подписки и настройки будут заменены. Подключение остановится.", "Servers, subscriptions and settings will be replaced. The connection stops.").into(), action: crate::tr!("Выбрать файл", "Pick a file").into(), danger: false, run: Confirmed::Restore });
                }
                w::divider(ui, &p);
                if w::setting_row(ui, &p, crate::tr!("Журнал", "Log"), crate::tr!("События приложения и ядра Xray", "App and Xray core events"), Some(Ic::Log), false, Trailing::Chevron, true).clicked() { app.pages.push(Page::Logs); }
                w::divider(ui, &p);
                if w::setting_row(ui, &p, crate::tr!("Папка данных", "Data folder"), &store::folder().display().to_string(), Some(Ic::Folder), false, Trailing::Chevron, true).clicked() { util::open_path(&store::folder()); }
                w::divider(ui, &p);
                if w::setting_row(ui, &p, crate::tr!("Сбросить настройки", "Reset settings"), crate::tr!("Серверы, подписки и правила останутся", "Servers, subscriptions and rules are kept"), Some(Ic::Refresh), true, Trailing::None, true).clicked() {
                    app.modal = Some(Modal::Confirm { title: crate::tr!("Сбросить настройки?", "Reset the settings?").into(), text: crate::tr!("Настройки вернутся к значениям по умолчанию.", "Settings return to defaults.").into(), action: crate::tr!("Сбросить", "Reset").into(), danger: true, run: Confirmed::Reset });
                }
            });
            w::section_title(ui, &p, crate::tr!("О приложении", "About"));
            w::group(ui, &p, |ui| {
                if w::setting_row(ui, &p, crate::tr!("Проверить обновления", "Check for updates"), &crate::trf!("Установлена {}", "Installed {}", env!("CARGO_PKG_VERSION")), Some(Ic::Sparkle), false, Trailing::Chevron, true).clicked() {
                    let shared = app.shared.clone();
                    app.background(crate::tr!("Проверяем обновления…", "Checking for updates…"), move || {
                        let r = updater::check()?;
                        if r.available { shared.lock().unwrap().release = Some(r.clone()); toast_update(r); Ok(String::new()) }
                        else { Ok(crate::trf!("Установлена последняя версия {}", "The latest version {} is installed", env!("CARGO_PKG_VERSION"))) }
                    });
                }
                w::divider(ui, &p);
                if w::setting_row(ui, &p, crate::tr!("О Wraith", "About Wraith"), crate::tr!("Клиент Xray. Без телеметрии и аналитики. GPL-3.0", "Xray client. No telemetry or analytics. GPL-3.0"), Some(Ic::Info), false, Trailing::Chevron, true).clicked() {
                    app.modal = Some(Modal::Info { title: format!("Wraith {}", env!("CARGO_PKG_VERSION")), text: crate::trf!("Клиент для VLESS, VMess, Trojan, Shadowsocks, Hysteria2 и WireGuard на ядре Xray.\n\nНикакой статистики об использовании не собирается. Обновления проверяются через GitHub, в запросе только версия приложения.\n\nЛицензия GPL-3.0-or-later. Xray-core — MPL-2.0, флаги — flag-icons (MIT).\n\nhttps://github.com/{}", "A client for VLESS, VMess, Trojan, Shadowsocks, Hysteria2 and WireGuard on the Xray core.\n\nNo usage statistics are collected. Updates are checked on GitHub; the request carries only the app version.\n\nLicence GPL-3.0-or-later. Xray-core — MPL-2.0, flags — flag-icons (MIT).\n\nhttps://github.com/{}", updater::REPO), copy: false });
                }
                w::divider(ui, &p);
                if w::setting_row(ui, &p, crate::tr!("Исходный код", "Source code"), &format!("github.com/{}", updater::REPO), Some(Ic::Link), false, Trailing::Chevron, true).clicked() { util::open_url(&format!("https://github.com/{}", updater::REPO)); }
            });
        });
    }

    fn routing_page(&mut self, ui: &mut Ui) {
        let p = self.pal;
        let s = store::get().settings.clone();
        let rules = store::get().rules.clone();
        let mode = s.s_or("mode", "rules");
        self.column(ui, "routing", 680.0, |app, ui| {
            app.page_header(ui, crate::tr!("Маршрутизация", "Routing"), crate::tr!("Какой трафик идёт через VPN", "Which traffic goes through the VPN"), |_, _| {});
            ui.add_space(14.0);
            if let Some(v) = w::segmented(ui, &p, &["rules", "global", "direct"], &[crate::tr!("Правила", "Rules"), crate::tr!("Всё в VPN", "All VPN"), crate::tr!("Напрямую", "Direct")], &mode) { app.set("mode", v.into()); }
            ui.add_space(6.0);
            ui.horizontal(|ui| { ui.add_space(8.0); ui.label(RichText::new(match mode.as_str() {
                "global" => crate::tr!("Весь трафик идёт через VPN; правила не применяются.", "All traffic goes through the VPN; rules are not applied."),
                "direct" => crate::tr!("VPN не используется: весь трафик идёт напрямую.", "The VPN is not used: all traffic goes direct."),
                _ => crate::tr!("Сначала правила провайдера, затем ваши правила ниже.", "Provider rules first, then your rules below."),
            }).font(font(12.5)).color(p.muted)); });
            w::section_title(ui, &p, crate::tr!("Общие", "General"));
            w::group(ui, &p, |ui| {
                if w::switch_row(ui, &p, crate::tr!("Локальная сеть напрямую", "Local network direct"), crate::tr!("Роутер, принтеры, частные IP", "Router, printers, private IPs"), Some(Ic::Wifi), s.b("bypassLan", true)) { app.toggle_setting("bypassLan"); }
                w::divider(ui, &p);
                if w::switch_row(ui, &p, crate::tr!("Определять домены", "Detect domains"), crate::tr!("Sniffing: правила по доменам работают и для программ с DoH", "Sniffing: domain rules work even for programs with DoH"), Some(Ic::Eye), s.b("sniff", true)) { app.toggle_setting("sniff"); }
            });
            w::section_title(ui, &p, crate::tr!("Свои правила", "Custom rules"));
            w::group(ui, &p, |ui| {
                if rules.is_empty() && w::setting_row(ui, &p, crate::tr!("Правил пока нет", "No rules yet"), crate::tr!("Например: сайты банков — напрямую", "For example: bank sites — direct"), Some(Ic::Route), false, Trailing::Chevron, true).clicked() {
                    app.modal = Some(Modal::Rule { index: None, rule: new_rule() });
                }
                for (i, r) in rules.iter().enumerate() {
                    if i > 0 { w::divider(ui, &p); }
                    let action = r.s_or("action", "proxy");
                    let values = r.s("value").replace('\n', " ");
                    let title = if r.s("name").is_empty() { values.chars().take(60).collect() } else { r.s("name") };
                    let subtitle = format!("{} → {}{}\n{}", crate::model::rule_kind_label(&r.s("kind")), match action.as_str() { "direct" => crate::tr!("напрямую", "direct"), "block" => crate::tr!("блокировать", "block"), _ => crate::tr!("через VPN", "through VPN") },
                        if r.b("enabled", true) { "" } else { crate::tr!(" · выключено", " · off") }, values.chars().take(120).collect::<String>());
                    let icon = match action.as_str() { "block" => Ic::Close, "direct" => Ic::Chevron, _ => Ic::Shield };
                    if w::setting_row(ui, &p, &title, &subtitle, Some(icon), false, Trailing::None, true).clicked() {
                        app.modal = Some(Modal::Sheet { title: r.s("name"), subtitle: crate::model::rule_kind_label(&r.s("kind")).into(), actions: vec![
                            (crate::tr!("Изменить", "Edit").into(), Ic::Edit, false, Act::RuleEdit(i)),
                            (if r.b("enabled", true) { crate::tr!("Выключить", "Turn off") } else { crate::tr!("Включить", "Turn on") }.into(), Ic::Power, false, Act::RuleToggle(i)),
                            (crate::tr!("Удалить", "Delete").into(), Ic::Delete, true, Act::RuleDelete(i)),
                        ] });
                    }
                }
            });
            ui.add_space(16.0);
            let each = (ui.available_width() - 10.0) / 2.0;
            ui.horizontal(|ui| {
                if w::pill(ui, &p, crate::tr!("Добавить", "Add"), Some(Ic::Plus), true, each).clicked() { app.modal = Some(Modal::Rule { index: None, rule: new_rule() }); }
                ui.add_space(2.0);
                if w::pill(ui, &p, crate::tr!("Импорт Happ", "Import Happ"), Some(Ic::Download), false, each).clicked() { app.input(crate::tr!("Ссылка happ://routing/…", "happ://routing/… link"), String::new(), Target::HappRouting); }
            });
        });
    }

    fn tunnel_page(&mut self, ui: &mut Ui) {
        let p = self.pal;
        let s = store::get().settings.clone();
        self.column(ui, "tunnel", 680.0, |app, ui| {
            app.page_header(ui, crate::tr!("Туннель", "Tunnel"), crate::tr!("Обход блокировок, сеть и локальный прокси", "DPI bypass, network and local proxy"), |_, _| {});
            w::section_title(ui, &p, crate::tr!("Обход DPI", "DPI bypass"));
            w::group(ui, &p, |ui| {
                if w::switch_row(ui, &p, crate::tr!("Фрагментация", "Fragmentation"), crate::tr!("Дробит начало TLS-соединения с сервером, чтобы DPI его не узнал", "Splits the start of the TLS connection so DPI does not recognise it"), Some(Ic::Layers), s.b("fragment", false)) { app.toggle_setting("fragment"); }
                if s.b("fragment", false) {
                    w::divider(ui, &p);
                    if w::setting_row(ui, &p, crate::tr!("Что дробить", "What to split"), &s.s_or("fragmentPackets", "tlshello"), None, false, Trailing::None, true).clicked() { app.choice(crate::tr!("Какие пакеты дробить", "Which packets"), &[crate::tr!("tlshello · только ClientHello", "tlshello · ClientHello only"), "1-3", "1-5"], &["tlshello", "1-3", "1-5"], "fragmentPackets"); }
                    w::divider(ui, &p);
                    if w::setting_row(ui, &p, crate::tr!("Длина частей", "Part length"), &s.s_or("fragmentLength", "50-100"), None, false, Trailing::None, true).clicked() { app.input(crate::tr!("Длина фрагмента (от-до)", "Fragment length (from-to)"), s.s_or("fragmentLength", "50-100"), Target::Setting("fragmentLength".into())); }
                    w::divider(ui, &p);
                    if w::setting_row(ui, &p, crate::tr!("Задержка, мс", "Delay, ms"), &s.s_or("fragmentInterval", "10-20"), None, false, Trailing::None, true).clicked() { app.input(crate::tr!("Задержка фрагмента, мс", "Fragment delay, ms"), s.s_or("fragmentInterval", "10-20"), Target::Setting("fragmentInterval".into())); }
                    w::divider(ui, &p);
                    let ms = s.s("fragmentMaxSplit");
                    if w::setting_row(ui, &p, crate::tr!("Максимум частей", "Maximum parts"), if ms.is_empty() { crate::tr!("без ограничения", "no limit") } else { &ms }, None, false, Trailing::None, true).clicked() { app.input(crate::tr!("Максимальное разделение (пусто — без ограничения)", "Maximum split (empty — no limit)"), ms.clone(), Target::Setting("fragmentMaxSplit".into())); }
                }
                w::divider(ui, &p);
                if w::switch_row(ui, &p, crate::tr!("Шумы", "Noise"), crate::tr!("Мусорные UDP-пакеты перед первым настоящим: помогает Hysteria2 и QUIC", "Junk UDP packets before the first real one: helps Hysteria2 and QUIC"), Some(Ic::Sparkle), s.b("noise", false)) { app.toggle_setting("noise"); }
                if s.b("noise", false) {
                    let t = s.s_or("noiseType", "rand");
                    w::divider(ui, &p);
                    if w::setting_row(ui, &p, crate::tr!("Тип", "Type"), &t, None, false, Trailing::None, true).clicked() { app.choice(crate::tr!("Тип шума", "Noise type"), &["rand", "array", "str", "hex", "base64"], &["rand", "array", "str", "hex", "base64"], "noiseType"); }
                    w::divider(ui, &p);
                    if t == "rand" { if w::setting_row(ui, &p, crate::tr!("Размер, байт", "Size, bytes"), &s.s_or("noiseRand", "10-20"), None, false, Trailing::None, true).clicked() { app.input(crate::tr!("Размер шума (от-до)", "Noise size (from-to)"), s.s_or("noiseRand", "10-20"), Target::Setting("noiseRand".into())); } }
                    else if w::setting_row(ui, &p, crate::tr!("Пакет", "Packet"), &s.s_or("noisePacket", "1A,2B,3C"), None, false, Trailing::None, true).clicked() { app.input(crate::tr!("Пакет шума", "Noise packet"), s.s_or("noisePacket", "1A,2B,3C"), Target::Setting("noisePacket".into())); }
                    w::divider(ui, &p);
                    if w::setting_row(ui, &p, crate::tr!("Задержка, мс", "Delay, ms"), &s.s_or("noiseDelay", "10-16"), None, false, Trailing::None, true).clicked() { app.input(crate::tr!("Задержка шума, мс", "Noise delay, ms"), s.s_or("noiseDelay", "10-16"), Target::Setting("noiseDelay".into())); }
                }
                w::divider(ui, &p);
                if w::switch_row(ui, &p, "Mux", crate::tr!("Несколько соединений в одном. Не применяется к Vision, Hysteria и WireGuard", "Several connections in one. Not used with Vision, Hysteria and WireGuard"), Some(Ic::Route), s.b("mux", false)) { app.toggle_setting("mux"); }
                if s.b("mux", false) {
                    w::divider(ui, &p);
                    if w::setting_row(ui, &p, crate::tr!("Потоков на соединение", "Streams per connection"), &s.i("muxConnections", 4).to_string(), None, false, Trailing::None, true).clicked() { app.number(crate::tr!("Потоков на соединение", "Streams per connection"), "muxConnections", 1, 1024); }
                }
                w::divider(ui, &p);
                let quic = s.s_or("quic", "allow");
                if w::setting_row(ui, &p, "QUIC (UDP 443)", if quic == "reject" { crate::tr!("Блокировать — браузеры уйдут на TCP", "Block — browsers fall back to TCP") } else { crate::tr!("Пропускать", "Allow") }, Some(Ic::Bolt), false, Trailing::Chevron, true).clicked() {
                    app.choice("QUIC", &[crate::tr!("Пропускать", "Allow"), crate::tr!("Блокировать, браузер перейдёт на TCP", "Block, the browser falls back to TCP")], &["allow", "reject"], "quic");
                }
            });
            w::section_title(ui, &p, "DNS");
            w::group(ui, &p, |ui| {
                if w::switch_row(ui, &p, crate::tr!("DNS из конфигурации", "DNS from the configuration"), crate::tr!("DNS провайдера из JSON или профиля маршрутизации", "Provider DNS from the JSON or routing profile"), Some(Ic::Dns), s.b("dnsFromJson", false)) { app.toggle_setting("dnsFromJson"); }
                w::divider(ui, &p);
                let custom = s.b("customDns", false);
                if w::switch_row(ui, &p, crate::tr!("Свой DNS", "Own DNS"), if custom { crate::tr!("Запросы идут на адрес ниже", "Queries go to the address below") } else { crate::tr!("Выключено — DNS системы (адаптер, роутер)", "Off — the system's DNS (adapter, router)") }, Some(Ic::Globe), custom) { app.toggle_setting("customDns"); }
                if custom {
                    w::divider(ui, &p);
                    if w::setting_row(ui, &p, crate::tr!("Адрес DNS", "DNS address"), &s.s("tunnelDns"), None, false, Trailing::None, true).clicked() { app.input(crate::tr!("DNS: https://…, tls://… или IP", "DNS: https://…, tls://… or IP"), s.s("tunnelDns"), Target::Setting("tunnelDns".into())); }
                }
                w::divider(ui, &p);
                let ip = crate::model::ip_mode(&s);
                if w::setting_row(ui, &p, crate::tr!("IP-протокол", "IP family"), match ip.as_str() { "v4" => "IPv4", "v6" => "IPv6", _ => crate::tr!("Авто", "Auto") }, Some(Ic::Layers), false, Trailing::Chevron, true).clicked() {
                    app.choice(crate::tr!("Какие адреса запрашивать", "Which addresses to query"), &[crate::tr!("Авто · оба", "Auto · both"), crate::tr!("Только IPv4 · меньше сбоев", "IPv4 only · fewer failures"), crate::tr!("Предпочитать IPv6", "Prefer IPv6")], &["auto", "v4", "v6"], "ipMode");
                }
            });
            w::section_title(ui, &p, crate::tr!("Сеть", "Network"));
            w::group(ui, &p, |ui| {
                if w::setting_row(ui, &p, "MTU", &crate::trf!("{} байт · адаптер Wraith", "{} bytes · adapter Wraith", s.i("mtu", 1500)), Some(Ic::Layers), false, Trailing::Chevron, true).clicked() { app.number("MTU", "mtu", 1280, 9000); }
            });
            w::section_title(ui, &p, crate::tr!("Локальный прокси", "Local proxy"));
            w::group(ui, &p, |ui| {
                let lan = s.b("allowLan", false);
                let host = if lan { "0.0.0.0" } else { "127.0.0.1" };
                if w::setting_row(ui, &p, "SOCKS5", &format!("{host}:{}", s.i("proxyPort", 2081)), Some(Ic::Lock), false, Trailing::Icon(Ic::Edit), true).clicked() { app.number(crate::tr!("Порт SOCKS5", "SOCKS5 port"), "proxyPort", 1024, 65535); }
                w::divider(ui, &p);
                if w::setting_row(ui, &p, "HTTP", &format!("{host}:{}", s.i("httpProxyPort", 2082)), Some(Ic::Globe), false, Trailing::Icon(Ic::Edit), true).clicked() { app.number(crate::tr!("Порт HTTP", "HTTP port"), "httpProxyPort", 1024, 65535); }
                w::divider(ui, &p);
                if w::switch_row(ui, &p, crate::tr!("Доступ из локальной сети", "Access from the local network"), crate::tr!("Другие устройства смогут подключиться по логину и паролю", "Other devices can connect with the login and password"), Some(Ic::Wifi), lan) { app.toggle_setting("allowLan"); }
                if lan {
                    w::divider(ui, &p);
                    if w::setting_row(ui, &p, crate::tr!("Логин", "Login"), "wraith", None, false, Trailing::Icon(Ic::Copy), true).clicked() { ui.ctx().copy_text("wraith".into()); app.toast(crate::tr!("Скопировано", "Copied")); }
                    w::divider(ui, &p);
                    let pw = s.s("proxyPassword");
                    if w::setting_row(ui, &p, crate::tr!("Пароль", "Password"), &format!("{}••••••••", pw.chars().take(3).collect::<String>()), None, false, Trailing::Icon(Ic::Copy), true).clicked() { ui.ctx().copy_text(pw.clone()); app.toast(crate::tr!("Пароль скопирован", "Password copied")); }
                    w::divider(ui, &p);
                    if w::setting_row(ui, &p, crate::tr!("Новый пароль", "New password"), crate::tr!("Старый перестанет работать", "The old one stops working"), Some(Ic::Refresh), false, Trailing::None, true).clicked() {
                        app.modal = Some(Modal::Confirm { title: crate::tr!("Сменить пароль прокси?", "Change the proxy password?").into(), text: crate::tr!("Устройствам, которые используют прокси, нужно будет ввести новый.", "Devices using the proxy will need the new one.").into(), action: crate::tr!("Сменить", "Change").into(), danger: false, run: Confirmed::NewProxyPassword });
                    }
                }
            });
        });
    }

    fn subscriptions_page(&mut self, ui: &mut Ui) {
        let p = self.pal;
        let s = store::get().settings.clone();
        self.column(ui, "subs", 680.0, |app, ui| {
            app.page_header(ui, crate::tr!("Подписки", "Subscriptions"), "", |_, _| {});
            w::section_title(ui, &p, crate::tr!("Обновление", "Updates"));
            w::group(ui, &p, |ui| {
                if w::switch_row(ui, &p, crate::tr!("Обновлять автоматически", "Update automatically"), crate::tr!("Фоновая проверка раз в час, по интервалу подписки", "Background check every hour, by the subscription's interval"), Some(Ic::Refresh), s.b("autoUpdate", true)) { app.toggle_setting("autoUpdate"); }
                w::divider(ui, &p);
                if w::switch_row(ui, &p, crate::tr!("Обновлять при запуске", "Update on start"), crate::tr!("Если подошёл интервал подписки", "If the subscription's interval has passed"), Some(Ic::Sparkle), s.b("updateOnOpen", false)) { app.toggle_setting("updateOnOpen"); }
                w::divider(ui, &p);
                if w::switch_row(ui, &p, crate::tr!("Пинг при запуске", "Ping on start"), "", Some(Ic::Pulse), s.b("pingOnOpen", false)) { app.toggle_setting("pingOnOpen"); }
            });
            w::section_title(ui, &p, crate::tr!("Порядок серверов", "Server order"));
            w::group(ui, &p, |ui| {
                let labels = [crate::tr!("Как у провайдера", "As the provider sends"), crate::tr!("По задержке", "By latency"), crate::tr!("По имени", "By name")];
                let values = ["none", "ping", "name"];
                let cur = values.iter().position(|v| *v == s.s_or("sort", "none")).unwrap_or(0);
                if w::setting_row(ui, &p, crate::tr!("Сортировка", "Sorting"), labels[cur], Some(Ic::Sort), false, Trailing::Chevron, true).clicked() { app.choice(crate::tr!("Сортировка", "Sorting"), &labels, &values, "sort"); }
            });
            w::section_title(ui, &p, crate::tr!("Что получает провайдер", "What the provider receives"));
            w::group(ui, &p, |ui| {
                if w::switch_row(ui, &p, crate::tr!("Передавать HWID", "Send HWID"), crate::tr!("Случайный ID установки, имя и версия системы — для лимита устройств", "Random install ID, system name and version — for device limits"), Some(Ic::Monitor), s.b("sendHwid", true)) { app.toggle_setting("sendHwid"); }
                w::divider(ui, &p);
                let ua = s.s("userAgent");
                if w::setting_row(ui, &p, "User-Agent", if ua.is_empty() { crate::tr!("автоматически", "automatic") } else { &ua }, Some(Ic::Code), false, Trailing::Chevron, true).clicked() { app.input(crate::tr!("User-Agent (пусто — автоматически)", "User-Agent (empty — automatic)"), ua.clone(), Target::Setting("userAgent".into())); }
            });
        });
    }

    fn ping_page(&mut self, ui: &mut Ui) {
        let p = self.pal;
        let s = store::get().settings.clone();
        let t = s.s_or("pingType", "proxyGet");
        self.column(ui, "ping", 680.0, |app, ui| {
            app.page_header(ui, crate::tr!("Проверка задержки", "Latency check"), "", |_, _| {});
            w::section_title(ui, &p, crate::tr!("Способ", "Method"));
            w::group(ui, &p, |ui| {
                if w::setting_row(ui, &p, crate::tr!("Как измерять", "How to measure"), ping_label(&t), Some(Ic::Pulse), false, Trailing::Chevron, true).clicked() {
                    app.choice(crate::tr!("Способ проверки", "Test method"), &[crate::tr!("Proxy GET · реальный запрос через сервер", "Proxy GET · a real request through the server"), crate::tr!("Proxy HEAD · без тела ответа", "Proxy HEAD · without a body"), crate::tr!("TCP · подключение к порту", "TCP · connect to the port"), "ICMP · ping"], &["proxyGet", "proxyHead", "tcp", "icmp"], "pingType");
                }
                if t.starts_with("proxy") {
                    w::divider(ui, &p);
                    let double = s.s("pingMode") == "double";
                    if w::setting_row(ui, &p, crate::tr!("Соединения", "Connections"), if double { crate::tr!("Два отдельных", "Two separate") } else { crate::tr!("Одно (keep-alive)", "One (keep-alive)") }, None, false, Trailing::None, true).clicked() {
                        app.choice(crate::tr!("Соединения", "Connections"), &[crate::tr!("Одно (keep-alive)", "One (keep-alive)"), crate::tr!("Два отдельных", "Two separate")], &["keepalive", "double"], "pingMode");
                    }
                    w::divider(ui, &p);
                    let url = s.s_or("pingUrl", "https://www.gstatic.com/generate_204");
                    if w::setting_row(ui, &p, crate::tr!("Адрес проверки", "Test address"), &url, None, false, Trailing::None, true).clicked() { app.input(crate::tr!("HTTPS-адрес", "HTTPS address"), url.clone(), Target::Setting("pingUrl".into())); }
                }
                w::divider(ui, &p);
                if w::setting_row(ui, &p, crate::tr!("Тайм-аут", "Timeout"), &crate::trf!("{} мс", "{} ms", s.i("pingTimeout", 5000)), None, false, Trailing::None, true).clicked() { app.number(crate::tr!("Тайм-аут, мс", "Timeout, ms"), "pingTimeout", 1000, 30000); }
            });
            ui.add_space(10.0);
            let text = match t.as_str() {
                x if x.starts_with("proxy") => crate::tr!("Запрос идёт через временное ядро Xray, VPN включать не нужно. Показывается лучший из двух результатов.", "The request goes through a temporary Xray core; the VPN need not be on. The better of two results is shown."),
                "tcp" => crate::tr!("TCP проверяет только, открыт ли порт. Для Hysteria2 и WireGuard автоматически используется запрос через сервер.", "TCP only checks that the port is open. Hysteria2 and WireGuard automatically use a request through the server."),
                _ => crate::tr!("ICMP может блокироваться сетью или провайдером.", "ICMP may be blocked by the network or provider."),
            };
            ui.horizontal(|ui| { ui.add_space(8.0); ui.add(egui::Label::new(RichText::new(format!("{text}{}", crate::tr!(" При включённом VPN проверка идёт мимо туннеля, напрямую к серверу.", " With the VPN on, the check bypasses the tunnel and goes straight to the server."))).font(font(12.5)).color(p.muted)).wrap()); });
        });
    }

    fn appearance_page(&mut self, ui: &mut Ui) {
        let p = self.pal;
        let s = store::get().settings.clone();
        let photo = backdrop::has_photo();
        self.column(ui, "appearance", 680.0, |app, ui| {
            app.page_header(ui, crate::tr!("Оформление", "Appearance"), "", |_, _| {});
            w::section_title(ui, &p, crate::tr!("Фон", "Background"));
            w::group(ui, &p, |ui| {
                if w::setting_row(ui, &p, if photo { crate::tr!("Сменить фото", "Change the photo") } else { crate::tr!("Поставить своё фото", "Use your own photo") }, crate::tr!("Копия хранится только в папке Wraith", "A copy is kept in Wraith's folder only"), Some(Ic::Image), false, Trailing::Chevron, true).clicked() {
                    app.background(crate::tr!("Ставим фон…", "Setting the background…"), || {
                        let Some(path) = util::file_dialog(false, crate::tr!("Фото для фона", "Background photo"), "", &["*.jpg", "*.jpeg", "*.png"])? else { return Ok(String::new()) };
                        backdrop::set_photo(&path)?;
                        Ok(crate::tr!("Фон обновлён", "Background updated").into())
                    });
                }
                if photo {
                    w::divider(ui, &p);
                    if w::setting_row(ui, &p, crate::tr!("Убрать фото", "Remove the photo"), crate::tr!("Вернуть живой градиент", "Back to the live gradient"), Some(Ic::Delete), true, Trailing::None, true).clicked() {
                        backdrop::remove_photo();
                        if store::get().s("accent") == "photo" { app.set("accent", "sakura".into()); }
                    }
                    w::divider(ui, &p);
                    let (mut blur, mut dim) = app.slider.unwrap_or((s.settings_f("bgBlur", 0.0), s.settings_f("bgDim", 0.35)));
                    Frame::new().inner_margin(Margin::symmetric(20, 12)).show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 6.0;
                        ui.spacing_mut().slider_width = ui.available_width() - 60.0;
                        ui.visuals_mut().selection.bg_fill = p.accent; ui.visuals_mut().widgets.inactive.bg_fill = p.glass_high;
                        ui.label(RichText::new(crate::tr!("Размытие", "Blur")).color(p.text));
                        let r1 = ui.add(egui::Slider::new(&mut blur, 0.0..=1.0).show_value(false).trailing_fill(true));
                        ui.label(RichText::new(crate::tr!("Затемнение", "Dimming")).color(p.text));
                        let r2 = ui.add(egui::Slider::new(&mut dim, 0.0..=1.0).show_value(false).trailing_fill(true));
                        if r1.changed() || r2.changed() { app.slider = Some((blur, dim)); }
                        // Blur is rebuilt on release (it is the costly part); dimming follows live.
                        if (r1.drag_stopped() || r2.drag_stopped() || ((r1.changed() || r2.changed()) && !r1.dragged() && !r2.dragged())) && app.slider.is_some() {
                            store::update(|st| { st.set("bgBlur", (blur as f64).into()); st.set("bgDim", (dim as f64).into()); });
                            app.slider = None;
                        }
                    });
                }
            });
            w::section_title(ui, &p, crate::tr!("Акцент", "Accent"));
            let current = s.s_or("accent", "sakura");
            let mut options: Vec<(String, Color32)> = theme::ACCENTS.iter().map(|(k, c)| (k.to_string(), *c)).collect();
            if let Some(c) = backdrop::photo_accent() { options.push(("photo".into(), c)); }
            if let Some(c) = theme::system_accent() { options.push(("system".into(), c)); }
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(10.0, 10.0);
                for (key, color) in options {
                    let on = key == current;
                    let (r, resp) = ui.allocate_exact_size(vec2(84.0, 84.0), Sense::click());
                    ui.painter().rect(r, CornerRadius::same(20), if on { p.glass_high } else { p.glass }, Stroke::new(if on { 2.0 } else { 1.0_f32 }, if on { color } else { p.stroke }), egui::StrokeKind::Inside);
                    if resp.hovered() { ui.painter().rect_filled(r, CornerRadius::same(20), alpha(p.text, 0.04)); }
                    let c = pos2(r.center().x, r.top() + 30.0);
                    ui.painter().circle_filled(c, 17.0, color);
                    if on { icons::at(ui.painter(), c, 18.0, Ic::Check, Color32::from_black_alpha(180)); }
                    ui.painter().text(pos2(r.center().x, r.bottom() - 16.0), Align2::CENTER_CENTER, theme::accent_name(&key), font(11.5), p.muted);
                    if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() && !on { app.set("accent", key.into()); }
                }
            });
            w::section_title(ui, &p, crate::tr!("Главный экран", "Home screen"));
            w::group(ui, &p, |ui| {
                let name = s.s("greetName");
                if w::setting_row(ui, &p, crate::tr!("Как к вам обращаться", "What to call you"), if name.is_empty() { crate::tr!("не задано", "not set") } else { &name }, Some(Ic::User), false, Trailing::Chevron, true).clicked() { app.input(crate::tr!("Имя в приветствии", "Name in the greeting"), name.clone(), Target::Setting("greetName".into())); }
                w::divider(ui, &p);
                if w::switch_row(ui, &p, crate::tr!("Внешний IP и задержка", "Exit IP and delay"), crate::tr!("Раз в минуту, через туннель (cloudflare.com/cdn-cgi/trace)", "Once a minute, through the tunnel (cloudflare.com/cdn-cgi/trace)"), Some(Ic::Globe), s.b("showExitIp", true)) { app.toggle_setting("showExitIp"); }
                w::divider(ui, &p);
                if w::switch_row(ui, &p, crate::tr!("Чёрный фон (AMOLED)", "Pure black (AMOLED)"), crate::tr!("Когда нет своего фото", "When there is no photo"), Some(Ic::Eye), s.b("amoled", false)) { app.toggle_setting("amoled"); }
            });
        });
    }

    fn logs_page(&mut self, ui: &mut Ui) {
        let p = self.pal;
        let version = crate::log::version();
        if self.log_cache.0 != version { self.log_cache = (version, crate::log::text()); }
        let total = ui.available_width();
        let col = (total - 40.0).min(900.0);
        let left = (total - col) / 2.0;
        let area = ui.available_rect_before_wrap();
        let inner = Rect::from_min_max(pos2(area.left() + left, area.top() + 18.0), pos2(area.left() + left + col, area.bottom() - 100.0));
        ui.scope_builder(egui::UiBuilder::new().max_rect(inner), |ui| {
            self.page_header(ui, crate::tr!("Журнал", "Log"), "", |app, ui| {
                if w::icon_btn(ui, &p, Ic::Delete, crate::tr!("Очистить", "Clear"), 44.0, false).clicked() { crate::log::clear(); }
                ui.add_space(2.0);
                if w::icon_btn(ui, &p, Ic::Folder, crate::tr!("Открыть файл", "Open the file"), 44.0, false).clicked() { util::open_path(&crate::log::path()); }
                ui.add_space(2.0);
                if w::icon_btn(ui, &p, Ic::Copy, crate::tr!("Скопировать", "Copy"), 44.0, false).clicked() { ui.ctx().copy_text(crate::log::text()); app.toast(crate::tr!("Журнал скопирован", "Log copied")); }
            });
            ui.add_space(12.0);
            w::glass(&p, 22).inner_margin(Margin::same(16)).show(ui, |ui| {
                ScrollArea::both().id_salt("log").auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
                    let text = if self.log_cache.1.trim().is_empty() { crate::tr!("Пусто", "Empty").to_string() } else { self.log_cache.1.clone() };
                    ui.add(egui::Label::new(RichText::new(text).font(egui::FontId::monospace(11.5)).color(p.text)).extend());
                });
            });
        });
    }

    // ------------------------------------------------------------------ dialogs

    fn modals(&mut self, ctx: &egui::Context) {
        let Some(mut modal) = self.modal.take() else { return };
        let p = self.pal;
        let mut keep = true;
        let mut after: Option<Box<dyn FnOnce(&mut App)>> = None;
        let width = (ctx.content_rect().width() - 48.0).min(if matches!(modal, Modal::Editor { .. } | Modal::Info { .. }) { 720.0 } else { 520.0 });
        let response = egui::Modal::new(egui::Id::new("modal")).backdrop_color(Color32::from_black_alpha(150))
            .frame(Frame::new().fill(p.surface).stroke(Stroke::new(1.0_f32, p.stroke)).corner_radius(CornerRadius::same(28)).inner_margin(Margin::same(24))).show(ctx, |ui| {
            ui.set_width(width);
            let title = |ui: &mut Ui, t: &str| { ui.label(RichText::new(t).font(font(22.0)).color(p.text)); };
            match &mut modal {
                Modal::Import { text } => {
                    title(ui, crate::tr!("Добавить сервер или подписку", "Add a server or subscription"));
                    ui.label(RichText::new(crate::tr!("Ссылка vless://, vmess://, trojan://, ss://, hy2://, wireguard://, HTTPS-подписка или JSON Xray.", "A vless://, vmess://, trojan://, ss://, hy2://, wireguard:// link, an HTTPS subscription or Xray JSON.")).font(font(12.5)).color(p.muted));
                    ui.add_space(8.0);
                    ScrollArea::vertical().max_height(240.0).show(ui, |ui| { ui.add(egui::TextEdit::multiline(text).desired_rows(7).desired_width(f32::INFINITY).hint_text(crate::tr!("Вставьте сюда…", "Paste here…")).font(egui::FontId::monospace(12.5)).margin(Margin::same(12))); });
                    ui.add_space(14.0);
                    ui.horizontal(|ui| {
                        if w::pill(ui, &p, crate::tr!("Из буфера", "Paste"), Some(Ic::Paste), false, 0.0).clicked() { if let Some(t) = util::clipboard_text() { *text = t; } }
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if w::pill(ui, &p, crate::tr!("Добавить", "Add"), None, true, 0.0).clicked() && !text.trim().is_empty() { let t = text.clone(); keep = false; after = Some(Box::new(move |app: &mut App| app.import(t))); }
                            if text_btn(ui, &p, crate::tr!("Отмена", "Cancel"), p.muted).clicked() { keep = false; }
                        });
                    });
                }
                Modal::Info { title: t, text, copy } => {
                    title(ui, t);
                    ui.add_space(8.0);
                    Frame::new().fill(p.bg).corner_radius(CornerRadius::same(16)).inner_margin(Margin::same(12)).show(ui, |ui| {
                        ScrollArea::both().max_height((ctx.content_rect().height() - 260.0).clamp(120.0, 520.0)).auto_shrink([false, true]).show(ui, |ui| {
                            ui.add(egui::Label::new(RichText::new(text.as_str()).font(egui::FontId::monospace(12.0)).color(p.text)).wrap());
                        });
                    });
                    ui.add_space(12.0);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if text_btn(ui, &p, crate::tr!("Готово", "Done"), p.accent).clicked() { keep = false; }
                        if *copy && text_btn(ui, &p, crate::tr!("Скопировать", "Copy"), p.accent).clicked() { ui.ctx().copy_text(text.clone()); }
                    });
                }
                Modal::Confirm { title: t, text, action, danger, run } => {
                    title(ui, t);
                    ui.add_space(4.0);
                    ui.label(RichText::new(text.as_str()).font(font(14.5)).color(p.muted));
                    ui.add_space(16.0);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if text_btn(ui, &p, action, if *danger { p.bad } else { p.accent }).clicked() { keep = false; let r = run.clone(); let c = ctx.clone(); after = Some(Box::new(move |app: &mut App| app.confirmed(&c, r))); }
                        if text_btn(ui, &p, crate::tr!("Отмена", "Cancel"), p.muted).clicked() { keep = false; }
                    });
                }
                Modal::Admin => {
                    title(ui, crate::tr!("Нужны права администратора", "Administrator rights needed"));
                    ui.add_space(4.0);
                    ui.label(RichText::new(crate::tr!("Режим TUN создаёт сетевой адаптер и меняет маршруты Windows. Перезапустите Wraith от имени администратора или подключитесь через системный прокси.", "TUN mode creates a network adapter and changes Windows routes. Restart Wraith as administrator or connect through the system proxy.")).color(p.muted));
                    ui.add_space(16.0);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if w::pill(ui, &p, crate::tr!("Перезапустить", "Restart"), Some(Ic::Shield), true, 0.0).clicked() {
                            keep = false;
                            after = Some(Box::new(|app: &mut App| {
                                if util::relaunch_elevated("--connect --elevated") { core::shutdown(); std::process::exit(0); } else { app.toast(crate::tr!("Запуск от администратора отменён", "Start as administrator was cancelled")); }
                            }));
                        }
                        if text_btn(ui, &p, crate::tr!("Системный прокси", "System proxy"), p.accent).clicked() { keep = false; after = Some(Box::new(|app: &mut App| { store::update(|s| s.set("tunnelMode", "proxy".into())); app.connect_selected(); })); }
                    });
                }
                Modal::Rule { index, rule } => {
                    title(ui, if index.is_some() { crate::tr!("Правило", "Rule") } else { crate::tr!("Новое правило", "New rule") });
                    ui.add_space(6.0);
                    let mut name = rule.s("name"); let mut kind = rule.s("kind"); let mut value = rule.s("value"); let mut action = rule.s_or("action", "direct");
                    field_label(ui, &p, crate::tr!("Название", "Name")); ui.add(egui::TextEdit::singleline(&mut name).desired_width(f32::INFINITY).margin(Margin::symmetric(12, 10)).hint_text(crate::tr!("Например: Банки", "For example: Banks")));
                    field_label(ui, &p, crate::tr!("Что сравнивать", "Match by"));
                    let current = crate::model::rule_kind_label(&kind).to_string();
                    egui::ComboBox::from_id_salt("rule-kind").selected_text(current).width(ui.available_width()).show_ui(ui, |ui| {
                        for (k, l) in crate::model::rule_kinds() { if ui.selectable_label(kind == k, l).clicked() { kind = k.to_string(); } }
                    });
                    field_label(ui, &p, crate::tr!("Значения — по одному в строке", "Values, one per line"));
                    let hint = match kind.as_str() { "domain_suffix" => "sberbank.ru\ngosuslugi.ru", "ip_cidr" => "10.0.0.0/8", "geosite" => "category-ads-all", "geoip" => "ru", "process" => "telegram.exe", _ => "" };
                    ui.add(egui::TextEdit::multiline(&mut value).desired_rows(5).desired_width(f32::INFINITY).hint_text(hint).margin(Margin::same(12)));
                    field_label(ui, &p, crate::tr!("Действие", "Action"));
                    if let Some(v) = w::segmented(ui, &p, &["direct", "proxy", "block"], &[crate::tr!("Напрямую", "Direct"), crate::tr!("Через VPN", "Through VPN"), crate::tr!("Блокировать", "Block")], &action) { action = v; }
                    rule.insert("name".into(), name.into()); rule.insert("kind".into(), kind.into()); rule.insert("value".into(), value.clone().into()); rule.insert("action".into(), action.into());
                    ui.add_space(16.0);
                    ui.horizontal(|ui| {
                        if let Some(i) = *index { if text_btn(ui, &p, crate::tr!("Удалить", "Delete"), p.bad).clicked() { keep = false; after = Some(Box::new(move |app: &mut App| app.run_act(&app_ctx(), Act::RuleDelete(i)))); } }
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if text_btn(ui, &p, crate::tr!("Сохранить", "Save"), p.accent).clicked() && !value.trim().is_empty() {
                                keep = false; let r = rule.clone(); let i = *index;
                                after = Some(Box::new(move |app: &mut App| { store::update(|s| match i { Some(i) if i < s.rules.len() => s.rules[i] = r, _ => s.rules.push(r) }); app.changed("rules"); }));
                            }
                            if text_btn(ui, &p, crate::tr!("Отмена", "Cancel"), p.muted).clicked() { keep = false; }
                        });
                    });
                }
                Modal::Input { title: t, value, hint, multiline, password, target, error } => {
                    title(ui, t);
                    if !hint.is_empty() { ui.label(RichText::new(hint.as_str()).color(p.muted)); }
                    ui.add_space(8.0);
                    let edit = if *multiline { ui.add(egui::TextEdit::multiline(value).desired_rows(5).desired_width(f32::INFINITY).margin(Margin::same(12)).font(egui::FontId::monospace(12.5))) }
                        else { ui.add(egui::TextEdit::singleline(value).desired_width(f32::INFINITY).password(*password).margin(Margin::symmetric(12, 10))) };
                    if !edit.has_focus() && ui.memory(|m| m.focused().is_none()) { edit.request_focus(); }
                    let enter = !*multiline && edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if !error.is_empty() { ui.label(RichText::new(error.as_str()).font(font(12.5)).color(p.bad)); }
                    ui.add_space(14.0);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if text_btn(ui, &p, crate::tr!("Сохранить", "Save"), p.accent).clicked() || enter {
                            let (t, v, ti, h, m, pw) = (target.clone(), value.clone(), t.clone(), hint.clone(), *multiline, *password);
                            keep = false;
                            // A value that fails validation keeps the dialog open with the reason.
                            after = Some(Box::new(move |app: &mut App| {
                                if let Err(e) = app.save_input(&t, &v) { app.modal = Some(Modal::Input { title: ti, value: v, hint: h, multiline: m, password: pw, target: t, error: e }); }
                            }));
                        }
                        if text_btn(ui, &p, crate::tr!("Отмена", "Cancel"), p.muted).clicked() { keep = false; }
                    });
                }
                Modal::Choice { title: t, labels, values, selected, key } => {
                    title(ui, t);
                    ui.add_space(6.0);
                    for (i, l) in labels.iter().enumerate() {
                        let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 46.0), Sense::click());
                        if resp.hovered() { ui.painter().rect_filled(r, CornerRadius::same(14), alpha(p.text, 0.05)); }
                        let c = pos2(r.left() + 20.0, r.center().y);
                        let on = i == *selected;
                        ui.painter().circle_stroke(c, 9.0, Stroke::new(2.0_f32, if on { p.accent } else { p.faint }));
                        if on { ui.painter().circle_filled(c, 5.0, p.accent); }
                        let g = w::line(ui, l, font(15.0), p.text, r.width() - 50.0);
                        ui.painter().galley(pos2(r.left() + 42.0, r.center().y - g.size().y / 2.0), g, p.text);
                        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                            keep = false; let (k, v) = (key.clone(), values[i].clone());
                            after = Some(Box::new(move |app: &mut App| { let val: Value = if let Ok(n) = v.parse::<i64>() { n.into() } else { v.into() }; app.set(&k, val); }));
                        }
                    }
                    ui.add_space(8.0);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| if text_btn(ui, &p, crate::tr!("Отмена", "Cancel"), p.muted).clicked() { keep = false; });
                }
                Modal::Sheet { title: t, subtitle, actions } => {
                    title(ui, t);
                    if !subtitle.is_empty() { ui.label(RichText::new(subtitle.as_str()).font(font(12.5)).color(p.muted)); }
                    ui.add_space(8.0);
                    for (label, icon, danger, act) in actions.iter() {
                        let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 48.0), Sense::click());
                        if resp.hovered() { ui.painter().rect_filled(r, CornerRadius::same(14), alpha(p.text, 0.05)); }
                        let color = if *danger { p.bad } else { p.text };
                        icons::at(ui.painter(), pos2(r.left() + 22.0, r.center().y), 20.0, *icon, if *danger { p.bad } else { p.accent });
                        ui.painter().text(pos2(r.left() + 50.0, r.center().y), Align2::LEFT_CENTER, label, font(15.5), color);
                        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() { keep = false; let a = act.clone(); let c = ctx.clone(); after = Some(Box::new(move |app: &mut App| app.run_act(&c, a))); }
                    }
                }
                Modal::Editor { id, title: t, value, error } => {
                    title(ui, t);
                    ui.add_space(8.0);
                    ScrollArea::vertical().max_height((ctx.content_rect().height() - 280.0).clamp(160.0, 560.0)).show(ui, |ui| {
                        ui.add(egui::TextEdit::multiline(value).code_editor().desired_rows(16).desired_width(f32::INFINITY).margin(Margin::same(12)));
                    });
                    if !error.is_empty() { ui.label(RichText::new(error.as_str()).font(font(12.5)).color(p.bad)); }
                    ui.add_space(12.0);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if text_btn(ui, &p, crate::tr!("Сохранить", "Save"), p.accent).clicked() {
                            match save_profile(id, value) { Ok(()) => { keep = false; after = Some(Box::new(|app: &mut App| { app.toast(crate::tr!("Сохранено", "Saved")); app.changed("mode"); })); } Err(e) => *error = e }
                        }
                        if text_btn(ui, &p, crate::tr!("Отмена", "Cancel"), p.muted).clicked() { keep = false; }
                    });
                }
                Modal::Update { release } => {
                    title(ui, &format!("Wraith {}", release.version));
                    ui.add_space(6.0);
                    ScrollArea::vertical().max_height(260.0).show(ui, |ui| { ui.label(RichText::new(&release.notes).color(p.muted)); });
                    let progress = self.shared.lock().unwrap().download;
                    if let Some((done, total)) = progress {
                        ui.add_space(8.0);
                        ui.add(egui::ProgressBar::new(if total > 0 { done as f32 / total as f32 } else { 0.0 }).fill(p.accent).text(crate::trf!("{} из {}", "{} of {}", util::bytes(done as i64), util::bytes(total as i64))));
                    }
                    ui.add_space(14.0);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if progress.is_none() && text_btn(ui, &p, crate::tr!("Скачать и установить", "Download and install"), p.accent).clicked() {
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
                        if text_btn(ui, &p, crate::tr!("Позже", "Later"), p.muted).clicked() { keep = false; }
                    });
                }
            }
        });
        if response.should_close() && !matches!(modal, Modal::Update { .. }) { keep = false; }
        if keep && self.modal.is_none() { self.modal = Some(modal); }
        if let Some(f) = after { f(self); }
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
        egui::Area::new(egui::Id::new("snackbar")).anchor(Align2::CENTER_BOTTOM, [0.0, -104.0]).order(egui::Order::Foreground).interactable(false).show(ctx, |ui| {
            Frame::new().fill(p.surface_highest).stroke(Stroke::new(1.0_f32, p.stroke)).corner_radius(CornerRadius::same(18)).inner_margin(Margin::symmetric(18, 13)).shadow(ctx.style().visuals.popup_shadow).show(ui, |ui| {
                ui.set_max_width(520.0);
                ui.horizontal(|ui| {
                    if busy.is_some() { ui.add(egui::Spinner::new().size(16.0).color(p.accent)); }
                    ui.label(RichText::new(text).color(p.text));
                });
            });
        });
        ctx.request_repaint_after(Duration::from_millis(500));
    }
}

static INFO_QUEUE: Mutex<Option<(String, String)>> = Mutex::new(None);
static UPDATE_QUEUE: Mutex<Option<Release>> = Mutex::new(None);
/// A result window from a background job (diagnostics).
fn show_info(title: &str, text: String) { *INFO_QUEUE.lock().unwrap() = Some((title.into(), text)); super::repaint(); }
fn toast_update(r: Release) { *UPDATE_QUEUE.lock().unwrap() = Some(r); super::repaint(); }
fn app_ctx() -> egui::Context { super::context().unwrap_or_default() }

fn field_label(ui: &mut Ui, p: &Palette, text: &str) { ui.add_space(6.0); ui.label(RichText::new(text).font(font_m(12.5)).color(p.muted)); }
fn text_btn(ui: &mut Ui, p: &Palette, text: &str, color: Color32) -> egui::Response {
    let g = ui.painter().layout_no_wrap(text.to_string(), font_m(15.0), color);
    let (r, resp) = ui.allocate_exact_size(vec2(g.size().x + 28.0, 40.0), Sense::click());
    if resp.hovered() { ui.painter().rect_filled(r, CornerRadius::same(20), alpha(color, 0.1)); }
    ui.painter().galley(r.center() - g.size() / 2.0, g, color);
    let _ = p;
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}
fn new_rule() -> Obj { crate::obj! {"name" => "", "kind" => "domain_suffix", "value" => "", "action" => "direct", "enabled" => true} }

/// The backup lands in the user's own folder; Windows gives it the folder's (user-only) permissions.
fn write_private(path: &std::path::Path, data: &[u8]) -> std::io::Result<()> { std::fs::write(path, data) }

/// The edited share link or JSON replaces the server, keeping its place, group and favourite.
fn save_profile(id: &str, value: &str) -> Result<(), String> {
    let parsed = crate::importer::parse(value.trim())?;
    if parsed.profiles.len() != 1 { return Err(crate::tr!("Нужна конфигурация ровно одного сервера.", "Exactly one server configuration is required.").into()); }
    let mut n = parsed.profiles.into_iter().next().unwrap();
    store::update(|s| {
        if let Some(old) = s.profiles.iter_mut().find(|p| p.id == id) {
            n.id = old.id.clone(); n.group = old.group.clone(); n.favorite = old.favorite;
            if n.routing.is_none() { n.routing = old.routing.clone(); }
            if n.description.is_empty() { n.description = old.description.clone(); }
            *old = n;
        }
    });
    Ok(())
}

/// What the core is given for one server and what it says about it.
fn diagnose(p: &Profile) -> String {
    let mut out = String::new();
    out += &crate::trf!("Сервер: {} · {}:{}\n", "Server: {} · {}:{}\n", p.summary(), p.host, p.port);
    if p.servers > 1 { out += &crate::trf!("Серверов в документе: {}{}\n", "Servers in the document: {}{}\n", p.servers, if p.balancer { crate::tr!(" (балансировщик)", " (balancer)") } else { "" }); }
    let state = store::get().clone();
    out += &crate::trf!("Маршрутизация: {}, режим: {}\n", "Routing: {}, mode: {}\n", routing_label(&state.s("mode")), mode_label(&state.s("tunnelMode")));
    let dns = crate::routing::system_resolvers();
    out += &crate::trf!("DNS системы: {}\n", "System DNS: {}\n", if dns.is_empty() { "—".into() } else { dns.join(", ") });
    let exe = core::xray_exe();
    out += &crate::trf!("Ядро: {}{}\n", "Core: {}{}\n", exe.display(), if exe.exists() { "" } else { crate::tr!(" (не найдено)", " (not found)") });
    out += &crate::trf!("Права администратора (TUN): {}\n", "Administrator rights (TUN): {}\n", if util::is_admin() { crate::tr!("есть", "yes") } else { crate::tr!("нет", "no") });
    match crate::xray::build(&state, p, &dns, util::free_port()) {
        Ok(b) => {
            out += crate::tr!("\nКонфигурация Xray собрана ✓\n", "\nXray configuration built ✓\n");
            if exe.exists() {
                let bytes = serde_json::to_vec(&b.config).unwrap_or_default();
                match util::run_hidden_stdin(&exe, &["run", "-test", "-c", "stdin:"], &bytes, 20) {
                    Ok(_) => out += crate::tr!("Ядро приняло конфигурацию ✓\n", "The core accepted the configuration ✓\n"),
                    Err(e) => out += &crate::trf!("Ядро отклонило конфигурацию: {}\n", "The core rejected the configuration: {}\n", util::tail(&e, 800)),
                }
            }
        }
        Err(e) => out += &crate::trf!("\nОшибка конфигурации: {}\n", "\nConfiguration error: {}\n", crate::log::safe(e)),
    }
    let err = core::status().error;
    if !err.is_empty() { out += &crate::trf!("\nПоследняя ошибка подключения: {err}\n", "\nLast connection error: {err}\n"); }
    let log = crate::log::text();
    let lines: Vec<&str> = log.lines().filter(|l| !l.contains(" accepted ")).collect();
    let tail = lines[lines.len().saturating_sub(25)..].join("\n");
    if !tail.trim().is_empty() { out += &crate::trf!("\nЖурнал:\n{}", "\nLog:\n{}", crate::log::safe(tail)); }
    out
}

trait SettingsF { fn settings_f(&self, key: &str, fallback: f32) -> f32; }
impl SettingsF for Obj { fn settings_f(&self, key: &str, fallback: f32) -> f32 { self.get(key).and_then(|v| v.as_f64()).map(|v| v as f32).unwrap_or(fallback) } }

/// Accent from the settings (preset, photo or desktop) on the dark base.
fn current_palette() -> Palette {
    let s = store::get();
    let key = s.s("accent");
    let accent = match key.as_str() {
        "photo" => backdrop::photo_accent(),
        "system" => theme::system_accent(),
        k => theme::preset(k),
    }.unwrap_or(theme::ACCENTS[0].1);
    theme::palette(accent, s.b("amoled"))
}

impl eframe::App for App {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] { egui::Rgba::from(self.pal.bg).to_array() }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) { self.frame(ctx); }
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) { start_ping(None); core::shutdown(); }
}

impl App {
    /// One frame of the window (also driven headless by the screenshot test).
    pub fn frame(&mut self, ctx: &egui::Context) {
        self.frames += 1;
        if self.frames == 2 { super::window_ready(self.start_hidden); }
        self.refresh_cache(ctx);
        if ctx.input(|i| i.viewport().close_requested()) {
            if store::get().b("minimizeToTray") && super::tray_available() { ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose); super::hide_window(); }
        }
        if let Some((t, text)) = INFO_QUEUE.lock().unwrap().take() { self.modal = Some(Modal::Info { title: t, text, copy: true }); }
        if let Some(r) = UPDATE_QUEUE.lock().unwrap().take() { self.modal = Some(Modal::Update { release: r }); }
        if self.modal.is_none() && ctx.input(|i| i.key_pressed(egui::Key::Escape) || i.pointer.button_pressed(egui::PointerButton::Extra1)) { self.back(); }
        let p = self.pal;
        let st = core::status();
        let s = store::get().settings.clone();
        let (blur, dim) = self.slider.unwrap_or((s.settings_f("bgBlur", 0.0), s.settings_f("bgDim", 0.35)));
        let energy = ctx.animate_value_with_time(egui::Id::new("energy"), match st.state { State::Connected => 1.25, State::Connecting => 1.0, _ => 0.8 }, 1.2);

        egui::CentralPanel::default().frame(Frame::new().fill(Color32::TRANSPARENT)).show(ctx, |ui| {
            let full = ui.max_rect();
            let animated = backdrop::paint(ctx, ui.painter(), full, &p, blur, dim, energy);
            if animated { ctx.request_repaint_after(Duration::from_millis(50)); }
            if self.in_settings { self.settings(ui); }
            else { match self.tab { Tab::Home => self.home(ui), Tab::Servers => self.servers(ui), Tab::Stats => self.stats(ui) } }
        });

        // A soft fade under the floating bar keeps scrolled content from colliding with it.
        let screen = ctx.content_rect();
        backdrop::vertical_gradient(&ctx.layer_painter(egui::LayerId::new(egui::Order::Middle, egui::Id::new("nav-fade"))), Rect::from_min_max(pos2(screen.left(), screen.bottom() - 150.0), screen.max), &[(0.0, Color32::TRANSPARENT), (1.0, Color32::from_black_alpha(140))]);
        let nav_w = (screen.width() - 36.0).min(560.0);
        egui::Area::new(egui::Id::new("nav")).fixed_pos(pos2(screen.center().x - nav_w / 2.0, screen.bottom() - 66.0 - 18.0)).order(egui::Order::Foreground).show(ctx, |ui| {
            ui.set_width(nav_w);
            let tab = match self.tab { Tab::Home => 0, Tab::Servers => 1, Tab::Stats => 2 };
            let items = [(Ic::Home, crate::tr!("Главная", "Home")), (Ic::Globe, crate::tr!("Серверы", "Servers")), (Ic::Bars, crate::tr!("Статистика", "Statistics"))];
            match w::floating_nav(ui, &p, &items, tab, self.in_settings) {
                Some(usize::MAX) => { if self.in_settings { self.pages.clear(); } else { self.in_settings = true; self.pages.clear(); } }
                Some(i) => { self.in_settings = false; self.pages.clear(); self.tab = [Tab::Home, Tab::Servers, Tab::Stats][i]; }
                None => {}
            }
        });
        self.modals(ctx);
        self.snackbar(ctx);
        ctx.request_repaint_after(Duration::from_millis(if st.state == State::Connected { 1000 } else { 2000 }));
    }
    /// Test hook: open a tab or a settings page.
    #[cfg(test)]
    pub fn show(&mut self, what: &str) {
        self.modal = None; self.in_settings = false; self.pages.clear();
        match what {
            "home" => self.tab = Tab::Home, "servers" => self.tab = Tab::Servers, "stats" => self.tab = Tab::Stats,
            "settings" => self.in_settings = true,
            "import" => { self.tab = Tab::Home; self.modal = Some(Modal::Import { text: String::new() }); }
            "sheet" => { let p = self.profiles[0].clone(); self.tab = Tab::Servers; self.server_sheet(&p); }
            page => { self.in_settings = true; self.pages.push(match page { "routing" => Page::Routing, "tunnel" => Page::Tunnel, "subscriptions" => Page::Subscriptions, "ping" => Page::Ping, "logs" => Page::Logs, _ => Page::Appearance }); }
        }
    }
}
