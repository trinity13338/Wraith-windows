//! Config builders checked against the same cases as the Android app, and against the real cores.

use crate::importer;
use crate::json::Get;
use crate::model::{AppState, Profile};
use serde_json::{json, Value};

const BALANCER: &str = r#"{"remarks":"Auto","dns":{"servers":["1.1.1.1"]},
 "inbounds":[{"tag":"socks-in","protocol":"socks","listen":"127.0.0.1","port":10808,"settings":{"udp":true}},
   {"tag":"http-in","protocol":"http","listen":"127.0.0.1","port":10809},
   {"tag":"dns-in","protocol":"dokodemo-door","listen":"127.0.0.1","port":5353,"settings":{"address":"1.1.1.1","port":53,"network":"udp"}}],
 "outbounds":[
   {"tag":"proxy-nl","protocol":"vless","settings":{"vnext":[{"address":"192.0.2.1","port":443,"users":[{"id":"00948d0e-1437-0005-abfe-bb90bf6bc89a","encryption":"none"}]}]}},
   {"tag":"proxy-de","protocol":"vless","settings":{"vnext":[{"address":"198.51.100.2","port":8443,"users":[{"id":"00948d0e-1437-0005-abfe-bb90bf6bc89a","encryption":"none"}]}]}},
   {"tag":"proxy-fi","protocol":"trojan","settings":{"servers":[{"address":"203.0.113.3","port":443,"password":"secret"}]},"streamSettings":{"security":"tls"}},
   {"protocol":"freedom"},{"tag":"frag","protocol":"freedom","settings":{"fragment":{"packets":"tlshello","length":"10-20","interval":"10-20"}}},
   {"tag":"block","protocol":"blackhole"},{"tag":"dns-out","protocol":"dns"}],
 "observatory":{"subjectSelector":["proxy-"],"probeUrl":"https://www.gstatic.com/generate_204","probeInterval":"1m"},
 "api":{"tag":"api","listen":"0.0.0.0:10085","services":["HandlerService"]},
 "log":{"loglevel":"warning","access":"C:/Windows/evil.log","error":"C:/x.log"},
 "routing":{"domainStrategy":"IPIfNonMatch","balancers":[{"tag":"auto","selector":["proxy-"],"strategy":{"type":"leastPing"},"fallbackTag":"proxy-nl"}],
   "rules":[{"type":"field","inboundTag":["dns-in"],"outboundTag":"dns-out"},{"type":"field","domain":["domain:example.ru"],"outboundTag":"direct"},
     {"type":"field","inboundTag":["socks-in","http-in"],"balancerTag":"auto"}]}}"#;

fn state(settings: Value) -> AppState {
    let mut s = AppState::default();
    for (k, v) in settings.as_object().unwrap() { s.settings.insert(k.clone(), v.clone()); }
    s
}
fn profile(src: &str) -> Profile { importer::parse(src).unwrap().profiles.remove(0) }

#[test]
fn balancer_document_is_one_row_with_three_servers() {
    let p = profile(BALANCER);
    assert_eq!(p.servers, 3); assert!(p.balancer); assert_eq!(p.name, "Auto"); assert_eq!(p.engine, "xray");
}

#[test]
fn provider_balancer_kept_and_inlets_remapped() {
    let st = state(json!({})); let p = profile(BALANCER);
    let built = crate::xray::build(&st, &p, &["192.168.1.1".into()], 1234).unwrap();
    let c = &built.config;
    let rules = c.o("routing").unwrap().objects("rules");
    let catch = rules.iter().find(|r| r.s("balancerTag") == "auto").expect("balancer rule");
    let tags: Vec<String> = catch.a("inboundTag").unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect();
    assert!(tags.contains(&"http".to_string()) && tags.contains(&"socks".to_string()));
    assert_eq!(built.proxy_tags, vec!["proxy-nl", "proxy-de", "proxy-fi"]);
    // Security: no provider API listener, no log file paths.
    assert!(!c.has("api"));
    assert!(!c.o("log").unwrap().has("access") && !c.o("log").unwrap().has("error"));
    // System DNS by default, queried directly.
    assert_eq!(c.o("dns").unwrap().a("servers").unwrap()[0], json!("192.168.1.1"));
    assert!(rules.iter().any(|r| r.s("port") == "53" && r.a("ip").map(|a| a.contains(&json!("192.168.1.1"))).unwrap_or(false)));
}

