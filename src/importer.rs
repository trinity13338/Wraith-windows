//! Share links, subscription bodies and JSON configs → server rows.
//! Everything is converted for Xray; the provider's JSON is used verbatim.

use crate::json::{base64_text, readable, Get, Obj};
use crate::model::Profile;
use crate::obj;
use serde_json::{json, Value};
use std::collections::HashMap;

pub const LOCAL_PROTOCOLS: &[&str] = &["freedom", "blackhole", "dns", "loopback"];

#[derive(Default)]
pub struct ImportResult {
    pub profiles: Vec<Profile>,
    pub skipped: Vec<String>,
    pub routing_link: String,
    pub json: bool,
}

/// Link conversion error.
enum LinkError { Bad(String) }
impl From<String> for LinkError { fn from(s: String) -> Self { LinkError::Bad(s) } }
impl From<&str> for LinkError { fn from(s: &str) -> Self { LinkError::Bad(s.to_string()) } }

/// Deep-link wrappers (happ://add/, wraith://add/, clash://, sing-box://) → the payload.
pub fn unwrap(input: &str) -> Result<String, String> {
    let mut s = input.trim().trim_start_matches('\u{FEFF}').to_string();
    if s.starts_with("happ://crypt") || s.starts_with("happ://limited") || s.starts_with("happ://hwid") {
        return Err(crate::tr!("Эта ссылка зашифрована сторонним клиентом. Нужна обычная ссылка подписки от провайдера.", "This link is encrypted for another client. Ask your provider for a regular subscription link.").into());
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
    if s.is_empty() { return Err(crate::tr!("Вставьте ссылку, JSON или адрес подписки.", "Paste a link, JSON or subscription address.").into()); }
    let mut result = ImportResult::default();
    if s.starts_with('{') {
        result.json = true;
        result.profiles.push(from_json(&crate::json::parse_obj(&s)?, &s)?);
        return Ok(result);
    }
    if s.starts_with('[') {
        result.json = true;
        let items: Vec<Value> = serde_json::from_str(&s).map_err(|e| crate::trf!("Некорректный JSON: {e}", "Invalid JSON: {e}"))?;
        for item in items {
            let Some(o) = item.as_object() else { continue };
            match from_json(o, &serde_json::to_string(o).unwrap_or_default()) { Ok(p) => result.profiles.push(p), Err(e) => result.skipped.push(crate::log::safe(e)) }
        }
    } else {
        let mut text = s.clone();
        if !text.contains("://") {
            text = base64_text(&text).map_err(|_| crate::tr!("Не удалось распознать ссылку, JSON или Base64-подписку.", "Could not recognise the link, JSON or Base64 subscription.").to_string())?;
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
                Err(e) => crate::log::add(crate::trf!("Профиль маршрутизации не разобран: {e}", "Routing profile could not be read: {e}")),
            }
        }
    }
    if result.profiles.is_empty() {
        if let Some(first) = result.skipped.first() { return Err(crate::trf!("Ни одна строка не поддерживается ({}). {first}", "No line is supported ({}). {first}", result.skipped.len())); }
        return Err(crate::tr!("Не найдено серверов поддерживаемого формата.", "No servers in a supported format.").into());
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
    Err(crate::tr!("Это не конфигурация Xray (JSON sing-box не поддерживается).", "This is not an Xray configuration (sing-box JSON is not supported).").into())
}

/// One provider Xray document = one row; its servers stay inside the document.
pub fn from_xray_document(root: &Obj, raw: &str) -> Result<Profile, String> {
    let single = [Value::Object(root.clone())];
    let outbounds: Vec<&Obj> = match root.a("outbounds") {
        Some(a) => a.iter().filter_map(|v| v.as_object()).collect(),
        None if root.has("protocol") => single.iter().filter_map(|v| v.as_object()).collect(),
        None => return Err(crate::tr!("В конфигурации Xray нет исходящих подключений.", "The Xray configuration has no outbounds.").into()),
    };
    let proxies: Vec<&Obj> = outbounds.into_iter().filter(|o| !LOCAL_PROTOCOLS.contains(&o.s("protocol").to_ascii_lowercase().as_str())).collect();
    if proxies.is_empty() { return Err(crate::tr!("В конфигурации Xray нет прокси, только direct/block.", "The Xray configuration has no proxy, only direct/block.").into()); }
    let endpoints: Vec<Endpoint> = proxies.iter().flat_map(|p| endpoints(p)).collect();
    let first = endpoints.first().ok_or(crate::tr!("Не удалось прочитать адрес сервера в конфигурации Xray.", "Could not read the server address in the Xray configuration."))?;
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
    if protocol == "wireguard" {
        // "host:port" (or "[v6]:port") of each peer.
        return settings.objects("peers").into_iter().filter_map(|p| {
            let e = p.s("endpoint"); let (h, port) = e.rsplit_once(':')?;
            Some(Endpoint { protocol: protocol.clone(), host: h.trim_matches(['[', ']']).to_string(), port: port.parse().unwrap_or(0), transport: "udp".into(), security: String::new(), tag: outbound.s("tag") })
        }).collect();
    }
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
    let mark = text.find("://").ok_or(crate::tr!("Это не ссылка подключения.", "This is not a connection link."))?;
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
        let close = rest.find(']').ok_or(crate::tr!("Незакрытый IPv6-адрес в ссылке.", "Unclosed IPv6 address in the link."))?;
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
    if l.host.is_empty() { return Err(crate::tr!("В ссылке нет адреса сервера.", "The link has no server address.").into()); }
    Ok(l)
}
fn number(v: &str) -> Result<i64, String> { v.trim().parse().map_err(|_| crate::trf!("Некорректный порт в ссылке: {v}", "Invalid port in the link: {v}")) }

pub fn from_link(text: &str) -> Result<Profile, String> {
    match link_to_xray(text) {
        Ok(mut outbound) => {
            let e = endpoints(&outbound).into_iter().next().ok_or(crate::tr!("В ссылке нет адреса сервера.", "The link has no server address."))?;
            let name = outbound.s("_name"); let description = outbound.s("_description");
            outbound.remove("_name"); outbound.remove("_description");
            Ok(Profile { name: if name.is_empty() { e.host.clone() } else { name }, source: text.into(), engine: "xray".into(), protocol: e.protocol, host: e.host, port: e.port, transport: e.transport, security: e.security, description, ..Default::default() })
        }
        Err(LinkError::Bad(e)) => Err(e),
    }
}

/// Share link → native Xray outbound tagged "proxy".
fn link_to_xray(text: &str) -> Result<Obj, LinkError> {
    let scheme = text[..text.find("://").ok_or(crate::tr!("Это не ссылка подключения.", "This is not a connection link."))?].to_ascii_lowercase();
    match scheme.as_str() {
        "vmess" => return vmess(text),
        "ss" => return shadowsocks(text).map_err(LinkError::Bad),
        "wireguard" | "wg" => return wireguard(text),
        "tuic" | "anytls" | "ssh" => return Err(crate::trf!("{} не поддерживается Xray.", "{} is not supported by Xray.", scheme.to_uppercase()).into()),
        _ => {}
    }
    let text = if ["socks", "socks5", "socks4", "http"].contains(&scheme.as_str()) { expand_base64_address(text, &scheme) } else { text.to_string() };
    let mut l = split(&text)?;
    if l.scheme == "hy2" { l.scheme = "hysteria2".into(); }
    if l.scheme == "socks5" || l.scheme == "socks4" { l.scheme = "socks".into(); }
    let qm = &l.query;
    let security = q(qm, "", &["security"]);
    if l.port < 1 && (l.scheme == "trojan" || l.scheme == "hysteria2" || security == "tls" || security == "reality") { l.port = 443; }
    if !(1..=65535).contains(&l.port) { return Err(crate::tr!("В ссылке нет корректного порта сервера.", "The link has no valid server port.").into()); }
    let auth = decode(&l.user_info);
    let mut outbound = match l.scheme.as_str() {
        "vless" => {
            if auth.is_empty() { return Err(crate::tr!("В ссылке VLESS нет UUID.", "The VLESS link has no UUID.").into()); }
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
            if !q(qm, "", &["obfs"]).is_empty() { return Err(crate::tr!("Hysteria2 с obfs не поддерживается Xray.", "Hysteria2 with obfs is not supported by Xray.").into()); }
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
            let pin = q(qm, "", &["pinSHA256", "pinnedPeerCertSha256", "pcs"]).replace(':', "").to_ascii_lowercase();
            // Xray 26 removed allowInsecure: the certificate is verified (pinned when the link has a pin).
            if !pin.is_empty() { tls.insert("pinnedPeerCertSha256".into(), pin.into()); }
            let mut o = obj! {"protocol" => "hysteria", "tag" => "proxy", "settings" => json!({"version": 2, "address": l.host, "port": l.port}),
                "streamSettings" => json!({"network": "hysteria", "hysteriaSettings": hy, "security": "tls", "tlsSettings": tls})};
            o.insert("_name".into(), l.fragment.clone().into()); o.insert("_description".into(), l.description.clone().into());
            return Ok(o);
        }
        other => return Err(crate::trf!("Неизвестный протокол: {other}", "Unknown protocol: {other}").into()),
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
            if !extra.is_empty() { x.insert("extra".into(), serde_json::from_str::<Value>(&extra).map_err(|_| crate::tr!("Параметр extra в ссылке XHTTP — некорректный JSON.", "The extra parameter of the XHTTP link is not valid JSON."))?); }
            s.insert("network".into(), "xhttp".into()); s.insert("xhttpSettings".into(), x.into());
        }
        "kcp" | "mkcp" => {
            let mut k = Obj::new();
            let header = q(qm, "", &["headerType"]); if !header.is_empty() && header != "none" { k.insert("header".into(), json!({"type": header})); }
            let seed = q(qm, "", &["seed"]); if !seed.is_empty() { k.insert("seed".into(), seed.into()); }
            s.insert("network".into(), "kcp".into()); s.insert("kcpSettings".into(), k.into());
        }
        // HTTP/2 and QUIC transports were removed from Xray.
        "http" | "h2" | "quic" => return Err(crate::trf!("Транспорт {kind} больше не поддерживается Xray.", "Transport {kind} is no longer supported by Xray.").into()),
        other => return Err(crate::trf!("Транспорт {other} не поддерживается.", "Transport {other} is not supported.").into()),
    }
    match security.as_str() {
        "tls" => {
            let mut t = obj! {"serverName" => q(qm, host, &["sni", "serverName", "peer"])};
            let fp = q(qm, "", &["fp", "fingerprint"]); if !fp.is_empty() { t.insert("fingerprint".into(), fp.into()); }
            let alpn = q(qm, "", &["alpn"]); if !alpn.is_empty() { t.insert("alpn".into(), json!(alpn.split(',').collect::<Vec<_>>())); }
            let pin = q(qm, "", &["pinnedPeerCertSha256", "pcs"]);
            // Xray 26 removed allowInsecure: the certificate is verified (pinned when the link has a pin).
            if !pin.is_empty() { t.insert("pinnedPeerCertSha256".into(), pin.into()); }
            let ech = q(qm, "", &["ech", "echConfigList"]); if !ech.is_empty() { t.insert("echConfigList".into(), ech.into()); }
            s.insert("security".into(), "tls".into()); s.insert("tlsSettings".into(), t.into());
        }
        "reality" => {
            let mut r = obj! {"serverName" => q(qm, host, &["sni", "serverName"]), "fingerprint" => q(qm, "chrome", &["fp", "fingerprint"]), "publicKey" => q(qm, "", &["pbk", "publicKey"]), "shortId" => q(qm, "", &["sid", "shortId"])};
            let spx = q(qm, "", &["spx", "spiderX"]); if !spx.is_empty() { r.insert("spiderX".into(), spx.into()); }
            let pqv = q(qm, "", &["pqv", "mldsa65Verify"]); if !pqv.is_empty() { r.insert("mldsa65Verify".into(), pqv.into()); }
            if r.s("publicKey").is_empty() { return Err(crate::tr!("В ссылке Reality нет публичного ключа (pbk).", "The Reality link has no public key (pbk).").into()); }
            s.insert("security".into(), "reality".into()); s.insert("realitySettings".into(), r.into());
        }
        "none" | "" => {}
        other => return Err(crate::trf!("Шифрование {other} не поддерживается.", "Security {other} is not supported.").into()),
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
            let l = split(s)?; if l.port < 1 { return Err(crate::tr!("Не удалось разобрать ссылку VMess.", "Could not read the VMess link.").into()); }
            let auth = decode(&l.user_info); let id = auth.splitn(2, ':').last().unwrap_or("").to_string();
            let mut o = obj! {"protocol" => "vmess", "tag" => "proxy", "settings" => json!({"vnext": [{"address": l.host, "port": l.port, "users": [{"id": id, "security": q(&l.query, "auto", &["encryption"])}]}]})};
            let st = stream(&l.query, &l.host, false)?; if !st.is_empty() { o.insert("streamSettings".into(), st.into()); }
            o.insert("_name".into(), l.fragment.into()); return Ok(o);
        }
    };
    let address = v.first(&["add", "address", "host"]).replace(['[', ']'], "");
    if address.is_empty() { return Err(crate::tr!("В ссылке VMess нет адреса сервера.", "The VMess link has no server address.").into()); }
    let id = v.first(&["id", "uuid"]); if id.is_empty() { return Err(crate::tr!("В ссылке VMess нет UUID.", "The VMess link has no UUID.").into()); }
    let port: i64 = v.s("port").trim().parse().map_err(|_| crate::tr!("Некорректный порт в ссылке VMess.", "Invalid port in the VMess link."))?;
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
        if query(&data[qm + 1..]).get("plugin").map(|p| !p.is_empty()).unwrap_or(false) { return Err(crate::tr!("Shadowsocks с plugin не поддерживается.", "Shadowsocks with a plugin is not supported.").into()); }
        data.truncate(qm);
    }
    let data = data.trim_end_matches('/').to_string();
    let data = if data.contains('@') { data } else { base64_text(&data)? };
    let at = data.rfind('@').ok_or(crate::tr!("Некорректная ссылка Shadowsocks.", "Invalid Shadowsocks link."))?;
    let user = &data[..at];
    let user = if user.contains(':') { decode(user) } else { base64_text(&decode(user))? };
    let mut auth = user.splitn(2, ':');
    let (Some(method), Some(password)) = (auth.next(), auth.next()) else { return Err(crate::tr!("В ссылке Shadowsocks нет пароля.", "The Shadowsocks link has no password.").into()) };
    let l = split(&format!("ss://{}", &data[at + 1..]))?;
    if l.port < 1 { return Err(crate::tr!("Некорректный адрес Shadowsocks.", "Invalid Shadowsocks address.").into()); }
    Ok(obj! {"protocol" => "shadowsocks", "tag" => "proxy", "settings" => json!({"servers": [{"address": l.host, "port": l.port, "method": method, "password": password}]}), "_name" => name, "_description" => description})
}

