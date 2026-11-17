//! Servers, subscriptions, rules and settings.

use crate::json::{Get, Obj};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Profile {
    pub id: String,
    pub name: String,
    /// Subscription id, or empty for a manually added server.
    pub group: String,
    /// The share link or the provider's JSON document, kept verbatim.
    pub source: String,
    /// "xray" or "singbox" (TUIC, AnyTLS, WireGuard, obfuscated Hysteria2, sing-box JSON).
    pub engine: String,
    pub protocol: String,
    pub host: String,
    pub port: i64,
    pub transport: String,
    pub security: String,
    pub description: String,
    /// Proxy servers inside an Xray document (1 for a link).
    pub servers: i64,
    /// The document routes through its own balancer (leastPing / observatory).
    pub balancer: bool,
    /// sing-box outbound (link) or complete sing-box config (full).
    pub singbox: Option<Obj>,
    pub full: bool,
    /// Happ routing profile attached to a link import.
    pub routing: Option<Obj>,
    pub latency: i64,
    pub ping_error: String,
    pub favorite: bool,
}
impl Default for Profile {
    fn default() -> Self {
        Profile { id: crate::util::random_hex(16), name: "Сервер".into(), group: String::new(), source: String::new(), engine: "xray".into(), protocol: String::new(), host: String::new(), port: 0, transport: String::new(), security: String::new(), description: String::new(), servers: 1, balancer: false, singbox: None, full: false, routing: None, latency: -1, ping_error: String::new(), favorite: false }
    }
}
impl Profile {
    pub fn is_singbox(&self) -> bool { self.engine == "singbox" }
    pub fn key(&self) -> String { format!("{}\n{}:{}\n{}", self.name, self.host, self.port, self.protocol) }
    pub fn summary(&self) -> String {
        if !self.description.is_empty() { return self.description.clone(); }
        let mut kind = if self.protocol.is_empty() { "JSON".to_string() } else { self.protocol.to_uppercase() };
        if !self.transport.is_empty() && !["tcp", "raw", "hysteria"].contains(&self.transport.as_str()) { kind += &format!(" · {}", self.transport); }
        if self.security == "reality" { kind += " · Reality" } else if self.security == "tls" { kind += " · TLS" }
        if self.servers > 1 { return format!("{kind} · {} {}{}", self.servers, crate::util::plural(self.servers, "сервер", "сервера", "серверов"), if self.balancer { " · авто" } else { "" }); }
        // The address stays out of the list (screenshots, screen sharing); it is in the config view.
        kind
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Subscription {
    pub id: String,
    pub name: String,
    pub url: String,
    pub fallback_url: String,
    pub interval_hours: i64,
    /// Unix time of the last successful update (0 = never).
    pub updated: i64,
    pub error: String,
    /// "json" or "links": JSON subscriptions use each server's own routing.
    pub format: String,
    pub user_info: String,
    pub announce: String,
    pub routing_link: String,
    pub routing_enabled: bool,
    pub support_url: String,
    pub web_page_url: String,
    pub info_text: String,
    pub parsed: i64,
    pub collapsed: bool,
}
impl Default for Subscription {
    fn default() -> Self {
        Subscription { id: crate::util::random_hex(16), name: "Подписка".into(), url: String::new(), fallback_url: String::new(), interval_hours: 24, updated: 0, error: String::new(), format: String::new(), user_info: String::new(), announce: String::new(), routing_link: String::new(), routing_enabled: true, support_url: String::new(), web_page_url: String::new(), info_text: String::new(), parsed: 0, collapsed: false }
    }
}
impl Subscription {
    /// "upload=…; download=…; total=…; expire=…" → (used, total, expire unix)
    pub fn usage(&self) -> (i64, i64, i64) {
        let (mut used, mut total, mut expire) = (0, 0, 0);
        for part in self.user_info.split(';') {
            let mut kv = part.splitn(2, '=');
            let (Some(k), Some(v)) = (kv.next(), kv.next()) else { continue };
            let Ok(v) = v.trim().parse::<i64>() else { continue };
            match k.trim().to_ascii_lowercase().as_str() { "upload" | "download" => used += v, "total" => total = v, "expire" => expire = v, _ => {} }
        }
        (used, total, expire)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct AppState {
    pub schema: i64,
    pub selected: String,
    pub profiles: Vec<Profile>,
    pub subscriptions: Vec<Subscription>,
    /// User routing rules: name, kind, value, action, enabled.
    pub rules: Vec<Obj>,
    pub settings: Obj,
}
impl Default for AppState {
    fn default() -> Self { AppState { schema: 4, selected: String::new(), profiles: vec![], subscriptions: vec![], rules: vec![], settings: defaults() } }
}
impl AppState {
    pub fn selected(&self) -> Option<&Profile> { self.profiles.iter().find(|p| p.id == self.selected) }
    pub fn subscription(&self, id: &str) -> Option<&Subscription> { self.subscriptions.iter().find(|s| s.id == id) }
    pub fn set(&mut self, key: &str, v: Value) { self.settings.insert(key.into(), v); }
    pub fn s(&self, key: &str) -> String { self.settings.s(key) }
    pub fn b(&self, key: &str) -> bool { self.settings.b(key, false) }
    pub fn i(&self, key: &str) -> i64 { self.settings.i(key, 0) }
}

pub fn defaults() -> Obj {
    let v = json!({
        // Connection
        "tunnelMode": "both", "core": "xray", "mode": "rules", "autoSelect": false, "bypassLan": true, "ipMode": "auto", "mtu": 1500, "sniff": true,
        // DNS: the system's resolvers unless the user opts into the provider's or their own.
        "dnsFromJson": false, "customDns": false, "tunnelDns": "https://8.8.8.8/dns-query",
        // Local proxy (127.0.0.1 only unless LAN access with a password)
        "proxyPort": 2081, "httpProxyPort": 2082, "allowLan": false, "proxyPassword": "",
        // Anti-censorship
        "fragment": false, "fragmentPackets": "tlshello", "fragmentLength": "50-100", "fragmentInterval": "10-20", "fragmentMaxSplit": "",
        "noise": false, "noiseType": "rand", "noisePacket": "1A,2B,3C", "noiseRand": "10-20", "noiseDelay": "10-16",
        "mux": false, "muxConnections": 4,
        // Latency checks
        "pingType": "proxyGet", "pingUrl": "https://www.gstatic.com/generate_204", "pingTimeout": 5000, "pingMode": "keepalive", "sort": "none",
        // Subscriptions
        "updateOnOpen": false, "userAgent": "", "sendHwid": true, "hwid": "", "clientNumericId": "",
        // App
        "theme": "system", "minimizeToTray": true, "autostart": false, "connectOnStart": false, "checkUpdates": true, "debugLogs": false
    });
    v.as_object().unwrap().clone()
}

/// Kinds of user routing rules.
pub const RULE_KINDS: &[(&str, &str)] = &[
    ("domain_suffix", "Домен и поддомены"), ("domain", "Точный домен"), ("domain_keyword", "Слово в домене"), ("domain_regex", "Регулярное выражение"),
    ("geosite", "GeoSite-категория"), ("ip_cidr", "IP / подсеть"), ("geoip", "GeoIP-страна"), ("process", "Программа (.exe)"),
];
pub fn rule_kind_label(k: &str) -> &str { RULE_KINDS.iter().find(|(key, _)| *key == k).map(|(_, l)| *l).unwrap_or(k) }
pub fn split_values(v: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for item in v.split(|c: char| c == '\n' || c == '\r' || c == ',' || c == ';' || c == ' ') {
        let t = item.trim(); if !t.is_empty() && !out.iter().any(|x| x == t) { out.push(t.to_string()); }
    }
    out
}

/// "proxy", "tun" or "both" (TUN plus the Windows system proxy).
pub fn uses_tun(s: &Obj) -> bool { matches!(s.s("tunnelMode").as_str(), "tun" | "both") }
pub fn uses_proxy(s: &Obj) -> bool { s.s("tunnelMode") != "tun" }
/// "auto" (both families), "v4" (IPv4 only) or "v6" (prefer IPv6).
pub fn ip_mode(s: &Obj) -> String { let m = s.s("ipMode"); if m.is_empty() { if s.b("ipv6", true) { "auto".into() } else { "v4".into() } } else { m } }
