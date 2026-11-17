//! Share links, subscription bodies and JSON configs → server rows.
//! Everything Xray can run stays for Xray (the provider's JSON is used verbatim); TUIC, AnyTLS,
//! WireGuard, obfuscated Hysteria2, HTTP/2 / QUIC transports and sing-box JSON go to sing-box.

use crate::json::{base64_text, readable, Get, Obj};
use crate::model::Profile;
use crate::obj;
use serde_json::{json, Value};
use std::collections::HashMap;

pub const LOCAL_PROTOCOLS: &[&str] = &["freedom", "blackhole", "dns", "loopback"];
const SINGBOX_TYPES: &[&str] = &["vless", "vmess", "trojan", "shadowsocks", "socks", "http", "hysteria2", "hysteria", "tuic", "anytls", "wireguard", "ssh"];

#[derive(Default)]
pub struct ImportResult {
    pub profiles: Vec<Profile>,
    pub skipped: Vec<String>,
    pub routing_link: String,
    pub json: bool,
}

/// Error type that tells the caller "Xray can't run this link, try sing-box".
enum LinkError { NotXray, Bad(String) }
impl From<String> for LinkError { fn from(s: String) -> Self { LinkError::Bad(s) } }
impl From<&str> for LinkError { fn from(s: &str) -> Self { LinkError::Bad(s.to_string()) } }

/// Deep-link wrappers (happ://add/, wraith://add/, clash://, sing-box://) → the payload.
pub fn unwrap(input: &str) -> Result<String, String> {
    let mut s = input.trim().trim_start_matches('\u{FEFF}').to_string();
    if s.starts_with("happ://crypt") || s.starts_with("happ://limited") || s.starts_with("happ://hwid") {
        return Err("Эта ссылка зашифрована сторонним клиентом. Нужна обычная ссылка подписки от провайдера.".into());
    }
    for prefix in ["happ://add/", "wraith://add/", "wraith://import/"] {
        if s.to_ascii_lowercase().starts_with(prefix) { s = s[prefix.len()..].to_string(); if !s.starts_with("https://") && !s.starts_with("http://") { s = decode(&s); } break; }
    }
    if s.starts_with("clash://") || s.starts_with("sing-box://import-remote-profile?") {
        s = s.find('?').and_then(|q| query(&s[q + 1..]).get("url").cloned()).unwrap_or_default();
    }
    Ok(s.trim().to_string())
}
pub fn is_subscription_url(s: &str) -> bool { s.to_ascii_lowercase().starts_with("https://") && !s.contains('\n') && !s.contains(' ') }

pub fn parse(raw: &str) -> Result<ImportResult, String> {
    let s = raw.trim().trim_start_matches('\u{FEFF}').to_string();
    if s.is_empty() { return Err("Вставьте ссылку, JSON или адрес подписки.".into()); }
    let mut result = ImportResult::default();
    if s.starts_with('{') {
        result.json = true;
        result.profiles.push(from_json(&crate::json::parse_obj(&s)?, &s)?);
        return Ok(result);
    }
    if s.starts_with('[') {
        result.json = true;
        let items: Vec<Value> = serde_json::from_str(&s).map_err(|e| format!("Некорректный JSON: {e}"))?;
        for item in items {
            let Some(o) = item.as_object() else { continue };
            match from_json(o, &serde_json::to_string(o).unwrap_or_default()) { Ok(p) => result.profiles.push(p), Err(e) => result.skipped.push(crate::log::safe(e)) }
        }
    } else {
        let mut text = s.clone();
        if !text.contains("://") {
            text = base64_text(&text).map_err(|_| "Не удалось распознать ссылку, JSON или Base64-подписку.".to_string())?;
            let t = text.trim(); if t.starts_with('{') || t.starts_with('[') { return parse(t); }
        }
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') { continue; }
            // Happ ships the routing profile next to the hosts; dropping it lost the provider's rules.
            if line.starts_with("happ://routing/") { if result.routing_link.is_empty() { result.routing_link = line.to_string(); } continue; }
            if !line.contains("://") { continue; }
            match from_link(line) { Ok(p) => result.profiles.push(p), Err(e) => result.skipped.push(crate::log::safe(e)) }
        }
        if !result.routing_link.is_empty() && !result.routing_link.starts_with("happ://routing/off") {
            match crate::routing::decode(&result.routing_link) {
                Ok(r) => for p in result.profiles.iter_mut() { if !p.full { p.routing = Some(r.clone()); } },
                Err(e) => crate::log::add(format!("Профиль маршрутизации не разобран: {e}")),
            }
        }
    }
    if result.profiles.is_empty() {
        if let Some(first) = result.skipped.first() { return Err(format!("Ни одна строка не поддерживается ({}). {first}", result.skipped.len())); }
        return Err("Не найдено серверов поддерживаемого формата.".into());
    }
    Ok(result)
}

// ------------------------------------------------------------------ JSON

pub fn looks_like_xray(root: &Obj) -> bool {
    if root.has("protocol") && !root.has("type") { return true; }
    for o in root.objects("outbounds") {
        if o.has("type") { return false; }
        if o.has("protocol") || o.has("streamSettings") || o.has("settings") { return true; }
    }
    if root.objects("inbounds").iter().any(|o| o.has("protocol")) { return true; }
    if root.o("routing").is_some() || root.o("policy").is_some() || root.has("remarks") { return true; }
    root.o("log").map(|l| l.has("loglevel")).unwrap_or(false)
}

