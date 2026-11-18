//! Native Xray JSON for a server row. The provider's document (balancers, observatory, dialer
//! chains) is preserved; Wraith adds its entry points, DNS policy, user rules and DPI options.
//! Port of the Android XrayRuntimeConfig.

use crate::importer::{self, endpoints, Endpoint, LOCAL_PROTOCOLS};
use crate::json::{child, child_list, objects_mut, Get, Obj};
use crate::model::{split_values, AppState, Profile};
use crate::obj;
use crate::routing;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

pub const AUTO_BALANCER: &str = "wraith-auto";

pub struct Built { pub config: Obj, pub proxy_tags: Vec<String> }

/// The Xray document behind a row: a link becomes a one-outbound document.
pub fn source(p: &Profile) -> Result<Obj, String> {
    let s = p.source.trim();
    if s.starts_with('{') {
        let root = crate::json::parse_obj(s)?;
        if !importer::looks_like_xray(&root) { return Err(crate::tr!("Это не конфигурация Xray.", "This is not an Xray configuration.").into()); }
        if root.a("outbounds").is_none() && root.has("protocol") {
            let mut outbound = root.clone();
            for f in ["dns", "routing", "log", "remarks", "name", "meta", "env"] { outbound.remove(f); }
            if outbound.s("tag").is_empty() { outbound.insert("tag".into(), "proxy".into()); }
            let mut doc = obj! {"outbounds" => [outbound]};
            for f in ["dns", "routing", "log"] { if let Some(v) = root.get(f) { doc.insert(f.into(), v.clone()); } }
            return Ok(doc);
        }
        return Ok(root);
    }
    if s.is_empty() { return Err(crate::tr!("У сервера нет исходной ссылки.", "The server has no source link.").into()); }
    let mut outbound = importer::link_outbound_for_runtime(s)?;
    outbound.remove("_name"); outbound.remove("_description");
    Ok(obj! {"outbounds" => [outbound]})
}

