//! sing-box config for what Xray cannot run: TUIC, AnyTLS, WireGuard, obfuscated Hysteria2, sing-box JSON.

use crate::json::{child, child_list, Get, Obj};
use crate::model::{split_values, AppState, Profile};
use crate::obj;
use crate::routing;
use serde_json::{json, Value};

pub fn build(state: &AppState, p: &Profile, system_dns: &[String], api_port: u16, api_secret: &str) -> Result<Obj, String> {
    let s = &state.settings; let tun = crate::model::uses_tun(s);
    let mut config;
    if p.full {
        config = p.singbox.clone().ok_or("Нет конфигурации sing-box.")?;
        normalize_legacy_dns(&mut config);
        // Wraith may run as administrator: no extra listening services, and no endpoints that keep state on disk.
        config.remove("services");
        if let Some(eps) = config.get_mut("endpoints").and_then(|v| v.as_array_mut()) { eps.retain(|e| e.s("type") != "tailscale"); }
        config.insert("inbounds".into(), Value::Array(inbounds(s, tun)?));
        let first = config.objects("outbounds").first().map(|o| o.s("tag")).unwrap_or_default();
        let route = child(&mut config, "route");
        route.insert("auto_detect_interface".into(), true.into());
        if !route.has("final") && !first.is_empty() { route.insert("final".into(), first.into()); }
        if tun {
            let rules = child_list(route, "rules");
            if !rules.iter().any(|r| r.s("action") == "hijack-dns") { rules.insert(0, json!({"protocol": ["dns"], "action": "hijack-dns"})); rules.insert(0, json!({"action": "sniff"})); }
        }
    } else {
        let mut outbound = p.singbox.clone().ok_or("Нет конфигурации sing-box.")?; outbound.insert("tag".into(), "proxy".into());
        let mut rules = vec![json!({"action": "sniff"})];
        if tun { rules.push(json!({"protocol": ["dns"], "action": "hijack-dns"})); }
        if s.b("bypassLan", true) { rules.push(json!({"ip_is_private": true, "outbound": "direct"})); }
        if !s.b("dnsFromJson", false) && routing::custom_dns(s).is_empty() && !system_dns.is_empty() {
            let cidrs: Vec<String> = system_dns.iter().map(|ip| if ip.contains(':') { format!("{ip}/128") } else { format!("{ip}/32") }).collect();
            rules.push(json!({"ip_cidr": cidrs, "port": 53, "outbound": "direct"}));
        }
        let mode = s.s_or("mode", "rules");
        if mode != "direct" { for rule in routing::rules_for(state, p) { append_rule(&mut rules, &rule); } }
        let provider = routing::profile_for(state, p);
        let fin = if mode == "direct" { "direct" } else if mode == "global" || routing::global_proxy(provider.as_ref()) { "proxy" } else { "direct" };
        config = obj! {
            "dns" => dns(state, p, system_dns),
            "inbounds" => inbounds(s, tun)?,
            "outbounds" => json!([outbound, {"type": "direct", "tag": "direct"}, {"type": "block", "tag": "block"}]),
            "route" => json!({"rules": rules, "final": fin, "auto_detect_interface": true, "default_domain_resolver": "local"})
        };
    }
    config.insert("log".into(), json!({"level": if s.b("debugLogs", false) { "info" } else { "warn" }, "timestamp": true}));
    // The Clash API gives the traffic counters; random port and a session secret.
    config.insert("experimental".into(), json!({"clash_api": {"external_controller": format!("127.0.0.1:{api_port}"), "secret": api_secret}}));
    Ok(config)
}

pub fn build_probe(state: &AppState, p: &Profile, port: u16) -> Result<Obj, String> {
    let mut copy = state.clone();
    copy.set("tunnelMode", "proxy".into()); copy.set("allowLan", false.into()); copy.set("mode", "global".into());
    let mut config = build(&copy, p, &[], crate::util::free_port(), &crate::util::random_hex(16))?;
    config.insert("inbounds".into(), json!([{"type": "mixed", "tag": "probe-in", "listen": "127.0.0.1", "listen_port": port}]));
    config.remove("experimental"); config.insert("log".into(), json!({"level": "error"}));
    let route = child(&mut config, "route");
    route.insert("rules".into(), json!([{"action": "sniff"}]));
    if !p.full { route.insert("final".into(), "proxy".into()); }
    Ok(config)
}

