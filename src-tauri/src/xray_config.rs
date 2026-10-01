//! Xray profiles are kept intact until a controlled loopback runtime is prepared.
use serde_json::{json, Value};
use std::collections::HashMap;

pub fn with_provider_fragment(line: &str, options: &crate::subscription_options::Options) -> Result<String,String> {
    let parameters=&options.parameters;
    let enabled=|key: &str|parameters.get(key).is_some_and(|v|v=="1"||v.eq_ignore_ascii_case("true"));
    let mut url=url::Url::parse(line).map_err(|_|"Некорректный URI")?;
    if enabled("fragmentation-enable") && !url.query_pairs().any(|(key,_)|key=="fragment" || key=="fragmentPackets" || key=="fragmentLength" || key=="fragmentInterval") {
        let get=|key: &str,default: &str|parameters.get(key).map(String::as_str).unwrap_or(default).to_owned();
        let mut value=format!("{},{},{}",get("fragmentation-length","50-100"),get("fragmentation-interval","10-20"),get("fragmentation-packets","tlshello"));
        if let Some(max)=parameters.get("fragmentation-maxsplit") {value.push(',');value.push_str(max);}
        fragment(&value)?;
        url.query_pairs_mut().append_pair("fragment",&value);
    }
    // A provider noise directive also requires an Xray-backed Trojan profile.
    if enabled("noises-enable") && url.scheme()=="trojan" {url.query_pairs_mut().append_pair("atlas-xray","1");}
    Ok(url.to_string())
}
pub fn apply_provider_noises(node: &mut Value, options: &crate::subscription_options::Options) -> Result<(),String> {
    if node["xray"]["outbounds"].as_array().is_some_and(|outbounds|outbounds.iter().any(|o|o["settings"].get("noises").is_some())) {return Ok(());}
    let p=&options.parameters;
    if !p.get("noises-enable").is_some_and(|v|v=="1"||v.eq_ignore_ascii_case("true")) {return Ok(());}
    let delay=range(p.get("noises-delay").map(String::as_str).unwrap_or("0"),true)?;
    let mut items=Vec::new();
    if let Some(packet)=p.get("noises-packet").filter(|v|!v.is_empty()) {
        let kind=p.get("noises-packet-type").map(String::as_str).unwrap_or("array");
        if !["array","str","hex","base64"].contains(&kind) {return Err("Неизвестный noises-packet-type".into());}
        if kind=="array" {
            let bytes: Vec<u8>=serde_json::from_str(&format!("[{}]",packet.trim().trim_start_matches('[').trim_end_matches(']'))).map_err(|_|"noises-packet: нужен массив байтов")?;
            items.push(json!({"type":kind,"packet":bytes,"delay":delay}));
        } else {
            for packet in packet.split(',') {items.push(json!({"type":kind,"packet":packet,"delay":delay}));}
        }
    }
    if let Some(rand)=p.get("noises-rand").filter(|v|!v.is_empty()) {
        let rand=range(rand,false)?;
        let rand_range=range(p.get("noises-rand-range").map(String::as_str).unwrap_or("0-255"),true)?;
        items.push(json!({"rand":rand,"randRange":rand_range,"delay":delay}));
    }
    if items.is_empty() {return Err("noises-enable включён, но данные шума не заданы".into());}
    let stream=&mut node["xray"]["outbounds"][0]["streamSettings"];
    if !stream["finalmask"].is_object() {stream["finalmask"]=json!({});}
    if !stream["finalmask"]["udp"].is_array() {stream["finalmask"]["udp"]=json!([]);}
    stream["finalmask"]["udp"].as_array_mut().unwrap().push(json!({"type":"noise","settings":{"noise":items}}));
    Ok(())
}