pub fn build(state: &AppState, profile: &Profile, system_dns: &[String], metrics_port: u16) -> Result<Built, String> {
    let settings = &state.settings;
    let tun = crate::model::uses_tun(settings);
    let mut root = prepare(profile, settings, &source(profile)?, None, false)?;

    // A provider "api" block opens its own gRPC listener that can add or remove outbounds; Wraith owns every entry point.
    for f in ["api", "remarks", "name", "meta", "version", "reverse"] { root.remove(f); }
    // The user's own DNS wins; the provider's DNS only with «DNS из JSON» and no own DNS.
    if root.o("dns").is_none() || !settings.b("dnsFromJson", false) || !routing::custom_dns(settings).is_empty() { root.insert("dns".into(), dns(state, profile, system_dns).into()); }
    else if !system_dns.is_empty() {
        // The provider's DNS may be unreachable from the VPN server (a resolver that only answers some
        // networks): the system's resolvers follow as a fallback, so names still resolve.
        let servers = child_list(child(&mut root, "dns"), "servers");
        for ip in system_dns { if !servers.iter().any(|s| routing::dns_host(s) == *ip) { servers.push(ip.clone().into()); } }
    }

    let client_inlets = client_inbound_tags(root.a("inbounds"));
    root.insert("inbounds".into(), Value::Array(inbounds(settings, tun, system_dns)?));
    let ours: Vec<&str> = if tun { vec!["tun-in", "socks", "http"] } else { vec!["socks", "http"] };
    if !client_inlets.is_empty() { remap_inbound_rules(&mut root, &client_inlets, &ours); }
    let app = ensure_app_outbounds(&mut root)?;
    let dns_tag = ensure_dns_outbound(&mut root);
    let resolver_ips = observatory_dns_ips(&root);
    let has_auto = has_balancer(&root, AUTO_BALANCER);
    let provider_balancing = provider_balancing(&root);
    let main_balancer = main_balancer(&root);
    let routing = child(&mut root, "routing");
    routing.entry("domainStrategy").or_insert_with(|| "IPIfNonMatch".into());
    let rules = child_list(routing, "rules");
    let mut head: Vec<Value> = vec![];
    if tun { head.push(json!({"type": "field", "inboundTag": ["tun-in"], "network": "tcp,udp", "port": "53", "outboundTag": dns_tag})); }
    // The system's own resolvers live in the local/ISP network and are queried directly, as without VPN.
    if routing::custom_dns(settings).is_empty() && !system_dns.is_empty() {
        head.push(json!({"type": "field", "ip": system_dns, "port": "53", "outboundTag": app[1]}));
    }
    // Observatory/balancers resolve their candidates independently of the balancer they measure.
    if !resolver_ips.is_empty() { head.push(json!({"type": "field", "ip": resolver_ips, "outboundTag": app[1]})); }
    // The local network and the router stay reachable.
    if settings.b("bypassLan", true) { head.push(json!({"type": "field", "ip": ["geoip:private"], "outboundTag": app[1]})); }
    let mode = settings.s_or("mode", "rules");
    if mode == "direct" || mode == "global" {
        // "Всё через VPN" with a provider balancer goes to the balancer, not to one fixed server.
        let balancer = if mode != "global" { String::new() } else if has_auto { AUTO_BALANCER.into() } else if provider_balancing { main_balancer } else { String::new() };
        let mut all = obj! {"type" => "field", "inboundTag" => ours};
        if !balancer.is_empty() { all.insert("balancerTag".into(), balancer.into()); } else { all.insert("outboundTag".into(), if mode == "direct" { app[1].clone() } else { app[0].clone() }.into()); }
        head.push(all.into());
    }
    let tail_rules: Vec<Value>;
    if profile.source.trim().starts_with('{') {
        let mut extra = vec![]; append_rules(&mut extra, &state.rules, &app);
        if has_auto { extra.push(json!({"type": "field", "inboundTag": ours, "balancerTag": AUTO_BALANCER})); }
        tail_rules = extra;
    } else {
        let provider = routing::profile_for(state, profile);
        let global = mode == "global" || routing::global_proxy(provider.as_ref());
        let mut extra = vec![];
        if mode != "direct" { append_rules(&mut extra, &routing::rules_for(state, profile), &app); }
        if !global && provider.is_some() { extra.push(json!({"type": "field", "inboundTag": ours, "outboundTag": app[1]})); }
        tail_rules = extra;
    }
    let existing = std::mem::take(rules);
    rules.extend(head); rules.extend(existing); rules.extend(tail_rules);

    let log = child(&mut root, "log");
    // Imported log paths could overwrite arbitrary files; the core logs to Wraith's journal only.
    log.remove("access"); log.remove("error");
    log.insert("loglevel".into(), if settings.b("debugLogs", false) { "info" } else { "warning" }.into());
    log.insert("dnsLog".into(), false.into());
    let system = child(child(&mut root, "policy"), "system");
    system.insert("statsOutboundDownlink".into(), true.into()); system.insert("statsOutboundUplink".into(), true.into());
    if root.o("stats").is_none() { root.insert("stats".into(), json!({})); }
    root.insert("metrics".into(), json!({"tag": "wraith-metrics", "listen": format!("127.0.0.1:{metrics_port}")}));
    root.remove("env");
    let proxy_tags = proxy_tags(&root);
    Ok(Built { config: root, proxy_tags })
}