fn inbounds(s: &Obj, tun: bool) -> Result<Vec<Value>, String> {
    let mut list = vec![];
    if tun {
        let mut address = vec!["172.19.0.1/30"]; if crate::model::ip_mode(s) != "v4" { address.push("fdfe:dcba:9876::1/126"); }
        list.push(json!({"type": "tun", "tag": "tun-in", "interface_name": "Wraith", "address": address, "mtu": s.i("mtu", 1500), "auto_route": true, "strict_route": true, "stack": "mixed"}));
    }
    let lan = s.b("allowLan", false);
    let mut http = obj! {"type" => "mixed", "tag" => "mixed-in", "listen" => if lan { "0.0.0.0" } else { "127.0.0.1" }, "listen_port" => s.i("httpProxyPort", 2082)};
    if lan {
        let password = s.s("proxyPassword"); if password.chars().count() < 8 { return Err("Для доступа из локальной сети задайте пароль прокси не короче 8 символов.".into()); }
        http.insert("users".into(), json!([{"username": "wraith", "password": password}]));
    }
    let mut socks = http.clone(); socks.insert("tag".into(), "socks-in".into()); socks.insert("listen_port".into(), s.i("proxyPort", 2081).into());
    list.push(http.into()); list.push(socks.into());
    Ok(list)
}

fn dns(state: &AppState, p: &Profile, system_dns: &[String]) -> Obj {
    let s = &state.settings;
    let mut value = if s.b("dnsFromJson", false) { routing::dns_address(routing::profile_for(state, p).as_ref(), "Remote") } else { String::new() };
    if value.is_empty() { value = routing::custom_dns(s); }
    let remote = if !value.is_empty() { server(&value, "remote", true) }
        else if let Some(ip) = system_dns.first() { obj! {"type" => "udp", "tag" => "remote", "server" => ip} }
        else { server(routing::DEFAULT_CUSTOM, "remote", true) };
    obj! {"servers" => json!([{"type": "local", "tag": "local"}, remote]), "final" => "remote", "strategy" => match crate::model::ip_mode(s).as_str() { "v4" => "ipv4_only", "v6" => "ipv6_only", _ => "prefer_ipv4" }}
}
fn server(address: &str, tag: &str, via_proxy: bool) -> Obj {
    let mut o = Obj::new();
    match url::Url::parse(address) {
        Ok(u) if ["https", "tls", "quic", "h3", "udp", "tcp"].contains(&u.scheme()) => {
            o.insert("type".into(), u.scheme().into()); o.insert("server".into(), u.host_str().unwrap_or("").trim_matches(['[', ']']).into());
            if let Some(port) = u.port() { o.insert("server_port".into(), port.into()); }
            if u.scheme() == "https" || u.scheme() == "h3" { o.insert("path".into(), if u.path().len() > 1 { u.path().to_string() } else { "/dns-query".into() }.into()); }
        }
        _ => { o.insert("type".into(), "udp".into()); o.insert("server".into(), address.trim().into()); }
    }
    o.insert("tag".into(), tag.into());
    if via_proxy { o.insert("detour".into(), "proxy".into()); }
    if !routing::is_ip(&o.s("server")) { o.insert("domain_resolver".into(), "local".into()); }
    o
}

fn append_rule(out: &mut Vec<Value>, rule: &Obj) {
    if !rule.b("enabled", true) { return; }
    let values = split_values(&rule.s("value"));
    let mut r = Obj::new();
    match rule.s("kind").as_str() {
        "domain" => { r.insert("domain".into(), json!(values)); }
        "domain_suffix" => { r.insert("domain_suffix".into(), json!(values)); }
        "domain_keyword" => { r.insert("domain_keyword".into(), json!(values)); }
        "domain_regex" => { r.insert("domain_regex".into(), json!(values)); }
        "ip_cidr" => { r.insert("ip_cidr".into(), json!(values)); }
        "ip_is_private" => { r.insert("ip_is_private".into(), true.into()); }
        "process" => { r.insert("process_name".into(), json!(values)); }
        _ => return, // geosite/geoip need remote rule sets; Xray handles them natively.
    }
    if let Ok(port) = rule.s("port").parse::<i64>() { r.insert("port".into(), port.into()); }
    match rule.s_or("action", "proxy").as_str() { "block" => { r.insert("action".into(), "reject".into()); } "direct" => { r.insert("outbound".into(), "direct".into()); } _ => { r.insert("outbound".into(), "proxy".into()); } }
    out.push(r.into());
}

/// sing-box 1.14 removed the { address } DNS server schema; migrate the runtime copy only.
fn normalize_legacy_dns(config: &mut Obj) {
    let Some(dns) = config.get_mut("dns").and_then(|d| d.as_object_mut()) else { return };
    let Some(servers) = dns.get("servers").and_then(|s| s.as_array()).cloned() else { return };
    let mut out = vec![];
    for entry in servers {
        match &entry {
            Value::String(a) if !a.trim().is_empty() => out.push(Value::Object(server(a, &format!("dns-{}", out.len()), false))),
            Value::Object(old) if old.s("type").is_empty() && !old.s("address").is_empty() => {
                let a = old.s("address");
                if a == "local" { out.push(json!({"type": "local", "tag": old.s_or("tag", "local")})); continue; }
                let mut s = server(&a, &old.s_or("tag", &format!("dns-{}", out.len())), false);
                if !old.s("detour").is_empty() { s.insert("detour".into(), old.s("detour").into()); }
                out.push(s.into());
            }
            _ => out.push(entry.clone()),
        }
    }
    dns.insert("servers".into(), Value::Array(out)); dns.remove("fakeip");
}
