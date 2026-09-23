mod applications;
mod background_probe;
mod broker;
mod cancellation;
mod config;
mod core;
mod country;
#[path = "network_diagnostics.rs"]
mod diagnostics;
mod job;
mod lan_policy;
mod latency;
#[cfg(test)]
mod auto_recovery;
mod resilient_selection;
mod model;
mod network_guard;
mod portable;
mod published_state;
mod process_stop;
mod query_jobs;
mod query_admission;
mod rule_probe;
mod rules;
mod service;
mod session_cleanup;
mod shutdown;
mod storage;
mod subscriptions;
mod support_report;
mod support_probes;
mod incident_history;
mod interface_evidence;
mod lan_diagnostics;
mod lan_discovery;
mod windows;
use model::*;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tauri::{Emitter, Manager};
use tauri_plugin_autostart::ManagerExt;
struct App {
    revision: u64,
    published: published_state::PublishedState,
    diagnostic_access: support_report::Access,
    cancellation: cancellation::Cancellation,
    settings: Settings,
    store: storage::Store,
    core: core::Core,
    status: String,
    error: Option<String>,
    logs: Vec<Value>,
    reconnect: Arc<std::sync::atomic::AtomicBool>,
}
impl App {
    fn log(&mut self, level: &str, message: &str) {
        incident_history::record("application_event", json!({"level":level,"message":message}), &[]);
        self.logs
            .push(json!({"time":model::now(),"level":level,"message":message}));
        if self.logs.len() > 1000 {
            self.logs.remove(0);
        }
    }
    fn snapshot(&mut self) -> Value {
        let running = self.core.running();
        self.diagnostic_access.update(self.revision,&self.settings,self.core.client());
        let mut s = self.settings.clone();
        for group in &mut s.groups {
            for rule in &mut group.rules {
                if let Ok(normalized) = rules::normalize(rule) {
                    *rule = normalized;
                }
            }
        }
        for sub in &mut s.subscriptions {
            for p in &mut sub.servers {
                *p = json!({"name":p["name"],"type":p["type"],"country":country::detect(p)});
            }
        }
        let value = json!({"settings":s,"status":self.status,"running":running,"guardActive":self.core.guard_active(),"error":self.error,"duration":self.core.started.map(|t|t.elapsed().as_secs()).unwrap_or(0),"logs":self.logs});
        let mut value = value;
        value["revision"] = json!(self.revision);
        self.published.set(value.clone());
        value
    }
    fn save(&mut self, mut next: Settings) -> Result<(), String> {
        next.mode = "tun".into();
        if next.tun_stack != self.settings.tun_stack && self.core.running() {
            return Err("Перед изменением сетевого стека отключите VPN".into());
        }
        rules::compile(&next)?;
        if !["light", "dark", "system"].contains(&next.theme.as_str())
            || next.startup.delay_seconds > 300
            || !(model::MIN_AUTO_TEST_INTERVAL_SECONDS..=model::MAX_AUTO_TEST_INTERVAL_SECONDS)
                .contains(&next.auto_test_interval_seconds)
            || !["system", "tun"].contains(&next.mode.as_str())
        {
            return Err("Некорректные настройки темы, режима или интервала".into());
        }
        if next.mode != self.settings.mode && self.core.running() {
            return Err("Перед изменением режима отключите соединение".into());
        }
        let same_config = config::same_network_config(&self.settings, &next);
        let selection_only = same_config && next.selected != self.settings.selected;
        if !next.subscriptions.is_empty() {
            if selection_only {
                if !["AUTO", "FAILOVER"].contains(&next.selected.as_str())
                    && !next.servers().iter().any(|p| p["name"] == next.selected)
                {
                    return Err("Выбранный сервер отсутствует в подписках".into());
                }
                if self.core.running() {
                    self.core.select(&next.selected)?;
                }
            } else if !same_config {
                self.core.apply(&next)?;
            }
        }
        let previous = self.settings.clone();
        if let Err(e) = self.store.save(&next) {
            if self.core.running() {
                if selection_only {
                    let _ = self.core.select(&previous.selected);
                } else if !same_config {
                    let _ = self.core.apply(&previous);
                }
            }
            return Err(e);
        }
        self.settings = next;
        self.revision = self.revision.wrapping_add(1);
        Ok(())
    }
    fn connect(&mut self) -> Result<(), String> {
        if self.status == "Connected" && self.core.running() {
            return Ok(());
        }
        self.status = "Connecting".into();
        self.revision = self.revision.wrapping_add(1);
        self.error = None;
        self.snapshot();
        self.core.continue_running = Some(self.cancellation.begin());
        if !self.reconnect.load(std::sync::atomic::Ordering::SeqCst) {
            self.status = "Disconnected".into();
            self.core.continue_running = None;
            return Err("Подключение отменено".into());
        }
        self.settings.mode = "tun".into();
        let result = windows::restore(&self.core.directory.join("proxy-restore.json"))
            .and_then(|_| self.core.start(&self.settings));
        match result {
            Ok(()) => {
                if !self.reconnect.load(std::sync::atomic::Ordering::SeqCst) {
                    self.disconnect()?;
                    return Err("Подключение отменено".into());
                }
                self.settings.was_connected = true;
                if let Err(e) = self.store.save(&self.settings) {
                    let _ = windows::restore(&self.core.directory.join("proxy-restore.json"));
                    self.status = "Error".into();
                    self.error = Some(e.clone());
                    let _ = self.core.stop();
                    return Err(e);
                }
                self.status = "Connected".into();
                self.log(
                    "INFO",
                    if self.settings.mode == "tun" {
                        "Подключён режим всей системы (TUN)"
                    } else {
                        "Включён системный прокси"
                    },
                );
                Ok(())
            }
            Err(e) => {
                let _ = self.core.stop();
                self.core.continue_running = None;
                let _ = windows::restore(&self.core.directory.join("proxy-restore.json"));
                self.status = if self.reconnect.load(std::sync::atomic::Ordering::SeqCst) { "Error" } else { "Disconnected" }.into();
                self.error = Some(e.clone());
                self.log("ERROR", &e);
                Err(e)
            }
        }
    }
    fn disconnect(&mut self) -> Result<(), String> {
        self.revision = self.revision.wrapping_add(1);
        self.reconnect
            .store(false, std::sync::atomic::Ordering::SeqCst);
        let stopped = self.core.stop();
        self.core.continue_running = None;
        let restored = windows::restore(&self.core.directory.join("proxy-restore.json"));
        self.status = "Disconnected".into();
        self.settings.was_connected = false;
        self.store.save(&self.settings)?;
        self.log(
            "INFO",
            "Соединение отключено; сетевые настройки восстановлены",
        );
        stopped?;
        restored?;
        Ok(())
    }
    fn prepare_restart(&mut self) -> Result<(), String> {
        let was_connected = self.status == "Connected" && self.core.running();
        windows::restore(&self.core.directory.join("proxy-restore.json"))?;
        self.core.stop()?;
        self.status = "Disconnected".into();
        self.settings.was_connected = was_connected;
        self.store.save(&self.settings)?;
        self.log(
            "INFO",
            "Приложение перезапускается; сетевые настройки восстановлены",
        );
        Ok(())
    }
    fn dispatch(
        &mut self,
        app: &tauri::AppHandle,
        action: &str,
        p: Value,
    ) -> Result<Value, String> {
        match action {
            "snapshot" => return Ok(self.snapshot()),
            "applications" => return Ok(json!(applications::list())),
            "rules_export" => return Ok(json!(portable::export(&self.settings)?)),
            "rules_open_file" => {
                let path = rfd::FileDialog::new()
                    .set_title("Открыть правила")
                    .add_filter("YAML", &["yaml", "yml"])
                    .pick_file();
                return match path {
                    Some(path) => {
                        if std::fs::metadata(&path)
                            .map_err(|_| "Не удалось прочитать файл")?
                            .len()
                            > 4 * 1024 * 1024
                        {
                            return Err("Файл превышает 4 МБ".into());
                        }
                        Ok(json!(std::fs::read_to_string(path)
                            .map_err(|_| "Не удалось прочитать YAML")?))
                    }
                    None => Ok(Value::Null),
                };
            }
            "rules_save_file" => {
                let yaml = portable::export(&self.settings)?;
                if let Some(path) = rfd::FileDialog::new()
                    .set_title("Сохранить правила")
                    .set_file_name("atlas-rules.yaml")
                    .add_filter("YAML", &["yaml"])
                    .save_file()
                {
                    std::fs::write(path, yaml).map_err(|_| "Не удалось сохранить YAML")?;
                }
                return Ok(Value::Null);
            }
            "choose_apps" => {
                return Ok(json!(rfd::FileDialog::new()
                    .set_title("Выберите приложения")
                    .add_filter("Приложения", &["exe"])
                    .pick_files()
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|p| p.file_name().map(|v| v.to_string_lossy().to_string()))
                    .collect::<Vec<_>>()))
            }
            "rules_preview" | "rules_apply" => {
                let imported = portable::parse(p["text"].as_str().ok_or("Нет YAML")?)?;
                let replace = p["replace"].as_bool().unwrap_or(false);
                let choices =
                    serde_json::from_value(p.get("choices").cloned().unwrap_or(json!({})))
                        .map_err(|_| "Некорректное разрешение конфликтов")?;
                let preview = portable::preview(
                    if replace { &[] } else { &self.settings.groups },
                    imported,
                    &choices,
                )?;
                if action == "rules_preview" {
                    return serde_json::to_value(preview).map_err(|e| e.to_string());
                }
                if !preview.conflicts.is_empty() {
                    return Err("Выберите маршрут для каждого конфликта".into());
                }
                let mut next = self.settings.clone();
                next.groups = preview.groups;
                if let Some(route) = preview.default_route {
                    next.default_route = route;
                }
                self.save(next)?;
            }
            "connect" => self.connect()?,
            "disconnect" => self.disconnect()?,
            "save" => {
                let mut next: Settings =
                    serde_json::from_value(p).map_err(|_| "Некорректные настройки")?;
                next.subscriptions = self.settings.subscriptions.clone();
                let enabled = next.startup.launch_with_windows;
                let previous = self.settings.startup.launch_with_windows;
                self.save(next)?;
                let registered = app.autolaunch().is_enabled().map_err(|e| e.to_string())?;
                let result = if registered == enabled {
                    Ok(())
                } else if enabled {
                    app.autolaunch().enable()
                } else {
                    app.autolaunch().disable()
                };
                if let Err(e) = result {
                    let mut back = self.settings.clone();
                    back.startup.launch_with_windows = previous;
                    self.save(back)?;
                    return Err(format!("Автозапуск Windows: {e}"));
                }
            }
            "subscription_add" | "subscription_refresh" => {
                let id = p["id"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                let entry = keyring::Entry::new("AtlasVPN", &id)
                    .map_err(|_| "Диспетчер учётных данных Windows недоступен")?;
                let url = if action == "subscription_add" {
                    p["url"].as_str().ok_or("Введите HTTPS URL")?.to_owned()
                } else {
                    entry
                        .get_password()
                        .map_err(|_| "Ссылка подписки отсутствует в хранилище Windows")?
                };
                let mut nodes = match subscriptions::download(&url, self.core.running()) {
                    Ok(n) => n,
                    Err(e) => {
                        if let Some(s) = self.settings.subscriptions.iter_mut().find(|s| s.id == id)
                        {
                            s.error = Some(e.clone());
                            self.store.save(&self.settings)?;
                        }
                        return Err(e);
                    }
                };
                for (i, node) in nodes.iter_mut().enumerate() {
                    let name = node["name"].as_str().unwrap_or("Сервер");
                    node["name"] = json!(format!("{} · {}-{}", name, &id[..8], i + 1));
                }
                let mut next = self.settings.clone();
                let old = next.subscriptions.iter().position(|s| s.id == id);
                let name = p["name"]
                    .as_str()
                    .map(str::to_owned)
                    .or_else(|| old.map(|i| next.subscriptions[i].name.clone()))
                    .unwrap_or("Подписка".into());
                let sub = Subscription {
                    id: id.clone(),
                    name,
                    masked_url: subscriptions::mask(&url),
                    updated_at: now(),
                    error: None,
                    servers: nodes,
                };
                if let Some(i) = old {
                    next.subscriptions[i] = sub
                } else {
                    next.subscriptions.push(sub)
                }
                if !next.servers().iter().any(|s| s["name"] == next.selected)
                    && !["AUTO", "FAILOVER"].contains(&next.selected.as_str())
                {
                    next.selected = "AUTO".into()
                };
                self.core.validate(&next)?;
                if action == "subscription_add" {
                    entry
                        .set_password(&url)
                        .map_err(|_| "Не удалось сохранить ссылку в хранилище Windows")?
                }
                self.save(next)?;
                self.log("INFO", "Подписка обновлена и проверена Mihomo");
            }
            "subscription_delete" => {
                let id = p["id"].as_str().ok_or("Нет ID")?;
                if self.core.running() {
                    return Err("Отключитесь перед удалением подписки".into());
                }
                let mut next = self.settings.clone();
                next.subscriptions.retain(|s| s.id != id);
                next.selected = "AUTO".into();
                self.save(next)?;
                let _ = keyring::Entry::new("AtlasVPN", id).and_then(|e| e.delete_credential());
            }
            "import_preview" => {
                return serde_json::to_value(rules::import(p["text"].as_str().ok_or("Нет YAML")?)?)
                    .map_err(|e| e.to_string())
            }
            "import_apply" => {
                let imported = rules::import(p["text"].as_str().ok_or("Нет YAML")?)?;
                let mut next = self.settings.clone();
                next.groups.extend(imported.groups);
                if let Some(r) = imported.default_route {
                    next.default_route = r
                }
                self.save(next)?;
            }
            "rollback" => {
                let s = self.store.previous()?;
                self.save(s)?;
                self.log("INFO", "Настройки восстановлены из резервной копии");
            }
            "connections" => return self.core.api("GET", "/connections", None),
            "close_connection" => {
                let id = p["id"].as_str().ok_or("Нет ID соединения")?;
                let encoded: String = url::form_urlencoded::byte_serialize(id.as_bytes()).collect();
                return self
                    .core
                    .api("DELETE", &format!("/connections/{encoded}"), None);
            }
            "latency" => {
                let name = p["name"].as_str().ok_or("Нет сервера")?;
                return serde_json::to_value(latency::test(self.core.client(), name))
                    .map_err(|e| e.to_string());
            }
            "proxies" => return self.core.api("GET", "/proxies", None),
            "public_ip" => {
                if !self.core.running() {
                    return Err("Сначала подключитесь".into());
                }
                let r = reqwest::blocking::Client::builder()
                    .proxy(
                        reqwest::Proxy::all("http://127.0.0.1:17890").map_err(|e| e.to_string())?,
                    )
                    .timeout(std::time::Duration::from_secs(8))
                    .build()
                    .map_err(|e| e.to_string())?
                    .get("https://api.ipify.org?format=json")
                    .send()
                    .map_err(|_| "Не удалось определить IP")?;
                return r.json().map_err(|_| "Не удалось прочитать IP".into());
            }
            "clear_logs" => self.logs.clear(),
            "export" => {
                let mut s = self.settings.clone();
                s.subscriptions.clear();
                s.selected = "AUTO".into();
                s.was_connected = false;
                return Ok(
                    json!({"version":1,"settings":s,"note":"Subscription URLs and server credentials excluded"}),
                );
            }
            _ => return Err("Неизвестная операция".into()),
        };
        Ok(self.snapshot())
    }
}
type Shared = Arc<Mutex<App>>;
struct ConnectionIntent(Arc<std::sync::atomic::AtomicBool>);
type ShuttingDown = Arc<std::sync::atomic::AtomicBool>;
pub fn cleanup() -> Result<(), String> {
    let dir = std::path::PathBuf::from(
        std::env::var_os("LOCALAPPDATA").ok_or("LOCALAPPDATA is unavailable")?,
    )
    .join("net.atlasvpn.desktop");
    windows::restore(&dir.join("proxy-restore.json"))?;
    // This command runs elevated from the uninstaller. Cleanup must never launch a VPN.
    network_guard::clear()?;
    let _ = std::fs::remove_file(dir.join("tun-guard.active"));
    use winreg::{enums::*, RegKey};
    for path in [
        r"Software\Microsoft\Windows\CurrentVersion\Run",
        r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run",
    ] {
        if let Ok(key) =
            RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(path, KEY_SET_VALUE)
        {
            let _ = key.delete_value("Atlas");
        }
    }
    Ok(())
}
#[tauri::command]
async fn request(
    app: tauri::AppHandle,
    state: tauri::State<'_, Shared>,
    action: String,
    payload: Option<Value>,
) -> Result<Value, String> {
    let shared = state.inner().clone();
    if action.starts_with("lan_") {
        let lan=app.state::<lan_diagnostics::Lan>().inner().clone();
        return tauri::async_runtime::spawn_blocking(move || lan.command(&action,payload.unwrap_or(Value::Null)))
            .await.map_err(|e|e.to_string())?;
    }
    if action == "snapshot" {
        return Ok(app.state::<published_state::PublishedState>().get());
    }
    if action == "pool_probe" {
        let (client, revision) = {
            let a = shared.try_lock().map_err(|_| "Atlas занят; проверка отложена")?;
            if a.status != "Connected" { return Err("Atlas не подключён".into()); }
            (a.core.client(), a.revision)
        };
        return tauri::async_runtime::spawn_blocking(move || {
            client.api("GET", "/version", None)?;
            let mut delays = serde_json::Map::new();
            for endpoint in latency::ENDPOINTS {
                let url: String = url::form_urlencoded::byte_serialize(endpoint.as_bytes()).collect();
                match client.api("GET", &format!("/group/AUTO/delay?timeout=5000&expected=204&url={url}"), None) {
                    Ok(value) => {
                        let health = client.api("GET","/proxies",None)?;
                        if let Some(values) = value.as_object() { delays.extend(values.iter()
                            .filter(|(name,_)|health["proxies"][name.as_str()]["extra"][endpoint]["alive"] == true)
                            .map(|(name,value)|(name.clone(),value.clone()))); }
                        if !delays.is_empty() { break; }
                    }
                    Err(error) if error == "Mihomo API: HTTP 504" => {}
                    Err(error) => return Err(error),
                }
            }
            client.api("GET", "/version", None)?;
            Ok(json!({"revision":revision,"delays":delays}))
        }).await.map_err(|e| e.to_string())?;
    }
    if action == "pool_refresh" {
        let expected = payload.as_ref().and_then(|v| v["revision"].as_u64()).ok_or("Нет ревизии")?;
        let mut next = {
            let a = shared.try_lock().map_err(|_| "Atlas занят; восстановление отложено")?;
            if a.revision != expected || a.status != "Connected" { return Err("Состояние изменилось; восстановление отменено".into()); }
            a.settings.clone()
        };
        return tauri::async_runtime::spawn_blocking(move || {
            // Download outside the app mutex; status and Disconnect remain responsive.
            let mut failures = Vec::new();
            for sub in &mut next.subscriptions {
                let result = keyring::Entry::new("AtlasVPN", &sub.id)
                    .map_err(|_| "Хранилище подписки недоступно".to_string())
                    .and_then(|entry| entry.get_password().map_err(|_| "Ссылка подписки недоступна".to_string()))
                    .and_then(|url| subscriptions::download(&url, true));
                match result {
                    Ok(mut nodes) => {
                        for (index, node) in nodes.iter_mut().enumerate() {
                            node["name"] = json!(format!("{} · {}-{}", node["name"].as_str().unwrap_or("Сервер"), sub.id.chars().take(8).collect::<String>(), index + 1));
                        }
                        sub.servers = nodes; sub.updated_at = model::now(); sub.error = None;
                    }
                    Err(error) => { sub.error = Some(error); failures.push(sub.name.clone()); }
                }
            }
            let mut a = shared.lock().map_err(|_| "Состояние Atlas недоступно")?;
            if a.revision != expected || a.status != "Connected" || !a.reconnect.load(std::sync::atomic::Ordering::SeqCst) {
                return Err("Состояние изменилось; восстановление отменено".into());
            }
            if !["AUTO","FAILOVER"].contains(&next.selected.as_str()) && !next.servers().iter().any(|n|n["name"] == next.selected) {
                return Err("Выбранный вручную сервер исчез из подписки. Автоматическая замена отменена; выберите сервер явно.".into());
            }
            a.save(next)?;
            a.log("INFO", "Восстановление: обновление подписок завершено; режим выбора сохранён");
            Ok(json!({"snapshot":a.snapshot(),"failedSubscriptions":failures}))
        }).await.map_err(|e| e.to_string())?;
    }
    if action == "connect" || action == "disconnect" {
        app.state::<ConnectionIntent>()
            .0
            .store(action == "connect", std::sync::atomic::Ordering::SeqCst);
        if action == "disconnect" {
            app.state::<cancellation::Cancellation>().cancel();
            if shared.try_lock().is_err() {
                let published = app.state::<published_state::PublishedState>();
                let mut value = published.get();
                value["status"] = json!("Disconnecting");
                published.set(value.clone());
            }
        }
    }
    if app
        .state::<ShuttingDown>()
        .load(std::sync::atomic::Ordering::SeqCst)
    {
        app.state::<ConnectionIntent>().0.store(false, std::sync::atomic::Ordering::SeqCst);
        return Err("Atlas завершает работу".into());
    }
    if action == "diagnostics_export" {
        // Export must remain usable while connect/apply holds the application lock.
        let (context, client, secrets) = match shared.try_lock() {
            Ok(a) => {
                let mut secrets = Vec::new();
                support_report::collect_secrets(&serde_json::to_value(&a.settings).map_err(|e| e.to_string())?, &mut secrets);
                let context = json!({"capturedAt":model::now(),"revision":a.revision,"status":a.status,"mode":a.settings.mode,
                    "routingMode":a.settings.routing_mode,"tunStack":a.settings.tun_stack,
                    "selected":a.settings.selected,"autoTestIntervalSeconds":a.settings.auto_test_interval_seconds,
                    "error":a.error,"logs":a.logs,
                    "connectionConfiguration":support_report::configuration_evidence(&a.settings),
                    "uiPoolHealth":payload.as_ref().and_then(|v|v.get("poolHealth"))});
                (serde_json::to_string_pretty(&context).map_err(|e| e.to_string())?, Some(a.core.client()), secrets)
            }
            Err(_) => match app.state::<support_report::Access>().get() {
                Some(cached) => (json!({"stateRead":"cached: application lock busy","capturedAt":cached.captured_at,
                    "revision":cached.revision,"connectionConfiguration":cached.configuration,
                    "publishedState":app.state::<published_state::PublishedState>().get()}).to_string(),Some(cached.client),cached.secrets),
                None => ("Atlas занят: кэш диагностической сессии недоступен. Снимок Windows собирается независимо.".into(),None,Vec::new()),
            },
        };
        let diagnostic_access = app.state::<support_report::Access>().inner().clone();
        return tauri::async_runtime::spawn_blocking(move || {
            support_report::save(context, client, secrets, diagnostic_access).map(|saved| json!({"saved":saved}))
        }).await.map_err(|e| e.to_string())?;
    }
    if action == "connections" || action == "proxies" {
        let client = shared
            .try_lock()
            .map_err(|_| "Atlas занят, повторите операцию")?
            .core
            .client();
        return tauri::async_runtime::spawn_blocking(move || {
            client.api(
                "GET",
                if action == "connections" {
                    "/connections"
                } else {
                    "/proxies"
                },
                None,
            )
        })
        .await
        .map_err(|e| e.to_string())?;
    }
    if action == "public_ip" {
        if !shared
            .try_lock()
            .map_err(|_| "Atlas занят, повторите операцию")?
            .core
            .running()
        {
            return Err("Сначала подключитесь".into());
        }
        return tauri::async_runtime::spawn_blocking(move || {
            reqwest::blocking::Client::builder()
                .proxy(reqwest::Proxy::all("http://127.0.0.1:17890").map_err(|e| e.to_string())?)
                .timeout(std::time::Duration::from_secs(8))
                .build()
                .map_err(|e| e.to_string())?
                .get("https://api.ipify.org?format=json")
                .send()
                .map_err(|_| "Не удалось определить IP")?
                .json::<Value>()
                .map_err(|_| "Не удалось прочитать IP".into())
        })
        .await
        .map_err(|e| e.to_string())?;
    }
    if action == "applications" {
        return tauri::async_runtime::spawn_blocking(move || Ok(json!(applications::list())))
            .await
            .map_err(|e| e.to_string())?;
    }
    if action == "site_check" {
        let id = payload
            .as_ref()
            .and_then(|p| p["id"].as_str())
            .ok_or("Нет сервиса")?;
        let url = match id {
            "chatgpt" => "https://chatgpt.com",
            "grok" => "https://grok.com",
            "gemini" => "https://gemini.google.com",
            "youtube" => "https://www.youtube.com",
            "telegram" => "https://web.telegram.org",
            _ => return Err("Неизвестный сервис".into()),
        };
        {
            let mut a = shared
                .try_lock()
                .map_err(|_| "Atlas занят, повторите операцию")?;
            if !a.core.running() {
                return Err("Сначала подключите Atlas".into());
            }
        }
        return tauri::async_runtime::spawn_blocking(move || {
            let client = reqwest::blocking::Client::builder()
                .proxy(reqwest::Proxy::all("http://127.0.0.1:17890").map_err(|e| e.to_string())?)
                .timeout(std::time::Duration::from_secs(10))
                .redirect(reqwest::redirect::Policy::limited(5))
                .build().map_err(|e| e.to_string())?;
            let start = std::time::Instant::now();
            match client.get(url).send() {
                Ok(response) => Ok(json!({"ok":response.status().is_success(),"ms":start.elapsed().as_millis(),"status":response.status().as_u16()})),
                Err(_) => Ok(json!({"ok":false,"ms":null,"status":null})),
            }
        }).await.map_err(|e| e.to_string())?;
    }
    if action == "rule_probe" {
        let payload = payload.ok_or("Нет правила")?;
        let key = payload["key"].as_str().ok_or("Нет ключа правила")?;
        let (settings, client, rule, route) = {
            let a = shared
                .try_lock()
                .map_err(|_| "Atlas занят, повторите операцию")?;
            if a.settings.routing_mode != RoutingMode::Rule {
                return Err("Для проверки правил включите режим «Правила» на Главной".into());
            }
            let (rule, route) = a
                .settings
                .groups
                .iter()
                .filter(|g| g.enabled)
                .flat_map(|g| g.rules.iter().map(move |r| (r, &g.route)))
                .find(|(r, _)| rules::key(r).is_ok_and(|k| k == key))
                .ok_or("Правило отсутствует или отключено")?;
            (
                a.settings.clone(),
                a.core.client(),
                rule.clone(),
                route.clone(),
            )
        };
        return tauri::async_runtime::spawn_blocking(move || {
            Ok(json!(rule_probe::run(&settings, client, &rule, &route)))
        })
        .await
        .map_err(|e| e.to_string())?;
    }
    if action == "diagnostics" {
        let (settings, client) = {
            let a = shared
                .try_lock()
                .map_err(|_| "Atlas занят, повторите операцию")?;
            (a.settings.clone(), a.core.client())
        };
        return tauri::async_runtime::spawn_blocking(move || {
            Ok(json!(diagnostics::run(&settings, client)))
        })
        .await
        .map_err(|e| e.to_string())?;
    }
    if action == "protection_status" {
        let (settings, client) = {
            let mut a = shared
                .try_lock()
                .map_err(|_| "Atlas занят, повторите операцию")?;
            if !a.core.running() || a.status != "Connected" {
                return Ok(json!({
                    "secure": false,
                    "detail": "Atlas не подключён.",
                    "checkedAt": model::now()
                }));
            }
            (a.settings.clone(), a.core.client())
        };
        return tauri::async_runtime::spawn_blocking(move || {
            Ok(diagnostics::protection_status(&settings, client))
        })
        .await
        .map_err(|e| e.to_string())?;
    }
    if action == "latency_batch" {
        let (client, names, revision) = {
            let a = shared.try_lock().map_err(|_| "Atlas занят, повторите проверку")?;
            if a.status != "Connected" { return Err("Atlas не подключён".into()); }
            (a.core.client(), a.settings.servers().iter()
                .filter_map(|node| node["name"].as_str().map(str::to_owned)).collect::<Vec<_>>(), a.revision)
        };
        return tauri::async_runtime::spawn_blocking(move || {
            latency::batch(client, &names).map(|results| json!({"revision":revision,"results":results}))
        }).await.map_err(|e| e.to_string())?;
    }
    if action == "latency" {
        let name = payload
            .as_ref()
            .and_then(|p| p["name"].as_str())
            .ok_or("Не выбран сервер")?
            .to_owned();
        let client = {
            let a = shared
                .try_lock()
                .map_err(|_| "Atlas занят, повторите операцию")?;
            if !a.settings.servers().iter().any(|p| p["name"] == name) {
                return Err("Сервер отсутствует в подписках".into());
            }
            a.core.client()
        };
        return tauri::async_runtime::spawn_blocking(move || {
            serde_json::to_value(latency::test(client, &name)).map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| e.to_string())?;
    }
    tauri::async_runtime::spawn_blocking(move || {
        let mut a = if action == "disconnect" {
            // The caller awaits actual cleanup (the updater relies on this).
            // Cancellation above already interrupted the pending network operation.
            shared.lock().map_err(|_| "Состояние Atlas недоступно")?
        } else {
            shared.try_lock().map_err(|_| "Atlas занят, повторите операцию")?
        };
        if app.state::<ShuttingDown>().load(std::sync::atomic::Ordering::SeqCst) {
            return Err("Atlas завершает работу".into());
        }
        let result = a.dispatch(&app, &action, payload.unwrap_or(Value::Null));
        a.snapshot();
        result
    })
    .await
    .map_err(|e| e.to_string())?
}
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .plugin(
            tauri_plugin_autostart::Builder::new()
                .app_name("Atlas")
                .args(["--autostart"])
                .build(),
        )
        .setup(|app| {
            let dir = app.path().app_local_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            windows::restore(&dir.join("proxy-restore.json")).map_err(std::io::Error::other)?;
            let store =
                storage::Store::open(&dir.join("atlas.db")).map_err(std::io::Error::other)?;
            let mut settings = store.load().map_err(std::io::Error::other)?;
            settings.mode = "tun".into();
            // Repair stale startup registration to match the saved user preference.
            if !settings.startup.launch_with_windows {
                let _ = app.autolaunch().disable();
                if std::env::args().any(|arg| arg == "--autostart") {
                    app.handle().exit(0);
                    return Ok(());
                }
            }
            let binary = app.path().resource_dir()?.join("resources/mihomo.exe");
            let history_path = dir.join("incident-history.ndjson");
            incident_history::load(&history_path);
            incident_history::record("application_start", json!({"version":env!("CARGO_PKG_VERSION")}), &[]);
            let core = core::Core::new(binary, dir.clone());
            let auto = settings.startup.auto_connect
                || (settings.startup.restore_connection && settings.was_connected);
            let delay = settings.startup.delay_seconds.min(300);
            if settings.startup.start_in_tray {
                if let Some(w) = app.get_webview_window("main") {
                    w.hide()?
                }
            }
            let intent = Arc::new(std::sync::atomic::AtomicBool::new(auto));
            app.manage(ConnectionIntent(intent.clone()));
            let published = published_state::PublishedState::default();
            app.manage(published.clone());
            let diagnostic_access = support_report::Access::default();
            app.manage(diagnostic_access.clone());
            let cancellation = cancellation::Cancellation::default();
            app.manage(cancellation.clone());
            let shared = Arc::new(Mutex::new(App {
                revision: 0,
                published,
                diagnostic_access,
                cancellation,
                settings,
                store,
                core,
                status: "Disconnected".into(),
                error: None,
                logs: vec![],
                reconnect: intent.clone(),
            }));
            shared.lock().unwrap().snapshot();
            app.manage(shared.clone());
            let shutdown: ShuttingDown = Arc::new(std::sync::atomic::AtomicBool::new(false));
            app.manage(shutdown.clone());
            let lan=lan_diagnostics::Lan::new(dir.clone(),app.state::<support_report::Access>().inner().clone(),
                app.state::<published_state::PublishedState>().inner().clone());
            lan.start(shutdown.clone());
            app.manage(lan);
            {
                let shared = shared.clone();
                let shutdown = shutdown.clone();
                std::thread::spawn(move || {
                    let mut recorder = support_report::Recorder::default();
                    let incident_busy = Arc::new(std::sync::atomic::AtomicBool::new(false));
                    let mut last_incident: Option<std::time::Instant> = None;
                    while !shutdown.load(std::sync::atomic::Ordering::SeqCst) {
                        let captured = shared.try_lock().ok().map(|a| {
                            let mut secrets = Vec::new();
                            if let Ok(settings) = serde_json::to_value(&a.settings) { support_report::collect_secrets(&settings,&mut secrets); }
                            (a.status.clone(),a.revision,a.error.clone(),a.settings.selected.clone(),a.core.client(),secrets)
                        });
                        if let Some((status,revision,error,selected,client,secrets)) = captured {
                            let (evidence, incident) = if status == "Connected" {
                                recorder.sample(&client)
                            } else { (json!({"skipped":"No connected session"}),false) };
                            if incident {
                                let allowed = last_incident.is_none_or(|t|t.elapsed() >= std::time::Duration::from_secs(60));
                                if allowed && !incident_busy.swap(true,std::sync::atomic::Ordering::SeqCst) {
                                    last_incident = Some(std::time::Instant::now());
                                    let busy = incident_busy.clone(); let c = client.clone(); let secrets = secrets.clone();
                                    incident_history::record("incident_capture_started",json!({"revision":revision}),&[]);
                                    std::thread::spawn(move || {
                                        support_report::automatic_incident(c,secrets,revision);
                                        busy.store(false,std::sync::atomic::Ordering::SeqCst);
                                    });
                                } else {
                                    incident_history::record("incident_capture_coalesced",json!({"revision":revision,"reason":"Capture active or 60-second cooldown; new errors retained in sample"}),&[]);
                                }
                            }
                            incident_history::record("passive_sample",json!({"status":status,"revision":revision,
                                "error":error,"selected":selected,"core":evidence}),&secrets);
                        } else {
                            incident_history::record("sample_skipped",json!({"reason":"Application state lock busy"}),&[]);
                        }
                        if let Err(e) = incident_history::flush(&history_path) {
                            incident_history::record("history_write_error",json!({"error":e}),&[]);
                        }
                        for _ in 0..15 {
                            if shutdown.load(std::sync::atomic::Ordering::SeqCst) { break; }
                            std::thread::sleep(std::time::Duration::from_secs(1));
                        }
                    }
                    let _ = incident_history::flush(&history_path);
                });
            }
            use tauri::menu::{Menu, MenuItem};
            let show = MenuItem::with_id(app, "show", "Открыть Атлас", true, None::<&str>)?;
            let connect = MenuItem::with_id(app, "connect", "Подключить", true, None::<&str>)?;
            let disconnect = MenuItem::with_id(app, "disconnect", "Отключить", true, None::<&str>)?;
            let restart = MenuItem::with_id(app, "restart", "Перезагрузить", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Выйти", true, None::<&str>)?;
            let menu =
                Menu::with_items(app, &[&show, &connect, &disconnect, &restart, &quit])?;
            let tray_icon = app.default_window_icon()
                .ok_or("В сборке отсутствует иконка Atlas")?.clone();
            tauri::tray::TrayIconBuilder::new()
                .icon(tray_icon)
                .tooltip("Atlas VPN")
                .menu(&menu)
                .on_menu_event(|app, event| {
                    if app.state::<ShuttingDown>().load(std::sync::atomic::Ordering::SeqCst) { return; }
                    let id = event.id.as_ref();
                    if id == "show" {
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                    } else {
                        let handle = app.clone();
                        let action = id.to_owned();
                        if action == "connect" || action == "disconnect" || action == "quit" {
                            app.state::<ConnectionIntent>().0.store(action == "connect", std::sync::atomic::Ordering::SeqCst);
                        }
                        if action == "disconnect" || action == "quit" {
                            app.state::<cancellation::Cancellation>().cancel();
                        }
                        if action == "quit" {
                            if app.state::<ShuttingDown>().swap(true, std::sync::atomic::Ordering::SeqCst) { return; }
                            // Never leave Exit queued behind a driver/configuration timeout.
                            // The service watches the parent process and releases its dynamic
                            // filters and owned core when the desktop process exits.
                            shutdown::begin(std::time::Duration::from_secs(8), move || {
                                let state = handle.state::<Shared>();
                                // A poisoned mutex must not prevent resource cleanup.
                                let mut a = state.lock().unwrap_or_else(|e| e.into_inner());
                                let _ = a.disconnect();
                                drop(a);
                                handle.exit(0);
                            }, || std::process::exit(0));
                            return;
                        }
                        tauri::async_runtime::spawn_blocking(move || {
                            let state = handle.state::<Shared>();
                            if let Ok(mut a) = state.lock() {
                                // Work queued before Quit must not reopen the VPN.
                                if handle.state::<ShuttingDown>().load(std::sync::atomic::Ordering::SeqCst) { return; }
                                if action == "restart" {
                                    if a.prepare_restart().is_ok() {
                                        drop(a);
                                        handle.restart()
                                    }
                                } else {
                                    let _ = a.dispatch(&handle, &action, Value::Null);
                                    a.snapshot();
                                }
                            };
                        });
                    }
                })
                .build(app)?;
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                let mut startup_at = auto.then(|| std::time::Instant::now() + std::time::Duration::from_secs(delay));
                let mut failures = 0u32;
                let mut retry_at = std::time::Instant::now();
                loop {
                    std::thread::sleep(std::time::Duration::from_secs(2));
                    if shutdown.load(std::sync::atomic::Ordering::SeqCst) { break; }
                    let Ok(mut a) = shared.try_lock() else { continue; };
                    if startup_at.is_some_and(|at| std::time::Instant::now() >= at) {
                        startup_at = None;
                        if intent.load(std::sync::atomic::Ordering::SeqCst) && a.status == "Disconnected" {
                            let _ = a.connect();
                            retry_at = std::time::Instant::now() + std::time::Duration::from_secs(3);
                        }
                    }
                    if !intent.load(std::sync::atomic::Ordering::SeqCst) && a.status == "Connected" {
                        let _ = a.disconnect();
                        a.snapshot();
                        continue;
                    }
                    if a.status == "Connected" && !a.core.running() {
                        let _ = windows::restore(&a.core.directory.join("proxy-restore.json"));
                        a.status = "Error".into();
                        let message = "Ядро завершилось. Сетевая служба освобождает TUN и временные защитные фильтры; соединение VPN потеряно.";
                        a.error = Some(message.into());
                        a.log("ERROR",message);
                        let _ = handle.emit("core-crashed", ());
                        retry_at = std::time::Instant::now() + std::time::Duration::from_secs(3);
                    }
                    if a.reconnect.load(std::sync::atomic::Ordering::SeqCst) && a.status == "Error" && std::time::Instant::now() >= retry_at {
                        a.log("INFO", "Попытка восстановления VPN после сбоя");
                        let _ = a.core.stop();
                        if a.connect().is_ok() { failures = 0; }
                        else { failures = failures.saturating_add(1); }
                        retry_at = std::time::Instant::now() + std::time::Duration::from_secs(retry_delay(failures));
                    }
                    a.snapshot();
                }
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if !window.state::<ShuttingDown>().load(std::sync::atomic::Ordering::SeqCst) {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![request])
        .run(tauri::generate_context!())
        .expect("Atlas failed to initialize");
}

fn retry_delay(failures: u32) -> u64 {
    (3u64.saturating_mul(1u64 << failures.min(5))).min(60)
}

#[cfg(test)]
mod recovery_tests {
    use super::*;
    #[test]
    fn repeated_outages_back_off_without_overflow_or_busy_loop() {
        assert_eq!(
            (0..7).map(retry_delay).collect::<Vec<_>>(),
            vec![3, 6, 12, 24, 48, 60, 60]
        );
        assert_eq!(retry_delay(u32::MAX), 60);
    }
}

pub fn network_service() -> Result<(), String> {
    service::run()
}

pub fn watch_network_session(pid: u32) -> Result<(), String> {
    session_cleanup::run(pid)
}

/// Exercises the installed service handshake without starting a network core.
pub fn check_network_service() -> Result<(), String> {
    let broker = broker::Broker::launch()?;
    if service::verify_server_pid(std::process::id()).is_ok() {
        return Err("Проверка службы приняла посторонний PID".into());
    }
    let status = broker.call("status", Value::Null)?;
    if status["running"] != false || status["guard"] != false {
        return Err("Проверка ожидала службу без активной VPN-сессии".into());
    }
    Ok(())
}

pub fn install_network_service() -> Result<(), String> {
    service::install()
}

pub fn uninstall_network_service() -> Result<(), String> {
    service::uninstall()
}
