//! Local, deterministic failure fixtures using the shipped Mihomo and Xray.
use crate::{core::Core, latency::display_probe, model::{Settings, Subscription}};
use serde_json::{json, Value};
use std::{io::{Read, Write}, net::{TcpListener,TcpStream}, sync::{Arc,atomic::{AtomicBool,AtomicUsize,Ordering}}, thread, time::Duration, path::PathBuf};

pub(crate) struct Socks {
    pub port:u16,
    pub mode:Arc<AtomicUsize>, // 0 relay, 1 stalled greeting, 2 stalled traffic
    pub hits:Arc<AtomicUsize>,
    stop:Arc<AtomicBool>,
    worker:Option<thread::JoinHandle<()>>,
}
impl Socks {
    pub fn new() -> Self {
        let listener=TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port=listener.local_addr().unwrap().port();
        let mode=Arc::new(AtomicUsize::new(0));let stop=Arc::new(AtomicBool::new(false));let hits=Arc::new(AtomicUsize::new(0));
        let (m,s,h)=(mode.clone(),stop.clone(),hits.clone());
        let worker=thread::spawn(move || {
            let mut workers=Vec::new();
            while !s.load(Ordering::SeqCst) {
                if let Ok((mut socket,_))=listener.accept() {
                socket.set_nonblocking(false).unwrap();
                    let mode=m.load(Ordering::SeqCst); h.fetch_add(1,Ordering::SeqCst);
                    workers.push(thread::spawn(move || {
                        let _=socket.set_read_timeout(Some(Duration::from_secs(2)));
                        let _=socket.set_write_timeout(Some(Duration::from_secs(2)));
                        if mode==1 {thread::sleep(Duration::from_millis(700));return;}
                        let _=(||->std::io::Result<()> {
                            let mut greeting=[0;2];socket.read_exact(&mut greeting)?;
                            let mut methods=vec![0;greeting[1] as usize];socket.read_exact(&mut methods)?;
                            socket.write_all(&[5,0])?;
                            let mut request=[0;4];socket.read_exact(&mut request)?;
                            let host=match request[3] {
                                1=>{let mut ip=[0;4];socket.read_exact(&mut ip)?;std::net::Ipv4Addr::from(ip).to_string()},
                                3=>{let mut n=[0];socket.read_exact(&mut n)?;let mut host=vec![0;n[0] as usize];socket.read_exact(&mut host)?;String::from_utf8_lossy(&host).into_owned()},
                                _=>return Ok(()),
                            };
                            let mut port=[0;2];socket.read_exact(&mut port)?;
                            if mode==2 {socket.write_all(&[5,0,0,1,127,0,0,1,0,0])?;thread::sleep(Duration::from_millis(700));return Ok(());}
                            // Fixtures may only forward to loopback.
                            if host!="127.0.0.1" && host!="localhost" {return Ok(());}
                            let mut target=TcpStream::connect((host.as_str(),u16::from_be_bytes(port)))?;
                            target.set_read_timeout(Some(Duration::from_secs(2)))?;
                            socket.write_all(&[5,0,0,1,127,0,0,1,0,0])?;
                            let mut incoming=socket.try_clone()?;let mut outgoing=target.try_clone()?;
                            let upload=thread::spawn(move || {let _=std::io::copy(&mut incoming,&mut outgoing);let _=outgoing.shutdown(std::net::Shutdown::Write);});
                            let _=std::io::copy(&mut target,&mut socket);let _=socket.shutdown(std::net::Shutdown::Both);let _=upload.join();
                            Ok(())
                        })();
                    }));
                } else {thread::sleep(Duration::from_millis(5));}
            }
            for worker in workers {let _=worker.join();}
        });
        Self{port,mode,hits,stop,worker:Some(worker)}
    }
    pub fn node(&self,name:&str)->Value {json!({"name":name,"type":"socks5","server":"127.0.0.1","port":self.port})}
}
impl Drop for Socks {fn drop(&mut self){self.stop.store(true,Ordering::SeqCst);let _=self.worker.take().unwrap().join();}}
struct Origin {url:String,status:Arc<AtomicUsize>,hits:Arc<AtomicUsize>,stop:Arc<AtomicBool>,worker:Option<thread::JoinHandle<()>>}
impl Origin {
    fn new()->Self {
        let listener=TcpListener::bind("127.0.0.1:0").unwrap();listener.set_nonblocking(true).unwrap();
        let url=format!("http://127.0.0.1:{}/generate_204",listener.local_addr().unwrap().port());
        let status=Arc::new(AtomicUsize::new(204));let hits=Arc::new(AtomicUsize::new(0));let stop=Arc::new(AtomicBool::new(false));
        let (code,h,s)=(status.clone(),hits.clone(),stop.clone());
        let worker=thread::spawn(move || while !s.load(Ordering::SeqCst) {
            if let Ok((mut socket,_))=listener.accept() {
                socket.set_nonblocking(false).unwrap();
                socket.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                if socket.read(&mut [0;4096]).unwrap_or(0)>0 {
                    h.fetch_add(1,Ordering::SeqCst);thread::sleep(Duration::from_millis(20));
                    let _=write!(socket,"HTTP/1.1 {} Test\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",code.load(Ordering::SeqCst));
                }
            } else {thread::sleep(Duration::from_millis(5));}
        });Self{url,status,hits,stop,worker:Some(worker)}
    }
}
impl Drop for Origin {fn drop(&mut self){self.stop.store(true,Ordering::SeqCst);let _=self.worker.take().unwrap().join();}}
fn settings(nodes:Vec<Value>)->Settings {
    let mut s=Settings::default();s.mode="system".into();
    s.subscriptions.push(Subscription {source:Default::default(),options:Default::default(),id:"test".into(),name:"test".into(),masked_url:String::new(),updated_at:0,error:None,servers:nodes});s
}
struct Fixture {core:Core,directory:PathBuf}
impl Fixture {
    fn new(s:&Settings)->Self {
        let directory=std::env::temp_dir().join(format!("atlas-certified-{}",uuid::Uuid::new_v4()));std::fs::create_dir_all(&directory).unwrap();
        let binary=PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/Atlas.Core.exe");
        let mut core=Core::new(binary,directory.clone());core.use_ephemeral_ports().unwrap();core.start(s).unwrap();Self{core,directory}
    }
}
impl Drop for Fixture {fn drop(&mut self){self.core.stop().unwrap();std::fs::remove_dir_all(&self.directory).unwrap();}}

