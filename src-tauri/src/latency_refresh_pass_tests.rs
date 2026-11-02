use super::*;
use crate::core::ApiClient;
use std::{io::{Read,Write},net::TcpListener,sync::atomic::AtomicUsize};

#[test]
fn refreshes_do_not_wait_for_the_unfinished_tail_or_hide_fresh_values() {
    let now=Instant::now();let wall=crate::probe_state::now_ms();let mut c=Catalog::default();
    c.reconcile((0..96).map(|i|(format!("n{i:03}"),format!("id-{i}"))).collect(),"fixture".into(),now);
    let first=c.take_pass(now,32,true);assert_eq!(first.len(),32);assert!(first.iter().all(|t|t.quick));
    for t in &first {assert!(c.accept(t,&json!({"status":"testing","retryFull":true}),now,wall));}
    c.batch_running=false;
    let second=c.take_pass(now,32,true);
    assert!(second.iter().all(|t|t.quick && !first.iter().any(|old|old.name==t.name)),"Untested nodes precede long retries");
    for t in &second {c.accept(t,&json!({"status":"ok","delay":51,"measuredAt":wall}),now,wall);}
    c.batch_running=false;
    // There are both untouched nodes and long retries left. Refresh due healthy
    // nodes before their ninety-second evidence expires.
    let refresh=c.take_pass(now+REFRESH_AFTER,32,true);
    assert!(refresh.iter().all(|t|second.iter().any(|old|old.name==t.name)));
    let value=&c.view(now+REFRESH_AFTER)["results"][&refresh[0].name];
    assert_eq!(value["status"],"ok");assert_eq!(value["delay"],51);assert_eq!(value["measuredAt"],wall);
}

#[test]
fn explicit_request_and_favorites_precede_the_catalogue() {
    let now=Instant::now();let mut c=Catalog::default();
    c.reconcile((0..100).map(|i|(format!("n{i:03}"),format!("id-{i}"))).collect(),"fixture".into(),now);
    c.priority.insert("n099".into());c.request(Some("n098"),true,now);
    let work=c.take_pass(now,32,true);
    assert!(work.iter().take(2).any(|t|t.name=="n099"));assert!(work.iter().take(2).any(|t|t.name=="n098"));
}