/// Keeps provider tags and dialers intact while choosing the default server (pin) or the automatic choice.
pub fn prepare(profile: &Profile, settings: &Obj, source: &Obj, expected: Option<&Endpoint>, pin_candidate: bool) -> Result<Obj, String> {
    let mut root = source.clone();
    let fallback = Endpoint { protocol: profile.protocol.clone(), host: profile.host.clone(), port: profile.port, ..Default::default() };
    let expected = expected.unwrap_or(&fallback);
    let tag = proxy_tag(&mut root, Some(expected))?;
    // A provider balancer is the automatic choice the user asked for; probes still pin one server.
    let pin = pin_candidate || (!settings.b("autoSelect", false) && !provider_balancing(&root));
    if pin {
        let outbounds = child_list(&mut root, "outbounds");
        let index = outbounds.iter().position(|o| o.s("tag") == tag).ok_or(crate::tr!("Не найден выбранный Xray outbound.", "The selected Xray outbound was not found."))?;
        let mut chosen = outbounds.remove(index);
        if let Some(peers) = chosen.get_mut("settings").and_then(|v| v.as_object_mut()) {
            for key in ["vnext", "servers"] {
                let Some(list) = peers.get(key).and_then(|v| v.as_array()) else { continue };
                if list.len() < 2 { continue; }
                if let Some(m) = list.iter().find(|p| p.as_object().map(|p| importer::host(p).eq_ignore_ascii_case(&expected.host) && importer::port_of(p) == expected.port).unwrap_or(false)).cloned() {
                    peers.insert(key.into(), json!([m]));
                }
            }
        }
        outbounds.insert(0, chosen);
        if let Some(routing) = root.get_mut("routing").and_then(|v| v.as_object_mut()) {
            let selected: Vec<String> = routing.objects("balancers").into_iter()
                .filter(|b| b.a("selector").map(|s| s.iter().any(|x| x.as_str().map(|x| tag.starts_with(x)).unwrap_or(false))).unwrap_or(false))
                .map(|b| b.s("tag")).collect();
            if let Some(rules) = routing.get_mut("rules").and_then(|v| v.as_array_mut()) {
                for rule in objects_mut(rules) {
                    if selected.contains(&rule.s("balancerTag")) { rule.remove("balancerTag"); rule.insert("outboundTag".into(), tag.clone().into()); }
                }
            }
        }
    }
    if let Some(outbounds) = root.get_mut("outbounds").and_then(|v| v.as_array_mut()) {
        for outbound in objects_mut(outbounds) {
            let Some(legacy) = outbound.o("proxySettings").cloned() else { continue };
            let dependency = legacy.s("tag");
            if dependency.is_empty() { outbound.remove("proxySettings"); }
            else if legacy.b("transportLayer", false) {
                let sockopt = child(child(outbound, "streamSettings"), "sockopt");
                if sockopt.has("dialerProxy") && sockopt.s("dialerProxy") != dependency { return Err(crate::tr!("В JSON конфликтуют proxySettings и dialerProxy.", "proxySettings and dialerProxy conflict in the JSON.").into()); }
                sockopt.insert("dialerProxy".into(), dependency.into()); outbound.remove("proxySettings");
            }
        }
    }
    // Xray 26 refuses to start on the removed "allowInsecure"; drop it so the rest of the document still runs.
    if let Some(outbounds) = root.get_mut("outbounds").and_then(|v| v.as_array_mut()) {
        for outbound in objects_mut(outbounds) {
            if let Some(tls) = outbound.get_mut("streamSettings").and_then(|s| s.get_mut("tlsSettings")).and_then(|t| t.as_object_mut()) {
                if tls.remove("allowInsecure").is_some() { crate::log::add(crate::trf!("{}: allowInsecure удалён (Xray 26 его не поддерживает), сертификат будет проверяться.", "{}: allowInsecure removed (Xray 26 does not support it), the certificate will be verified.", outbound_tag_hint(tls))); }
            }
        }
    }
    // DPI options apply to every server the core may pick.
    let candidates = proxy_tags(&root);
    if pin { outbound_options(&mut root, settings, &tag)?; } else { for c in &candidates { outbound_options(&mut root, settings, c)?; } }
    if !pin && !provider_balancing(&root) && candidates.len() > 1 { add_auto_balancer(&mut root, &candidates, &tag, settings); }
    Ok(root)
}

fn add_auto_balancer(root: &mut Obj, candidates: &[String], fallback: &str, settings: &Obj) {
    if root.o("observatory").is_none() {
        root.insert("observatory".into(), json!({"subjectSelector": candidates, "probeUrl": settings.s_or("pingUrl", "https://www.gstatic.com/generate_204"), "probeInterval": "3m", "enableConcurrency": true}));
    }
    let routing = child(root, "routing");
    child_list(routing, "balancers").push(json!({"tag": AUTO_BALANCER, "selector": candidates, "strategy": {"type": "leastPing"}, "fallbackTag": fallback}));
    for rule in objects_mut(child_list(routing, "rules")) {
        if rule.s("outboundTag") == fallback { rule.remove("outboundTag"); rule.insert("balancerTag".into(), AUTO_BALANCER.into()); }
    }
}