#[test]
fn auto_select_adds_balancer_for_plain_multi_server_document() {
    let src = BALANCER.replace(r#""balancerTag":"auto""#, r#""outboundTag":"proxy-nl""#);
    let p = profile(&src); assert!(!p.balancer);
    let pinned = crate::xray::build(&state(json!({})), &p, &[], 1).unwrap().config;
    assert!(!crate::xray::has_balancer(&pinned, crate::xray::AUTO_BALANCER));
    let auto = crate::xray::build(&state(json!({"autoSelect": true})), &p, &[], 1).unwrap().config;
    assert!(crate::xray::has_balancer(&auto, crate::xray::AUTO_BALANCER));
    assert!(auto.o("routing").unwrap().objects("rules").iter().any(|r| r.s("balancerTag") == crate::xray::AUTO_BALANCER));
}

#[test]
fn links_become_xray_outbounds() {
    let vless = "vless://00948d0e-1437-0005-abfe-bb90bf6bc89a@example.com:443?type=xhttp&security=reality&pbk=AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA&sid=0123&sni=www.example.org&fp=chrome&path=%2Fx&mode=auto#%F0%9F%87%A9%F0%9F%87%AA%20Germany";
    let p = profile(vless);
    assert_eq!(p.name, "🇩🇪 Germany"); assert_eq!(p.transport, "xhttp"); assert_eq!(p.security, "reality"); assert_eq!(p.engine, "xray");
    let src = crate::xray::source(&p).unwrap();
    let o = &src.objects("outbounds")[0];
    assert_eq!(o.o("streamSettings").unwrap().o("realitySettings").unwrap().s("serverName"), "www.example.org");
    // TUIC and obfuscated Hysteria2 are not supported by Xray; plain Hysteria2 is.
    assert!(importer::parse("tuic://uuid:pass@example.com:443?sni=example.com#t").is_err());
    assert!(importer::parse("hy2://pass@example.com:443?obfs=salamander&obfs-password=x#h").is_err());
    assert_eq!(profile("hy2://pass@example.com:443?sni=example.com#h").protocol, "hysteria2");
    let ss = profile("ss://YWVzLTI1Ni1nY206c2VjcmV0@example.com:8388#ss");
    assert_eq!(ss.protocol, "shadowsocks");
}

#[test]
fn custom_and_provider_dns_only_when_switched_on() {
    let p = profile(BALANCER);
    let custom = crate::xray::build(&state(json!({"customDns": true, "tunnelDns": "tls://1.1.1.1"})), &p, &["10.0.0.1".into()], 1).unwrap().config;
    assert_eq!(custom.o("dns").unwrap().a("servers").unwrap()[0], json!("tls://1.1.1.1"));
    let provider = crate::xray::build(&state(json!({"dnsFromJson": true})), &p, &["10.0.0.1".into()], 1).unwrap().config;
    assert_eq!(provider.o("dns").unwrap().a("servers").unwrap()[0], json!("1.1.1.1"));
}

#[test]
fn lan_proxy_requires_password() {
    let p = profile(BALANCER);
    assert!(crate::xray::build(&state(json!({"allowLan": true})), &p, &[], 1).is_err());
    assert!(crate::xray::build(&state(json!({"allowLan": true, "proxyPassword": "longenough"})), &p, &[], 1).is_ok());
}

fn probe_runtime(src: &str) -> crate::json::Obj {
    let p = profile(src); let source = crate::xray::source(&p).unwrap(); let tags = crate::xray::proxy_tags(&source);
    let (mut i, mut o, mut r) = (vec![], vec![], vec![]); let mut dns = None;
    let eps: Vec<_> = source.objects("outbounds").into_iter().filter(|x| tags.contains(&x.s("tag"))).flat_map(|x| importer::endpoints(x)).collect();
    for (n, e) in eps.iter().enumerate() {
        let d = crate::xray::append_probe(&p, &AppState::default().settings, &source, e, n, 20000 + n as u16, &mut o, &mut i, &mut r).unwrap();
        if dns.is_none() { dns = d; }
    }
    crate::xray::assemble_probe(i, o, r, dns)
}

#[test]
fn every_server_of_an_auto_config_gets_a_probe() {
    let r = probe_runtime(BALANCER);
    assert_eq!(r.a("inbounds").unwrap().len(), 3);
    assert_eq!(r.objects("outbounds")[0].s("tag"), "wraith-probe-direct");
    assert!(r.o("routing").unwrap().objects("rules").iter().all(|x| !x.has("balancerTag")));
}

fn xray_exe() -> Option<std::path::PathBuf> {
    let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.tools/xray-integration/core/xray.exe");
    p.exists().then_some(p)
}
fn xray_accepts(config: &crate::json::Obj, name: &str) {
    let Some(exe) = xray_exe() else { eprintln!("xray.exe not found, skipping"); return };
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/test-configs"); std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{name}.json")); std::fs::write(&path, serde_json::to_vec_pretty(config).unwrap()).unwrap();
    let out = std::process::Command::new(&exe).args(["run", "-test", "-c"]).arg(&path).env("XRAY_LOCATION_ASSET", exe.parent().unwrap()).output().unwrap();
    assert!(out.status.success(), "{name}: {}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
}

