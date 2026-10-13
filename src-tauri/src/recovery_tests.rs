use super::*;
use std::{io::{Read,Write}, net::TcpListener, sync::{Arc,Mutex,atomic::{AtomicBool,Ordering}}};

struct Fixture {
    client: ApiClient, settings: Settings, selected: Arc<Mutex<String>>,
    traffic: Arc<AtomicBool>, stop: Arc<AtomicBool>, worker: Option<std::thread::JoinHandle<()>>,
}
impl Fixture {
    fn new(fast_delay: u64, traffic_ok: bool) -> Self {
        let listener=TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let client=ApiClient::loopback_fixture(listener.local_addr().unwrap().port());
        let selected=Arc::new(Mutex::new("slow".to_owned()));
        let traffic=Arc::new(AtomicBool::new(traffic_ok));
        let stop=Arc::new(AtomicBool::new(false));
        let (selection,healthy,done)=(selected.clone(),traffic.clone(),stop.clone());
        let worker=std::thread::spawn(move || {
            while !done.load(Ordering::SeqCst) {
                let Ok((mut socket,_))=listener.accept() else { std::thread::sleep(Duration::from_millis(1));continue; };
                socket.set_nonblocking(false).unwrap();
                socket.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                let mut input=Vec::new(); let mut buf=[0;4096];
                loop {
                    let n=socket.read(&mut buf).unwrap_or(0); if n==0 {break;} input.extend_from_slice(&buf[..n]);
                    let text=String::from_utf8_lossy(&input);
                    if let Some(end)=text.find("\r\n\r\n") {
                        let length=text[..end].lines().find_map(|l|l.to_ascii_lowercase().strip_prefix("content-length: ").and_then(|v|v.parse::<usize>().ok())).unwrap_or(0);
                        if input.len()>=end+4+length {break;}
                    }
                }
                let request=String::from_utf8_lossy(&input);
                let mut code=200;
                let body=if request.starts_with("PUT ") {
                    if let Some(body)=request.split("\r\n\r\n").nth(1) {
                        if let Ok(v)=serde_json::from_str::<Value>(body) { *selection.lock().unwrap()=v["name"].as_str().unwrap().into(); }
                    }
                    json!({})
                } else if request.starts_with("GET http://") {
                    code=if healthy.load(Ordering::SeqCst) {204} else {503}; Value::Null
                } else if request.contains("/delay?") {
                    json!({"delay":if request.contains("/slow/") {200} else {fast_delay}})
                } else if request.starts_with("GET /proxies/AUTO ") {
                    json!({"now":*selection.lock().unwrap()})
                } else {
                    let health=json!({"alive":true,"extra":{crate::latency::DISPLAY_URL:{"alive":true}}});
                    json!({"proxies":{"AUTO":{"now":*selection.lock().unwrap()},"slow":health.clone(),"fast":health}})
                };
                let body=if code==204 {String::new()} else {body.to_string()};
                let _=write!(socket,"HTTP/1.1 {code} Test\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{body}",body.len());
            }
        });
        let mut settings=Settings::default();settings.auto_optimize=true;
        settings.subscriptions.push(crate::model::Subscription {source:Default::default(),options:Default::default(),id:"fixture".into(),name:"fixture".into(),masked_url:String::new(),updated_at:0,error:None,
            servers:vec![json!({"name":"slow","server":"127.0.0.1","port":1}),json!({"name":"fast","server":"127.0.0.1","port":2})]});
        Self{client,settings,selected,traffic,stop,worker:Some(worker)}
    }
    fn drive(&self, recovery: &mut Recovery, phase: Phase) {
        let deadline=Instant::now()+Duration::from_secs(5);
        while recovery.phase!=phase {
            recovery.tick(self.client.clone(),&self.settings);
            assert!(Instant::now()<deadline,"wanted {phase:?}, got {:?}: {:?}",recovery.phase,self.client.logs());
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}
impl Drop for Fixture { fn drop(&mut self) {self.stop.store(true,Ordering::SeqCst);self.worker.take().unwrap().join().unwrap();} }
fn sample(recovery: &mut Recovery, fixture: &Fixture, delay: Option<u64>, alive: bool) {
    let (tx,rx)=mpsc::channel(); tx.send(vec![("slow".into(),delay,alive)]).unwrap();
    recovery.active=Some(Scheduled{rx,generation:recovery.generation});
    recovery.poll_active(&fixture.client,&fixture.settings,Instant::now());
}
#[test] fn threshold_timeout_and_emergency_priority() {
    let f=Fixture::new(60,true);let mut r=Recovery::default();
    sample(&mut r,&f,Some(80),true);assert!(!r.needed);
    sample(&mut r,&f,Some(150),true);assert!(!r.needed);
    sample(&mut r,&f,Some(200),true);assert!(r.needed);
    r.last_switch=Some(Instant::now());r.retry_at=Some(Instant::now()+Duration::from_secs(100));
    sample(&mut r,&f,None,false);assert_eq!(r.slow_samples,0);assert!(r.retry_at.is_none());
    f.drive(&mut r,Phase::ConnectedHealthy);
}
#[test] fn replacement_is_confirmed_by_real_proxy_traffic() {
    let f=Fixture::new(60,true);let mut r=Recovery::default();
    sample(&mut r,&f,Some(200),true);
    f.drive(&mut r,Phase::Verifying);
    assert_ne!(r.phase,Phase::ConnectedHealthy);
    f.drive(&mut r,Phase::ConnectedHealthy);
    assert_eq!(*f.selected.lock().unwrap(),"fast");
    assert!(f.client.logs().unwrap().iter().any(|line|line.contains("RecoverySucceeded")));
}
#[test] fn broken_tunnel_rolls_back_and_refreshes_once() {
    let f=Fixture::new(60,false);let mut r=Recovery::default();
    sample(&mut r,&f,Some(200),true);
    f.drive(&mut r,Phase::RefreshingSubscriptions);
    assert_eq!(*f.selected.lock().unwrap(),"slow");
    assert!(!f.client.logs().unwrap().iter().any(|line|line.contains("RecoverySucceeded")));
    let id=r.operation.clone().unwrap();
    let ticket=r.refresh_ticket.clone().unwrap();
    assert!(r.refresh_finished(&ticket,false,&f.client));
    f.drive(&mut r,Phase::WaitingRetry);
    assert_eq!(f.client.logs().unwrap().iter().filter(|line|line.contains("SubscriptionRefreshStarted")).count(),1);
    assert!(!r.refresh_finished(&ticket,true,&f.client));
    for _ in 0..10 { sample(&mut r,&f,None,false);r.tick(f.client.clone(),&f.settings); }
    assert_eq!(r.operation.as_deref(),Some(id.as_str()));
    assert_eq!(r.phase,Phase::WaitingRetry);
}
#[test] fn refresh_rescan_can_recover_and_manual_selection_cancels_old_operation() {
    let f=Fixture::new(60,false);let mut r=Recovery::default();
    sample(&mut r,&f,None,false);f.drive(&mut r,Phase::RefreshingSubscriptions);
    let ticket=r.refresh_ticket.clone().unwrap();
    r.configuration_changed();
    f.traffic.store(true,Ordering::SeqCst);
    assert!(r.refresh_finished(&ticket,true,&f.client));
    f.drive(&mut r,Phase::ConnectedHealthy);
    r.invalidate();assert!(!r.refresh_finished(&ticket,true,&f.client));
    assert_eq!(r.phase,Phase::Disconnected);
}
#[test] fn no_candidate_above_threshold_can_replace_current_path() {
    let f=Fixture::new(300,true);let mut r=Recovery::default();
    sample(&mut r,&f,Some(200),true);f.drive(&mut r,Phase::RefreshingSubscriptions);
    assert_eq!(*f.selected.lock().unwrap(),"slow");
}
#[test] fn bounded_backoff_and_legacy_settings_defaults() {
    let p=crate::model::RecoveryPolicy::default();
    assert_eq!((1..=7).map(|n|p.retry_delay(n)).collect::<Vec<_>>(),vec![10,20,40,80,160,300,300]);
    let mut legacy=serde_json::to_value(Settings::default()).unwrap();legacy.as_object_mut().unwrap().remove("recoveryPolicy");
    assert_eq!(serde_json::from_value::<Settings>(legacy).unwrap().recovery_policy,p);
    let mut settings=Settings::default();assert_eq!(probe_timeout(&settings),1500);
    settings.auto_search_ping_ms=5000;assert!(probe_timeout(&settings)>5000);
}
#[test] fn later_refresh_uses_new_ticket_and_rejects_late_completion() {
    let f=Fixture::new(300,true);let mut r=Recovery::default();
    let now=Instant::now();let policy=crate::model::RecoveryPolicy::default();
    r.operation=Some("same-incident".into());
    r.exhausted(&f.client,now,&policy);
    let first=r.refresh_ticket.clone().unwrap();
    assert!(r.refresh_finished(&first,false,&f.client));
    r.exhausted(&f.client,now,&policy);assert_eq!(r.phase,Phase::WaitingRetry);
    r.refreshed=false;
    r.exhausted(&f.client,now+Duration::from_secs(policy.refresh_interval_seconds),&policy);
    let second=r.refresh_ticket.clone().unwrap();assert_ne!(first,second);
    assert!(!r.refresh_finished(&first,true,&f.client));
    assert!(r.refresh_finished(&second,true,&f.client));
}