pub fn provider_balancing(root: &Obj) -> bool {
    let Some(routing) = root.o("routing") else { return false };
    let balancers: HashSet<String> = routing.objects("balancers").iter().map(|b| b.s("tag")).filter(|t| !t.is_empty()).collect();
    !balancers.is_empty() && routing.objects("rules").iter().any(|r| balancers.contains(&r.s("balancerTag")))
}
pub fn has_balancer(root: &Obj, tag: &str) -> bool { root.o("routing").map(|r| r.objects("balancers").iter().any(|b| b.s("tag") == tag)).unwrap_or(false) }
fn main_balancer(root: &Obj) -> String {
    let mut found = String::new();
    for rule in root.o("routing").map(|r| r.objects("rules")).unwrap_or_default() {
        let b = rule.s("balancerTag"); if b.is_empty() { continue; }
        if !["domain", "ip", "port", "protocol"].iter().any(|k| rule.has(k)) { return b; }
        found = b;
    }
    found
}
/// Real proxy outbounds in provider order, excluding local helpers and hops used only as another server's dialer.
pub fn proxy_tags(root: &Obj) -> Vec<String> {
    let outbounds = root.objects("outbounds");
    let dialers: HashSet<String> = outbounds.iter().filter_map(|o| o.o("streamSettings").and_then(|s| s.o("sockopt")).map(|s| s.s("dialerProxy"))).filter(|d| !d.is_empty()).collect();
    outbounds.iter().filter(|o| !o.s("tag").is_empty() && !dialers.contains(&o.s("tag")) && !LOCAL_PROTOCOLS.contains(&o.s("protocol").to_ascii_lowercase().as_str())).map(|o| o.s("tag")).collect()
}
fn client_inbound_tags(inbounds: Option<&Vec<Value>>) -> HashSet<String> {
    let mut tags = HashSet::new();
    for inbound in inbounds.map(|a| a.iter().filter_map(|v| v.as_object()).collect::<Vec<_>>()).unwrap_or_default() {
        let protocol = inbound.s("protocol").to_ascii_lowercase(); let tag = inbound.s("tag"); if tag.is_empty() { continue; }
        let transparent = protocol == "dokodemo-door" && inbound.o("settings").b("followRedirect", false);
        if ["socks", "http", "mixed", "tun"].contains(&protocol.as_str()) || transparent { tags.insert(tag); }
    }
    tags
}
fn remap_inbound_rules(root: &mut Obj, inlets: &HashSet<String>, ours: &[&str]) {
    let Some(rules) = root.get_mut("routing").and_then(|r| r.get_mut("rules")).and_then(|r| r.as_array_mut()) else { return };
    for rule in objects_mut(rules) {
        let Some(tags) = rule.get_mut("inboundTag").and_then(|t| t.as_array_mut()) else { continue };
        if !tags.iter().any(|t| t.as_str().map(|t| inlets.contains(t)).unwrap_or(false)) { continue; }
        for t in ours { if !tags.iter().any(|x| x == t) { tags.push((*t).into()); } }
    }
}

/// Tag of the outbound serving `expected` (or the first proxy); an untagged one is named.
pub fn proxy_tag(root: &mut Obj, expected: Option<&Endpoint>) -> Result<String, String> {
    let outbounds = root.get_mut("outbounds").and_then(|v| v.as_array_mut()).ok_or(crate::tr!("В конфигурации Xray нет outbounds.", "The Xray configuration has no outbounds."))?;
    let mut chosen = None; let mut first = None;
    for (i, o) in outbounds.iter().enumerate() {
        let Some(o) = o.as_object() else { continue };
        if LOCAL_PROTOCOLS.contains(&o.s("protocol").to_ascii_lowercase().as_str()) { continue; }
        if first.is_none() { first = Some(i); }
        if let Some(e) = expected {
            if endpoints(o).iter().any(|x| x.host.eq_ignore_ascii_case(&e.host) && x.port == e.port && (e.protocol.is_empty() || x.protocol == e.protocol)) { chosen = Some(i); break; }
        }
    }
    let index = chosen.or(first).ok_or(crate::tr!("В конфигурации Xray нет прокси-outbound.", "The Xray configuration has no proxy outbound."))?;
    let tag = outbounds[index].s("tag");
    if !tag.is_empty() { return Ok(tag); }
    let tag = unique_tag(outbounds, "proxy");
    outbounds[index].as_object_mut().unwrap().insert("tag".into(), tag.clone().into());
    Ok(tag)
}

fn observatory_dns_ips(root: &Obj) -> Vec<String> {
    let mut ips = vec![];
    let has_obs = root.o("observatory").is_some() || root.o("burstObservatory").is_some();
    if !has_obs || root.o("routing").map(|r| r.objects("balancers").is_empty()).unwrap_or(true) { return ips; }
    for server in root.o("dns").and_then(|d| d.a("servers")).cloned().unwrap_or_default() {
        let host = routing::dns_host(&server);
        if routing::is_ip(&host) && !ips.contains(&host) { ips.push(host); }
    }
    ips
}