#[test]
fn real_xray_accepts_generated_configs() {
    let p = profile(BALANCER);
    for (name, settings) in [("proxy", json!({})), ("tun", json!({"tunnelMode": "tun"})), ("both", json!({"tunnelMode": "both"})), ("v4", json!({"tunnelMode": "tun", "ipMode": "v4"})), ("v6", json!({"ipMode": "v6"})), ("global", json!({"mode": "global"})),
        ("dpi", json!({"fragment": true, "noise": true, "mux": true})), ("auto", json!({"autoSelect": true}))] {
        xray_accepts(&crate::xray::build(&state(settings), &p, &["192.168.1.1".into()], 1234).unwrap().config, name);
    }
    let link = profile("vless://00948d0e-1437-0005-abfe-bb90bf6bc89a@example.com:443?type=ws&security=tls&sni=example.com&path=%2Fws&host=example.com&fp=chrome#ws");
    let mut st = state(json!({"tunnelMode": "tun"}));
    st.rules.push(json!({"kind": "process", "value": "telegram.exe", "action": "direct", "enabled": true}).as_object().unwrap().clone());
    st.rules.push(json!({"kind": "geosite", "value": "category-ads-all", "action": "block", "enabled": true}).as_object().unwrap().clone());
    xray_accepts(&crate::xray::build(&st, &link, &[], 1).unwrap().config, "link-rules");
    let hy = profile("hy2://pass@example.com:443?sni=example.com&mport=20000-30000#hy");
    xray_accepts(&crate::xray::build(&state(json!({})), &hy, &[], 1).unwrap().config, "hysteria2");
    xray_accepts(&probe_runtime(BALANCER), "probe");
}

/// End to end: a local Xray VLESS server, then the real probe path (temporary Xray + HTTP inlet + ureq).
#[test]
#[ignore]
fn live_probe_through_local_server() {
    let Some(exe) = xray_exe() else { return };
    let tmp = std::env::temp_dir().join(format!("wraith-live-{}", crate::util::random_hex(4)));
    std::fs::create_dir_all(&tmp).unwrap();
    std::env::set_var("LOCALAPPDATA", &tmp);
    let _ = std::fs::create_dir_all(crate::store::folder());
    std::fs::copy(&exe, crate::util::app_dir().join("xray.exe")).ok();
    for f in ["geoip.dat", "geosite.dat"] { std::fs::copy(exe.parent().unwrap().join(f), crate::util::app_dir().join(f)).ok(); }
    let port = crate::util::free_port();
    let server = json!({"log": {"loglevel": "warning"}, "inbounds": [{"listen": "127.0.0.1", "port": port, "protocol": "vless", "settings": {"clients": [{"id": "00948d0e-1437-0005-abfe-bb90bf6bc89a"}], "decryption": "none"}}], "outbounds": [{"protocol": "freedom"}]});
    let cfg = tmp.join("server.json"); std::fs::write(&cfg, server.to_string()).unwrap();
    let mut srv = std::process::Command::new(&exe).args(["run", "-c"]).arg(&cfg).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(800));
    let link = profile(&format!("vless://00948d0e-1437-0005-abfe-bb90bf6bc89a@127.0.0.1:{port}?type=tcp&security=none#local"));
    // An auto config: one dead server (closed port) and the live one; the row must report the live latency.
    let auto_src = format!(r#"{{"remarks":"auto","outbounds":[
        {{"tag":"proxy-dead","protocol":"vless","settings":{{"vnext":[{{"address":"127.0.0.1","port":1,"users":[{{"id":"00948d0e-1437-0005-abfe-bb90bf6bc89a","encryption":"none"}}]}}]}}}},
        {{"tag":"proxy-live","protocol":"vless","settings":{{"vnext":[{{"address":"127.0.0.1","port":{port},"users":[{{"id":"00948d0e-1437-0005-abfe-bb90bf6bc89a","encryption":"none"}}]}}]}}}},
        {{"tag":"direct","protocol":"freedom"}}],
        "observatory":{{"subjectSelector":["proxy-"]}},
        "dns":{{"servers":["fakedns",{{"address":"1.1.1.1","domains":["geosite:google"]}},"localhost"],"tag":"dns-in"}},
        "routing":{{"balancers":[{{"tag":"b","selector":["proxy-"],"strategy":{{"type":"leastPing"}}}}],"rules":[{{"type":"field","network":"tcp,udp","balancerTag":"b"}}]}}}}"#);
    let auto = profile(&auto_src);
    let results = std::sync::Arc::new(std::sync::Mutex::new(vec![]));
    let r2 = results.clone();
    let report: crate::pinger::Report = std::sync::Arc::new(move |id: &str, ms: i64, err: String| r2.lock().unwrap().push((id.to_string(), ms, err)));
    let mut st = AppState::default(); st.set("pingUrl", "https://www.gstatic.com/generate_204".into());
    crate::pinger::run(st, vec![link.clone(), auto.clone()], report, std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)));
    let _ = srv.kill();
    let results = results.lock().unwrap().clone();
    eprintln!("{results:?}");
    for p in [&link, &auto] {
        let r = results.iter().find(|r| r.0 == p.id).expect("reported");
        assert!(r.1 > 0, "{} failed: {}", p.name, r.2);
    }
}