#[test]
fn real_core_success_handshake_failure_traffic_timeout_bad_status_and_recovery() {
    let origin=Origin::new();let socks=Socks::new();let stable=Socks::new();
    let fixture=Fixture::new(&settings(vec![socks.node("flaky"),stable.node("stable"),json!({"name":"bypass","type":"direct"})]));
    let client=fixture.core.client();
    for mode in [0,1,2,0,2,0] {
        socks.mode.store(mode,Ordering::SeqCst);
        let hits=origin.hits.load(Ordering::SeqCst);
        let result=display_probe(&client,"flaky",&origin.url,350);
        if mode==0 {assert_eq!(result.status,"ok","{result:?}");assert!(result.delay.unwrap()>=20);assert_eq!(result.evidence["udp"],"untested");}
        else {assert_ne!(result.status,"ok","{result:?}");assert_eq!(result.delay,None);assert_eq!(origin.hits.load(Ordering::SeqCst),hits);}
        assert_eq!(display_probe(&client,"stable",&origin.url,1000).status,"ok");
    }
    assert!(socks.hits.load(Ordering::SeqCst)>=6);
    for status in [200,302,500] {
        origin.status.store(status,Ordering::SeqCst);
        let result=display_probe(&client,"stable",&origin.url,1000);
        assert_eq!(result.delay,None);assert_eq!(result.evidence["errorKind"],"unexpected_response");
    }
    origin.status.store(204,Ordering::SeqCst);
    let before=origin.hits.load(Ordering::SeqCst);
    let bypass=display_probe(&client,"bypass",&origin.url,1000);
    assert_eq!(bypass.evidence["errorKind"],"outbound_unverified");assert_eq!(origin.hits.load(Ordering::SeqCst),before);
    assert_eq!(display_probe(&client,"stable",&origin.url,1000).status,"ok");
    // A real UDP/STUN timeout is independent from this TCP-only relay.
    let sink=std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let udp=std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    udp.set_read_timeout(Some(Duration::from_millis(50))).unwrap();
    udp.send_to(&[0,1,0,0,0x21,0x12,0xa4,0x42,0,0,0,0,0,0,0,0,0,0,0,1],sink.local_addr().unwrap()).unwrap();
    assert!(udp.recv_from(&mut [0;512]).is_err());
    assert_eq!(display_probe(&client,"stable",&origin.url,1000).status,"ok");
}