pub fn inbounds(s: &Obj, tun: bool, system_dns: &[String]) -> Result<Vec<Value>, String> {
    let mut list = vec![];
    if tun {
        let mtu = s.i("mtu", 1500); if !(1280..=9000).contains(&mtu) { return Err(crate::tr!("MTU должен быть от 1280 до 9000.", "MTU must be between 1280 and 9000.").into()); }
        // Two halves of the address space beat the Ethernet default route whatever the interface metrics are.
        let mut routes: Vec<String> = vec!["0.0.0.0/1".into(), "128.0.0.0/1".into()]; if crate::model::ip_mode(s) != "v4" { routes.push("::/1".into()); routes.push("8000::/1".into()); }
        // Windows asks the network's resolvers (usually the router, an on-link address) directly, past the
        // tunnel. Host routes pull those queries into TUN, where the port-53 rule hands them to Xray's DNS.
        for ip in system_dns { if routing::is_ip(ip) { routes.push(if ip.contains(':') { format!("{ip}/128") } else { format!("{ip}/32") }); } }
        list.push(json!({"tag": "tun-in", "protocol": "tun",
            "settings": {"name": "Wraith", "MTU": mtu, "autoSystemRoutingTable": routes, "autoOutboundsInterface": "auto"},
            // The sniffed domain replaces the IP: an ISP resolver's answer for a blocked site is not used, the server resolves it.
            "sniffing": {"enabled": s.b("sniff", true), "routeOnly": false, "destOverride": ["http", "tls", "quic"]}}));
    }
    let lan = s.b("allowLan", false); let listen = if lan { "0.0.0.0" } else { "127.0.0.1" };
    let sniff = json!({"enabled": s.b("sniff", true), "routeOnly": true, "destOverride": ["http", "tls"]});
    let (socks_port, http_port) = (s.i("proxyPort", 2081), s.i("httpProxyPort", 2082));
    if !lan {
        list.push(json!({"tag": "socks", "listen": listen, "port": socks_port, "protocol": "socks", "settings": {"auth": "noauth", "udp": true}, "sniffing": sniff}));
        list.push(json!({"tag": "http", "listen": listen, "port": http_port, "protocol": "http", "settings": {}, "sniffing": sniff}));
    } else {
        // A LAN-reachable proxy without credentials is an open relay for everyone on the network.
        let password = s.s("proxyPassword"); if password.chars().count() < 8 { return Err(crate::tr!("Для доступа из локальной сети задайте пароль прокси не короче 8 символов.", "For access from the local network set a proxy password of at least 8 characters.").into()); }
        let accounts = json!([{"user": "wraith", "pass": password}]);
        list.push(json!({"tag": "socks", "listen": listen, "port": socks_port, "protocol": "socks", "settings": {"auth": "password", "accounts": accounts, "udp": true}, "sniffing": sniff}));
        list.push(json!({"tag": "http", "listen": listen, "port": http_port, "protocol": "http", "settings": {"accounts": accounts}, "sniffing": sniff}));
    }
    Ok(list)
}

fn ensure_dns_outbound(root: &mut Obj) -> String {
    let outbounds = child_list(root, "outbounds");
    if let Some(i) = outbounds.iter().position(|o| o.s("protocol") == "dns") {
        if outbounds[i].s("tag").is_empty() { let t = unique_tag(outbounds, "wraith-dns"); outbounds[i].as_object_mut().unwrap().insert("tag".into(), t.into()); }
        return outbounds[i].s("tag");
    }
    let tag = unique_tag(outbounds, "wraith-dns");
    outbounds.push(json!({"protocol": "dns", "tag": tag, "settings": {"nonIPQuery": "skip"}}));
    tag
}
fn ensure_app_outbounds(root: &mut Obj) -> Result<[String; 3], String> {
    let proxy = match proxy_tags(root).into_iter().next() { Some(t) => t, None => proxy_tag(root, None)? };
    let outbounds = child_list(root, "outbounds");
    let mut direct = String::new(); let mut block = String::new();
    for o in outbounds.iter() {
        let helper = o.o("settings").map(|s| s.has("fragment") || s.has("noises")).unwrap_or(false);
        if o.s("protocol") == "freedom" && direct.is_empty() && !o.s("tag").is_empty() && !helper { direct = o.s("tag"); }
        if o.s("protocol") == "blackhole" && block.is_empty() && !o.s("tag").is_empty() { block = o.s("tag"); }
    }
    if direct.is_empty() { direct = unique_tag(outbounds, "direct"); outbounds.push(json!({"protocol": "freedom", "tag": direct, "settings": {}})); }
    if block.is_empty() { block = unique_tag(outbounds, "block"); outbounds.push(json!({"protocol": "blackhole", "tag": block, "settings": {}})); }
    Ok([proxy, direct, block])
}
pub fn unique_tag(outbounds: &[Value], base: &str) -> String {
    let mut candidate = base.to_string(); let mut n = 2;
    while outbounds.iter().any(|o| o.s("tag") == candidate) { candidate = format!("{base}-{n}"); n += 1; }
    candidate
}

