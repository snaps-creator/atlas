//! Opt-in peer diagnostics. No routing changes, remote shell or VPN control.
use ring::{
    aead, pbkdf2,
    rand::{SecureRandom, SystemRandom},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    io::{Read, Write},
    net::{IpAddr, SocketAddr, TcpListener, TcpStream, UdpSocket},
    num::NonZeroU32,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

pub const PORT: u16 = 17943;
const LIMIT: usize = 2 * 1024 * 1024;
const REQUEST_AAD: &[u8] = b"atlas-lan-diagnostics/request/v1";
const RESPONSE_AAD: &[u8] = b"atlas-lan-diagnostics/response/v1";
const SALT: &[u8] = b"Atlas LAN diagnostics v1 password key";
const DISCOVER_REQUEST: &[u8] = b"atlas-lan-discover/request/v1";
const DISCOVER_RESPONSE: &[u8] = b"atlas-lan-discover/response/v1";

#[derive(Clone, Serialize, Deserialize)]
struct Peer {
    id: String,
    address: String,
    name: String,
    reported_name: String,
    last_seen: u64,
}
#[derive(Clone, Serialize, Deserialize)]
struct Config {
    id: String,
    name: String,
    enabled: bool,
    key: Option<[u8; 32]>,
    peers: Vec<Peer>,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name: std::env::var("COMPUTERNAME").unwrap_or("Atlas".into()),
            enabled: false,
            key: None,
            peers: vec![],
        }
    }
}
struct State {
    config: Config,
    sessions: HashMap<String, Instant>,
    next_login: Instant,
    next_capture: Instant,
    requests: HashMap<String, Instant>,
    capture_running: bool,
    capture_completed: Option<u64>,
    listener_error: Option<String>,
    discovery_error: Option<String>,
    load_error: Option<String>,
}
#[derive(Clone)]
pub struct Lan {
    state: Arc<Mutex<State>>,
    directory: PathBuf,
    access: crate::support_report::Access,
    published: crate::published_state::PublishedState,
}

