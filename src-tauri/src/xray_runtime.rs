//! Owned Xray workers. URI nodes share one process; complete JSON profiles retain
//! their own DNS/routing context. Only authenticated loopback SOCKS is exposed.
use crate::{model::Settings, xray_config};
use serde_json::json;
use std::{net::{TcpListener,UdpSocket},path::{Path,PathBuf},process::{Child,Command,Stdio},time::{Duration,Instant}};
use std::os::windows::process::CommandExt;

struct Worker { path: PathBuf, ports: Vec<u16> }
pub struct Prepared {
    pub settings: Settings,
    binary: PathBuf,
    directory: PathBuf,
    workers: Vec<Worker>,
    reservations: Vec<(TcpListener,UdpSocket)>,
    secrets: Vec<String>,
    cancellation: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}
struct Process { child: Child, job: Option<crate::job::Job> }
pub struct Runtime { processes: Vec<Process>, prepared: Prepared }
type LogBuffer=std::sync::Arc<std::sync::Mutex<std::collections::VecDeque<String>>>;

fn command(binary: &Path, directory: &Path) -> Command {
    let mut c=Command::new(binary);
    c.creation_flags(0x08000000).current_dir(directory)
        .env("XRAY_LOCATION_ASSET",binary.parent().unwrap_or(directory).join("xray-assets"))
        .stdout(Stdio::null()).stderr(Stdio::null()).stdin(Stdio::null());
    c
}
fn reserve() -> Result<(TcpListener,UdpSocket),String> {
    for _ in 0..128 {
        let udp=UdpSocket::bind(("127.0.0.1",0)).map_err(|_|"Xray: не удалось выделить UDP-порт")?;
        let port=udp.local_addr().map_err(|_|"Xray: порт недоступен")?.port();
        if let Ok(tcp)=TcpListener::bind(("127.0.0.1",port)) {return Ok((tcp,udp));}
    }
    Err("Xray: не удалось выделить общий TCP/UDP-порт".into())
}
fn default_interface() -> Result<String,String> {
    use windows_sys::Win32::NetworkManagement::IpHelper::*;
    unsafe {
        let mut table=std::ptr::null_mut();
        if GetIpForwardTable2(2,&mut table)!=0 {return Err("Xray: недоступна таблица маршрутов IPv4".into());}
        let rows=std::slice::from_raw_parts((*table).Table.as_ptr(),(*table).NumEntries as usize);
        let mut choices=Vec::new();
        for route in rows.iter().filter(|r|r.DestinationPrefix.PrefixLength==0 && r.Loopback==0) {
            let mut interface: MIB_IF_ROW2=std::mem::zeroed();interface.InterfaceLuid=route.InterfaceLuid;
            if GetIfEntry2(&mut interface)!=0 || interface.OperStatus!=1 {continue;}
            let name=String::from_utf16_lossy(&interface.Alias[..interface.Alias.iter().position(|c|*c==0).unwrap_or(interface.Alias.len())]);
            if name=="Atlas-TUN" {continue;}
            let mut ip: MIB_IPINTERFACE_ROW=std::mem::zeroed();ip.Family=2;ip.InterfaceLuid=route.InterfaceLuid;
            if GetIpInterfaceEntry(&mut ip)!=0 {continue;}
            choices.push((u64::from(route.Metric)+u64::from(ip.Metric),name));
        }
        FreeMibTable(table.cast());
        choices.sort();choices.into_iter().next().map(|(_,name)|name).ok_or("Xray: нет исходящего интерфейса с маршрутом по умолчанию".into())
    }
}
impl Prepared {
    pub fn new(settings: &Settings, mihomo: &Path, directory: &Path) -> Result<Self,String> {
        let mut prepared=Self {settings:settings.clone(),binary:mihomo.with_file_name("Atlas.Xray.exe"),
            directory:directory.join(format!("xray-{}",uuid::Uuid::new_v4())),workers:vec![],reservations:vec![],secrets:vec![],cancellation:None};
        crate::support_report::collect_secrets(&serde_json::to_value(settings).map_err(|_|"Xray: настройки недоступны")?,&mut prepared.secrets);
        let count=settings.servers().iter().filter(|n|n["type"]=="xray").count();
        if count==0 {return Ok(prepared);}
        if count>512 {return Err("Не более 512 Xray-серверов в одной сессии".into());}
        if !prepared.binary.is_file() {return Err("В установке Atlas отсутствует Atlas.Xray.exe".into());}
        let interface=if settings.mode=="tun" {Some(default_interface()?)} else {None};
        std::fs::create_dir_all(&prepared.directory).map_err(|_|"Не удалось создать каталог Xray")?;
        let mut simple_inbounds=Vec::new(); let mut simple_outbounds=Vec::new(); let mut simple_rules=Vec::new();
        let mut simple_ports=Vec::new(); let mut profiles=Vec::new(); let mut index=0;
        for sub in &mut prepared.settings.subscriptions {
            for node in &mut sub.servers {
                if node["type"]!="xray" {continue;}
                let reservation=reserve()?;
                let port=reservation.0.local_addr().map_err(|_|"Xray: порт недоступен")?.port();
                prepared.reservations.push(reservation);
                let tag=format!("atlas-{index}"); index+=1;
                let user=uuid::Uuid::new_v4().simple().to_string();
                let password=uuid::Uuid::new_v4().simple().to_string();
                let inbound=json!({"tag":tag,"listen":"127.0.0.1","port":port,"protocol":"socks",
                    "settings":{"auth":"password","accounts":[{"user":user,"pass":password}],"udp":true,"ip":"127.0.0.1"}});
                if node["xraySimple"]==true {
                    let prefix=format!("{tag}/");
                    let mut outbounds=xray_config::namespace_simple(&node["xray"],&prefix)?;
                    for outbound in &mut outbounds {
                        if !outbound["streamSettings"].is_object() {outbound["streamSettings"]=json!({});}
                        if !outbound["streamSettings"]["sockopt"].is_object() {outbound["streamSettings"]["sockopt"]=json!({});}
                        outbound["streamSettings"]["sockopt"]["domainStrategy"]=json!(if settings.dns.ipv6 {"UseIP"} else {"UseIPv4"});
                    }
                    simple_outbounds.extend(outbounds);
                    simple_inbounds.push(inbound);
                    simple_rules.push(json!({"type":"field","inboundTag":[tag],"outboundTag":format!("{prefix}proxy")}));
                    simple_ports.push(port);
                } else {
                    if profiles.len()>=16 {return Err("Не более 16 независимых полных Xray JSON-профилей в одной сессии".into());}
                    profiles.push((xray_config::controlled_profile(&node["xray"],inbound)?,vec![port]));
                }
                *node=json!({"name":node["name"],"type":"socks5","server":"127.0.0.1","port":port,
                    "username":user,"password":password,"udp":true,"atlas-xray-bridge":true});
            }
        }
        if !simple_ports.is_empty() {
            profiles.push((json!({"log":{"loglevel":"warning"},"inbounds":simple_inbounds,"outbounds":simple_outbounds,
                "dns":{"servers":["https+local://1.1.1.1/dns-query","https+local://8.8.8.8/dns-query"],
                    "queryStrategy":if settings.dns.ipv6 {"UseIP"} else {"UseIPv4"}},
                "routing":{"rules":simple_rules}}),simple_ports));
        }
        for (index,(mut config,ports)) in profiles.into_iter().enumerate() {
            if let Some(interface)=&interface {
                for outbound in config["outbounds"].as_array_mut().ok_or("Xray: отсутствуют outbounds")? {
                    if !outbound["streamSettings"].is_object() {outbound["streamSettings"]=json!({});}
                    if !outbound["streamSettings"]["sockopt"].is_object() {outbound["streamSettings"]["sockopt"]=json!({});}
                    outbound["streamSettings"]["sockopt"]["interface"]=json!(interface);
                }
            }
            let path=prepared.directory.join(format!("worker-{index}.json"));
            prepared.workers.push(Worker {path:path.clone(),ports});
            std::fs::write(path,serde_json::to_vec(&config).map_err(|_|"Xray: сериализация не удалась")?)
                .map_err(|_|"Не удалось записать конфигурацию Xray")?;
        }
        Ok(prepared)
    }
    pub fn cancellable(mut self, flag: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>) -> Self {self.cancellation=flag;self}
    fn cancelled(&self) -> bool {self.cancellation.as_ref().is_some_and(|f|!f.load(std::sync::atomic::Ordering::SeqCst))}
    pub fn validate(&self) -> Result<(),String> {
        for worker in &self.workers {
            if self.cancelled() {return Err("Запуск Xray отменён".into());}
            let log_path=worker.path.with_extension("validation.log");
            let log=std::fs::File::create(&log_path).map_err(|_|"Xray: не удалось создать журнал проверки")?;
            let mut child=command(&self.binary,&self.directory).stdout(log.try_clone().map_err(|_|"Xray: журнал недоступен")?).stderr(log)
                .args(["run","-test","-config"]).arg(&worker.path)
                .spawn().map_err(|_|"Не удалось проверить конфигурацию Xray")?;
            let job=match crate::job::Job::attach(&child) {
                Ok(job)=>job,Err(error)=>{let _=child.kill();let _=child.wait();return Err(error);}
            };
            let deadline=Instant::now()+Duration::from_secs(10);
            loop {
                if self.cancelled() {
                    crate::process_stop::stop(&mut child,||drop(job),Duration::from_secs(5))?;
                    return Err("Запуск Xray отменён".into());
                }
                match child.try_wait() {
                    Ok(Some(status))=>{if !status.success() {
                        use std::io::Read;
                        let mut details=String::new();
                        if let Ok(file)=std::fs::File::open(&log_path) {let _=file.take(32768).read_to_string(&mut details);}
                        return Err(format!("Xray отклонил профиль: {}",crate::support_report::redact(&details,&self.secrets)));
                    } break;},
                    Ok(None) if Instant::now()<deadline=>std::thread::sleep(Duration::from_millis(10)),
                    _=>{
                        crate::process_stop::stop(&mut child,||drop(job),Duration::from_secs(5))?;
                        return Err("Проверка конфигурации Xray не завершилась вовремя".into());
                    }
                }
            }
        }
        Ok(())
    }
    #[cfg(test)]
    pub fn start(self) -> Result<Runtime,String> {
        self.start_logged(Default::default())
    }
    pub fn start_logged(mut self, logs: LogBuffer) -> Result<Runtime,String> {
        self.validate()?;
        let mut secrets=self.secrets.clone();
        crate::support_report::collect_secrets(&serde_json::to_value(&self.settings).map_err(|_|"Xray: настройки недоступны")?,&mut secrets);
        self.reservations.clear();
        let mut runtime=Runtime {processes:vec![],prepared:self};
        for worker in &runtime.prepared.workers {
            if runtime.prepared.cancelled() {return Err("Запуск Xray отменён".into());}
            let mut child=command(&runtime.prepared.binary,&runtime.prepared.directory)
                .stdout(Stdio::piped()).stderr(Stdio::piped())
                .args(["run","-config"]).arg(&worker.path).spawn().map_err(|_|"Не удалось запустить Xray")?;
            let mut outputs: Vec<Box<dyn std::io::Read+Send>>=Vec::new();
            if let Some(stdout)=child.stdout.take() {outputs.push(Box::new(stdout));}
            if let Some(stderr)=child.stderr.take() {outputs.push(Box::new(stderr));}
            for output in outputs {
                let logs=logs.clone();let secrets=secrets.clone();
                std::thread::spawn(move || {
                    use std::io::BufRead;
                    for line in std::io::BufReader::new(output).lines().map_while(Result::ok) {
                        let line=crate::support_report::redact(&line,&secrets);
                        if let Ok(mut logs)=logs.lock() {
                            logs.push_back(format!("Atlas.Xray: {}",line.chars().take(4096).collect::<String>()));
                            while logs.len()>4096 {logs.pop_front();}
                        }
                    }
                });
            }
            let job=match crate::job::Job::attach(&child) {
                Ok(job)=>job,Err(error)=>{let _=child.kill();let _=child.wait();return Err(error);}
            };
            runtime.processes.push(Process {child,job:Some(job)});
        }
        let deadline=Instant::now()+Duration::from_secs(10);
        loop {
            if runtime.prepared.cancelled() {return Err("Запуск Xray отменён".into());}
            for process in &mut runtime.processes {
                if process.child.try_wait().map_err(|_|"Xray: статус процесса недоступен")?.is_some() {
                    return Err("Xray завершился до готовности; подключение не изменено".into());
                }
            }
            if runtime.prepared.workers.iter().flat_map(|w|&w.ports).all(|port|
                std::net::TcpStream::connect_timeout(&([127,0,0,1],*port).into(),Duration::from_millis(20)).is_ok()) {return Ok(runtime);}
            if Instant::now()>=deadline {return Err("Xray не открыл локальные порты вовремя".into());}
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
impl Runtime {
    pub fn healthy(&mut self) -> bool {self.processes.iter_mut().all(|p|matches!(p.child.try_wait(),Ok(None)))}
    pub fn stop(&mut self) -> Result<(),String> {
        let mut errors=Vec::new();
        // Release every Job Object up front so all owned Xray workers terminate
        // together; one shared deadline prevents N workers multiplying Exit time.
        for process in &mut self.processes {
            drop(process.job.take());
            if let Err(error)=process.child.kill() {
                if !matches!(process.child.try_wait(),Ok(Some(_))) { errors.push(error.to_string()); }
            }
        }
        let deadline=Instant::now()+Duration::from_secs(1);
        loop {
            let mut pending=false;
            for process in &mut self.processes {
                match process.child.try_wait() {
                    Ok(Some(_))=>{},
                    Ok(None)=>pending=true,
                    Err(error)=>errors.push(error.to_string()),
                }
            }
            if !pending { break; }
            if Instant::now()>=deadline {
                errors.push("истёк общий срок ожидания выхода процессов Xray".into());
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        if errors.is_empty() {Ok(())} else {Err(format!("Не подтверждена остановка Xray: {}",errors.join("; ")))}
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        // Signal every owned worker together, then collect process exits.
        let _=self.stop();
    }
}
impl Drop for Prepared {
    fn drop(&mut self) {
        for worker in &self.workers {let _=std::fs::remove_file(&worker.path);let _=std::fs::remove_file(worker.path.with_extension("validation.log"));}
        let _=std::fs::remove_dir(&self.directory);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read,Write};
    #[test]
    fn owned_runtime_authenticates_forwards_and_exits() {
        let mut settings=Settings::default();
        settings.mode="system".into();
        settings.subscriptions.push(crate::model::Subscription {options:Default::default(),id:"fixture".into(),name:"fixture".into(),
            masked_url:String::new(),updated_at:0,error:None,servers:vec![json!({"name":"test","type":"xray","server":"127.0.0.1","port":443,
            "xray":{"outbounds":[{"protocol":"freedom"}]}})]});
        let directory=std::env::temp_dir().join(format!("atlas-runtime-test-{}",uuid::Uuid::new_v4()));
        let binary=Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/Atlas.Core.exe");
        let prepared=Prepared::new(&settings,&binary,&directory).unwrap();
        let mapped=prepared.settings.servers()[0].clone();
        let port=mapped["port"].as_u64().unwrap() as u16;
        let files=prepared.directory.clone();
        let mut runtime=prepared.start().unwrap();
        assert!(runtime.healthy());
        let mut denied=std::net::TcpStream::connect(("127.0.0.1",port)).unwrap();
        denied.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        denied.write_all(&[5,1,0]).unwrap();
        let mut reply=[0;2];denied.read_exact(&mut reply).unwrap();assert_eq!(reply,[5,255]);
        drop(denied);
        let target=TcpListener::bind(("127.0.0.1",0)).unwrap();
        let target_port=target.local_addr().unwrap().port();
        let mut stream=std::net::TcpStream::connect(("127.0.0.1",port)).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        stream.write_all(&[5,1,2]).unwrap();stream.read_exact(&mut reply).unwrap();assert_eq!(reply,[5,2]);
        let user=mapped["username"].as_str().unwrap();let password=mapped["password"].as_str().unwrap();
        let mut auth=vec![1,user.len() as u8];auth.extend(user.as_bytes());auth.push(password.len() as u8);auth.extend(password.as_bytes());
        stream.write_all(&auth).unwrap();stream.read_exact(&mut reply).unwrap();assert_eq!(reply,[1,0]);
        let mut request=vec![5,1,0,1,127,0,0,1];request.extend(target_port.to_be_bytes());
        stream.write_all(&request).unwrap();
        let mut response=[0;10];stream.read_exact(&mut response).unwrap();assert_eq!(response[1],0);
        stream.write_all(b"atlas").unwrap();
        target.set_nonblocking(true).unwrap();
        let deadline=Instant::now()+Duration::from_secs(3);
        let (mut accepted,_)=loop {match target.accept() {Ok(pair)=>break pair,Err(_) if Instant::now()<deadline=>std::thread::sleep(Duration::from_millis(10)),Err(e)=>panic!("{e}")}};
        accepted.set_nonblocking(false).unwrap();
        accepted.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        let mut payload=[0;5];accepted.read_exact(&mut payload).unwrap();assert_eq!(&payload,b"atlas");
        accepted.write_all(b"ready").unwrap();stream.read_exact(&mut payload).unwrap();assert_eq!(&payload,b"ready");
        drop(runtime);
        assert!(std::net::TcpStream::connect(("127.0.0.1",port)).is_err());
        assert!(!files.exists());
        let _=std::fs::remove_dir(directory);
    }
}