/// Provider DNS (with «DNS из JSON»), else the user's own DNS (with «Свой DNS»), else the system's resolvers.
fn dns(state: &AppState, p: &Profile, system_dns: &[String]) -> Obj {
    let s = &state.settings;
    let mut value = routing::custom_dns(s);
    if value.is_empty() && s.b("dnsFromJson", false) { value = routing::dns_address(routing::profile_for(state, p).as_ref(), "Remote"); }
    let mut servers: Vec<Value> = if !value.is_empty() { vec![value.into()] } else { system_dns.iter().map(|ip| ip.clone().into()).collect() };
    if servers.is_empty() { servers.push(routing::DEFAULT_CUSTOM.into()); }
    obj! {"servers" => servers, "queryStrategy" => match crate::model::ip_mode(s).as_str() { "v4" => "UseIPv4", "v6" => "UseIPv6", _ => "UseIP" }}
}

fn append_rules(out: &mut Vec<Value>, source: &[Obj], tags: &[String; 3]) {
    for rule in source {
        if !rule.b("enabled", true) { continue; }
        let kind = rule.s("kind"); let action = rule.s_or("action", "proxy");
        let (mut domains, mut ips, mut processes): (Vec<String>, Vec<String>, Vec<String>) = (vec![], vec![], vec![]);
        for item in split_values(&rule.s("value")) {
            match kind.as_str() {
                "domain" => domains.push(format!("full:{item}")),
                "domain_suffix" => domains.push(format!("domain:{}", item.trim_start_matches('.'))),
                "domain_keyword" => domains.push(format!("keyword:{item}")),
                "domain_regex" => domains.push(format!("regexp:{item}")),
                "geosite" => domains.push(format!("geosite:{item}")),
                "ip_cidr" => ips.push(item),
                "ip_is_private" => ips.push("geoip:private".into()),
                "geoip" => ips.push(format!("geoip:{item}")),
                "process" => processes.push(item),
                _ => {}
            }
        }
        let mut r = obj! {"type" => "field"};
        if !domains.is_empty() { r.insert("domain".into(), json!(domains)); }
        if !ips.is_empty() { r.insert("ip".into(), json!(ips)); }
        if !processes.is_empty() { r.insert("process".into(), json!(processes)); }
        let port = rule.s("port"); if !port.is_empty() { r.insert("port".into(), port.into()); }
        let network = rule.s("network"); if !network.is_empty() { r.insert("network".into(), network.into()); }
        if let Some(p) = rule.a("protocol").filter(|p| !p.is_empty()) { r.insert("protocol".into(), Value::Array(p.clone())); }
        if r.len() <= 1 { continue; }
        r.insert("outboundTag".into(), match action.as_str() { "direct" => tags[1].clone(), "block" => tags[2].clone(), _ => tags[0].clone() }.into());
        out.push(r.into());
    }
}

// ------------------------------------------------------------------ DPI options