fn range(raw: &str, zero: bool) -> Result<String,String> {
    let parts: Vec<_> = raw.split('-').collect();
    if parts.is_empty() || parts.len()>2 { return Err("Некорректный диапазон Xray".into()); }
    let values: Vec<u32> = parts.iter().map(|s|s.parse::<u32>().map_err(|_|"Некорректный диапазон Xray".to_owned())).collect::<Result<_,_>>()?;
    if (!zero && values[0]==0) || values.iter().any(|v|*v>i32::MAX as u32)
        || (values.len()==2 && values[0]>values[1]) { return Err("Некорректные границы диапазона Xray".into()); }
    Ok(raw.into())
}
pub fn fragment(raw: &str) -> Result<Value,String> {
    let p: Vec<_> = raw.split(',').collect();
    if !(3..=4).contains(&p.len()) { return Err("fragment: нужны length,interval,packets[,maxSplit]".into()); }
    let packets=match p[2] {"tlshello"=>"tlshello".to_owned(),"all"=>String::new(),value=>range(value,false)?};
    let mut v=json!({"length":range(p[0],false)?,"interval":range(p[1],true)?,"packets":packets});
    if p.len()==4 {v["maxSplit"]=json!(range(p[3],false)?);}
    Ok(v)
}
pub fn noises(raw: &str) -> Result<Value,String> {
    let mut noises=Vec::new();
    for entry in raw.split(';') {
        let p: Vec<_>=entry.split(',').collect();
        if !(3..=4).contains(&p.len()) || !["rand","str","base64","hex"].contains(&p[0]) {
            return Err("noises: нужны type,packet,delay[,applyTo]".into());
        }
        if p[1].is_empty() {return Err("noises: пустой packet".into());}
        let mut v=json!({"type":p[0],"packet":p[1],"delay":range(p[2],true)?});
        if p.len()==4 {
            if !["ip","ipv4","ipv6"].contains(&p[3]) {return Err("noises: неверный applyTo".into());}
            v["applyTo"]=json!(p[3]);
        }
        noises.push(v);
    }
    Ok(json!(noises))
}

