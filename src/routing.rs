//! Happ routing profiles and the DNS choice (provider's, user's own, or the system's resolvers).

use crate::json::{base64_text, parse_obj, Get, Obj};
use crate::model::{AppState, Profile};
use crate::obj;
use serde_json::Value;

pub fn decode(link: &str) -> Result<Obj, String> {
    if !link.starts_with("happ://routing/add/") && !link.starts_with("happ://routing/onadd/") { return Err(crate::tr!("Некорректный профиль маршрутизации.", "Invalid routing profile.").into()); }
    let slash = link["happ://routing/".len()..].find('/').map(|i| i + "happ://routing/".len()).ok_or(crate::tr!("Некорректный профиль маршрутизации.", "Invalid routing profile."))?;
    parse_obj(&base64_text(link[slash + 1..].trim())?)
}

/// Inline profile of a link import first, then the subscription's profile (never for JSON subscriptions).
pub fn profile_for(state: &AppState, p: &Profile) -> Option<Obj> {
    if let Some(r) = &p.routing { if !r.is_empty() { return Some(r.clone()); } }
    if p.group.is_empty() { return None; }
    let sub = state.subscription(&p.group)?;
    if !sub.routing_enabled || sub.format == "json" || sub.routing_link.is_empty() || sub.routing_link.starts_with("happ://routing/off") { return None; }
    match decode(&sub.routing_link) { Ok(r) => Some(r), Err(e) => { crate::log::add(crate::trf!("Профиль маршрутизации подписки не разобран: {e}", "The subscription's routing profile could not be read: {e}")); None } }
}

/// DNS address from the documented triple (type + domain + ip) or the short RemoteDns form.
pub fn dns_address(routing: Option<&Obj>, prefix: &str) -> String {
    let Some(r) = routing else { return String::new() };
    let legacy = r.first(&[&format!("{prefix}Dns"), &format!("{prefix}DNS")]);
    let kind = r.first(&[&format!("{prefix}DNSType"), &format!("{prefix}DnsType")]).to_ascii_uppercase();
    let domain = r.first(&[&format!("{prefix}DNSDomain"), &format!("{prefix}DnsDomain")]);
    let ip = r.first(&[&format!("{prefix}DNSIP"), &format!("{prefix}DnsIp"), &format!("{prefix}DNSIp")]);
    if domain.is_empty() && ip.is_empty() { return legacy; }
    let scheme = match kind.as_str() { "DOH" | "HTTPS" => "https", "DOT" | "TLS" => "tls", "DOQ" | "QUIC" => "quic", "DOH3" | "H3" => "h3", _ => "" };
    let host = if domain.is_empty() { ip } else { domain };
    if host.contains("://") { return host; }
    match scheme { "https" => format!("https://{host}/dns-query"), "" => host, s => format!("{s}://{host}") }
}

/// GlobalProxy=false means unmatched traffic stays direct (ignored without any Proxy rule).
pub fn global_proxy(routing: Option<&Obj>) -> bool {
    let Some(r) = routing else { return true };
    if !r.s("GlobalProxy").trim().eq_ignore_ascii_case("false") { return true; }
    let has_proxy_rules = r.a("ProxySites").map(|a| !a.is_empty()).unwrap_or(false) || r.a("ProxyIp").map(|a| !a.is_empty()).unwrap_or(false);
    !has_proxy_rules
}

/// Provider rules then user rules, as {name, kind, value, action, port?, network?}.
pub fn rules_for(state: &AppState, p: &Profile) -> Vec<Obj> {
    let mut out = vec![];
    if let Some(provider) = profile_for(state, p) { append(&mut out, &provider); }
    out.extend(state.rules.iter().cloned());
    out
}

pub fn append(out: &mut Vec<Obj>, json: &Obj) {
    let name = json.s_or("Name", crate::tr!("Профиль провайдера", "Provider profile"));
    if let Some(ordered) = json.a("Rules").filter(|a| !a.is_empty()) {
        for rule in ordered.iter().filter_map(|v| v.as_object()) {
            let action = rule.s_or("action", "proxy");
            emit(out, &name, &action, rule.a("domain"), false, Some(rule));
            emit(out, &name, &action, rule.a("ip"), true, Some(rule));
        }
        return;
    }
    for prefix in ["Block", "Direct", "Proxy"] {
        for field in ["Sites", "Ip"] {
            let action = match prefix { "Block" => "block", "Direct" => "direct", _ => "proxy" };
            emit(out, &format!("{name} · {prefix}"), action, json.a(&format!("{prefix}{field}")), field == "Ip", None);
        }
    }
}