pub fn fingerprint(v: &str) -> String {
    let v = v.trim().to_ascii_lowercase();
    if ["chrome", "firefox", "edge", "safari", "360", "qq", "ios", "android", "random", "randomized"].contains(&v.as_str()) { v } else { "chrome".into() }
}

/// The Xray outbound for a link row (the runtime rebuilds it from the stored link).
pub fn link_outbound_for_runtime(s: &str) -> Result<Obj, String> { link_to_xray(s).map_err(|LinkError::Bad(e)| e) }

/// wireguard://<private key>@host:port?publickey=…&address=…&reserved=…&mtu=…
fn wireguard(text: &str) -> Result<Obj, LinkError> {
    let l = split(text)?; let qm = &l.query;
    let key = decode(&l.user_info); if key.is_empty() { return Err(crate::tr!("В ссылке WireGuard нет приватного ключа.", "The WireGuard link has no private key.").into()); }
    let public = q(qm, "", &["publickey", "public_key", "peer_public_key"]); if public.is_empty() { return Err(crate::tr!("В ссылке WireGuard нет публичного ключа сервера.", "The WireGuard link has no server public key.").into()); }
    let port = if l.port > 0 { l.port } else { 51820 };
    let host = if l.host.contains(':') { format!("[{}]", l.host) } else { l.host.clone() };
    let address: Vec<String> = q(qm, "", &["address", "ip"]).split(',').map(str::trim).filter(|a| !a.is_empty()).map(|a| if a.contains('/') { a.to_string() } else if a.contains(':') { format!("{a}/128") } else { format!("{a}/32") }).collect();
    let mut peer = obj! {"publicKey" => public, "endpoint" => format!("{host}:{port}"), "allowedIPs" => ["0.0.0.0/0", "::/0"]};
    let psk = q(qm, "", &["presharedkey", "psk"]); if !psk.is_empty() { peer.insert("preSharedKey".into(), psk.into()); }
    let mut settings = obj! {"secretKey" => key, "peers" => [peer]};
    if !address.is_empty() { settings.insert("address".into(), json!(address)); }
    let reserved: Vec<i64> = q(qm, "", &["reserved"]).split(',').filter_map(|r| r.trim().parse().ok()).collect();
    if !reserved.is_empty() { settings.insert("reserved".into(), json!(reserved)); }
    if let Ok(mtu) = q(qm, "", &["mtu"]).parse::<i64>() { settings.insert("mtu".into(), mtu.into()); }
    Ok(obj! {"protocol" => "wireguard", "tag" => "proxy", "settings" => settings, "_name" => l.fragment, "_description" => l.description})
}