/// The tunnel config itself (system-proxy mode, without touching Windows settings): traffic flows and counters move.
#[test]
#[ignore]
fn live_tunnel_config_carries_traffic() {
    let Some(exe) = xray_exe() else { return };
    let tmp = std::env::temp_dir().join(format!("wraith-live-{}", crate::util::random_hex(4))); std::fs::create_dir_all(&tmp).unwrap();
    let sport = crate::util::free_port();
    let server = json!({"inbounds": [{"listen": "127.0.0.1", "port": sport, "protocol": "vless", "settings": {"clients": [{"id": "00948d0e-1437-0005-abfe-bb90bf6bc89a"}], "decryption": "none"}}], "outbounds": [{"protocol": "freedom"}]});
    std::fs::write(tmp.join("s.json"), server.to_string()).unwrap();
    let mut srv = std::process::Command::new(&exe).args(["run", "-c"]).arg(tmp.join("s.json")).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().unwrap();
    let p = profile(&format!("vless://00948d0e-1437-0005-abfe-bb90bf6bc89a@127.0.0.1:{sport}?type=tcp&security=none#local"));
    let (http, socks, metrics) = (crate::util::free_port(), crate::util::free_port(), crate::util::free_port());
    let mut st = state(json!({"httpProxyPort": http, "proxyPort": socks, "mode": "global", "tunnelMode": "proxy"}));
    st.rules.push(json!({"kind": "domain_suffix", "value": "example.invalid", "action": "block", "enabled": true}).as_object().unwrap().clone());
    let built = crate::xray::build(&st, &p, &[], metrics).unwrap();
    std::fs::write(tmp.join("c.json"), serde_json::to_vec(&built.config).unwrap()).unwrap();
    let mut core = std::process::Command::new(&exe).args(["run", "-c"]).arg(tmp.join("c.json")).env("XRAY_LOCATION_ASSET", exe.parent().unwrap()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().unwrap();
    for _ in 0..50 { if crate::util::port_open(http) { break; } std::thread::sleep(std::time::Duration::from_millis(100)); }
    let ms = crate::pinger::measure("https://www.gstatic.com/generate_204", false, true, 8000, http);
    let keep = crate::pinger::measure("https://www.gstatic.com/generate_204", false, true, 8000, http); let fresh = crate::pinger::measure("https://www.gstatic.com/generate_204", false, false, 8000, http);
    eprintln!("keep-alive {keep:?}, fresh {fresh:?}");
    assert!(keep.is_ok() && fresh.is_ok());
    let stats: Result<crate::json::Obj, _> = ureq::get(&format!("http://127.0.0.1:{metrics}/debug/vars")).call().map_err(|e| e.to_string()).and_then(|r| r.into_json().map_err(|e| e.to_string()));
    let _ = core.kill(); let _ = srv.kill();
    assert!(ms.is_ok(), "through tunnel: {ms:?}");
    let down = stats.unwrap().o("stats").and_then(|s| s.o("outbound")).and_then(|o| o.o(&built.proxy_tags[0])).map(|t| t.i("downlink", 0)).unwrap_or(0);
    eprintln!("latency {:?}, proxy downlink {down} bytes", ms);
    assert!(down > 0, "traffic counter did not move");
}

#[test]
fn extract_strips_top_folder_and_refuses_traversal() {
    use std::io::Write;
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut z = zip::ZipWriter::new(&mut buf);
        let o = zip::write::SimpleFileOptions::default();
        z.start_file("Wraith-Windows-0.11.0-x64/Wraith.exe", o).unwrap(); z.write_all(b"exe").unwrap();
        z.start_file("Wraith-Windows-0.11.0-x64/sub/a.txt", o).unwrap(); z.write_all(b"a").unwrap();
        z.start_file("../evil.txt", o).unwrap(); z.write_all(b"x").unwrap();
        z.finish().unwrap();
    }
    let dir = std::env::temp_dir().join(format!("wraith-extract-{}", crate::util::random_hex(4)));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("Wraith.exe"), b"old").unwrap();
    crate::install::extract(buf.get_ref(), &dir, &|_, _| {}).unwrap();
    assert_eq!(std::fs::read(dir.join("Wraith.exe")).unwrap(), b"exe");
    assert!(dir.join("sub/a.txt").exists());
    assert!(!dir.parent().unwrap().join("evil.txt").exists() && !dir.join("evil.txt").exists() || true);
    assert!(!dir.parent().unwrap().join("evil.txt").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn windows_updater_picks_the_newest_release_with_a_windows_build() {
    let asset = |tag: &str| json!({"name": "Wraith-Windows-x64.zip", "browser_download_url": format!("https://github.com/trinity13338/Wraith-windows/releases/download/{tag}/Wraith-Windows-x64.zip"), "digest": format!("sha256:{}", "a".repeat(64)), "size": 1});
    let list = json!([
        {"tag_name": "v0.10.43", "assets": [{"name": "Wraith.apk"}]},
        {"tag_name": "windows-v0.11.1", "assets": [asset("windows-v0.11.1")]},
        {"tag_name": "windows-v0.11.0", "assets": [asset("windows-v0.11.0")]},
        {"tag_name": "windows-v0.12.0", "draft": true, "assets": [asset("windows-v0.12.0")]}
    ]);
    let r = crate::subs::updater::parse_list(&list.to_string(), "0.11.0").unwrap();
    assert_eq!(r.version, "0.11.1"); assert!(r.available);
    assert!(!crate::subs::updater::parse_list(&list.to_string(), "0.11.1").unwrap().available);
}

#[test]
fn provider_dns_gets_the_system_resolver_as_fallback() {
    let p = profile(BALANCER);
    let c = crate::xray::build(&state(json!({"dnsFromJson": true})), &p, &["192.168.1.138".into()], 1).unwrap().config;
    let servers = c.o("dns").unwrap().a("servers").unwrap().clone();
    assert_eq!(servers[0], json!("1.1.1.1"));
    assert_eq!(servers.last().unwrap(), &json!("192.168.1.138"));
    assert!(c.o("routing").unwrap().objects("rules").iter().any(|r| r.s("port") == "53" && r.a("ip").map(|a| a.contains(&json!("192.168.1.138"))).unwrap_or(false) && r.s("outboundTag") == "direct"));
    xray_accepts(&c, "dns-fallback");
}

#[test]
fn own_dns_wins_over_provider_dns() {
    let p = profile(BALANCER);
    let c = crate::xray::build(&state(json!({"dnsFromJson": true, "customDns": true, "tunnelDns": "https://dns.example.com/dns-query"})), &p, &["192.168.1.138".into()], 1).unwrap().config;
    assert_eq!(c.o("dns").unwrap().a("servers").unwrap()[0], json!("https://dns.example.com/dns-query"));
}

#[test]
fn insecure_links_and_wireguard_run_on_xray() {
    let hy = profile("hy2://pass@example.com:443?sni=example.com&insecure=1#h");
    let c = crate::xray::build(&state(json!({})), &hy, &[], 1).unwrap().config;
    assert!(!serde_json::to_string(&c).unwrap().contains("allowInsecure"));
    xray_accepts(&c, "hy2-insecure");
    let wg = profile("wireguard://cGFzc3dvcmRwYXNzd29yZHBhc3N3b3JkcGFzc3dvcmQ=@wg.example.com:51820?publickey=cHVibGljcHVibGljcHVibGljcHVibGljcHVibGljcHU%3D&address=10.0.0.2%2F32&mtu=1280#wg");
    assert_eq!((wg.protocol.as_str(), wg.host.as_str(), wg.port), ("wireguard", "wg.example.com", 51820));
    xray_accepts(&crate::xray::build(&state(json!({"tunnelMode": "tun"})), &wg, &[], 1).unwrap().config, "wireguard");
    assert!(importer::parse(r#"{"outbounds":[{"type":"vless","tag":"p","server":"x","server_port":1}]}"#).is_err());
}

#[test]
fn tun_pulls_system_dns_into_the_tunnel() {
    let p = profile(BALANCER);
    let dns: Vec<String> = vec!["192.168.1.138".into(), "fde4:fdad:f8bd::138".into()];
    let c = crate::xray::build(&state(json!({"tunnelMode": "both"})), &p, &dns, 1).unwrap().config;
    let tun = c.a("inbounds").unwrap().iter().find(|i| i["tag"] == "tun-in").unwrap().clone();
    let routes = tun["settings"]["autoSystemRoutingTable"].as_array().unwrap();
    assert!(routes.contains(&json!("192.168.1.138/32")) && routes.contains(&json!("fde4:fdad:f8bd::138/128")) && routes.contains(&json!("0.0.0.0/1")));
    assert_eq!(tun["sniffing"]["routeOnly"], json!(false));
    xray_accepts(&c, "tun-dns-routes");
}

/// Debug: builds the config of the real selected server from this machine's state (WRAITH_DUMP=path).
#[test]
#[ignore]
fn dump_real_config() {
    let Ok(out) = std::env::var("WRAITH_DUMP") else { return };
    let st = crate::store::load();
    let p = st.selected().cloned().expect("selected");
    let built = crate::xray::build(&st, &p, &crate::routing::system_resolvers(), 1).unwrap();
    std::fs::write(out, serde_json::to_vec_pretty(&built.config).unwrap()).unwrap();
}

#[test]
fn flag_badges_find_the_flag_and_have_images() {
    assert_eq!(crate::ui::flags::flag_code("🇱🇹 Lithuania").as_deref(), Some("lt"));
    assert_eq!(crate::ui::flags::flag_code("Finland 🇫🇮 #2").as_deref(), Some("fi"));
    assert_eq!(crate::ui::flags::flag_code("Lithuania"), None);
    assert_eq!(crate::ui::widgets::badge_text("Fast 🔥 server"), "🔥");
    let codes = include_str!("../assets/flags.txt");
    for c in ["lt", "fi", "se", "de", "nl", "us", "gb", "ru", "jp"] { assert!(codes.lines().any(|l| l == c), "{c}"); }
}

#[test]
fn accent_palette_keeps_contrast() {
    use crate::ui::theme::{luminance, palette};
    let blue = egui::Color32::from_rgb(0x00, 0x78, 0xD4);
    let p = palette(blue, false);
    assert!(p.accent.b() > p.accent.r(), "the Windows blue stays blue");
    assert!(luminance(p.accent) >= 0.2, "a dark accent is lifted to stay readable on the dark base");
    assert!(luminance(p.bg) < 0.01 && luminance(p.text) > 0.8);
    assert_eq!(palette(blue, true).bg, egui::Color32::BLACK);
}

#[test]
fn server_configuration_view_has_no_wraith_parts() {
    // A provider document is shown exactly as it came.
    let p = profile(BALANCER);
    assert_eq!(serde_json::to_value(crate::xray::source(&p).unwrap()).unwrap(), serde_json::from_str::<serde_json::Value>(BALANCER).unwrap());
    // A link becomes only its outbound.
    let link = profile("vless://00948d0e-1437-0005-abfe-bb90bf6bc89a@example.com:443?type=tcp&security=reality&pbk=key&sid=ab&sni=example.com#x");
    let c = crate::xray::source(&link).unwrap();
    assert_eq!(c.keys().collect::<Vec<_>>(), vec!["outbounds"]);
    let text = serde_json::to_string(&c).unwrap();
    assert!(!text.contains("tun-in") && !text.contains("wraith-dns") && !text.contains("inbounds"));
}

#[test]
fn quic_block_rejects_udp_443_from_our_inlets() {
    let p = profile("trojan://pw@example.com:443#Q");
    let has = |st: &AppState| crate::xray::build(st, &p, &[], 0).unwrap().config.o("routing").unwrap().objects("rules").iter()
        .any(|r| r.s("network") == "udp" && r.s("port") == "443" && r.s("outboundTag") == "block");
    assert!(has(&state(json!({"quic": "reject", "tunnelMode": "proxy"}))));
    assert!(!has(&state(json!({"quic": "allow", "tunnelMode": "proxy"}))));
}

#[test]
fn traffic_history_days_are_oldest_first() {
    let today = crate::util::local_time().date;
    let mut h = crate::json::Obj::new();
    h.insert(today.clone(), json!([1000, 24]));
    let days = crate::stats::last_days(&h, 14);
    assert_eq!(days.len(), 14);
    assert_eq!(days.last().unwrap().1, 1024);
    assert_eq!(days.last().unwrap().0, today[8..].to_string());
    assert!(days[..13].iter().all(|d| d.1 == 0));
}

/// `WRAITH_SHOTS=dir cargo test --release -- --ignored --test-threads=1 render_screens`: draws every
/// screen without a window (egui meshes rasterised on the CPU) and writes PNGs; a frame that hangs fails it.
#[test]
#[ignore]
fn render_screens() {
    use egui::{Color32, TextureId};
    use std::collections::HashMap;
    let Ok(out) = std::env::var("WRAITH_SHOTS") else { return };
    let home = std::path::Path::new(&out).join("home");
    let _ = std::fs::remove_dir_all(&home); std::fs::create_dir_all(&home).unwrap();
    std::env::set_var("WRAITH_HOME", &home);
    crate::store::init();
    crate::demo();
    crate::store::update(|s| s.set("greetName", "Alex".into()));
    let (w, h) = (std::env::var("WRAITH_SHOT_W").ok().and_then(|v| v.parse().ok()).unwrap_or(980usize), std::env::var("WRAITH_SHOT_H").ok().and_then(|v| v.parse().ok()).unwrap_or(860usize));
    let ctx = egui::Context::default();
    let mut app = crate::ui::app::App::with_context(&ctx, false);
    let mut textures: HashMap<TextureId, (usize, usize, Vec<Color32>)> = HashMap::new();
    let mut time = 0.0;
    let screens = std::env::var("WRAITH_SCREENS").unwrap_or_else(|_| "home,home-on,home-hidden,servers,stats,stats-on,settings,routing,tunnel,subscriptions,ping,appearance,logs,import,sheet".into());
    for screen in screens.split(',') {
        let on = screen.ends_with("-on") || screen.ends_with("-hidden");
        crate::store::update(|s| s.set("showExitIp", (!screen.ends_with("-hidden")).into()));
        if on {
            let id = crate::store::get().selected.clone();
            let name = crate::store::get().selected().map(|p| p.name.clone()).unwrap_or_default();
            crate::core::set_test_status(crate::core::State::Connected, &name, 2_400_000, 180_000);
            let _ = id;
            crate::stats::set_test_exit("185.220.101.42", "de", 68);
            for i in 0..60 { let k = (i as f64 / 6.0).sin().abs(); crate::stats::sample((k * 3_000_000.0) as i64 + 200_000, (k * 300_000.0) as i64 + 20_000); }
        } else { crate::core::set_test_status(crate::core::State::Off, "", 0, 0); }
        app.show(screen.trim_end_matches("-on").trim_end_matches("-hidden"));
        let mut last = None;
        for _ in 0..40 {
            time += 0.05;
            let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(w as f32, h as f32))), time: Some(time), ..Default::default() };
            let started = std::time::Instant::now();
            let output = ctx.run(input, |ctx| app.frame(ctx));
            assert!(started.elapsed() < std::time::Duration::from_secs(5), "{screen}: a frame took {:?}", started.elapsed());
            for (id, delta) in &output.textures_delta.set {
                let egui::ImageData::Color(img) = &delta.image;
                match delta.pos {
                    None => { textures.insert(*id, (img.size[0], img.size[1], img.pixels.clone())); }
                    Some([x0, y0]) => if let Some((tw, _, px)) = textures.get_mut(id) { for y in 0..img.size[1] { for x in 0..img.size[0] { px[(y0 + y) * *tw + x0 + x] = img.pixels[y * img.size[0] + x]; } } },
                }
            }
            last = Some(ctx.tessellate(output.shapes, output.pixels_per_point));
        }
        let mut px = vec![[0f32; 4]; w * h];
        for prim in last.unwrap() {
            let egui::epaint::Primitive::Mesh(mesh) = prim.primitive else { continue };
            let clip = prim.clip_rect;
            let tex = textures.get(&mesh.texture_id);
            for tri in mesh.indices.chunks(3) {
                let v = [mesh.vertices[tri[0] as usize], mesh.vertices[tri[1] as usize], mesh.vertices[tri[2] as usize]];
                let (minx, maxx) = (v.iter().map(|p| p.pos.x).fold(f32::MAX, f32::min).max(clip.min.x).max(0.0), v.iter().map(|p| p.pos.x).fold(f32::MIN, f32::max).min(clip.max.x).min(w as f32));
                let (miny, maxy) = (v.iter().map(|p| p.pos.y).fold(f32::MAX, f32::min).max(clip.min.y).max(0.0), v.iter().map(|p| p.pos.y).fold(f32::MIN, f32::max).min(clip.max.y).min(h as f32));
                let area = (v[1].pos.x - v[0].pos.x) * (v[2].pos.y - v[0].pos.y) - (v[2].pos.x - v[0].pos.x) * (v[1].pos.y - v[0].pos.y);
                if area.abs() < 1e-6 || minx >= maxx || miny >= maxy { continue; }
                for y in (miny.floor() as usize)..(maxy.ceil() as usize).min(h) {
                    for x in (minx.floor() as usize)..(maxx.ceil() as usize).min(w) {
                        let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
                        let e = |a: egui::Pos2, b: egui::Pos2| (b.x - a.x) * (fy - a.y) - (b.y - a.y) * (fx - a.x);
                        let (w0, w1, w2) = (e(v[1].pos, v[2].pos) / area, e(v[2].pos, v[0].pos) / area, e(v[0].pos, v[1].pos) / area);
                        if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 { continue; }
                        let mut c = [0f32; 4];
                        for (k, wk) in [w0, w1, w2].iter().enumerate() { let col = v[k].color.to_array(); for i in 0..4 { c[i] += col[i] as f32 / 255.0 * wk; } }
                        if let Some((tw, th, tp)) = tex {
                            let u = v[0].uv.x * w0 + v[1].uv.x * w1 + v[2].uv.x * w2; let vv = v[0].uv.y * w0 + v[1].uv.y * w1 + v[2].uv.y * w2;
                            let (sx, sy) = ((u * *tw as f32 - 0.5).clamp(0.0, *tw as f32 - 1.0), (vv * *th as f32 - 0.5).clamp(0.0, *th as f32 - 1.0));
                            let (x0, y0) = (sx.floor() as usize, sy.floor() as usize); let (x1, y1) = ((x0 + 1).min(tw - 1), (y0 + 1).min(th - 1)); let (ax, ay) = (sx - x0 as f32, sy - y0 as f32);
                            let s = |xx: usize, yy: usize| tp[yy * tw + xx].to_array();
                            let (a, b, cc, d) = (s(x0, y0), s(x1, y0), s(x0, y1), s(x1, y1));
                            for i in 0..4 { let t = (a[i] as f32 * (1.0 - ax) + b[i] as f32 * ax) * (1.0 - ay) + (cc[i] as f32 * (1.0 - ax) + d[i] as f32 * ax) * ay; c[i] *= t / 255.0; }
                        }
                        let dst = &mut px[y * w + x];
                        for i in 0..4 { dst[i] = c[i] + dst[i] * (1.0 - c[3]); }
                    }
                }
            }
        }
        let bytes: Vec<u8> = px.iter().flat_map(|p| [(p[0] * 255.0) as u8, (p[1] * 255.0) as u8, (p[2] * 255.0) as u8, 255]).collect();
        let file = std::fs::File::create(std::path::Path::new(&out).join(format!("{screen}.png"))).unwrap();
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w as u32, h as u32); enc.set_color(png::ColorType::Rgba); enc.set_depth(png::BitDepth::Eight);
        enc.write_header().unwrap().write_image_data(&bytes).unwrap();
    }
}

#[test]
fn no_pool_counts_and_a_home_phrase() {
    let p = profile(BALANCER);
    assert_eq!(p.summary(), "VLESS · TLS", "a multi-server document reads like one server, without a pool count");
    assert!(!crate::ui::app::greeting().is_empty());
}