fn outbound_options(root: &mut Obj, s: &Obj, tag: &str) -> Result<(), String> {
    let fragment = s.b("fragment", false); let noise = s.b("noise", false); let mux = s.b("mux", false);
    if !fragment && !noise && !mux { return Ok(()); }
    let outbounds = child_list(root, "outbounds");
    let Some(index) = outbounds.iter().position(|o| o.s("tag") == tag) else { return Ok(()) };
    let protocol = outbounds[index].s("protocol").to_ascii_lowercase();
    if LOCAL_PROTOCOLS.contains(&protocol.as_str()) || protocol == "hysteria" { return Ok(()); }
    if fragment || noise {
        let helper_tag = unique_tag(outbounds, "wraith-dialer");
        let mut options = Obj::new();
        if fragment { options.insert("fragment".into(), self::fragment(s)?.into()); }
        if noise { options.insert("noises".into(), json!([self::noise(s)?])); }
        let primary = outbounds[index].as_object_mut().unwrap();
        let sockopt = child(child(primary, "streamSettings"), "sockopt");
        let previous = sockopt.s("dialerProxy");
        sockopt.insert("dialerProxy".into(), helper_tag.clone().into());
        let mut helper = obj! {"tag" => helper_tag, "protocol" => "freedom", "settings" => options};
        if !previous.is_empty() { helper.insert("streamSettings".into(), json!({"sockopt": {"dialerProxy": previous}})); }
        outbounds.push(helper.into());
    }
    let primary = outbounds[index].as_object_mut().unwrap();
    let vision = primary.o("settings").map(|st| st.objects("vnext").iter().flat_map(|v| v.objects("users")).any(|u| u.s("flow").starts_with("xtls-rprx-vision"))).unwrap_or(false);
    if mux && !vision && ["vless", "vmess", "trojan", "shadowsocks"].contains(&protocol.as_str()) {
        let concurrency = s.i("muxConnections", 4); if !(1..=1024).contains(&concurrency) { return Err(crate::tr!("Мультиплексор: от 1 до 1024 потоков.", "Multiplexer: 1 to 1024 streams.").into()); }
        let m = child(primary, "mux"); m.insert("enabled".into(), true.into()); m.insert("concurrency".into(), concurrency.into());
    }
    Ok(())
}
pub fn fragment(s: &Obj) -> Result<Obj, String> {
    let mut packets = s.s_or("fragmentPackets", "tlshello").trim().to_ascii_lowercase();
    if packets != "tlshello" && !packets.is_empty() { packets = range(&packets, crate::tr!("Фрагментирование пакетов", "Packet fragmentation"), 1, 100)?; }
    let mut o = obj! {"packets" => packets, "length" => range(&s.s_or("fragmentLength", "50-100"), crate::tr!("Длина фрагмента", "Fragment length"), 1, 16384)?, "interval" => range(&s.s_or("fragmentInterval", "10-20"), crate::tr!("Задержка фрагмента", "Fragment delay"), 0, 1000)?};
    let split = s.s("fragmentMaxSplit"); if !split.trim().is_empty() { o.insert("maxSplit".into(), range(&split, crate::tr!("Максимальное разделение", "Maximum split"), 1, 10000)?.into()); }
    Ok(o)
}
pub fn noise(s: &Obj) -> Result<Obj, String> {
    let mut kind = s.s_or("noiseType", "rand").trim().to_ascii_lowercase(); let mut packet = s.s_or("noisePacket", "1A,2B,3C").trim().to_string();
    match kind.as_str() {
        "array" => {
            let mut hex = String::new();
            for b in packet.split(',') { let b = b.trim().trim_start_matches("0x").trim_start_matches("0X"); let v = u8::from_str_radix(b, 16).map_err(|_| crate::tr!("Пакет шума: байты в hex через запятую, например 1A,2B,3C.", "Noise packet: hex bytes separated by commas, for example 1A,2B,3C."))?; hex += &format!("{v:02x}"); }
            packet = hex; kind = "hex".into();
        }
        "rand" => packet = range(&s.s_or("noiseRand", "10-20"), crate::tr!("Размер шума", "Noise size"), 1, 8192)?,
        "str" => if packet.is_empty() { return Err(crate::tr!("Пакет шума пуст.", "The noise packet is empty.").into()); },
        "hex" => if packet.is_empty() || packet.len() % 2 != 0 || !packet.chars().all(|c| c.is_ascii_hexdigit()) { return Err(crate::tr!("Пакет hex должен содержать целые байты.", "A hex packet must contain whole bytes.").into()); },
        _ => return Err(crate::tr!("Тип шума: array, rand, str или hex.", "Noise type: array, rand, str or hex.").into()),
    }
    Ok(obj! {"type" => kind, "packet" => packet, "delay" => range(&s.s_or("noiseDelay", "10-16"), crate::tr!("Задержка шума", "Noise delay"), 0, 1000)?})
}
pub fn range(v: &str, name: &str, min: i64, max: i64) -> Result<String, String> {
    let v = v.trim(); let mut parts = v.splitn(2, '-');
    let a: Option<i64> = parts.next().and_then(|x| x.parse().ok()); let b: Option<i64> = match parts.next() { Some(x) => x.parse().ok(), None => a };
    match (a, b) { (Some(a), Some(b)) if a >= min && b <= max && a <= b => Ok(v.to_string()), (Some(_), Some(_)) => Err(crate::trf!("{name}: от {min} до {max}.", "{name}: from {min} to {max}.")), _ => Err(crate::trf!("{name}: число или диапазон, например 10-20.", "{name}: a number or a range, for example 10-20.")) }
}

// ------------------------------------------------------------------ latency probes