#[test]
fn compare_bounded_policies_on_silent_and_responding_nodes() {
    let listener=TcpListener::bind("127.0.0.1:0").unwrap();let port=listener.local_addr().unwrap().port();listener.set_nonblocking(true).unwrap();
    let stop=Arc::new(AtomicBool::new(false));let stopped=stop.clone();
    let active=Arc::new(AtomicUsize::new(0));let maximum=Arc::new(AtomicUsize::new(0));let peak=maximum.clone();
    let server=std::thread::spawn(move || {
        let mut handlers=Vec::new();
        while !stopped.load(Ordering::SeqCst) {
            if let Ok((mut socket,_))=listener.accept() {
                let active=active.clone();let maximum=maximum.clone();
                handlers.push(std::thread::spawn(move || {
                    socket.set_nonblocking(false).unwrap();socket.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
                    let mut bytes=Vec::new();let mut chunk=[0;4096];
                    while !bytes.windows(4).any(|w|w==b"\r\n\r\n") {let n=socket.read(&mut chunk).unwrap_or(0);if n==0 {return;}bytes.extend_from_slice(&chunk[..n]);}
                    let request=String::from_utf8_lossy(&bytes);let path=request.split_whitespace().nth(1).unwrap();let url=url::Url::parse(&format!("http://fixture{path}")).unwrap();
                    let delay=url.path().ends_with("/delay");let timeout=url.query_pairs().find(|(k,_)|k=="timeout").unwrap_or_default().1.parse::<u64>().unwrap_or(200);
                    let index=url.path().strip_prefix("/proxies/n").and_then(|p|p.strip_suffix("/delay")).and_then(|p|p.parse::<usize>().ok()).unwrap_or(999);
                    // Eight slow but healthy nodes exceed discovery's deadline
                    // and must still succeed during the full retry.
                    let slow=delay && index%12==4;
                    let blocked=delay && (index%12<4 || (slow && timeout<80));
                    if delay {let count=active.fetch_add(1,Ordering::SeqCst)+1;maximum.fetch_max(count,Ordering::SeqCst);std::thread::sleep(Duration::from_millis(if blocked {timeout}else if slow {80}else{3}));}
                    let body=if delay {json!({"delay":51})}else {
                        let proxies:serde_json::Map<_,_>=(0..96).map(|i|(format!("n{i:03}"),json!({"type":"Socks5","alive":false,"extra":{format!("{}?atlas_probe=1",latency::DISPLAY_URL):{"alive":true}}}))).collect();
                        json!({"proxies":proxies})
                    }.to_string();
                    if delay {active.fetch_sub(1,Ordering::SeqCst);}
                    let _=write!(socket,"HTTP/1.1 {} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",if blocked {504}else{200},body.len(),body);
                }));
            }else{std::thread::sleep(Duration::from_millis(1));}
        }
        for worker in handlers {worker.join().unwrap();}
    });
    let mut settings=crate::model::Settings::default();settings.subscriptions.push(crate::model::Subscription {id:"fixture".into(),name:"fixture".into(),source:Default::default(),options:Default::default(),masked_url:String::new(),updated_at:0,error:None,
        servers:(0..96).map(|i|json!({"name":format!("n{i:03}"),"type":"socks5","server":"127.0.0.1","port":20000+i})).collect()});
    let names:Vec<_>=(0..96).map(|i|format!("n{i:03}")).collect();let mut measurements=Vec::new();
    for workers in [8,16,32] {
        peak.store(0,Ordering::SeqCst);let client=ApiClient::loopback_fixture(port).with_probe_settings(&settings);let start=Instant::now();let first=Mutex::new(None);
        let value=if workers==8 {
            latency::fixture_full(&client,&names,200,8,&|_,v|{if v["status"]=="ok" {first.lock().unwrap().get_or_insert(start.elapsed().as_millis());}})
        } else {
            let mut catalog=Catalog::default();catalog.reconcile(names.iter().map(|n|(n.clone(),n.clone())).collect(),"fixture".into(),Instant::now());let shared=Mutex::new(catalog);
            loop {
                let tickets=shared.lock().unwrap().take_pass(Instant::now(),workers,true);if tickets.is_empty(){break;}
                let names:Vec<_>=tickets.iter().map(|t|t.name.clone()).collect();
                latency::fixture_pass(client.clone(),&names,if tickets[0].quick {40}else{200},tickets[0].quick,&|name,v| {
                    let t=tickets.iter().find(|t|t.name==name).unwrap();
                    assert!(!(t.quick && v["status"]=="unreachable"),"Short deadline is not a final timeout");
                    shared.lock().unwrap().accept(t,v,Instant::now(),crate::probe_state::now_ms());
                    if v["status"]=="ok" {first.lock().unwrap().get_or_insert(start.elapsed().as_millis());}
                }).unwrap();shared.lock().unwrap().batch_running=false;
            }
            shared.into_inner().unwrap().view(Instant::now())["results"].clone()
        };
        let ok=value.as_object().unwrap().values().filter(|v|v["status"]=="ok").count();let failed=value.as_object().unwrap().values().filter(|v|v["status"]=="unreachable").count();
        assert_eq!((ok,failed),(64,32));assert!(peak.load(Ordering::SeqCst)<=workers);
        measurements.push(json!({"workers":workers,"phased":workers!=8,"fullMs":start.elapsed().as_millis(),"firstSuccessMs":first.into_inner().unwrap(),"maxConcurrentDelay":peak.load(Ordering::SeqCst),"healthy":ok,"timeout":failed}));
    }
    stop.store(true,Ordering::SeqCst);server.join().unwrap();
    let path=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().join("artifacts/fast-ping-benchmark.json");
    // Clean CI checkouts do not contain the ignored artifacts directory.
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path,serde_json::to_vec_pretty(&measurements).unwrap()).unwrap();
    println!("PING_BENCH {}",json!(measurements));
    assert!(measurements[2]["firstSuccessMs"].as_u64().unwrap()<measurements[0]["firstSuccessMs"].as_u64().unwrap()/2);
    assert!(measurements[2]["fullMs"].as_u64().unwrap()<measurements[0]["fullMs"].as_u64().unwrap());
}