fn derive(password: &str) -> Result<[u8; 32], String> {
    if !(8..=256).contains(&password.len()) {
        return Err("Пароль должен содержать от 8 до 256 байт".into());
    }
    let mut key = [0; 32];
    pbkdf2::derive(
        pbkdf2::PBKDF2_HMAC_SHA256,
        NonZeroU32::new(600_000).unwrap(),
        SALT,
        password.as_bytes(),
        &mut key,
    );
    Ok(key)
}
fn same_key(a: &[u8; 32], b: &[u8; 32]) -> bool {
    // Constant-time comparison through HMAC verification, without exposing the key.
    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, a);
    let tag = ring::hmac::sign(&key, b"unlock");
    ring::hmac::verify(
        &ring::hmac::Key::new(ring::hmac::HMAC_SHA256, b),
        b"unlock",
        tag.as_ref(),
    )
    .is_ok()
}
fn seal(key: &[u8; 32], value: &Value, aad: &[u8]) -> Result<Vec<u8>, String> {
    let mut nonce = [0; 12];
    SystemRandom::new()
        .fill(&mut nonce)
        .map_err(|_| "Нет системной случайности")?;
    let mut bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    if bytes.len() > LIMIT {
        return Err("Отчёт превышает лимит".into());
    }
    let cipher = aead::LessSafeKey::new(aead::UnboundKey::new(&aead::AES_256_GCM, key).unwrap());
    cipher
        .seal_in_place_append_tag(
            aead::Nonce::assume_unique_for_key(nonce),
            aead::Aad::from(aad),
            &mut bytes,
        )
        .map_err(|_| "Ошибка шифрования")?;
    Ok([nonce.to_vec(), bytes].concat())
}
fn open(key: &[u8; 32], bytes: &[u8], aad: &[u8]) -> Result<Value, String> {
    if bytes.len() < 28 || bytes.len() > LIMIT + 28 {
        return Err("Неверный размер пакета".into());
    }
    let nonce: [u8; 12] = bytes[..12].try_into().unwrap();
    let mut data = bytes[12..].to_vec();
    let cipher = aead::LessSafeKey::new(aead::UnboundKey::new(&aead::AES_256_GCM, key).unwrap());
    let plain = cipher
        .open_in_place(
            aead::Nonce::assume_unique_for_key(nonce),
            aead::Aad::from(aad),
            &mut data,
        )
        .map_err(|_| "Неверный пароль или повреждённый пакет")?;
    serde_json::from_slice(plain).map_err(|_| "Неверный формат отчёта".into())
}
fn local(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => v.is_private() || v.is_loopback() || v.is_link_local(),
        _ => false,
    }
}
fn address(value: &str) -> Result<SocketAddr, String> {
    let ip: IpAddr = value
        .parse()
        .map_err(|_| "Укажите локальный IPv4-адрес компьютера")?;
    if !local(ip) {
        return Err("Разрешены только локальные IPv4-адреса".into());
    }
    Ok(SocketAddr::new(ip, PORT))
}
fn name(value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > 80 || value.chars().any(char::is_control) {
        return Err("Имя должно содержать 1–80 символов без управляющих знаков".into());
    }
    Ok(value.into())
}
fn read_frame(stream: &mut TcpStream, limit: usize) -> Result<Vec<u8>, String> {
    let deadline = Instant::now() + Duration::from_secs(8);
    fn read(stream: &mut TcpStream, data: &mut [u8], deadline: Instant) -> Result<(), String> {
        let mut offset = 0;
        while offset < data.len() {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .filter(|v| !v.is_zero())
                .ok_or("Истекло время чтения")?;
            stream
                .set_read_timeout(Some(remaining))
                .map_err(|e| e.to_string())?;
            let n = stream
                .read(&mut data[offset..])
                .map_err(|e| e.to_string())?;
            if n == 0 {
                return Err("Соединение закрыто".into());
            }
            offset += n;
        }
        Ok(())
    }
    let mut header = [0; 4];
    read(stream, &mut header, deadline)?;
    let size = u32::from_be_bytes(header) as usize;
    if size < 28 || size > limit {
        return Err("Неверный размер пакета".into());
    }
    let mut data = vec![0; size];
    read(stream, &mut data, deadline)?;
    Ok(data)
}
fn write_frame(stream: &mut TcpStream, data: &[u8]) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(8);
    let framed = [(data.len() as u32).to_be_bytes().as_slice(), data].concat();
    let mut offset = 0;
    while offset < framed.len() {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .filter(|v| !v.is_zero())
            .ok_or("Истекло время отправки")?;
        stream
            .set_write_timeout(Some(remaining))
            .map_err(|e| e.to_string())?;
        let n = stream.write(&framed[offset..]).map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("Соединение закрыто".into());
        }
        offset += n;
    }
    Ok(())
}
impl Lan {
    pub fn new(
        directory: PathBuf,
        access: crate::support_report::Access,
        published: crate::published_state::PublishedState,
    ) -> Self {
        let path = directory.join("lan-diagnostics.bin");
        let loaded: Result<Config, String> = if path.exists() {
            std::fs::read(&path)
                .map_err(|e| e.to_string())
                .and_then(|b| crate::storage::crypt(&b, false))
                .and_then(|b| serde_json::from_slice(&b).map_err(|e| e.to_string()))
        } else {
            Ok(Config::default())
        };
        let load_error = loaded.as_ref().err().cloned();
        Self {
            directory,
            access,
            published,
            state: Arc::new(Mutex::new(State {
                config: loaded.unwrap_or_default(),
                sessions: HashMap::new(),
                next_login: Instant::now(),
                next_capture: Instant::now(),
                requests: HashMap::new(),
                capture_running: false,
                capture_completed: None,
                listener_error: None,
                discovery_error: None,
                load_error,
            })),
        }
    }
    fn persist(&self, config: &Config) -> Result<(), String> {
        let bytes = crate::storage::crypt(
            &serde_json::to_vec(config).map_err(|e| e.to_string())?,
            true,
        )?;
        let temporary = self.directory.join("lan-diagnostics.bin.tmp");
        std::fs::write(&temporary, bytes).map_err(|e| e.to_string())?;
        std::fs::rename(temporary, self.directory.join("lan-diagnostics.bin"))
            .map_err(|e| e.to_string())
    }
    pub fn start(&self, shutdown: Arc<AtomicBool>) {
        self.start_discovery(shutdown.clone());
        let this = self.clone();
        std::thread::spawn(move || {
            let active = Arc::new(AtomicUsize::new(0));
            let mut listener = None;
            while !shutdown.load(Ordering::SeqCst) {
                let enabled = this.state.lock().unwrap().config.enabled;
                if !enabled {
                    listener = None;
                    std::thread::sleep(Duration::from_millis(250));
                    continue;
                }
                if listener.is_none() {
                    match TcpListener::bind(("0.0.0.0", PORT)).and_then(|l| {
                        l.set_nonblocking(true)?;
                        Ok(l)
                    }) {
                        Ok(l) => {
                            listener = Some(l);
                            this.state.lock().unwrap().listener_error = None;
                        }
                        Err(e) => {
                            this.state.lock().unwrap().listener_error = Some(e.to_string());
                            std::thread::sleep(Duration::from_secs(2));
                            continue;
                        }
                    }
                }
                if let Ok((mut stream, peer)) = listener.as_ref().unwrap().accept() {
                    if local(peer.ip()) && active.load(Ordering::SeqCst) < 4 {
                        active.fetch_add(1, Ordering::SeqCst);
                        let active = active.clone();
                        let this = this.clone();
                        std::thread::spawn(move || {
                            let _ = stream.set_nonblocking(false);
                            let _ = this.serve(&mut stream);
                            active.fetch_sub(1, Ordering::SeqCst);
                        });
                    }
                }
                std::thread::sleep(Duration::from_millis(30));
            }
        });
    }
    fn start_discovery(&self, shutdown: Arc<AtomicBool>) {
        let this=self.clone();
        std::thread::spawn(move || {
            let mut socket:Option<UdpSocket>=None;
            let mut buffer=[0u8;1024];
            while !shutdown.load(Ordering::SeqCst) {
                let (enabled,key,id,device_name)={let s=this.state.lock().unwrap();
                    (s.config.enabled,s.config.key,s.config.id.clone(),s.config.name.clone())};
                if !enabled {socket=None;std::thread::sleep(Duration::from_millis(250));continue;}
                if socket.is_none() {
                    match UdpSocket::bind(("0.0.0.0",crate::lan_discovery::PORT)).and_then(|s|{s.set_nonblocking(true)?;Ok(s)}) {
                        Ok(s)=>{socket=Some(s);this.state.lock().unwrap().discovery_error=None;}
                        Err(e)=>{this.state.lock().unwrap().discovery_error=Some(e.to_string());std::thread::sleep(Duration::from_secs(2));continue;}
                    }
                }
                if let (Some(socket),Some(key))=(&socket,key) {
                    if let Ok((size,peer))=socket.recv_from(&mut buffer) {
                        if local(peer.ip()) {
                            if let Some(reply)=discovery_reply(&key,&buffer[..size],&id,&device_name) {let _=socket.send_to(&reply,peer);}
                        }
                    }
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        });
    }
    fn discover(&self)->Result<Value,String> {
        let (key,own_id)={let s=self.state.lock().unwrap();(s.config.key.ok_or("Нет пароля")?,s.config.id.clone())};
        let request_id=uuid::Uuid::new_v4().to_string();
        let packet=seal(&key,&json!({"id":request_id,"at":crate::model::now()}),DISCOVER_REQUEST)?;
        let mut sockets=Vec::new();
        for (ip,broadcast) in crate::lan_discovery::targets() {
            if let Ok(s)=UdpSocket::bind((ip,0)) {
                if s.set_broadcast(true).and_then(|_|s.set_nonblocking(true)).is_ok() {
                    let _=s.set_ttl(1);
                    if s.send_to(&packet,(broadcast,crate::lan_discovery::PORT)).is_ok() {sockets.push((s,broadcast));}
                }
            }
        }
        if sockets.is_empty() {return Err("Не найден доступный локальный IPv4-интерфейс для поиска".into());}
        let deadline=Instant::now()+Duration::from_secs(3);
        let mut repeat=false;
        let mut found=HashMap::<String,Peer>::new();
        let mut buffer=[0u8;1024];
        while Instant::now()<deadline {
            if !repeat && deadline.saturating_duration_since(Instant::now())<Duration::from_secs(2) {
                for (s,broadcast) in &sockets {let _=s.send_to(&packet,(*broadcast,crate::lan_discovery::PORT));} repeat=true;
            }
            for (socket,_) in &sockets {
                // Bounded receive work even on a noisy LAN.
                for _ in 0..32 {
                    let Ok((size,peer))=socket.recv_from(&mut buffer) else {break};
                    if !local(peer.ip()) || peer.port()!=crate::lan_discovery::PORT {continue;}
                    let Ok(reply)=open(&key,&buffer[..size],DISCOVER_RESPONSE) else {continue};
                    if reply["requestId"]!=request_id {continue;}
                    let Some(id)=reply["id"].as_str().filter(|id|*id!=own_id && uuid::Uuid::parse_str(id).is_ok()) else {continue};
                    let Ok(device_name)=name(reply["name"].as_str().unwrap_or("")) else {continue};
                    if found.len()<256 {found.insert(id.into(),Peer{id:id.into(),address:peer.ip().to_string(),name:device_name.clone(),reported_name:device_name,last_seen:crate::model::now()});}
                }
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        let mut s=self.state.lock().unwrap();
        let mut config=s.config.clone();let mut changed=false;
        for saved in &mut config.peers {if let Some(peer)=found.get(&saved.id) {
            changed|=saved.address!=peer.address;saved.address=peer.address.clone();
        }}
        if changed {self.persist(&config)?;s.config=config;}
        let mut peers:Vec<_>=found.into_values().collect();peers.sort_by(|a,b|a.name.cmp(&b.name));
        Ok(json!({"peers":peers}))
    }
    fn serve(&self, stream: &mut TcpStream) -> Result<(), String> {
        let key = {
            let s = self.state.lock().unwrap();
            if !s.config.enabled {
                return Err("Обмен выключен".into());
            }
            s.config.key.ok_or("Не настроен пароль")?
        };
        let request = open(&key, &read_frame(stream, 4096)?, REQUEST_AAD)?;
        let at = request["at"].as_u64().ok_or("Нет времени запроса")?;
        if crate::model::now().abs_diff(at) > 120 {
            return Err("Проверьте время на компьютерах".into());
        }
        let id = request["id"]
            .as_str()
            .filter(|s| uuid::Uuid::parse_str(s).is_ok())
            .ok_or("Нет ID запроса")?;
        {
            let mut s = self.state.lock().unwrap();
            s.requests
                .retain(|_, at| at.elapsed() < Duration::from_secs(240));
            if s.requests.contains_key(id) || s.requests.len() >= 512 {
                return Err("Повторный запрос или превышен лимит".into());
            }
            s.requests.insert(id.into(), Instant::now());
        }
        let action = request["action"].as_str().unwrap_or("");
        if !matches!(action, "status" | "report" | "capture") {
            return Err("Неизвестная операция".into());
        }
        if action == "capture" {
            self.capture();
        }
        let response = json!({"id":id,"report":self.report(action!="status")});
        write_frame(stream, &seal(&key, &response, RESPONSE_AAD)?)
    }
    fn capture(&self) {
        let Some(c) = self.access.get() else {
            return;
        };
        {
            let mut s = self.state.lock().unwrap();
            if s.capture_running || Instant::now() < s.next_capture {
                return;
            }
            s.capture_running = true;
            s.next_capture = Instant::now() + Duration::from_secs(60);
        }
        let this = self.clone();
        std::thread::spawn(move || {
            crate::support_report::automatic_incident(c.client, c.secrets, c.revision);
            let mut s = this.state.lock().unwrap();
            s.capture_running = false;
            s.capture_completed = Some(crate::model::now());
        });
    }
    fn report(&self, detailed: bool) -> Value {
        let (id, name, running, completed) = {
            let s = self.state.lock().unwrap();
            (
                s.config.id.clone(),
                s.config.name.clone(),
                s.capture_running,
                s.capture_completed,
            )
        };
        let published = self.published.get();
        let mut result = json!({"deviceId":id,"name":name,"at":crate::model::now(),"version":env!("CARGO_PKG_VERSION"),
            "status":published["status"],"running":published["running"],"selected":published["settings"]["selected"],
            "captureRunning":running,"captureCompletedAt":completed});
        if detailed {
            let history = crate::incident_history::snapshot();
            let mut total = 0;
            let mut entries = Vec::new();
            let mut omitted = 0;
            for entry in history["entries"].as_array().into_iter().flatten().rev() {
                let size = entry.to_string().len();
                if total + size > LIMIT / 2 {
                    omitted += 1;
                    continue;
                }
                total += size;
                entries.push(entry.clone());
            }
            entries.reverse();
            result["history"] = json!({"entries":entries,"omittedEntries":omitted,"droppedEntries":history["droppedEntries"]});
        }
        let secrets = self.access.get().map(|c| c.secrets).unwrap_or_default();
        let safe = crate::support_report::redact(&result.to_string(), &secrets);
        let mut safe: Value = serde_json::from_str(&safe).unwrap_or_else(
            |_| json!({"name":result["name"],"at":result["at"],"redactedReport":safe}),
        );
        // The generic VPN redactor masks UUIDs. This independently generated device
        // identity is public protocol metadata, not a subscription credential.
        safe["deviceId"] = result["deviceId"].clone();
        safe
    }
    pub fn command(&self, action: &str, payload: Value) -> Result<Value, String> {
        if action == "lan_info" {
            let s = self.state.lock().unwrap();
            return Ok(json!({"configured":s.config.key.is_some(),"loadError":s.load_error}));
        }
        if action == "lan_unlock" {
            {
                let mut s = self.state.lock().unwrap();
                if let Some(e) = &s.load_error {
                    return Err(e.clone());
                }
                if Instant::now() < s.next_login {
                    return Err("Повторите вход через несколько секунд".into());
                }
                s.next_login = Instant::now() + Duration::from_secs(2);
            }
            let key = derive(payload["password"].as_str().unwrap_or(""))?;
            let mut s = self.state.lock().unwrap();
            if let Some(expected) = s.config.key {
                if !same_key(&expected, &key) {
                    return Err("Неверный пароль".into());
                }
            } else {
                let mut config = s.config.clone();
                config.key = Some(key);
                self.persist(&config)?;
                s.config = config;
            }
            let token = uuid::Uuid::new_v4().to_string();
            s.sessions.retain(|_, until| *until > Instant::now());
            if s.sessions.len() >= 8 {
                s.sessions.clear();
            }
            s.sessions
                .insert(token.clone(), Instant::now() + Duration::from_secs(1800));
            return Ok(json!({"token":token}));
        }
        let token = payload["token"].as_str().unwrap_or("");
        {
            let s = self.state.lock().unwrap();
            if !s.sessions.get(token).is_some_and(|t| *t > Instant::now()) {
                return Err("Введите пароль заново".into());
            }
        }
        match action {
            "lan_discover" => self.discover(),
            "lan_firewall" => {
                use base64::Engine;
                let exe = std::env::current_exe()
                    .map_err(|e| e.to_string())?
                    .to_string_lossy()
                    .replace('\'', "''");
                let script=format!("$ErrorActionPreference='Stop'; Get-NetFirewallRule -Name 'Atlas-LAN-Diagnostics' -ErrorAction SilentlyContinue | Remove-NetFirewallRule; New-NetFirewallRule -Name 'Atlas-LAN-Diagnostics' -DisplayName 'Atlas LAN diagnostics' -Direction Inbound -Action Allow -Protocol TCP -LocalPort {PORT} -RemoteAddress LocalSubnet -Profile Private -Program '{exe}' | Out-Null; Get-NetFirewallRule -Name 'Atlas-LAN-Discovery' -ErrorAction SilentlyContinue | Remove-NetFirewallRule; New-NetFirewallRule -Name 'Atlas-LAN-Discovery' -DisplayName 'Atlas LAN discovery' -Direction Inbound -Action Allow -Protocol UDP -LocalPort 17944 -RemoteAddress LocalSubnet -Profile Private -Program '{exe}' | Out-Null");
                let encoded = base64::engine::general_purpose::STANDARD.encode(
                    script
                        .encode_utf16()
                        .flat_map(u16::to_le_bytes)
                        .collect::<Vec<_>>(),
                );
                let mut command = std::process::Command::new("powershell.exe");
                command.args(["-NoProfile","-NonInteractive","-Command",&format!("$ErrorActionPreference='Stop'; $p=Start-Process powershell.exe -Verb RunAs -WindowStyle Hidden -ArgumentList '-NoProfile -NonInteractive -EncodedCommand {encoded}' -PassThru -Wait; if($p.ExitCode -ne 0){{throw 'Firewall configuration failed'}}; 'ATLAS_LAN_FIREWALL_OK'")]);
                let text = crate::support_report::run_bounded(command, Duration::from_secs(60))?;
                if !text.contains("ATLAS_LAN_FIREWALL_OK") {
                    return Err("Правило не подтверждено. Возможно, запрос Windows был отменён или истекло время ожидания.".into());
                }
                Ok(json!({"message":"Приём разрешён для Atlas в частной локальной сети."}))
            }
            "lan_lock" => {
                self.state.lock().unwrap().sessions.remove(token);
                Ok(Value::Null)
            }
            "lan_state" => {
                let s = self.state.lock().unwrap();
                Ok(
                    json!({"id":s.config.id,"name":s.config.name,"enabled":s.config.enabled,"peers":s.config.peers,"listenerError":s.listener_error,"discoveryError":s.discovery_error,"port":PORT}),
                )
            }
            "lan_save" => {
                let mut s = self.state.lock().unwrap();
                let mut c = s.config.clone();
                c.name = name(payload["name"].as_str().unwrap_or(""))?;
                c.enabled = payload["enabled"].as_bool().ok_or("Нет режима обмена")?;
                self.persist(&c)?;
                s.config = c;
                Ok(Value::Null)
            }
            "lan_rename" | "lan_remove" => {
                let mut s = self.state.lock().unwrap();
                let mut c = s.config.clone();
                let id = payload["id"].as_str().ok_or("Нет ID")?;
                if action == "lan_remove" {
                    uuid::Uuid::parse_str(id).map_err(|_| "Неверный ID")?;
                    c.peers.retain(|p| p.id != id);
                    let _ =
                        std::fs::remove_file(self.directory.join(format!("lan-report-{id}.bin")));
                } else {
                    let p = c
                        .peers
                        .iter_mut()
                        .find(|p| p.id == id)
                        .ok_or("Компьютер не найден")?;
                    p.name = name(payload["name"].as_str().unwrap_or(""))?;
                }
                self.persist(&c)?;
                s.config = c;
                Ok(Value::Null)
            }
            "lan_fetch" => self.fetch(&payload),
            "lan_cached" => {
                let id = payload["id"].as_str().ok_or("Нет ID")?;
                uuid::Uuid::parse_str(id).map_err(|_| "Неверный ID")?;
                let bytes = std::fs::read(self.directory.join(format!("lan-report-{id}.bin")))
                    .map_err(|_| "Сохранённого отчёта нет")?;
                serde_json::from_slice(&crate::storage::crypt(&bytes, false)?)
                    .map_err(|e| e.to_string())
            }
            _ => Err("Неизвестная команда диагностики".into()),
        }
    }
    fn fetch(&self, payload: &Value) -> Result<Value, String> {
        let address_text = payload["address"].as_str().ok_or("Нет адреса")?;
        self.fetch_from(payload, address(address_text)?)
    }
    fn fetch_from(&self, payload: &Value, peer_address: SocketAddr) -> Result<Value, String> {
        let address_text = payload["address"].as_str().ok_or("Нет адреса")?;
        let key = self
            .state
            .lock()
            .unwrap()
            .config
            .key
            .ok_or("Не настроен пароль")?;
        let id = uuid::Uuid::new_v4().to_string();
        let action = if payload["capture"] == true {
            "capture"
        } else if payload["statusOnly"] == true {
            "status"
        } else {
            "report"
        };
        let mut stream = TcpStream::connect_timeout(&peer_address, Duration::from_secs(3))
            .map_err(|_| "ПК недоступен: проверьте IP, обмен в Atlas и брандмауэр (TCP 17943)")?;
        write_frame(
            &mut stream,
            &seal(
                &key,
                &json!({"id":id,"at":crate::model::now(),"action":action}),
                REQUEST_AAD,
            )?,
        )?;
        let response = open(&key, &read_frame(&mut stream, LIMIT + 28)?, RESPONSE_AAD)?;
        if response["id"] != id {
            return Err("Ответ относится к другому запросу".into());
        }
        let report = &response["report"];
        let device = report["deviceId"].as_str().ok_or("Нет ID компьютера")?;
        uuid::Uuid::parse_str(device).map_err(|_| "Неверный ID компьютера")?;
        if let Some(expected) = payload["expectedId"].as_str() {
            if !expected.is_empty() && expected != device {
                return Err("По этому IP отвечает другой компьютер. Добавьте его отдельно.".into());
            }
        }
        let reported_name = name(report["name"].as_str().unwrap_or("Atlas"))?;
        let mut s = self.state.lock().unwrap();
        let mut c = s.config.clone();
        if let Some(p) = c.peers.iter_mut().find(|p| p.id == device) {
            p.address = address_text.into();
            p.reported_name = reported_name;
            p.last_seen = crate::model::now();
        } else {
            if c.peers.len() >= 32 {
                return Err("В списке уже 32 компьютера".into());
            }
            c.peers.push(Peer {
                id: device.into(),
                address: address_text.into(),
                name: reported_name.clone(),
                reported_name,
                last_seen: crate::model::now(),
            });
        }
        self.persist(&c)?;
        s.config = c;
        if action != "status" {
            let bytes = crate::storage::crypt(
                &serde_json::to_vec(report).map_err(|e| e.to_string())?,
                true,
            )?;
            std::fs::write(
                self.directory.join(format!("lan-report-{device}.bin")),
                bytes,
            )
            .map_err(|e| e.to_string())?;
        }
        Ok(report.clone())
    }
}

fn discovery_reply(key:&[u8;32],packet:&[u8],id:&str,device_name:&str)->Option<Vec<u8>> {
    let request=open(key,packet,DISCOVER_REQUEST).ok()?;
    if !request["at"].as_u64().is_some_and(|at|crate::model::now().abs_diff(at)<=120) ||
        !request["id"].as_str().is_some_and(|v|uuid::Uuid::parse_str(v).is_ok()) {return None;}
    seal(key,&json!({"requestId":request["id"],"id":id,"name":device_name}),DISCOVER_RESPONSE).ok()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn discovery_returns_only_authenticated_names_and_correlates_nonce() {
        let key=[7;32];let id=uuid::Uuid::new_v4().to_string();
        let packet=seal(&key,&json!({"id":id,"at":crate::model::now()}),DISCOVER_REQUEST).unwrap();
        let reply=discovery_reply(&key,&packet,"device-id","Бухгалтерия").unwrap();
        let decoded=open(&key,&reply,DISCOVER_RESPONSE).unwrap();
        assert_eq!(decoded["name"],"Бухгалтерия");assert_eq!(decoded["requestId"],id);
        assert!(discovery_reply(&[8;32],&packet,"id","Name").is_none());
        assert!(discovery_reply(&key,&reply,"id","Name").is_none());
        let stale=seal(&key,&json!({"id":id,"at":0}),DISCOVER_REQUEST).unwrap();
        assert!(discovery_reply(&key,&stale,"id","Name").is_none());
    }
    #[test]
    fn encrypted_frames_reject_wrong_password_tampering_and_reflection() {
        let key = derive("test-password").unwrap();
        let other = derive("different-password").unwrap();
        let v = json!({"id":"test","private":"diagnostics"});
        let mut encrypted = seal(&key, &v, REQUEST_AAD).unwrap();
        assert!(!encrypted.windows(11).any(|w| w == b"diagnostics"));
        assert_eq!(open(&key, &encrypted, REQUEST_AAD).unwrap(), v);
        assert!(open(&other, &encrypted, REQUEST_AAD).is_err());
        assert!(open(&key, &encrypted, RESPONSE_AAD).is_err());
        encrypted[15] ^= 1;
        assert!(open(&key, &encrypted, REQUEST_AAD).is_err());
        assert!(same_key(&key, &key));
        assert!(!same_key(&key, &other));
    }
    #[test]
    fn public_targets_and_bad_names_are_rejected() {
        assert!(address("192.168.1.2").is_ok());
        assert!(address("127.0.0.1").is_ok());
        for v in ["8.8.8.8", "example.com", "http://192.168.1.2", "::1"] {
            assert!(address(v).is_err());
        }
        assert_eq!(
            name("  Офис • бухгалтерия  ").unwrap(),
            "Офис • бухгалтерия"
        );
        assert!(name("a\nb").is_err());
    }
    #[test]
    fn peer_exchange_names_cache_and_auth_survive_restart() {
        fn instance() -> Lan {
            let directory =
                std::env::temp_dir().join(format!("atlas-lan-test-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&directory).unwrap();
            Lan::new(directory, Default::default(), Default::default())
        }
        let host = instance();
        let viewer = instance();
        assert!(host.command("lan_state", json!({})).is_err());
        let host_token = host
            .command("lan_unlock", json!({"password":"test-password"}))
            .unwrap()["token"]
            .clone();
        host.command(
            "lan_save",
            json!({"token":host_token,"name":"Офис","enabled":true}),
        )
        .unwrap();
        let viewer_token = viewer
            .command("lan_unlock", json!({"password":"test-password"}))
            .unwrap()["token"]
            .clone();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let remote = host.clone();
        let worker = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            remote.serve(&mut stream).unwrap();
        });
        let report = viewer
            .fetch_from(&json!({"address":"127.0.0.1"}), addr)
            .unwrap();
        worker.join().unwrap();
        assert_eq!(report["name"], "Офис");
        let id = report["deviceId"].clone();
        viewer
            .command(
                "lan_rename",
                json!({"token":viewer_token,"id":id,"name":"Бухгалтерия"}),
            )
            .unwrap();
        let restarted = Lan::new(
            viewer.directory.clone(),
            Default::default(),
            Default::default(),
        );
        assert!(restarted
            .command("lan_state", json!({"token":viewer_token}))
            .is_err());
        let token = restarted
            .command("lan_unlock", json!({"password":"test-password"}))
            .unwrap()["token"]
            .clone();
        let state = restarted
            .command("lan_state", json!({"token":token}))
            .unwrap();
        assert_eq!(state["peers"][0]["name"], "Бухгалтерия");
        assert_eq!(state["peers"][0]["id"], id);
        assert_eq!(
            restarted
                .command("lan_cached", json!({"token":token,"id":id}))
                .unwrap()["deviceId"],
            id
        );
        restarted
            .command("lan_lock", json!({"token":token}))
            .unwrap();
        assert!(restarted
            .command("lan_state", json!({"token":token}))
            .is_err());
        let bytes = std::fs::read(viewer.directory.join("lan-diagnostics.bin")).unwrap();
        assert!(!bytes
            .windows("Бухгалтерия".len())
            .any(|w| w == "Бухгалтерия".as_bytes()));
        std::fs::remove_dir_all(&viewer.directory).unwrap();
        std::fs::remove_dir_all(&host.directory).unwrap();
    }
    #[test]
    fn replay_and_oversized_frames_are_rejected() {
        let host = Lan::new(std::env::temp_dir(), Default::default(), Default::default());
        let key = [9; 32];
        {
            let mut s = host.state.lock().unwrap();
            s.config.enabled = true;
            s.config.key = Some(key);
        }
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let packet=seal(&key,&json!({"id":uuid::Uuid::new_v4().to_string(),"at":crate::model::now(),"action":"status"}),REQUEST_AAD).unwrap();
        let worker = std::thread::spawn(move || {
            for i in 0..3 {
                let (mut stream, _) = listener.accept().unwrap();
                let r = host.serve(&mut stream);
                if i == 0 {
                    assert!(r.is_ok());
                } else {
                    assert!(r.is_err());
                }
            }
        });
        {
            let mut s = TcpStream::connect(addr).unwrap();
            write_frame(&mut s, &packet).unwrap();
            assert!(open(&key, &read_frame(&mut s, LIMIT + 28).unwrap(), RESPONSE_AAD).is_ok());
        }
        {
            let mut s = TcpStream::connect(addr).unwrap();
            write_frame(&mut s, &packet).unwrap();
            assert!(read_frame(&mut s, LIMIT + 28).is_err());
        }
        {
            let mut s = TcpStream::connect(addr).unwrap();
            s.write_all(&5000u32.to_be_bytes()).unwrap();
            assert!(read_frame(&mut s, LIMIT + 28).is_err());
        }
        worker.join().unwrap();
    }
}