/// Return None for links which should keep using their existing Mihomo adapter.
pub fn uri(line: &str) -> Result<Option<Value>,String> {
    let u=url::Url::parse(line).map_err(|_|"Некорректный URI")?;
    let mut q: HashMap<String,String>=u.query_pairs().into_owned().collect();
    // INCY share links use three separate fields rather than Happ's tuple.
    // Normalize before adapter selection so Trojan cannot silently lose them.
    if !q.contains_key("fragment") && ["fragmentPackets","fragmentLength","fragmentInterval"].iter().any(|key|q.contains_key(*key)) {
        let value=format!("{},{},{}",q.get("fragmentLength").map(String::as_str).unwrap_or("50-100"),
            q.get("fragmentInterval").map(String::as_str).unwrap_or("10-20"),
            q.get("fragmentPackets").map(String::as_str).unwrap_or("tlshello"));
        fragment(&value)?;
        q.insert("fragment".into(),value);
    }
    let needs_xray=["fragment","noises","atlas-xray","fm","pcs","vcn","ech","peer"].iter().any(|key|q.contains_key(*key))
        || q.get("type").is_some_and(|v|["xhttp","splithttp","httpupgrade","kcp"].contains(&v.as_str()));
    if u.scheme()!="vless" && !(u.scheme()=="trojan" && needs_xray) {return Ok(None);}
    let host=u.host_str().ok_or("В URI отсутствует сервер")?;
    let port=u.port().ok_or("В URI отсутствует порт")?;
    let decode=|s: &str|url::form_urlencoded::parse(format!("v={}",s.replace('+',"%2B")).as_bytes()).next().map(|(_,v)|v.into_owned()).unwrap_or_default();
    let name=u.fragment().map(decode).unwrap_or_else(||format!("{host}:{port}"));
    let get=|key: &str,default: &str|q.get(key).cloned().unwrap_or_else(||default.to_owned());
    let mut stream=json!({});
    let network=get("type","tcp");
    stream["network"]=json!(match network.as_str(){"tcp"=>"raw","splithttp"=>"xhttp",other=>other});
    match network.as_str() {
        "tcp"|"raw" => {
            if get("headerType","none")!="none" {
                if get("headerType","none")!="http" {return Err("Неизвестный TCP headerType".into());}
                stream["rawSettings"]=json!({"header":{"type":"http","request":{"path":[get("path","/")],"headers":{"Host":[get("host",host)]}}}});
            }
        }
        "ws" => stream["wsSettings"]=json!({"path":get("path","/"),"headers":{"Host":get("host",host)}}),
        "grpc" => stream["grpcSettings"]=json!({"serviceName":get("serviceName",""),"multiMode":get("mode","")=="multi","authority":get("authority","")}),
        "httpupgrade" => stream["httpupgradeSettings"]=json!({"path":get("path","/"),"host":get("host",host)}),
        "xhttp"|"splithttp" => {
            let mut settings=json!({"path":get("path","/"),"host":get("host",host),"mode":get("mode","auto")});
            if let Some(extra)=q.get("extra") {
                let extra: Value=serde_json::from_str(extra).map_err(|_|"XHTTP extra: некорректный JSON")?;
                if !extra.is_object() {return Err("XHTTP extra должен быть объектом".into());}
                settings["extra"]=extra;
            }
            stream["xhttpSettings"]=settings;
        }
        "kcp" => {
            stream["kcpSettings"]=json!({});
            let seed=get("seed",""); let header=get("headerType","none");
            let mut masks=vec![if seed.is_empty() {json!({"type":"mkcp-original"})}
                else {json!({"type":"mkcp-aes128gcm","settings":{"password":seed}})}];
            if header!="none" {
                if !["srtp","utp","wechat-video","dtls","wireguard"].contains(&header.as_str()) {return Err("Неизвестный KCP headerType".into());}
                masks.push(json!({"type":format!("header-{}",if header=="wechat-video" {"wechat"} else {&header})}));
            }
            stream["finalmask"]=json!({"udp":masks});
        }
        _ => return Err(format!("Xray: неподдерживаемый transport {network}")),
    }
    let security=get("security",if u.scheme()=="trojan" {"tls"} else {"none"});
    stream["security"]=json!(security);
    match security.as_str() {
        "none"=>{},
        "tls"|"reality"=>{
            let mut tls=json!({"serverName":get("sni",q.get("peer").map(String::as_str).unwrap_or(host)),"fingerprint":get("fp","chrome")});
            if security=="tls" {
                for (query,field) in [("pcs","pinnedPeerCertSha256"),("vcn","verifyPeerCertByName"),("ech","echConfigList")] {
                    if let Some(value)=q.get(query) {tls[field]=json!(value);}
                }
            }
            if let Some(alpn)=q.get("alpn") {tls["alpn"]=json!(alpn.split(',').collect::<Vec<_>>());}
            if security=="reality" {
                tls["publicKey"]=json!(q.get("pbk").ok_or("Reality: отсутствует public key")?);
                tls["shortId"]=json!(get("sid","")); tls["spiderX"]=json!(get("spx",""));
                if let Some(value)=q.get("pqv") {tls["mldsa65Verify"]=json!(value);}
            }
            if security=="tls" && q.get("allowInsecure").is_some_and(|v|v=="1"||v=="true") {tls["allowInsecure"]=json!(true);}
            stream[if security=="tls" {"tlsSettings"} else {"realitySettings"}]=tls;
        }
        _=>return Err("Xray: неизвестный security".into()),
    }
    if let Some(raw)=q.get("fm") {
        let mask: Value=serde_json::from_str(raw).map_err(|_|"Xray fm: некорректный JSON")?;
        if !mask.is_object() {return Err("Xray fm должен быть объектом".into());}
        stream["finalmask"]=mask;
    }
    let credentials=decode(u.username());
    let settings=if u.scheme()=="vless" {
        json!({"vnext":[{"address":host,"port":port,"users":[{"id":credentials,"encryption":get("encryption","none"),"flow":get("flow","")}]}]})
    } else {json!({"servers":[{"address":host,"port":port,"password":credentials}]})};
    let mut outbound=json!({"tag":"proxy","protocol":u.scheme(),"settings":settings,"streamSettings":stream});
    let mut outbounds=Vec::new();
    if q.contains_key("fragment") || q.contains_key("noises") {
        let mut settings=json!({});
        if let Some(raw)=q.get("fragment") {settings["fragment"]=fragment(raw)?;}
        if let Some(raw)=q.get("noises") {settings["noises"]=noises(raw)?;}
        outbound["streamSettings"]["sockopt"]=json!({"dialerProxy":"atlas-transport"});
        outbounds.push(json!({"tag":"atlas-transport","protocol":"freedom","settings":settings}));
    }
    outbounds.insert(0,outbound);
    Ok(Some(json!({"name":name,"type":"xray","server":host,"port":port,"xraySimple":true,"xray": {"outbounds":outbounds}})))
}