#[test]
fn real_https_204_verified_tls_timeout_and_recovery_through_concrete_proxy() {
    use std::os::windows::process::CommandExt;
    let root=PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_owned();
    let directory=root.join("temp").join(format!("health-https-{}",uuid::Uuid::new_v4()));
    std::fs::create_dir(&directory).unwrap();
    let generated=std::process::Command::new("pwsh").creation_flags(0x08000000)
        .args(["-NoProfile","-File"]).arg(root.join("scripts/atlas-health-fixture-cert.ps1"))
        .arg("-Directory").arg(&directory).output().unwrap();
    assert!(generated.status.success(),"{}",String::from_utf8_lossy(&generated.stderr));
    let mut child=std::process::Command::new("node").creation_flags(0x08000000)
        .arg(root.join("scripts/atlas-health-https-fixture.mjs")).arg(&directory).spawn().unwrap();
    let job=crate::job::Job::attach(&child).unwrap();
    let outcome=std::panic::catch_unwind(||{
        let deadline=std::time::Instant::now()+Duration::from_secs(8);
        while !directory.join("ready.json").exists(){assert!(std::time::Instant::now()<deadline);thread::sleep(Duration::from_millis(20));}
        let ready:Value=serde_json::from_slice(&std::fs::read(directory.join("ready.json")).unwrap()).unwrap();
        let endpoint=format!("https://127.0.0.1:{}/generate_204",ready["port"].as_u64().unwrap());
        let socks=Socks::new();let fixture=Fixture::new(&settings(vec![socks.node("https-node")]));
        let client=fixture.core.client();
        // First require actual TLS authentication failure. Never install a CA
        // globally or use skip-cert-verify. Only this disposable core trusts it.
        let untrusted=display_probe(&client,"https-node",&endpoint,1500);
        assert_ne!(untrusted.status,"ok");assert_eq!(untrusted.delay,None);
        let path=fixture.directory.join("candidate.yaml");
        let mut config:Value=serde_yaml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        config["tls"]=json!({"custom-certifactes":[std::fs::read_to_string(directory.join("cert.pem")).unwrap()]});
        std::fs::write(&path,serde_yaml::to_string(&config).unwrap()).unwrap();
        client.api("PUT","/configs?force=true",Some(json!({"path":path}))).unwrap();
        let mut records=Vec::new();
        for mode in [0,1,2,0] {
            socks.mode.store(mode,Ordering::SeqCst);
            let result=display_probe(&client,"https-node",&endpoint,500);
            if mode==0 {assert_eq!(result.status,"ok","{result:?}");assert!(result.delay.unwrap()>=25);}
            else {assert_eq!(result.status,"unreachable","{result:?}");assert_eq!(result.evidence["errorKind"],"timeout");assert_eq!(result.delay,None);}
            records.push(serde_json::to_value(result).unwrap());
        }
        let hits:Value=serde_json::from_slice(&std::fs::read(directory.join("hits.json")).unwrap()).unwrap();
        assert_eq!(hits["hits"],2);assert_eq!(hits["method"],"HEAD");
        assert_eq!(hits["url"],"/generate_204?atlas_probe=1");assert!(hits["tls"].as_str().unwrap().starts_with("TLSv"));
        assert!(socks.hits.load(Ordering::SeqCst)>=5);
        std::fs::write(directory.join("evidence.json"),serde_json::to_vec_pretty(&json!({"untrustedRejected":true,"records":records,"origin":hits})).unwrap()).unwrap();
    });
    crate::process_stop::stop(&mut child,||drop(job),Duration::from_secs(3)).unwrap();
    for name in ["key.pem","cert.pem"] {let _=std::fs::remove_file(directory.join(name));}
    if let Err(error)=outcome {std::panic::resume_unwind(error);}
}