/// Adds one server (and the dialers it depends on) behind its own loopback HTTP inlet. Returns the provider DNS, if any.
pub fn append_probe(profile: &Profile, settings: &Obj, source: &Obj, endpoint: &Endpoint, index: usize, port: u16, outbounds: &mut Vec<Value>, inbounds: &mut Vec<Value>, rules: &mut Vec<Value>) -> Result<Option<Obj>, String> {
    let mut root = prepare(profile, settings, source, Some(endpoint), true)?;
    let selected = proxy_tag(&mut root, Some(endpoint))?;
    let mut by_tag: HashMap<String, Obj> = HashMap::new();
    for o in root.objects("outbounds") { let t = o.s("tag"); if t.is_empty() { continue; } if by_tag.insert(t.clone(), o.clone()).is_some() { return Err(crate::trf!("Повторяется outbound tag: {t}", "Duplicate outbound tag: {t}")); } }
    let mut deps = vec![]; collect(&selected, &by_tag, &mut deps, &mut HashSet::new())?;
    let renamed: HashMap<String, String> = deps.iter().enumerate().map(|(i, t)| (t.clone(), format!("wraith-probe-{index}-{i}"))).collect();
    for t in &deps {
        let mut copy = by_tag[t].clone(); copy.insert("tag".into(), renamed[t].clone().into());
        if let Some(so) = copy.get_mut("streamSettings").and_then(|s| s.get_mut("sockopt")).and_then(|s| s.as_object_mut()) {
            let d = so.s("dialerProxy"); if !d.is_empty() { so.insert("dialerProxy".into(), renamed[&d].clone().into()); }
        }
        outbounds.push(copy.into());
    }
    let inbound_tag = format!("wraith-probe-in-{index}");
    inbounds.push(json!({"tag": inbound_tag, "listen": "127.0.0.1", "port": port, "protocol": "http", "settings": {}}));
    rules.push(json!({"type": "field", "inboundTag": [inbound_tag], "outboundTag": renamed[&selected]}));
    Ok(probe_dns(root.o("dns")))
}
fn collect(tag: &str, by_tag: &HashMap<String, Obj>, ordered: &mut Vec<String>, visiting: &mut HashSet<String>) -> Result<(), String> {
    if ordered.iter().any(|t| t == tag) { return Ok(()); }
    if !visiting.insert(tag.to_string()) { return Err(crate::trf!("Циклическая зависимость Xray outbound: {tag}", "Circular Xray outbound dependency: {tag}")); }
    let outbound = by_tag.get(tag).ok_or(crate::trf!("Не найдена зависимость Xray outbound: {tag}", "Xray outbound dependency not found: {tag}"))?;
    let dep = outbound.o("streamSettings").and_then(|s| s.o("sockopt")).map(|s| s.s("dialerProxy")).unwrap_or_default();
    if !dep.is_empty() { collect(&dep, by_tag, ordered, visiting)?; }
    visiting.remove(tag); ordered.push(tag.to_string()); Ok(())
}
pub fn assemble_probe(inbounds: Vec<Value>, outbounds: Vec<Value>, rules: Vec<Value>, dns: Option<Obj>) -> Obj {
    // Xray's own DNS queries and anything unmatched leave directly, never through a random probed server.
    let mut all = vec![json!({"tag": "wraith-probe-direct", "protocol": "freedom"})]; all.extend(outbounds);
    let mut runtime = obj! {"log" => json!({"loglevel": "error"}), "inbounds" => inbounds, "outbounds" => all, "routing" => json!({"domainStrategy": "AsIs", "rules": rules})};
    if let Some(d) = dns { runtime.insert("dns".into(), d.into()); }
    runtime
}
/// Only plain resolver addresses from a provider DNS section; tags, fakedns and geosite lists made Xray refuse to start.
pub fn probe_dns(dns: Option<&Obj>) -> Option<Obj> {
    let dns = dns?;
    let mut servers = vec![];
    for item in dns.a("servers").cloned().unwrap_or_default() {
        let address = match &item { Value::Object(o) => o.s("address"), other => other.as_str().unwrap_or("").to_string() };
        let address = address.trim();
        if address.is_empty() || address.eq_ignore_ascii_case("localhost") || address.starts_with("fakedns") || address.contains("+local") { continue; }
        servers.push(address.to_string()); if servers.len() >= 3 { break; }
    }
    if servers.is_empty() { return None; }
    let mut out = obj! {"servers" => servers};
    let strategy = dns.s("queryStrategy"); if !strategy.is_empty() { out.insert("queryStrategy".into(), strategy.into()); }
    if let Some(hosts) = dns.o("hosts") {
        let plain: Obj = hosts.iter().filter(|(k, _)| !k.contains(':')).map(|(k, v)| (k.clone(), v.clone())).collect();
        if !plain.is_empty() { out.insert("hosts".into(), plain.into()); }
    }
    Some(out)
}
fn outbound_tag_hint(tls: &Obj) -> String { let n = tls.s("serverName"); if n.is_empty() { crate::tr!("Сервер", "Server").into() } else { n } }