/// Prefix every reference as well as the definition; separate URI profiles can
/// then share one Xray process without accidentally selecting another outbound.
pub fn namespace_simple(profile: &Value, prefix: &str) -> Result<Vec<Value>,String> {
    let mut outbounds=profile["outbounds"].as_array().ok_or("Xray: отсутствуют outbounds")?.clone();
    for outbound in &mut outbounds {
        let tag=outbound["tag"].as_str().ok_or("Xray: отсутствует tag")?;
        outbound["tag"]=json!(format!("{prefix}{tag}"));
        if let Some(tag)=outbound["streamSettings"]["sockopt"]["dialerProxy"].as_str() {
            outbound["streamSettings"]["sockopt"]["dialerProxy"]=json!(format!("{prefix}{tag}"));
        }
    }
    Ok(outbounds)
}

/// Provider profiles may define networking, but cannot instruct the elevated
/// service to read arbitrary host files or create uncontrolled listeners.
pub fn controlled_profile(profile: &Value, inbound: Value) -> Result<Value,String> {
    fn check(value: &Value) -> Result<(),String> {
        match value {
            Value::Object(map)=>for (key,value) in map {
                if ["certificateFile","keyFile","secretsLog","masterKeyLog","interface","bindToDevice"].contains(&key.as_str())
                    && !value.is_null() && value.as_str()!=Some("") {
                    return Err(format!("Xray JSON: поле {key} обращается к настройкам или файлам компьютера"));
                }
                check(value)?;
            },
            Value::Array(values)=>for value in values {check(value)?;},
            Value::String(s) if s.starts_with("ext:") || s.starts_with("ext-ip:") || s.starts_with("ext-domain:") => {
                return Err("Xray JSON: внешние файлы правил не разрешены".into());
            },
            _=>{}
        }
        Ok(())
    }
    let mut profile=profile.clone();
    let object=profile.as_object_mut().ok_or("Xray JSON должен быть объектом")?;
    if object.contains_key("reverse") {return Err("Xray reverse-профили не являются клиентскими VPN-профилями".into());}
    for field in ["api","metrics","inbounds","log"] {object.remove(field);}
    check(&profile)?;
    // Existing inbound-tag routing must not silently stop matching the imported
    // profile. Map the source listener tags to Atlas's controlled listener.
    let inbound_tag=inbound["tag"].as_str().ok_or("Xray: отсутствует tag входа")?;
    if let Some(rules)=profile["routing"]["rules"].as_array_mut() {
        for rule in rules {
            if rule.get("inboundTag").is_some() {rule["inboundTag"]=json!([inbound_tag]);}
        }
    }
    for outbound in profile["outbounds"].as_array_mut().ok_or("Xray: отсутствуют outbounds")? {
        if outbound["protocol"]=="wireguard" {outbound["settings"]["noKernelTun"]=json!(true);}
    }
    prepare_observatory_dns(&mut profile)?;
    profile["inbounds"]=json!([inbound]);
    profile["log"]=json!({"loglevel":"warning"});
    Ok(profile)
}

/// Bootstrap observatory without waiting for the balancer it is measuring.
/// Only literal DNS endpoints and their DNS ports bypass the balancer; application
/// traffic to the same address must continue to obey the provider's rules.
fn prepare_observatory_dns(profile: &mut Value) -> Result<(),String> {
    if profile.get("observatory").is_none() && profile.get("burstObservatory").is_none() {return Ok(());}
    if profile.get("stats").is_none() {profile["stats"]=json!({});}
    if !profile["routing"]["balancers"].as_array().is_some_and(|v|!v.is_empty()) {return Ok(());}
    let mut endpoints=std::collections::BTreeSet::new();
    if let Some(servers)=profile["dns"]["servers"].as_array() {
        for server in servers {
            let Some(address)=server.as_str().or_else(||server["address"].as_str()) else {continue;};
            if let Ok(ip)=address.parse::<std::net::IpAddr>() {
                let port=server.get("port").and_then(Value::as_u64).unwrap_or(53);
                if port==0 || port>65535 {return Err("Xray DNS: некорректный порт".into());}
                endpoints.insert((ip.to_string(),port,"tcp,udp"));
            } else if let Ok(url)=url::Url::parse(address) {
                // +local resolvers already bypass Xray routing.
                if url.scheme()!="https" && url.scheme()!="tcp" {continue;}
                let ip=match url.host() {Some(url::Host::Ipv4(ip))=>ip.to_string(),Some(url::Host::Ipv6(ip))=>ip.to_string(),_=>continue};
                endpoints.insert((ip,url.port().unwrap_or(if url.scheme()=="https" {443} else {53}) as u64,"tcp"));
            }
        }
    }
    if endpoints.is_empty() {return Ok(());}
    let outbounds=profile["outbounds"].as_array_mut().ok_or("Xray: отсутствуют outbounds")?;
    let mut tag="atlas-dns-bootstrap".to_owned();
    while outbounds.iter().any(|o|o["tag"].as_str()==Some(&tag)) {tag.push('_');}
    outbounds.push(json!({"tag":tag,"protocol":"freedom","settings":{"domainStrategy":"AsIs"}}));
    if profile["routing"].get("rules").is_none() {profile["routing"]["rules"]=json!([]);}
    let rules=profile["routing"]["rules"].as_array_mut().ok_or("Xray routing.rules должен быть массивом")?;
    for (ip,port,network) in endpoints.into_iter().rev() {
        rules.insert(0,json!({"type":"field","ip":[ip],"port":port.to_string(),"network":network,"outboundTag":tag}));
    }
    Ok(())
}