fn from_json(o: &Obj, raw: &str) -> Result<Profile, String> {
    if looks_like_xray(o) { return from_xray_document(o, raw); }
    if o.has("outbounds") {
        // A complete sing-box config stays intact; the core validates it before start.
        let proxies: Vec<&Obj> = o.objects("outbounds").into_iter().filter(|x| SINGBOX_TYPES.contains(&x.s("type").as_str())).collect();
        let mut p = Profile { name: o.first(&["name", "tag", "remarks"]), source: raw.into(), engine: "singbox".into(), full: true, singbox: Some(o.clone()), protocol: "sing-box".into(), servers: proxies.len().max(1) as i64, ..Default::default() };
        if p.name.is_empty() { p.name = "sing-box JSON".into(); }
        if let Some(f) = proxies.first() { p.host = f.s("server"); p.port = f.i("server_port", 0); }
        return Ok(p);
    }
    if SINGBOX_TYPES.contains(&o.s("type").as_str()) { return Ok(singbox_profile(o.clone(), &o.first(&["tag", "name"]), raw, "")); }
    Err("Ожидается конфигурация Xray или sing-box с исходящими подключениями.".into())
}

/// One provider Xray document = one row; its servers stay inside the document.
pub fn from_xray_document(root: &Obj, raw: &str) -> Result<Profile, String> {
    let single = [Value::Object(root.clone())];
    let outbounds: Vec<&Obj> = match root.a("outbounds") {
        Some(a) => a.iter().filter_map(|v| v.as_object()).collect(),
        None if root.has("protocol") => single.iter().filter_map(|v| v.as_object()).collect(),
        None => return Err("В конфигурации Xray нет исходящих подключений.".into()),
    };
    let proxies: Vec<&Obj> = outbounds.into_iter().filter(|o| !LOCAL_PROTOCOLS.contains(&o.s("protocol").to_ascii_lowercase().as_str())).collect();
    if proxies.is_empty() { return Err("В конфигурации Xray нет прокси, только direct/block.".into()); }
    let endpoints: Vec<Endpoint> = proxies.iter().flat_map(|p| endpoints(p)).collect();
    let first = endpoints.first().ok_or("Не удалось прочитать адрес сервера в конфигурации Xray.")?;
    let mut name = root.first(&["remarks", "name"]);
    if name.is_empty() { name = proxies[0].s_or("tag", "Xray"); }
    Ok(Profile {
        name, source: raw.into(), engine: "xray".into(), protocol: first.protocol.clone(), host: first.host.clone(), port: first.port,
        transport: first.transport.clone(), security: first.security.clone(), servers: endpoints.len().max(1) as i64,
        balancer: crate::xray::provider_balancing(root), description: describe(&root.o("meta").s("serverDescription")), ..Default::default()
    })
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Endpoint { pub protocol: String, pub host: String, pub port: i64, pub transport: String, pub security: String, pub tag: String }

/// Every server an Xray outbound can dial (vnext / servers / Hysteria address).
pub fn endpoints(outbound: &Obj) -> Vec<Endpoint> {
    let protocol = outbound.s("protocol").to_ascii_lowercase();
    let empty = Obj::new();
    let settings = outbound.o("settings").unwrap_or(&empty);
    let stream = outbound.o("streamSettings");
    let mut network = stream.first(&["network", "type"]).to_ascii_lowercase(); if network.is_empty() { network = "tcp".into(); }
    let mut security = stream.s("security").to_ascii_lowercase(); if security == "none" { security.clear(); }
    let mut peers: Vec<&Obj> = settings.objects("vnext"); peers.extend(settings.objects("servers"));
    if peers.is_empty() && settings.has("address") { peers.push(settings); }
    peers.into_iter().map(|peer| Endpoint {
        protocol: if protocol == "hysteria" { "hysteria2".into() } else { protocol.clone() }, host: host(peer), port: port_of(peer),
        transport: network.clone(), security: security.clone(), tag: outbound.s("tag"),
    }).collect()
}
pub fn host(peer: &Obj) -> String { peer.first(&["address", "server", "host"]).replace(['[', ']'], "") }
pub fn port_of(peer: &Obj) -> i64 { peer.s("port").trim().parse().unwrap_or(0) }

/// Happ server caption: base64 or plain text, max 30 characters.
pub fn describe(value: &str) -> String {
    let mut text = value.trim().to_string();
    if text.is_empty() { return text; }
    if text.len() >= 4 && text.chars().all(|c| c.is_ascii_alphanumeric() || "+/=_-".contains(c)) {
        if let Ok(d) = base64_text(&text) { if readable(&d) { text = d; } }
    }
    let text = text.replace(['\r', '\n', '\t'], " ").trim().to_string();
    text.chars().take(30).collect()
}

// ------------------------------------------------------------------ links

pub fn decode(s: &str) -> String {
    let s = s.replace('+', "%2B");
    url::form_urlencoded::parse(format!("x={s}").as_bytes()).next().map(|(_, v)| v.into_owned()).unwrap_or(s)
}
pub fn query(raw: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for part in raw.split(['&', ';']) {
        if part.is_empty() { continue; }
        let mut kv = part.splitn(2, '=');
        let k = decode(kv.next().unwrap_or("")).to_ascii_lowercase();
        let v = kv.next().map(decode).unwrap_or_default();
        map.entry(k).or_insert(v);
    }
    map
}
fn q(map: &HashMap<String, String>, fallback: &str, keys: &[&str]) -> String {
    for k in keys { if let Some(v) = map.get(&k.to_ascii_lowercase()) { if !v.trim().is_empty() { return v.trim().to_string(); } } }
    fallback.to_string()
}
fn flag(map: &HashMap<String, String>, keys: &[&str]) -> bool { matches!(q(map, "", keys).as_str(), "1" | "true") }

#[derive(Default)]
pub struct Link { pub scheme: String, pub user_info: String, pub host: String, pub fragment: String, pub description: String, pub ports: String, pub port: i64, pub query: HashMap<String, String> }

/// Split without a URL parser: provider links carry raw spaces and emoji that strict parsers reject.
pub fn split(text: &str) -> Result<Link, String> {
    let mark = text.find("://").ok_or("Это не ссылка подключения.")?;
    let mut l = Link { scheme: text[..mark].to_ascii_lowercase(), port: -1, ..Default::default() };
    let mut rest = text[mark + 3..].to_string();
    if let Some(hash) = rest.find('#') {
        let mut fragment = rest[hash + 1..].to_string();
        if let Some(p) = fragment.find('?') {
            if fragment[p..].contains("serverDescription=") {
                l.description = describe(query(&fragment[p + 1..]).get("serverdescription").map(String::as_str).unwrap_or(""));
                fragment.truncate(p);
            }
        }
        l.fragment = decode(&fragment).trim().to_string();
        rest.truncate(hash);
    }
    if let Some(qm) = rest.find('?') { l.query = query(&rest[qm + 1..]); rest.truncate(qm); }
    if let Some(slash) = rest.find('/') { rest.truncate(slash); }
    if let Some(at) = rest.rfind('@') { l.user_info = rest[..at].to_string(); rest = rest[at + 1..].to_string(); }
    if rest.starts_with('[') {
        let close = rest.find(']').ok_or("Незакрытый IPv6-адрес в ссылке.")?;
        l.host = rest[1..close].to_string();
        if let Some(t) = rest[close + 1..].strip_prefix(':') { l.port = number(t)?; }
    } else if let Some(colon) = rest.rfind(':') {
        l.host = rest[..colon].to_string();
        let tail = rest[colon + 1..].trim();
        // Hysteria port hopping writes a range list in the port position: host:5000-6000,7044
        if tail.contains(['-', ',']) && tail.chars().all(|c| c.is_ascii_digit() || c == '-' || c == ',') { l.ports = tail.into(); l.port = number(tail.split([',', '-']).next().unwrap_or(""))?; }
        else { l.port = number(tail)?; }
    } else { l.host = rest; }
    l.host = l.host.trim().to_string();
    if l.host.is_empty() { return Err("В ссылке нет адреса сервера.".into()); }
    Ok(l)
}
fn number(v: &str) -> Result<i64, String> { v.trim().parse().map_err(|_| format!("Некорректный порт в ссылке: {v}")) }

pub fn from_link(text: &str) -> Result<Profile, String> {
    match link_to_xray(text) {
        Ok(mut outbound) => {
            let e = endpoints(&outbound).into_iter().next().ok_or("В ссылке нет адреса сервера.")?;
            let name = outbound.s("_name"); let description = outbound.s("_description");
            outbound.remove("_name"); outbound.remove("_description");
            Ok(Profile { name: if name.is_empty() { e.host.clone() } else { name }, source: text.into(), engine: "xray".into(), protocol: e.protocol, host: e.host, port: e.port, transport: e.transport, security: e.security, description, ..Default::default() })
        }
        Err(LinkError::Bad(e)) => Err(e),
        Err(LinkError::NotXray) => {
            // Something Xray cannot express (TUIC, AnyTLS, obfuscated Hysteria2, HTTP/2 transport…): sing-box runs it.
            let sb = link_to_singbox(text)?;
            let l = split(text).ok();
            Ok(singbox_profile(sb, &l.as_ref().map(|l| l.fragment.clone()).unwrap_or_default(), text, &l.map(|l| l.description).unwrap_or_default()))
        }
    }
}

fn singbox_profile(o: Obj, name: &str, source: &str, description: &str) -> Profile {
    let tls = o.o("tls");
    Profile {
        name: if name.is_empty() { o.s_or("server", "Сервер") } else { name.into() }, source: source.into(), engine: "singbox".into(),
        protocol: o.s("type"), host: o.s("server"), port: o.i("server_port", 0),
        security: match tls { None => String::new(), Some(t) if t.o("reality").map(|r| r.b("enabled", false)).unwrap_or(false) => "reality".into(), Some(_) => "tls".into() },
        transport: o.o("transport").s("type"), description: description.into(), singbox: Some(o), ..Default::default()
    }
}

/// Share link → native Xray outbound tagged "proxy".
fn link_to_xray(text: &str) -> Result<Obj, LinkError> {
    let scheme = text[..text.find("://").ok_or("Это не ссылка подключения.")?].to_ascii_lowercase();
    match scheme.as_str() {
        "vmess" => return vmess(text),
        "ss" => return shadowsocks(text).map_err(LinkError::Bad),
        "tuic" | "anytls" | "wireguard" | "wg" | "ssh" => return Err(LinkError::NotXray),
        _ => {}
    }
    let text = if ["socks", "socks5", "socks4", "http"].contains(&scheme.as_str()) { expand_base64_address(text, &scheme) } else { text.to_string() };
    let mut l = split(&text)?;
    if l.scheme == "hy2" { l.scheme = "hysteria2".into(); }
    if l.scheme == "socks5" || l.scheme == "socks4" { l.scheme = "socks".into(); }
    let qm = &l.query;
    let security = q(qm, "", &["security"]);
    if l.port < 1 && (l.scheme == "trojan" || l.scheme == "hysteria2" || security == "tls" || security == "reality") { l.port = 443; }
    if !(1..=65535).contains(&l.port) { return Err("В ссылке нет корректного порта сервера.".into()); }
    let auth = decode(&l.user_info);
    let mut outbound = match l.scheme.as_str() {
        "vless" => {
            if auth.is_empty() { return Err("В ссылке VLESS нет UUID.".into()); }
            let mut user = obj! {"id" => auth, "encryption" => q(qm, "none", &["encryption"])};
            let flow = q(qm, "", &["flow"]); if !flow.is_empty() { user.insert("flow".into(), flow.into()); }
            obj! {"protocol" => "vless", "settings" => json!({"vnext": [{"address": l.host, "port": l.port, "users": [user]}]})}
        }
        "trojan" => obj! {"protocol" => "trojan", "settings" => json!({"servers": [{"address": l.host, "port": l.port, "password": auth}]})},
        "socks" | "http" => {
            let mut server = obj! {"address" => l.host, "port" => l.port};
            let cred = credentials(&auth);
            if !cred.is_empty() { let mut it = cred.splitn(2, ':'); server.insert("users".into(), json!([{"user": it.next().unwrap_or(""), "pass": it.next().unwrap_or("")}])); }
            obj! {"protocol" => l.scheme, "settings" => json!({"servers": [server]})}
        }
        "hysteria2" => {
            if !q(qm, "", &["obfs"]).is_empty() { return Err(LinkError::NotXray); }
            let mut hy = obj! {"version" => 2, "auth" => auth};
            let hops = q(qm, &l.ports, &["mport", "ports"]);
            if !hops.is_empty() {
                let mut hop = obj! {"port" => hops.replace(':', "-")};
                let interval = q(qm, "", &["mportHopInt", "hopInterval", "hop_interval"]); if !interval.is_empty() { hop.insert("interval".into(), interval.trim_end_matches('s').into()); }
                hy.insert("udphop".into(), hop.into());
            }
            let mut tls = obj! {"serverName" => q(qm, &l.host, &["sni", "peer", "serverName"])};
            let alpn = q(qm, "", &["alpn"]);
            tls.insert("alpn".into(), if alpn.is_empty() { json!(["h3"]) } else { json!(alpn.split(',').collect::<Vec<_>>()) });
            // Xray 26 removed allowInsecure: an unverified certificate without a pin runs on sing-box.
            let pin = q(qm, "", &["pinSHA256", "pinnedPeerCertSha256", "pcs"]).replace(':', "").to_ascii_lowercase();
            if flag(qm, &["insecure", "allowInsecure"]) && pin.is_empty() { return Err(LinkError::NotXray); }
            if !pin.is_empty() { tls.insert("pinnedPeerCertSha256".into(), pin.into()); }
            let mut o = obj! {"protocol" => "hysteria", "tag" => "proxy", "settings" => json!({"version": 2, "address": l.host, "port": l.port}),
                "streamSettings" => json!({"network": "hysteria", "hysteriaSettings": hy, "security": "tls", "tlsSettings": tls})};
            o.insert("_name".into(), l.fragment.clone().into()); o.insert("_description".into(), l.description.clone().into());
            return Ok(o);
        }
        other => return Err(format!("Неизвестный протокол: {other}").into()),
    };
    outbound.insert("tag".into(), "proxy".into());
    let stream = stream(qm, &l.host, l.scheme == "trojan")?;
    if !stream.is_empty() { outbound.insert("streamSettings".into(), stream.into()); }
    outbound.insert("_name".into(), l.fragment.into()); outbound.insert("_description".into(), l.description.into());
    Ok(outbound)
}

/// streamSettings from the standard share-link parameters (XTLS share link proposal).
fn stream(qm: &HashMap<String, String>, host: &str, tls_by_default: bool) -> Result<Obj, LinkError> {
    let mut s = Obj::new();
    let kind = q(qm, "tcp", &["type", "net"]).to_ascii_lowercase();
    let mut security = q(qm, if tls_by_default { "tls" } else { "none" }, &["security"]).to_ascii_lowercase();
    if security == "xtls" { security = "tls".into(); }
    let path = q(qm, "/", &["path"]); let host_header = q(qm, "", &["host"]);
    match kind.as_str() {
        "tcp" | "raw" | "none" => {
            s.insert("network".into(), "raw".into());
            if q(qm, "none", &["headerType"]) == "http" {
                let mut request = obj! {"path" => [path]};
                if !host_header.is_empty() { request.insert("headers".into(), json!({"Host": host_header.split(',').collect::<Vec<_>>()})); }
                s.insert("rawSettings".into(), json!({"header": {"type": "http", "request": request}}));
            }
        }
        "ws" => {
            let mut ws = obj! {"path" => path.clone()}; if !host_header.is_empty() { ws.insert("host".into(), host_header.into()); }
            let ed = q(qm, "", &["ed"]);
            if !ed.is_empty() && !path.contains("ed=") { ws.insert("path".into(), format!("{path}{}ed={ed}", if path.contains('?') { "&" } else { "?" }).into()); }
            s.insert("network".into(), "ws".into()); s.insert("wsSettings".into(), ws.into());
        }
        "grpc" => {
            let mut g = obj! {"serviceName" => q(qm, "", &["serviceName", "path"])};
            if q(qm, "", &["mode"]) == "multi" { g.insert("multiMode".into(), true.into()); }
            let authority = q(qm, "", &["authority"]); if !authority.is_empty() { g.insert("authority".into(), authority.into()); }
            s.insert("network".into(), "grpc".into()); s.insert("grpcSettings".into(), g.into());
        }
        "httpupgrade" => {
            let mut h = obj! {"path" => path}; if !host_header.is_empty() { h.insert("host".into(), host_header.into()); }
            s.insert("network".into(), "httpupgrade".into()); s.insert("httpupgradeSettings".into(), h.into());
        }
        "xhttp" | "splithttp" => {
            let mut x = obj! {"path" => path}; if !host_header.is_empty() { x.insert("host".into(), host_header.into()); }
            let mode = q(qm, "", &["mode"]); if !mode.is_empty() { x.insert("mode".into(), mode.into()); }
            let extra = q(qm, "", &["extra"]);
            if !extra.is_empty() { x.insert("extra".into(), serde_json::from_str::<Value>(&extra).map_err(|_| "Параметр extra в ссылке XHTTP — некорректный JSON.")?); }
            s.insert("network".into(), "xhttp".into()); s.insert("xhttpSettings".into(), x.into());
        }
        "kcp" | "mkcp" => {
            let mut k = Obj::new();
            let header = q(qm, "", &["headerType"]); if !header.is_empty() && header != "none" { k.insert("header".into(), json!({"type": header})); }
            let seed = q(qm, "", &["seed"]); if !seed.is_empty() { k.insert("seed".into(), seed.into()); }
            s.insert("network".into(), "kcp".into()); s.insert("kcpSettings".into(), k.into());
        }
        // HTTP/2 and QUIC transports were removed from Xray; sing-box still speaks them.
        "http" | "h2" | "quic" => return Err(LinkError::NotXray),
        other => return Err(format!("Транспорт {other} не поддерживается.").into()),
    }
    match security.as_str() {
        "tls" => {
            let mut t = obj! {"serverName" => q(qm, host, &["sni", "serverName", "peer"])};
            let fp = q(qm, "", &["fp", "fingerprint"]); if !fp.is_empty() { t.insert("fingerprint".into(), fp.into()); }
            let alpn = q(qm, "", &["alpn"]); if !alpn.is_empty() { t.insert("alpn".into(), json!(alpn.split(',').collect::<Vec<_>>())); }
            let pin = q(qm, "", &["pinnedPeerCertSha256", "pcs"]);
            if flag(qm, &["allowInsecure", "insecure"]) && pin.is_empty() { return Err(LinkError::NotXray); }
            if !pin.is_empty() { t.insert("pinnedPeerCertSha256".into(), pin.into()); }
            let ech = q(qm, "", &["ech", "echConfigList"]); if !ech.is_empty() { t.insert("echConfigList".into(), ech.into()); }
            s.insert("security".into(), "tls".into()); s.insert("tlsSettings".into(), t.into());
        }
        "reality" => {
            let mut r = obj! {"serverName" => q(qm, host, &["sni", "serverName"]), "fingerprint" => q(qm, "chrome", &["fp", "fingerprint"]), "publicKey" => q(qm, "", &["pbk", "publicKey"]), "shortId" => q(qm, "", &["sid", "shortId"])};
            let spx = q(qm, "", &["spx", "spiderX"]); if !spx.is_empty() { r.insert("spiderX".into(), spx.into()); }
            let pqv = q(qm, "", &["pqv", "mldsa65Verify"]); if !pqv.is_empty() { r.insert("mldsa65Verify".into(), pqv.into()); }
            if r.s("publicKey").is_empty() { return Err("В ссылке Reality нет публичного ключа (pbk).".into()); }
            s.insert("security".into(), "reality".into()); s.insert("realitySettings".into(), r.into());
        }
        "none" | "" => {}
        other => return Err(format!("Шифрование {other} не поддерживается.").into()),
    }
    Ok(s)
}

/// socks://<base64 of user:pass@host:port> — the whole address encoded in one blob.
fn expand_base64_address(text: &str, scheme: &str) -> String {
    let start = text.find("://").unwrap() + 3;
    let body = text[start..].split(['#', '?']).next().unwrap_or("");
    if !body.contains('@') && !body.contains(':') && body.len() >= 8 && body.chars().all(|c| c.is_ascii_alphanumeric() || "+/=_-".contains(c)) {
        if let Ok(d) = base64_text(body) { if readable(&d) && d.contains(':') { return format!("{scheme}://{}{}", d.trim(), &text[start + body.len()..]); } }
    }
    text.to_string()
}
/// Happ passes socks/http credentials base64-encoded instead of user:pass.
fn credentials(auth: &str) -> String {
    if auth.is_empty() || auth.contains(':') || auth.len() < 4 || !auth.chars().all(|c| c.is_ascii_alphanumeric() || "+/=_-".contains(c)) { return auth.into(); }
    match base64_text(auth) { Ok(d) if d.contains(':') && readable(&d) => d, _ => auth.into() }
}

fn vmess(s: &str) -> Result<Obj, LinkError> {
    let mut payload = s[8..].to_string(); let mut label = String::new();
    if let Some(h) = payload.find('#') { label = decode(&payload[h + 1..]); payload.truncate(h); }
    let v = match base64_text(&payload).ok().and_then(|t| crate::json::parse_obj(&t).ok()) {
        Some(v) => v,
        None => {
            // Some panels emit vmess:// as a plain URI.
            let l = split(s)?; if l.port < 1 { return Err("Не удалось разобрать ссылку VMess.".into()); }
            let auth = decode(&l.user_info); let id = auth.splitn(2, ':').last().unwrap_or("").to_string();
            let mut o = obj! {"protocol" => "vmess", "tag" => "proxy", "settings" => json!({"vnext": [{"address": l.host, "port": l.port, "users": [{"id": id, "security": q(&l.query, "auto", &["encryption"])}]}]})};
            let st = stream(&l.query, &l.host, false)?; if !st.is_empty() { o.insert("streamSettings".into(), st.into()); }
            o.insert("_name".into(), l.fragment.into()); return Ok(o);
        }
    };
    let address = v.first(&["add", "address", "host"]).replace(['[', ']'], "");
    if address.is_empty() { return Err("В ссылке VMess нет адреса сервера.".into()); }
    let id = v.first(&["id", "uuid"]); if id.is_empty() { return Err("В ссылке VMess нет UUID.".into()); }
    let port: i64 = v.s("port").trim().parse().map_err(|_| "Некорректный порт в ссылке VMess.")?;
    let mut scy = v.first(&["scy", "security"]); if scy.is_empty() { scy = "auto".into(); }
    let mut qm = HashMap::new();
    for k in ["path", "host", "sni", "fp", "alpn", "serviceName", "mode", "authority"] { let val = v.s(k); if !val.is_empty() { qm.insert(k.to_ascii_lowercase(), val); } }
    qm.insert("type".into(), v.first(&["net"]));
    let header = v.first(&["type"]); if !header.is_empty() { qm.insert("headertype".into(), header); }
    if qm.get("type").map(|t| t == "grpc").unwrap_or(false) && !qm.contains_key("servicename") { if let Some(p) = qm.get("path").cloned() { qm.insert("servicename".into(), p); } }
    let tls = v.s("tls"); qm.insert("security".into(), if tls == "tls" || tls == "reality" { tls } else { "none".into() });
    let mut o = obj! {"protocol" => "vmess", "tag" => "proxy", "settings" => json!({"vnext": [{"address": address, "port": port, "users": [{"id": id, "alterId": v.i("aid", v.i("alterId", 0)), "security": scy}]}]})};
    let st = stream(&qm, &address, false)?; if !st.is_empty() { o.insert("streamSettings".into(), st.into()); }
    let name = if label.is_empty() { v.first(&["ps", "remarks", "name"]) } else { label };
    o.insert("_name".into(), name.into()); o.insert("_description".into(), describe(&v.s("serverDescription")).into());
    Ok(o)
}

fn shadowsocks(s: &str) -> Result<Obj, String> {
    let mut data = s[5..].to_string(); let (mut name, mut description) = (String::new(), String::new());
    if let Some(h) = data.find('#') {
        let mut fragment = data[h + 1..].to_string();
        if let Some(p) = fragment.find('?') { if fragment[p..].contains("serverDescription=") { description = describe(query(&fragment[p + 1..]).get("serverdescription").map(String::as_str).unwrap_or("")); fragment.truncate(p); } }
        name = decode(&fragment); data.truncate(h);
    }
    if let Some(qm) = data.find('?') {
        if query(&data[qm + 1..]).get("plugin").map(|p| !p.is_empty()).unwrap_or(false) { return Err("Shadowsocks с plugin не поддерживается.".into()); }
        data.truncate(qm);
    }
    let data = data.trim_end_matches('/').to_string();
    let data = if data.contains('@') { data } else { base64_text(&data)? };
    let at = data.rfind('@').ok_or("Некорректная ссылка Shadowsocks.")?;
    let user = &data[..at];
    let user = if user.contains(':') { decode(user) } else { base64_text(&decode(user))? };
    let mut auth = user.splitn(2, ':');
    let (Some(method), Some(password)) = (auth.next(), auth.next()) else { return Err("В ссылке Shadowsocks нет пароля.".into()) };
    let l = split(&format!("ss://{}", &data[at + 1..]))?;
    if l.port < 1 { return Err("Некорректный адрес Shadowsocks.".into()); }
    Ok(obj! {"protocol" => "shadowsocks", "tag" => "proxy", "settings" => json!({"servers": [{"address": l.host, "port": l.port, "method": method, "password": password}]}), "_name" => name, "_description" => description})
}

// ------------------------------------------------------------------ sing-box links

/// Links Xray cannot run → sing-box outbound.
fn link_to_singbox(text: &str) -> Result<Obj, String> {
    let scheme = text[..text.find("://").unwrap_or(0)].to_ascii_lowercase();
    if scheme == "vmess" { return Err("VMess с транспортом HTTP/2 или QUIC не поддерживается.".into()); }
    let mut l = split(text)?;
    if l.scheme == "hy2" { l.scheme = "hysteria2".into(); }
    if l.scheme == "wg" { l.scheme = "wireguard".into(); }
    if l.port < 1 { l.port = 443; }
    let qm = &l.query; let auth = decode(&l.user_info);
    let mut o = obj! {"type" => l.scheme, "server" => l.host, "server_port" => l.port};
    match l.scheme.as_str() {
        "vless" => { if q(qm, "none", &["encryption"]) != "none" { return Err("VLESS encryption не поддерживается sing-box.".into()); } o.insert("uuid".into(), auth.into()); let flow = q(qm, "", &["flow"]); if !flow.is_empty() { o.insert("flow".into(), flow.into()); o.insert("packet_encoding".into(), "xudp".into()); } }
        "trojan" | "anytls" | "hysteria2" => { o.insert("password".into(), auth.into()); }
        "tuic" => {
            let mut it = auth.splitn(2, ':');
            let (Some(uuid), Some(pw)) = (it.next(), it.next()) else { return Err("TUIC требует UUID и пароль.".into()) };
            o.insert("uuid".into(), uuid.into()); o.insert("password".into(), pw.into()); o.insert("congestion_control".into(), q(qm, "bbr", &["congestion_control", "congestion"]).into());
        }
        "wireguard" => {
            o.insert("private_key".into(), auth.into()); o.insert("peer_public_key".into(), q(qm, "", &["publickey", "public_key"]).into());
            let address = q(qm, "", &["address", "ip"]);
            if !address.is_empty() { o.insert("local_address".into(), json!(address.split(',').map(|a| { let a = a.trim(); if a.contains('/') { a.to_string() } else if a.contains(':') { format!("{a}/128") } else { format!("{a}/32") } }).collect::<Vec<_>>())); }
            let reserved = q(qm, "", &["reserved"]); if !reserved.is_empty() { o.insert("reserved".into(), json!(reserved.split(',').filter_map(|r| r.trim().parse::<i64>().ok()).collect::<Vec<_>>())); }
            if let Ok(mtu) = q(qm, "", &["mtu"]).parse::<i64>() { o.insert("mtu".into(), mtu.into()); }
            return Ok(o);
        }
        other => return Err(format!("Протокол {other} не поддерживается.")),
    }
    let security = q(qm, "", &["security"]);
    if l.scheme != "vless" || security == "tls" || security == "reality" {
        let mut tls = obj! {"enabled" => true, "server_name" => q(qm, &l.host, &["sni", "peer", "serverName"])};
        if flag(qm, &["insecure", "allowInsecure"]) { tls.insert("insecure".into(), true.into()); }
        let alpn = q(qm, "", &["alpn"]); if !alpn.is_empty() && security != "reality" { tls.insert("alpn".into(), json!(alpn.split(',').collect::<Vec<_>>())); }
        let mut fp = q(qm, "", &["fp"]);
        if security == "reality" { tls.insert("reality".into(), json!({"enabled": true, "public_key": q(qm, "", &["pbk"]), "short_id": q(qm, "", &["sid"])})); if fp.is_empty() { fp = "chrome".into(); } }
        if !fp.is_empty() { tls.insert("utls".into(), json!({"enabled": true, "fingerprint": fingerprint(&fp)})); }
        o.insert("tls".into(), tls.into());
    }
    let host_header = q(qm, "", &["host"]);
    match q(qm, "tcp", &["type"]).to_ascii_lowercase().as_str() {
        "ws" => { let mut t = obj! {"type" => "ws", "path" => q(qm, "/", &["path"])}; if !host_header.is_empty() { t.insert("headers".into(), json!({"Host": host_header})); } o.insert("transport".into(), t.into()); }
        "grpc" => { o.insert("transport".into(), json!({"type": "grpc", "service_name": q(qm, "", &["serviceName"])})); }
        "http" | "h2" => { let mut t = obj! {"type" => "http", "path" => q(qm, "/", &["path"])}; if !host_header.is_empty() { t.insert("host".into(), json!([host_header])); } o.insert("transport".into(), t.into()); }
        "httpupgrade" => { let mut t = obj! {"type" => "httpupgrade", "path" => q(qm, "/", &["path"])}; if !host_header.is_empty() { t.insert("host".into(), host_header.into()); } o.insert("transport".into(), t.into()); }
        "quic" => { o.insert("transport".into(), json!({"type": "quic"})); }
        "tcp" | "raw" | "none" | "" => {}
        other => return Err(format!("Транспорт {other} не поддерживается sing-box.")),
    }
    if l.scheme == "hysteria2" {
        let obfs = q(qm, "", &["obfs"]); if !obfs.is_empty() { o.insert("obfs".into(), json!({"type": obfs, "password": q(qm, "", &["obfs-password", "obfs_password"])})); }
        let hops = q(qm, &l.ports, &["mport", "ports"]);
        if !hops.is_empty() {
            let ranges: Vec<String> = hops.split(',').map(str::trim).filter(|v| !v.is_empty()).map(|v| if v.contains('-') { v.replace('-', ":") } else { format!("{v}:{v}") }).collect();
            o.insert("server_ports".into(), json!(ranges)); o.insert("hop_interval".into(), format!("{}s", q(qm, "30", &["mportHopInt", "hopInterval"]).trim_end_matches('s')).into());
        }
    }
    Ok(o)
}
pub fn fingerprint(v: &str) -> String {
    let v = v.trim().to_ascii_lowercase();
    if ["chrome", "firefox", "edge", "safari", "360", "qq", "ios", "android", "random", "randomized"].contains(&v.as_str()) { v } else { "chrome".into() }
}

/// The Xray outbound for a link row (the runtime rebuilds it from the stored link).
pub fn link_outbound_for_runtime(s: &str) -> Result<Obj, String> {
    match link_to_xray(s) { Ok(o) => Ok(o), Err(LinkError::NotXray) => Err("Этот сервер запускается через sing-box.".into()), Err(LinkError::Bad(e)) => Err(e) }
}

/// The row as it runs on the preferred core ("xray" or "singbox"). A link is converted when the
/// other core can express it; anything else (Xray JSON, XHTTP, sing-box JSON…) keeps its own core.
pub fn for_engine(p: &Profile, preferred: &str) -> Profile {
    let link = !p.source.trim().starts_with('{') && !p.source.trim().starts_with('[') && p.source.contains("://");
    if !link || p.full { return p.clone(); }
    match (preferred, p.is_singbox()) {
        ("singbox", false) => match link_to_singbox_any(&p.source) {
            Ok(o) => Profile { engine: "singbox".into(), singbox: Some(o), ..p.clone() },
            Err(_) => p.clone(),
        },
        ("xray", true) => match link_to_xray(&p.source) { Ok(_) => Profile { engine: "xray".into(), singbox: None, ..p.clone() }, Err(_) => p.clone() },
        _ => p.clone(),
    }
}

/// sing-box outbound for any link sing-box supports, including VMess, Shadowsocks and SOCKS/HTTP.
fn link_to_singbox_any(text: &str) -> Result<Obj, String> {
    let scheme = text[..text.find("://").unwrap_or(0)].to_ascii_lowercase();
    match scheme.as_str() {
        "vmess" | "ss" | "socks" | "socks5" | "http" => {
            // Reuse the Xray conversion, then express the same server in sing-box terms.
            let x = link_to_xray(text).map_err(|e| match e { LinkError::Bad(e) => e, LinkError::NotXray => "Не поддерживается".into() })?;
            xray_to_singbox(&x)
        }
        _ => link_to_singbox(text),
    }
}

fn xray_to_singbox(x: &Obj) -> Result<Obj, String> {
    let e = endpoints(x).into_iter().next().ok_or("Нет адреса сервера.")?;
    let settings = x.o("settings").cloned().unwrap_or_default();
    let mut o = obj! {"server" => e.host, "server_port" => e.port};
    match x.s("protocol").as_str() {
        "vmess" => {
            let user = settings.objects("vnext").first().and_then(|v| v.objects("users").first().map(|u| (*u).clone())).unwrap_or_default();
            o.insert("type".into(), "vmess".into()); o.insert("uuid".into(), user.s("id").into());
            o.insert("security".into(), user.s_or("security", "auto").into()); o.insert("alter_id".into(), user.i("alterId", 0).into());
        }
        "shadowsocks" => {
            let s = settings.objects("servers").first().map(|s| (*s).clone()).unwrap_or_default();
            o.insert("type".into(), "shadowsocks".into()); o.insert("method".into(), s.s("method").into()); o.insert("password".into(), s.s("password").into());
        }
        p @ ("socks" | "http") => {
            o.insert("type".into(), p.into());
            if let Some(u) = settings.objects("servers").first().and_then(|s| s.objects("users").first().map(|u| (*u).clone())) { o.insert("username".into(), u.s("user").into()); o.insert("password".into(), u.s("pass").into()); }
        }
        other => return Err(format!("{other}: используйте Xray")),
    }
    let stream = x.o("streamSettings").cloned().unwrap_or_default();
    match stream.s("security").as_str() {
        "tls" => { let t = stream.o("tlsSettings").cloned().unwrap_or_default(); let mut tls = obj! {"enabled" => true, "server_name" => t.s_or("serverName", &e.host)}; if !t.s("fingerprint").is_empty() { tls.insert("utls".into(), json!({"enabled": true, "fingerprint": fingerprint(&t.s("fingerprint"))})); } o.insert("tls".into(), tls.into()); }
        "reality" => return Err("Reality для VMess не поддерживается sing-box".into()),
        _ => {}
    }
    match stream.s("network").as_str() {
        "" | "raw" | "tcp" => {}
        "ws" => { let w = stream.o("wsSettings").cloned().unwrap_or_default(); let mut t = obj! {"type" => "ws", "path" => w.s_or("path", "/")}; if !w.s("host").is_empty() { t.insert("headers".into(), json!({"Host": w.s("host")})); } o.insert("transport".into(), t.into()); }
        "grpc" => { o.insert("transport".into(), json!({"type": "grpc", "service_name": stream.o("grpcSettings").s("serviceName")})); }
        "httpupgrade" => { let h = stream.o("httpupgradeSettings").cloned().unwrap_or_default(); o.insert("transport".into(), json!({"type": "httpupgrade", "path": h.s_or("path", "/"), "host": h.s("host")})); }
        other => return Err(format!("Транспорт {other}: используйте Xray")),
    }
    Ok(o)
}
