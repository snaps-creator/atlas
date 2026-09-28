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
mod storage;
mod subscriptions;
#[cfg(test)]
mod site_checks;
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
    reads: published_state::ReadState,
    diagnostic_access: support_report::Access,
    cancellation: cancellation::Cancellation,
    settings: Settings,
    store: storage::Store,
    core: core::Core,
    status: String,
    error: Option<String>,
    control_error: Option<String>,
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
        self.reads.set(published_state::ReadSnapshot {
            revision:self.revision, settings:self.settings.clone(), status:self.status.clone(),
            client:self.core.client(), binary:self.core.binary.clone(), directory:self.core.directory.clone(),
        });
        // Only the independent service observer changes connection state.
        // Publishing UI data must never perform network I/O under App's mutex.
        let running = self.status == "Connected";
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
        let value = json!({"settings":s,"status":self.status,"running":running,"guardActive":self.core.guard_active(),"error":self.error,"duration":self.core.started.map(|t|t.elapsed().as_secs()).unwrap_or(0),"logs":self.logs,"buildId":env!("ATLAS_BUILD_ID")});
        let mut value = value;
        value["controlError"] = json!(self.control_error);
        value["revision"] = json!(self.revision);
        self.published.set(value.clone());
        value
    }
    fn install_subscription(&mut self, id: String, url: String, mut nodes: Vec<Value>, label: Option<String>, entry: Option<keyring::Entry>) -> Result<(),String> {
                for (i, node) in nodes.iter_mut().enumerate() {
                    let name = node["name"].as_str().unwrap_or("Сервер");
                    node["name"] = json!(format!("{} · {}-{}", name, &id[..8], i + 1));
                }
                let mut next = self.settings.clone();
                let old = next.subscriptions.iter().position(|s| s.id == id);
                let name = label
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
                if let Some(entry) = entry { entry.set_password(&url).map_err(|_| "Не удалось сохранить ссылку в хранилище Windows")?; }
                self.save(next)?;
                self.log("INFO", "Подписка обновлена и проверена Mihomo");
        Ok(())
    }
    fn save(&mut self, mut next: Settings) -> Result<(), String> {
        next.mode = "tun".into();
        if next.rules_semantics_version < self.settings.rules_semantics_version {
            return Err("Возврат к старой семантике правил требует отдельной миграции".into());
        }
        if next.tun_stack != self.settings.tun_stack && self.core.running() {
            return Err("Перед изменением сетевого стека отключите VPN".into());
        }
        rules::compile(&next)?;
        if !["light", "dark", "system"].contains(&next.theme.as_str())
            || next.startup.delay_seconds > 300
            || !(1..=5000).contains(&next.auto_search_ping_ms)
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
        let mut changed_live = false;
        if !next.subscriptions.is_empty() {
            if selection_only {
                if !["AUTO", "FAILOVER"].contains(&next.selected.as_str())
                    && !next.servers().iter().any(|p| p["name"] == next.selected)
                {
                    return Err("Выбранный сервер отсутствует в подписках".into());
                }
                if self.core.running() {
                    self.core.select(&next.selected)?;
                    changed_live = true;
                }
            } else if !same_config {
                changed_live = self.core.service_reachable();
                self.core.apply(&next)?;
            }
        }
        let previous = self.settings.clone();
        if let Err(e) = self.store.save(&next) {
            let rollback = if changed_live {
                if selection_only {
                    self.core.select(&previous.selected)
                } else if !same_config {
                    self.core.apply(&previous)
                } else { Ok(()) }
            } else { Ok(()) };
            if let Err(rollback_error) = rollback {
                let directory = self.core.directory.clone();
                let cleanup = cleanup_network_session(Some(&mut self.core), &directory, false);
                self.status = if cleanup.is_err() { "CleanupError" } else { "Error" }.into();
                let error = format!("Не удалось сохранить настройки: {e}; откат не подтверждён: {rollback_error}{}",
                    cleanup.err().map(|failure| format!("; {failure}")).unwrap_or_default());
                self.error = Some(error.clone());
                return Err(error);
            }
            return Err(e);
        }
        self.settings = next;
        self.revision = self.revision.wrapping_add(1);
        Ok(())
    }
    fn connect(&mut self) -> Result<(), String> {
        if self.status == "CleanupError" {
            return Err("Сначала завершите безопасное восстановление сети Atlas".into());
        }
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
                    self.settings.was_connected = false;
                    let directory = self.core.directory.clone();
                    let cleanup = cleanup_network_session(Some(&mut self.core), &directory, false);
                    let cleanup_failed = cleanup.is_err();
                    self.core.continue_running = None;
                    let error = match cleanup {
                        Ok(()) => e,
                        Err(cleanup) => format!("{e}; {cleanup}"),
                    };
                    self.status = if cleanup_failed { "CleanupError" } else { "Error" }.into();
                    self.error = Some(error.clone());
                    return Err(error);
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
                let directory = self.core.directory.clone();
                let cleanup = cleanup_network_session(Some(&mut self.core), &directory, false);
                let cleanup_failed = cleanup.is_err();
                self.core.continue_running = None;
                let error = match cleanup {
                    Ok(()) => e,
                    Err(cleanup) => format!("{e}; {cleanup}"),
                };
                self.status = if cleanup_failed { "CleanupError" }
                    else if self.reconnect.load(std::sync::atomic::Ordering::SeqCst) { "Error" }
                    else { "Disconnected" }.into();
                self.error = Some(error.clone());
                self.log("ERROR", &error);
                Err(error)
            }
        }
    }
    fn disconnect(&mut self) -> Result<(), String> {
        self.revision = self.revision.wrapping_add(1);
        self.status = "Stopping".into();
        self.reconnect
            .store(false, std::sync::atomic::Ordering::SeqCst);
        self.cancellation.cancel();
        let directory = self.core.directory.clone();
        let cleanup = cleanup_network_session(Some(&mut self.core), &directory, false);
        self.core.continue_running = None;
        if let Err(error) = cleanup {
            self.status = "CleanupError".into();
            self.error = Some(error.clone());
            self.log("ERROR", &error);
            return Err(error);
        }
        self.settings.was_connected = false;
        if let Err(error) = self.store.save(&self.settings) {
            self.status = "CleanupError".into();
            self.error = Some(format!("Сеть восстановлена, но не удалось сохранить отключённое состояние: {error}"));
            return Err(self.error.clone().unwrap());
        }
        self.status = "Disconnected".into();
        self.error = None;
        self.log(
            "INFO",
            "Соединение отключено; сетевые настройки восстановлены",
        );
        Ok(())
    }
    fn prepare_restart(&mut self) -> Result<(), String> {
        self.disconnect()?;
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
            "rules_migration_preview" => {
                let old = rules::compile(&self.settings)?;
                let mut next = self.settings.clone();
                next.rules_semantics_version = 2;
                let proposed = rules::compile(&next)?;
                return Ok(json!({"fromVersion":self.settings.rules_semantics_version,
                    "toVersion":2,"changed":old != proposed,"before":old,"after":proposed}));
            }
            "rules_migration_apply" => {
                if self.settings.rules_semantics_version != 1 {
                    return Err("Миграция правил уже выполнена".into());
                }
                let mut next = self.settings.clone();
                next.rules_semantics_version = 2;
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
                let nodes = match subscriptions::download(&url, self.core.running()) {
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
                self.install_subscription(id, url, nodes, p["name"].as_str().map(str::to_owned),
                    (action == "subscription_add").then_some(entry))?;
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
    // Runs elevated from the uninstaller; uses the same session cleanup as
    // Disconnect and Exit, plus exact-key removal of legacy WFP filters.
    cleanup_network_session(None, &dir, true)?;
    windows::restore_loaded_profiles()?;
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
fn cleanup_network_session(
    core: Option<&mut core::Core>,
    directory: &std::path::Path,
    clear_legacy_filters: bool,
) -> Result<(), String> {
    let stage = |name: &str, operation: &mut dyn FnMut() -> Result<(), String>| {
        let started = std::time::Instant::now();
        incident_history::record("shutdown_stage_started", json!({"stage":name}), &[]);
        let _ = incident_history::flush(&directory.join("incident-history.ndjson"));
        let result = operation();
        incident_history::record("shutdown_stage_completed", json!({"stage":name,
            "elapsedMs":started.elapsed().as_millis(),"error":result.as_ref().err()}), &[]);
        let _ = incident_history::flush(&directory.join("incident-history.ndjson"));
        result
    };
    let mut core = core;
    let owned_core = stage("core", &mut || core.as_deref_mut().map(|c|c.stop()).unwrap_or(Ok(())));
    let service = stage("service", &mut || service::stop_and_wait());
    let proxy = stage("proxy_restore", &mut || windows::restore(&directory.join("proxy-restore.json")));
    let legacy = stage("legacy_filters", &mut || if clear_legacy_filters { network_guard::clear() } else { Ok(()) });
    let tun = stage("tun_release", &mut || network_guard::wait_for_tun_release(std::time::Duration::from_secs(10)));
    let _ = incident_history::flush(&directory.join("incident-history.ndjson"));
    let errors: Vec<_> = [owned_core, service, proxy, legacy, tun].into_iter()
        .filter_map(Result::err).collect();
    if errors.is_empty() {
        let _ = std::fs::remove_file(directory.join("tun-guard.active"));
        Ok(())
    } else {
        Err(format!("Очистка Atlas не завершена: {}", errors.join("; ")))
    }
}
#[tauri::command]
async fn request(
    app: tauri::AppHandle,
    state: tauri::State<'_, Shared>,
    action: String,
    payload: Option<Value>,
) -> Result<Value,String> {
    let started=std::time::Instant::now();
    let result=request_inner(app,state,action.clone(),payload).await;
    if let Err(error)=&result {
        incident_history::record("request_failed",json!({"action":action,"elapsedMs":started.elapsed().as_millis(),"error":error}),&[]);
    }
    result
}
async fn request_inner(
    app: tauri::AppHandle,
    state: tauri::State<'_, Shared>,
    action: String,
    payload: Option<Value>,
) -> Result<Value, String> {
    let shared = state.inner().clone();
    if action == "frontend_diagnostic" {
        let value=payload.unwrap_or(Value::Null);
        let text=|key: &str,limit|value[key].as_str().unwrap_or("").chars().take(limit).collect::<String>();
        incident_history::record("frontend_event",json!({"area":text("area",80),"stage":text("stage",80),"detail":text("detail",4096)}),&[]);
        let directory=app.state::<published_state::ReadState>().get()?.directory.clone();
        return tauri::async_runtime::spawn_blocking(move || {
            incident_history::flush(&directory.join("incident-history.ndjson"))?;
            Ok(json!({"recorded":true}))
        }).await.map_err(|e|e.to_string())?;
    }
    if action.starts_with("lan_") {
        let lan=app.state::<lan_diagnostics::Lan>().inner().clone();
        return tauri::async_runtime::spawn_blocking(move || lan.command(&action,payload.unwrap_or(Value::Null)))
            .await.map_err(|e|e.to_string())?;
    }
    if action == "snapshot" {
        return Ok(app.state::<published_state::PublishedState>().get());
    }
    if action == "pool_probe" {
        let (client, revision, names) = {
            let a = app.state::<published_state::ReadState>().get()?;
            if a.status != "Connected" { return Err("Atlas не подключён".into()); }
            (a.client.clone(), a.revision, a.settings.servers().iter()
                .filter_map(|n|n["name"].as_str().map(str::to_owned)).collect::<Vec<_>>())
        };
        return tauri::async_runtime::spawn_blocking(move || {
            client.api("GET", "/version", None)?;
            let mut delays = serde_json::Map::new();
            let primary = latency::verified_batch(client.clone(), &names, latency::DISPLAY_URL);
            let mut unresolved = Vec::new();
            for name in &names {
                if let Some(delay)=primary[name]["delay"].as_u64() { delays.insert(name.clone(),json!(delay)); }
                else { unresolved.push(name.clone()); }
            }
            let mut secondary_healthy = Vec::new();
            if !unresolved.is_empty() {
                let secondary = latency::verified_batch(client.clone(), &unresolved, latency::SECONDARY_URL);
                for name in &unresolved {
                    if secondary[name]["expectedStatusMatched"] == true { secondary_healthy.push(name.clone()); }
                }
            }
            client.api("GET", "/version", None)?;
            Ok(json!({"revision":revision,"delays":delays,"secondaryHealthy":secondary_healthy,
                "checkedNodes":names.len()}))
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
                    "activeCheckIntervalSeconds":10,"autoSearchPingMs":a.settings.auto_search_ping_ms,
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
        let client = app.state::<published_state::ReadState>().get()?.client.clone();
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
        if app.state::<published_state::ReadState>().get()?.status != "Connected"
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
        let reads = app.state::<published_state::ReadState>().inner().clone();
        return tauri::async_runtime::spawn_blocking(move || {
            {
                let a = reads.get()?;
                if a.status != "Connected" {
                    return Err("Сначала подключите Atlas".into());
                }
            }
            let client = reqwest::blocking::Client::builder()
                .proxy(reqwest::Proxy::all("http://127.0.0.1:17890").map_err(|e| e.to_string())?)
                .timeout(std::time::Duration::from_secs(10))
                .redirect(reqwest::redirect::Policy::limited(5))
                .build().map_err(|e| e.to_string())?;
            let start = std::time::Instant::now();
            match client.get(url).send() {
                Ok(response) => Ok(json!({"ok":response.status().is_success(),"ms":start.elapsed().as_millis(),"status":response.status().as_u16()})),
                Err(error) => Ok(json!({"ok":false,"ms":null,"status":null,
                    "errorKind":if error.is_timeout() {"timeout"} else {"network"},
                    "error":error.to_string()})),
            }
        }).await.map_err(|e| e.to_string())?;
    }
    if action == "rule_probe" {
        let payload = payload.ok_or("Нет правила")?;
        let key = payload["key"].as_str().ok_or("Нет ключа правила")?;
        let (settings, client, rule, route) = {
            let a = app.state::<published_state::ReadState>().get()?;
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
                a.client.clone(),
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
            let a = app.state::<published_state::ReadState>().get()?;
            (a.settings.clone(), a.client.clone())
        };
        return tauri::async_runtime::spawn_blocking(move || {
            Ok(json!(diagnostics::run(&settings, client)))
        })
        .await
        .map_err(|e| e.to_string())?;
    }
    if action == "protection_status" {
        let (settings, client) = {
            let a = app.state::<published_state::ReadState>().get()?;
            if a.status != "Connected" {
                return Ok(json!({
                    "secure": false,
                    "detail": "Atlas не подключён.",
                    "checkedAt": model::now()
                }));
            }
            (a.settings.clone(), a.client.clone())
        };
        return tauri::async_runtime::spawn_blocking(move || {
            Ok(diagnostics::protection_status(&settings, client))
        })
        .await
        .map_err(|e| e.to_string())?;
    }
    if action == "latency_batch" {
        let batch_id = payload.as_ref().and_then(|p| p["batchId"].as_str())
            .filter(|id| !id.is_empty() && id.len() <= 128)
            .ok_or("Не указан идентификатор проверки")?.to_owned();
        let reads = app.state::<published_state::ReadState>().inner().clone();
        return tauri::async_runtime::spawn_blocking(move || {
            let (settings, client, binary, directory, names, revision) = {
                // A snapshot or save may briefly own the app mutex. Queue the
                // read instead of falsely marking every node as a probe error.
                let a = reads.get()?;
                let connected = a.status == "Connected";
                if !connected && a.directory.join("tun-guard.active").exists() {
                    return Err("Atlas в защищённой паузе; автономная проверка недоступна".into());
                }
                (a.settings.clone(), connected.then(|| a.client.clone()),
                    a.binary.clone(), a.directory.clone(),
                    a.settings.servers().iter().filter_map(|node| node["name"].as_str().map(str::to_owned)).collect::<Vec<_>>(),
                    a.revision)
            };
            let progress = |name: &str, result: &Value| {
                let _ = app.emit("latency-result", json!({
                    "batchId":batch_id,"revision":revision,"name":name,"result":result
                }));
            };
            let results = if let Some(client) = client { latency::batch_stream(client, &names, &progress)? }
                else { latency::offline_batch_stream(settings, binary, directory, &names, &progress)? };
            Ok(json!({"revision":revision,"results":results}))
        }).await.map_err(|e| e.to_string())?;
    }
    if action == "latency" {
        let name = payload
            .as_ref()
            .and_then(|p| p["name"].as_str())
            .ok_or("Не выбран сервер")?
            .to_owned();
        let reads = app.state::<published_state::ReadState>().inner().clone();
        return tauri::async_runtime::spawn_blocking(move || {
            let (settings, client, binary, directory) = {
                let a = reads.get()?;
                if !a.settings.servers().iter().any(|p| p["name"] == name) {
                    return Err("Сервер отсутствует в подписках".into());
                }
                let connected = a.status == "Connected";
                if !connected && a.directory.join("tun-guard.active").exists() {
                    return Err("Atlas в защищённой паузе; автономная проверка недоступна".into());
                }
                (a.settings.clone(), connected.then(|| a.client.clone()),
                    a.binary.clone(), a.directory.clone())
            };
            let result = if let Some(client) = client { latency::test(client, &name) }
                else { latency::offline_test(settings, binary, directory, &name) };
            serde_json::to_value(result).map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| e.to_string())?;
    }
    tauri::async_runtime::spawn_blocking(move || {
        let queued=std::time::Instant::now();
        // Only mutations are serialized. Readers use ReadState and never wait
        // for this lock; queued mutations do not fail just because another runs.
        let mut a = shared.lock().map_err(|_| "Состояние Atlas недоступно")?;
        let waited_ms=queued.elapsed().as_millis();
        if app.state::<ShuttingDown>().load(std::sync::atomic::Ordering::SeqCst) {
            return Err("Atlas завершает работу".into());
        }
        let started=std::time::Instant::now();
        let command_id=format!("{}-{}",std::process::id(),std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos());
        incident_history::record("command_started",json!({"id":command_id,"action":action,
            "waitedMs":waited_ms,"revision":a.revision}),&[]);
        let result = a.dispatch(&app, &action, payload.unwrap_or(Value::Null));
        incident_history::record("command_completed",json!({"id":command_id,"action":action,
            "waitedMs":waited_ms,"executionMs":started.elapsed().as_millis(),
            "success":result.is_ok(),"error":result.as_ref().err(),"revision":a.revision}),&[]);
        if result.is_err() { let _ = incident_history::flush(&a.core.directory.join("incident-history.ndjson")); }
        a.snapshot();
        result
    })
    .await
    .map_err(|e| e.to_string())?
}
pub fn run() {
    tauri::Builder::default()
        .runtime(tauri_runtime_cef::Cef::default())
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
            // A previous UI may have crashed while its on-demand service was
            // still alive. Finish that Atlas-owned session before opening UI.
            if service::is_running() {
                service::stop_and_wait().map_err(std::io::Error::other)?;
            }
            windows::restore(&dir.join("proxy-restore.json")).map_err(std::io::Error::other)?;
            let _ = std::fs::remove_file(dir.join("tun-guard.active"));
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
            let binary = app.path().resource_dir()?.join("resources/Atlas.Core.exe");
            let history_path = dir.join("incident-history.ndjson");
            incident_history::load(&history_path);
            incident_history::record("application_start", json!({"version":env!("CARGO_PKG_VERSION"),"buildId":env!("ATLAS_BUILD_ID")}), &[]);
            let core = core::Core::new(binary, dir.clone());
            // A stale service or marker is crash residue, never permission to
            // resume VPN traffic. Only the explicit user preference can start it.
            let auto = settings.startup.auto_connect;
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
            let reads = published_state::ReadState::default();
            app.manage(reads.clone());
            let diagnostic_access = support_report::Access::default();
            app.manage(diagnostic_access.clone());
            let cancellation = cancellation::Cancellation::default();
            app.manage(cancellation.clone());
            let shared = Arc::new(Mutex::new(App {
                revision: 0,
                published,
                reads,
                diagnostic_access,
                cancellation,
                settings,
                store,
                core,
                status: "Disconnected".into(),
                error: None,
                control_error: None,
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
                        if action == "connect" || action == "disconnect" {
                            app.state::<ConnectionIntent>().0.store(action == "connect", std::sync::atomic::Ordering::SeqCst);
                        }
                        if action == "disconnect" {
                            app.state::<cancellation::Cancellation>().cancel();
                        }
                        if action == "quit" {
                            let shutdown = app.state::<ShuttingDown>().inner().clone();
                            if shutdown.swap(true, std::sync::atomic::Ordering::SeqCst) { return; }
                            app.state::<ConnectionIntent>().0.store(false, std::sync::atomic::Ordering::SeqCst);
                            app.state::<cancellation::Cancellation>().cancel();
                            tauri::async_runtime::spawn_blocking(move || {
                                let result = handle.state::<Shared>().lock()
                                    .map_err(|_| "Состояние Atlas недоступно".to_owned())
                                    .and_then(|mut state| {
                                        let outcome = state.disconnect();
                                        state.snapshot();
                                        outcome
                                    });
                                match result {
                                    Ok(()) => handle.exit(0),
                                    Err(error) => {
                                        shutdown.store(false, std::sync::atomic::Ordering::SeqCst);
                                        if let Some(window) = handle.get_webview_window("main") {
                                            let _ = window.show();
                                            let _ = window.set_focus();
                                        }
                                        let _ = handle.emit("cleanup-failed", error);
                                    }
                                }
                            });
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
            let refresh_done = Arc::new(std::sync::atomic::AtomicBool::new(false));
            {
                let shared = shared.clone();
                let done = refresh_done.clone();
                let stop = shutdown.clone();
                std::thread::spawn(move || {
                    struct Finish(Arc<std::sync::atomic::AtomicBool>);
                    impl Drop for Finish { fn drop(&mut self) { self.0.store(true,std::sync::atomic::Ordering::SeqCst); } }
                    let _finish = Finish(done);
                    let ids = shared.lock().map(|a|a.settings.subscriptions.iter().map(|s|s.id.clone()).collect::<Vec<_>>()).unwrap_or_default();
                    for id in ids {
                        if stop.load(std::sync::atomic::Ordering::SeqCst) { break; }
                        let started = std::time::Instant::now();
                        incident_history::record("subscription_refresh_started",json!({"subscription":id,"trigger":"startup"}),&[]);
                        let result = (|| -> Result<(),String> {
                            let url = keyring::Entry::new("AtlasVPN",&id).map_err(|_|"Хранилище Windows недоступно")?
                                .get_password().map_err(|_|"Ссылка подписки отсутствует в хранилище Windows")?;
                            let captured = shared.lock().map_err(|_|"Состояние Atlas недоступно")?.reads.get()?;
                            let nodes = subscriptions::download(&url,captured.status == "Connected")?;
                            let mut a = shared.lock().map_err(|_|"Состояние Atlas недоступно")?;
                            if stop.load(std::sync::atomic::Ordering::SeqCst) { return Err("Atlas завершает работу".into()); }
                            // Never recreate a subscription deleted while the download was running.
                            let Some(current) = a.settings.subscriptions.iter().find(|s|s.id==id) else { return Ok(()); };
                            let before = captured.settings.subscriptions.iter().find(|s|s.id==id);
                            if !subscriptions::refresh_is_current(before,Some(current)) {
                                return Err("Подписка уже изменена; фоновый результат не применён".into());
                            }
                            a.install_subscription(id.clone(),url,nodes,None,None)?;
                            a.snapshot();
                            Ok(())
                        })();
                        incident_history::record("subscription_refresh_completed",json!({"subscription":id,"trigger":"startup",
                            "elapsedMs":started.elapsed().as_millis(),"success":result.is_ok(),"error":result.as_ref().err()}),&[]);
                        if let Err(error) = result {
                            if let Ok(mut a) = shared.lock() {
                                if let Some(sub)=a.settings.subscriptions.iter_mut().find(|s|s.id==id) {sub.error=Some(error.clone());}
                                a.log("WARN",&format!("Автообновление подписки: {error}"));
                                a.snapshot();
                            }
                        }
                    }
                });
            }
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                let mut startup_at = auto.then(|| std::time::Instant::now() + std::time::Duration::from_secs(delay));
                loop {
                    std::thread::sleep(std::time::Duration::from_secs(2));
                    if shutdown.load(std::sync::atomic::Ordering::SeqCst)
                        || handle.state::<ShuttingDown>().load(std::sync::atomic::Ordering::SeqCst) { break; }
                    // Query the service without holding the mutation lock. A
                    // result from an earlier revision cannot affect a new session.
                    let observed=handle.state::<published_state::ReadState>().get().ok()
                        .filter(|v|matches!(v.status.as_str(),"Connected"|"ProtectedPause"))
                        .map(|v|(v.revision,v.client.session_running()));
                    let Ok(mut a) = shared.try_lock() else { continue; };
                    let service_running=observed.and_then(|(revision,result)| {
                        if revision != a.revision { return None; }
                        match result {
                            Ok(running)=>{a.control_error=None;Some(running)},
                            Err(error)=>{
                                if a.control_error.as_ref()!=Some(&error) {
                                    a.log("WARN",&format!("Проверка состояния службы: {error}"));
                                }
                                a.control_error=Some(error);None
                            }
                        }
                    });
                    if refresh_done.load(std::sync::atomic::Ordering::SeqCst) && startup_at.is_some_and(|at| std::time::Instant::now() >= at) {
                        startup_at = None;
                        if intent.load(std::sync::atomic::Ordering::SeqCst) && a.status == "Disconnected" {
                            let _ = a.connect();
                        }
                    }
                    if !intent.load(std::sync::atomic::Ordering::SeqCst) && a.status == "Connected" {
                        let _ = a.disconnect();
                        a.snapshot();
                        continue;
                    }
                    if a.status == "Connected" && service_running == Some(false) {
                        let _ = windows::restore(&a.core.directory.join("proxy-restore.json"));
                        a.status = if a.core.service_reachable() { "ProtectedPause" } else { "Error" }.into();
                        let message = "Путь VPN не подтверждён. Служба выполняет ограниченное восстановление либо завершает сессию.";
                        a.error = Some(message.into());
                        a.log("ERROR",message);
                        let _ = handle.emit("core-crashed", ());
                    }
                    if a.status == "ProtectedPause" {
                        if service_running == Some(true) {
                            a.status = "Connected".into();
                            a.error = None;
                            a.log("INFO", "Служба восстановила проверенное соединение");
                        } else if !a.core.service_reachable() {
                            a.status = "Error".into();
                        }
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

pub fn network_service() -> Result<(), String> {
    service::run()
}

pub fn watch_network_session(pid: u32, directory: &std::path::Path) -> Result<(), String> {
    session_cleanup::run(pid, directory)
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