pub fn import_json(text: &str) -> Result<Option<Vec<Value>>,String> {
    let Ok(value)=serde_json::from_str::<Value>(text) else {return Ok(None);};
    let profiles=if value.is_array() {value.as_array().unwrap().clone()} else {vec![value]};
    if !profiles.iter().any(|v|v.get("outbounds").is_some()) {return Ok(None);}
    let mut nodes=Vec::new();
    for (index,profile) in profiles.into_iter().enumerate() {
        let outbounds=profile["outbounds"].as_array().filter(|v|!v.is_empty()).ok_or("Xray JSON: отсутствуют outbounds")?;
        let endpoint=outbounds.iter().find_map(|o|o["settings"]["vnext"].as_array().or_else(||o["settings"]["servers"].as_array()).and_then(|a|a.first()));
        let server=endpoint.and_then(|e|e["address"].as_str()).unwrap_or("xray-profile").to_owned();
        let port=endpoint.and_then(|e|e["port"].as_u64()).unwrap_or(443);
        let name=profile["remarks"].as_str().or_else(||profile["name"].as_str()).map(str::to_owned).unwrap_or_else(||format!("Xray {}",index+1));
        nodes.push(json!({"name":name,"type":"xray","server":server,"port":port,"xray":profile}));
    }
    Ok(Some(nodes))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn incy_security_and_fragment_fields_survive_import() {
        let link="trojan://secret@example.com:443?peer=tls.example&fragmentPackets=all&fragmentLength=10-30&fragmentInterval=0-1&pcs=pin&vcn=tls.example&ech=config&fm=%7B%22tcp%22%3A%5B%5D%7D";
        let (_,options)=crate::subscription_options::extract("#fragmentation-enable: 1",&Default::default(),&Default::default(),"https://example.com/sub").unwrap();
        let normalized=with_provider_fragment(link,&options).unwrap();
        let node=uri(&normalized).unwrap().unwrap();
        let stream=&node["xray"]["outbounds"][0]["streamSettings"];
        assert_eq!(stream["tlsSettings"]["serverName"],"tls.example");
        assert_eq!(stream["tlsSettings"]["pinnedPeerCertSha256"],"pin");
        assert_eq!(stream["tlsSettings"]["verifyPeerCertByName"],"tls.example");
        assert_eq!(stream["tlsSettings"]["echConfigList"],"config");
        assert_eq!(stream["finalmask"],json!({"tcp":[]}));
        assert_eq!(node["xray"]["outbounds"][1]["settings"]["fragment"],json!({"packets":"","length":"10-30","interval":"0-1"}));
        assert!(uri("vless://id@example.com:443?fm=[]").is_err());
        assert!(uri("trojan://secret@example.com:443?fragmentLength=30-10").is_err());
        let reality=uri("vless://id@example.com:443?security=reality&pbk=key&pqv=verify").unwrap().unwrap();
        assert_eq!(reality["xray"]["outbounds"][0]["streamSettings"]["realitySettings"]["mldsa65Verify"],"verify");
    }
    #[test]
    fn observatory_dns_bootstrap_is_narrow_and_preserves_provider_rules() {
        let original=json!({"outbounds":[{"tag":"atlas-dns-bootstrap","protocol":"blackhole"}],
            "dns":{"servers":["1.1.1.1",{"address":"https://[2606:4700:4700::1111]/dns-query"},"https+local://8.8.8.8/dns-query"]},
            "observatory":{"subjectSelector":["proxy"]},
            "routing":{"balancers":[{"tag":"balance","selector":["proxy"]}],"rules":[{"type":"field","network":"tcp,udp","balancerTag":"balance"}]}});
        let result=controlled_profile(&original,json!({"tag":"atlas"})).unwrap();
        assert!(result["stats"].is_object());
        let rules=result["routing"]["rules"].as_array().unwrap();
        assert_eq!(rules.len(),3);
        assert_eq!(rules[0]["port"],"53");
        assert_eq!(rules[1]["port"],"443");
        assert_eq!(rules[0]["outboundTag"],"atlas-dns-bootstrap_");
        assert_eq!(rules[2],original["routing"]["rules"][0]);
        assert_eq!(result["routing"]["balancers"],original["routing"]["balancers"]);
        let mut no_balancer=original.clone();no_balancer["routing"]["balancers"]=json!([]);
        assert_eq!(controlled_profile(&no_balancer,json!({"tag":"atlas"})).unwrap()["routing"],no_balancer["routing"]);
    }
    #[test]
    fn fragmentation_is_attached_to_the_transport_dialer() {
        let n=uri("vless://00000000-0000-0000-0000-000000000001@a.example:443?security=tls&type=xhttp&fragment=10-20,0-1,tlshello,100&noises=rand,10-20,0-1,ipv4#A").unwrap().unwrap();
        assert_eq!(n["xray"]["outbounds"][0]["streamSettings"]["sockopt"]["dialerProxy"],"atlas-transport");
        assert_eq!(n["xray"]["outbounds"][1]["settings"]["fragment"]["maxSplit"],"100");
        assert_eq!(n["xray"]["outbounds"][1]["settings"]["noises"][0]["applyTo"],"ipv4");
    }
    #[test]
    fn malformed_ranges_are_rejected() {
        for v in ["0,1,tlshello","20-10,1,tlshello","1,-1,tlshello","1,1,0"] {assert!(fragment(v).is_err());}
    }
    #[test]
    fn full_json_retains_dns_routing_and_all_outbounds() {
        let original=json!({"remarks":"Profile","dns":{"servers":["1.1.1.1"]},"routing":{"rules":[{"type":"field","domain":["a.example"],"outboundTag":"direct"}]},"outbounds":[{"tag":"direct","protocol":"freedom"},{"tag":"block","protocol":"blackhole"}]});
        let nodes=import_json(&original.to_string()).unwrap().unwrap();
        assert_eq!(nodes[0]["xray"],original);
    }
    #[test]
    fn merged_profiles_do_not_share_transport_tags() {
        let node=uri("vless://id@a.example:443?fragment=10,1,tlshello").unwrap().unwrap();
        let a=namespace_simple(&node["xray"],"a/").unwrap();
        let b=namespace_simple(&node["xray"],"b/").unwrap();
        assert_eq!(a[0]["streamSettings"]["sockopt"]["dialerProxy"],"a/atlas-transport");
        assert_eq!(b[1]["tag"],"b/atlas-transport");
    }
    #[test]
    fn imported_profile_cannot_open_public_listeners_or_log_to_host_files() {
        let profile=json!({"inbounds":[{"listen":"0.0.0.0","port":1080}],"log":{"error":"C:/private.txt"},"outbounds":[{"protocol":"freedom"}]});
        let controlled=controlled_profile(&profile,json!({"tag":"atlas","listen":"127.0.0.1","port":12345})).unwrap();
        assert_eq!(controlled["inbounds"][0]["listen"],"127.0.0.1");
        assert!(controlled["log"].get("error").is_none());
        let mut invalid=profile;
        invalid["outbounds"][0]["streamSettings"]=json!({"tlsSettings":{"certificates":[{"keyFile":"C:/secret"}]}});
        assert!(controlled_profile(&invalid,json!({"tag":"atlas"})).is_err());
    }
    #[test]
    fn official_xray_accepts_generated_transport_configs() {
        use std::os::windows::process::CommandExt;
        let binary=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/Atlas.Xray.exe");
        assert!(binary.is_file(),"Pinned Xray binary is required for compatibility tests");
        for transport in ["tcp","ws","grpc","httpupgrade","xhttp","kcp"] {
            let uri=format!("vless://00000000-0000-0000-0000-000000000001@example.com:443?security=none&type={transport}&fragment=10-20,0-1,tlshello&noises=rand,10-20,0-1,ipv4");
            let node=super::uri(&uri).unwrap().unwrap();
            let profile=controlled_profile(&node["xray"],json!({"tag":"atlas","listen":"127.0.0.1","port":19001,"protocol":"socks","settings":{"auth":"password","accounts":[{"user":"test","pass":"test"}],"udp":true}})).unwrap();
            let path=std::env::temp_dir().join(format!("atlas-xray-test-{}.json",uuid::Uuid::new_v4()));
            std::fs::write(&path,serde_json::to_vec(&profile).unwrap()).unwrap();
            let result=std::process::Command::new(&binary).creation_flags(0x08000000).args(["run","-test","-config"]).arg(&path).output().unwrap();
            let _=std::fs::remove_file(path);
            assert!(result.status.success(),"{transport}: {} {}",String::from_utf8_lossy(&result.stdout),String::from_utf8_lossy(&result.stderr));
        }
    }
    #[test]
    #[cfg(windows)]
    fn pinned_xray_accepts_incy_fragments_and_observatory_bootstrap() {
        use std::os::windows::process::CommandExt;
        let binary=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/Atlas.Xray.exe");
        let inbound=json!({"tag":"atlas","listen":"127.0.0.1","port":19001,"protocol":"socks","settings":{}});
        let mut profiles=Vec::new();
        for packets in ["all","tlshello","1-3"] {
            let link=format!("trojan://secret@example.com:443?peer=tls.example&fragmentPackets={packets}&fragmentLength=10-30&fragmentInterval=0-1&vcn=tls.example&fm=%7B%22tcp%22%3A%5B%5D%7D");
            profiles.push(uri(&link).unwrap().unwrap()["xray"].clone());
        }
        profiles.push(json!({"outbounds":[{"tag":"proxy","protocol":"freedom"}],
            "dns":{"servers":["1.1.1.1", "https://[2606:4700:4700::1111]/dns-query"]},
            "observatory":{"subjectSelector":["proxy"],"probeURL":"https://example.com","probeInterval":"30s"},
            "routing":{"balancers":[{"tag":"balance","selector":["proxy"],"strategy":{"type":"leastPing"}}],
                "rules":[{"type":"field","network":"tcp,udp","balancerTag":"balance"}]}}));
        for profile in profiles {
            let config=controlled_profile(&profile,inbound.clone()).unwrap();
            let path=std::env::temp_dir().join(format!("atlas-incy-test-{}.json",uuid::Uuid::new_v4()));
            std::fs::write(&path,serde_json::to_vec(&config).unwrap()).unwrap();
            let result=std::process::Command::new(&binary).creation_flags(0x08000000).args(["run","-test","-config"]).arg(&path).output().unwrap();
            let _=std::fs::remove_file(path);
            assert!(result.status.success(),"{} {}",String::from_utf8_lossy(&result.stdout),String::from_utf8_lossy(&result.stderr));
        }
    }
    #[test]
    fn provider_fragment_and_noise_parameters_reach_xray() {
        let (_,options)=crate::subscription_options::extract("#fragmentation-enable: 1\n#fragmentation-length: 12-24\n#noises-enable: true\n#noises-packet-type: base64\n#noises-packet: YWJj\n#noises-rand: 1-10",&Default::default(),&Default::default(),"https://example.com/sub").unwrap();
        let link=with_provider_fragment("vless://id@example.com:443?security=tls",&options).unwrap();
        let mut node=uri(&link).unwrap().unwrap();apply_provider_noises(&mut node,&options).unwrap();
        assert_eq!(node["xray"]["outbounds"][1]["settings"]["fragment"]["length"],"12-24");
        let items=&node["xray"]["outbounds"][0]["streamSettings"]["finalmask"]["udp"][0]["settings"]["noise"];
        assert_eq!(items[0]["packet"],"YWJj");assert_eq!(items[1]["rand"],"1-10");
        let explicit=with_provider_fragment("vless://id@example.com:443?fragment=5,1,tlshello",&options).unwrap();
        assert_eq!(uri(&explicit).unwrap().unwrap()["xray"]["outbounds"][1]["settings"]["fragment"]["length"],"5");
    }
}