#[test]
fn real_xray_vless_outbound_carries_probe_and_rejects_wrong_credentials() {
    use std::os::windows::process::CommandExt;
    let origin=Origin::new();
    let reservation=TcpListener::bind("127.0.0.1:0").unwrap();let port=reservation.local_addr().unwrap().port();drop(reservation);
    let directory=std::env::temp_dir().join(format!("atlas-xray-origin-{}",uuid::Uuid::new_v4()));std::fs::create_dir(&directory).unwrap();
    let id="7e57438c-bc90-4eb1-a388-372785b9b1a2";
    let config=json!({"log":{"loglevel":"none"},"inbounds":[{"listen":"127.0.0.1","port":port,"protocol":"vless","settings":{"clients":[{"id":id}],"decryption":"none"}}],"outbounds":[{"protocol":"freedom"}]});
    let path=directory.join("server.json");std::fs::write(&path,config.to_string()).unwrap();
    let binary=PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/Atlas.Xray.exe");
    let mut child=std::process::Command::new(binary).creation_flags(0x08000000).args(["run","-config"]).arg(&path)
        .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().unwrap();
    let job=crate::job::Job::attach(&child).unwrap();
    let outcome=std::panic::catch_unwind(|| {
        let deadline=std::time::Instant::now()+Duration::from_secs(5);
        while TcpStream::connect(("127.0.0.1",port)).is_err() {assert!(std::time::Instant::now()<deadline);thread::sleep(Duration::from_millis(20));}
        let node=crate::xray_config::uri(&format!("vless://{id}@127.0.0.1:{port}?security=none&type=tcp#xray-good")).unwrap().unwrap();
        let mut bad=node.clone();bad["name"]=json!("xray-bad");bad["xray"]["outbounds"][0]["settings"]["vnext"][0]["users"][0]["id"]=json!(uuid::Uuid::new_v4().to_string());
        let full = |mut n:Value,name:&str| {
            n["name"]=json!(name);n["xraySimple"]=json!(false);
            n["xray"]["outbounds"].as_array_mut().unwrap().push(json!({"tag":"direct","protocol":"freedom"}));
            n["xray"]["routing"]=json!({"rules":[{"type":"field","network":"tcp,udp","outboundTag":"direct"}]});n
        };
        let full_good=full(node.clone(),"profile-good");let full_bad=full(bad.clone(),"profile-bad");
        let fixture=Fixture::new(&settings(vec![node,bad,full_good,full_bad]));
        for _ in 0..3 {
            let ok=display_probe(&fixture.core.client(),"xray-good",&origin.url,1500);
            assert_eq!(ok.status,"ok","{ok:?}");
            let bad=display_probe(&fixture.core.client(),"xray-bad",&origin.url,350);
            assert_ne!(bad.status,"ok");assert!(bad.delay.is_none());
        }
        // The ordinary profile deliberately routes to DIRECT and answers.
        // Certification must bypass those rules and test its concrete VLESS.
        let path=format!("/proxies/profile-bad/delay?timeout=1000&url={}",crate::latency::encode_name(&origin.url));
        assert!(fixture.core.api("GET",&path,None).unwrap()["delay"].as_u64().is_some());
        assert_eq!(display_probe(&fixture.core.client(),"profile-good",&origin.url,1500).status,"ok");
        let bad=display_probe(&fixture.core.client(),"profile-bad",&origin.url,350);
        assert_ne!(bad.status,"ok");assert!(bad.delay.is_none());
        // The pinned health inbound must not rewrite the provider's ordinary
        // routing, even after its dedicated VLESS probe has failed.
        assert!(fixture.core.api("GET",&path,None).unwrap()["delay"].as_u64().is_some());
    });
    crate::process_stop::stop(&mut child,||drop(job),Duration::from_secs(2)).unwrap();std::fs::remove_dir_all(directory).unwrap();
    if let Err(error)=outcome {std::panic::resume_unwind(error);}
}