fn emit(out: &mut Vec<Obj>, name: &str, action: &str, values: Option<&Vec<Value>>, addresses: bool, source: Option<&Obj>) {
    let Some(values) = values.filter(|v| !v.is_empty()) else { return };
    let mut grouped: Vec<(String, Vec<String>)> = vec![];
    let mut add = |kind: &str, value: &str| {
        let mut v = value.trim().to_string(); if v.is_empty() { return; }
        if kind.starts_with("domain") && kind != "domain_regex" { v = ascii_domain(&v); }
        match grouped.iter_mut().find(|(k, _)| k == kind) { Some((_, list)) => if !list.contains(&v) { list.push(v) }, None => grouped.push((kind.to_string(), vec![v])) }
    };
    for item in values {
        let value = match item { Value::String(s) => s.trim().to_string(), other => other.to_string() };
        if value.is_empty() || value == "null" || value.starts_with("ext:") { continue; }
        if let Some(g) = value.strip_prefix("geoip:") { if g.trim().eq_ignore_ascii_case("private") { add("ip_is_private", "true") } else { add("geoip", g) } continue; }
        if let Some(g) = value.strip_prefix("geosite:") { add("geosite", g); continue; }
        if addresses { add("ip_cidr", &value); continue; }
        if let Some(v) = value.strip_prefix("full:") { add("domain", v); continue; }
        if let Some(v) = value.strip_prefix("regexp:") { add("domain_regex", v); continue; }
        if let Some(v) = value.strip_prefix("keyword:") { add("domain_keyword", v); continue; }
        if let Some(v) = value.strip_prefix("domain:") { add("domain_suffix", v); continue; }
        add("domain_keyword", &value);
    }
    for (kind, list) in grouped {
        let mut rule = obj! {"name" => name, "kind" => kind, "value" => list.join("\n"), "action" => action, "enabled" => true, "provider" => true};
        if let Some(src) = source {
            let port = src.s("port"); if !port.trim().is_empty() { rule.insert("port".into(), port.trim().into()); }
            let network = src.s("network"); if !network.trim().is_empty() { rule.insert("network".into(), network.trim().into()); }
            if let Some(p) = src.a("protocol").filter(|a| !a.is_empty()) { rule.insert("protocol".into(), Value::Array(p.clone())); }
        }
        out.push(rule);
    }
}

/// Requests carry lower-case punycode names; "честныйзнак.рф" in a list would never match as written.
pub fn ascii_domain(v: &str) -> String {
    let lower = v.to_lowercase();
    let (dot, body) = match lower.strip_prefix('.') { Some(b) => (true, b.to_string()), None => (false, lower.clone()) };
    let body = if body.is_ascii() { body } else { idna::domain_to_ascii(&body).unwrap_or(body) };
    if dot { format!(".{body}") } else { body }
}

// ------------------------------------------------------------------ DNS

pub const DEFAULT_CUSTOM: &str = "https://8.8.8.8/dns-query";

pub fn custom_dns(s: &Obj) -> String {
    if !s.b("customDns", false) { return String::new(); }
    let v = s.s("tunnelDns"); if v.trim().is_empty() { DEFAULT_CUSTOM.into() } else { v.trim().into() }
}

pub fn is_ip(v: &str) -> bool { v.trim_matches(['[', ']']).parse::<std::net::IpAddr>().is_ok() }
pub fn dns_host(v: &Value) -> String {
    match v {
        Value::Object(o) => dns_host(&Value::String(o.first(&["address", "server"]))),
        Value::String(s) => {
            let s = s.trim();
            if is_ip(s) { return s.trim_matches(['[', ']']).into(); }
            if s.contains("://") { if let Ok(u) = url::Url::parse(s) { return u.host_str().unwrap_or("").trim_matches(['[', ']']).into(); } }
            s.into()
        }
        _ => String::new(),
    }
}

/// Resolvers of the active physical adapter (read before the TUN exists). IPv4 first, no link-local.
pub fn system_resolvers() -> Vec<String> {
    use windows_sys::Win32::NetworkManagement::IpHelper::{GetAdaptersAddresses, IP_ADAPTER_ADDRESSES_LH, GAA_FLAG_INCLUDE_GATEWAYS, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_MULTICAST};
    use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6, AF_UNSPEC, SOCKADDR_IN, SOCKADDR_IN6};
    let (mut v4, mut v6) = (vec![], vec![]);
    let mut size: u32 = 32 * 1024;
    let mut buffer = vec![0u8; size as usize];
    let flags = GAA_FLAG_INCLUDE_GATEWAYS | GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST;
    let mut rc = unsafe { GetAdaptersAddresses(AF_UNSPEC as u32, flags, std::ptr::null(), buffer.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH, &mut size) };
    if rc == 111 { buffer = vec![0u8; size as usize]; rc = unsafe { GetAdaptersAddresses(AF_UNSPEC as u32, flags, std::ptr::null(), buffer.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH, &mut size) }; }
    if rc != 0 { return vec![]; }
    let mut adapter = buffer.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;
    unsafe {
        while !adapter.is_null() {
            let a = &*adapter; adapter = a.Next;
            if a.OperStatus != 1 || a.IfType == 24 || a.IfType == 131 { continue; } // up; not loopback or tunnel
            let name = wide_str(a.FriendlyName); let desc = wide_str(a.Description);
            if name.contains("Wraith") || desc.contains("Wintun") || desc.contains("WireGuard") { continue; }
            if a.FirstGatewayAddress.is_null() { continue; }
            let mut dns = a.FirstDnsServerAddress;
            while !dns.is_null() {
                let d = &*dns; dns = d.Next;
                let sa = d.Address.lpSockaddr; if sa.is_null() { continue; }
                match (*sa).sa_family {
                    f if f == AF_INET => { let s = &*(sa as *const SOCKADDR_IN); let ip = std::net::Ipv4Addr::from(u32::from_be(s.sin_addr.S_un.S_addr)).to_string(); if !v4.contains(&ip) { v4.push(ip); } }
                    f if f == AF_INET6 => {
                        let s = &*(sa as *const SOCKADDR_IN6); let ip = std::net::Ipv6Addr::from(s.sin6_addr.u.Byte);
                        let seg = ip.segments()[0];
                        if (seg & 0xffc0) == 0xfe80 || (seg & 0xffc0) == 0xfec0 { continue; }
                        let t = ip.to_string(); if !v6.contains(&t) { v6.push(t); }
                    }
                    _ => {}
                }
            }
        }
    }
    v4.extend(v6); v4
}
unsafe fn wide_str(p: *const u16) -> String {
    if p.is_null() { return String::new(); }
    let mut len = 0; while *p.add(len) != 0 { len += 1; }
    String::from_utf16_lossy(std::slice::from_raw_parts(p, len))
}
