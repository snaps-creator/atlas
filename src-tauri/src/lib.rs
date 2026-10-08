pub use session_cleanup::verify_desktop_owner;
mod applications;
mod background_probe;
mod broker;
mod cancellation;
mod connection_retry;
mod config;
mod core;
mod country;
#[path = "network_diagnostics.rs"]
mod diagnostics;
mod job;
pub use job::DesktopJob;
mod lan_policy;
mod latency;
mod resilient_selection;
mod model;
mod network_guard;
mod recovery;
mod network_change;
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
mod xray_config;
mod xray_runtime;
mod subscription_options;
mod subscription_refresh;
#[cfg(test)]
mod site_checks;
mod support_report;
mod support_probes;
mod incident_history;
mod interface_evidence;
mod lan_diagnostics;
mod lan_discovery;
mod windows;
mod update_lock;
mod update_process;
mod update_health;
mod update_frontend;
use model::*;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tauri::{Emitter, Manager};
mod startup;
mod startup_coordinator;
type StartupState = Arc<Mutex<startup_coordinator::StartupCoordinator>>;
struct WindowVisibility(Arc<std::sync::atomic::AtomicBool>);
struct App {
    baseline_ready: bool,
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
            sub.options.fallback_url = sub.options.fallback_url.as_deref().map(subscriptions::mask);
            sub.options.effective_url = sub.options.effective_url.as_deref().map(subscriptions::mask);
            // Provider directives can contain private URLs. The UI only edits
            // the explicit User-Agent override through its dedicated command.
            sub.options.parameters.clear();
            sub.options.provider_id = None;
            for p in &mut sub.servers {
                *p = json!({"name":p["name"],"type":p["type"],"protocol":p["xray"]["outbounds"][0]["protocol"].as_str().or_else(||p["type"].as_str()),"nodeId":p["atlas"]["nodeId"],"sourceId":sub.id,"sourceName":sub.name,"sourceType":sub.source,"stableIdentity":p["atlas"]["stableIdentity"],"transport":p["atlas"]["transport"],"country":country::detect(p),"probeId":latency::node_identity(p)});
            }
        }
        let value = json!({"settings":s,"status":self.status,"running":running,"guardActive":self.core.guard_active(),"error":self.error,"duration":self.core.started.map(|t|t.elapsed().as_secs()).unwrap_or(0),"logs":self.logs,"buildId":env!("ATLAS_BUILD_ID")});
        let mut value = value;
        value["controlError"] = json!(self.control_error);
        value["refreshingSubscriptions"] = json!(subscription_refresh::active());
        value["revision"] = json!(self.revision);
        self.published.set(value.clone());
        value
    }
    fn install_subscription(&mut self, id: String, url: String, mut downloaded: subscriptions::Downloaded, label: Option<String>, entry: Option<keyring::Entry>, source: SubscriptionSource) -> Result<(),String> {
                let before=downloaded.nodes.len();
                let mut skipped=downloaded.diagnostics.len();
                downloaded.nodes.retain(|node|source.accepts(node));
                skipped+=before-downloaded.nodes.len();
                if downloaded.nodes.is_empty() {return Err("Источник не содержит серверов выбранного типа подписок".into());}
                if downloaded.nodes.len()!=before {downloaded.diagnostics.push(format!("Пропущено {} узлов другого типа",before-downloaded.nodes.len()));}
                let previous=self.settings.subscriptions.iter().find(|s|s.id==id).map(|s|s.servers.as_slice()).unwrap_or(&[]);
                let nodes=subscriptions::reconcile_nodes(&id,downloaded.nodes,previous);
                let mut next = self.settings.clone();
                let old = next.subscriptions.iter().position(|s| s.id == id);
                let name = label
                    .or_else(|| old.map(|i| next.subscriptions[i].name.clone()))
                    .unwrap_or("Подписка".into());
                let sub = Subscription { source,
                    options: downloaded.options,
                    id: id.clone(),
                    name,
                    masked_url: subscriptions::mask(&url),
                    updated_at: now(),
                    error: (!downloaded.diagnostics.is_empty()).then(||downloaded.diagnostics.iter().take(100).cloned().collect::<Vec<_>>().join("; ")),
                    servers: nodes,
                };
                if let Some(i) = old {
                    next.subscriptions[i] = sub
                } else {
                    next.subscriptions.push(sub)
                }
                model::repository::reconcile_references(&self.settings,&mut next);
                if !next.servers().iter().any(|s| s["name"] == next.selected)
                    && !["AUTO", "FAILOVER"].contains(&next.selected.as_str())
                {
                    next.selected = "AUTO".into()
                };
                let mut validation=next.clone();
                if let Err(batch_error)=self.core.validate(&validation) {
                    let index=next.subscriptions.iter().position(|s|s.id==id).ok_or("Подписка отсутствует")?;
                    let candidates=next.subscriptions[index].servers.clone();
                    let mut accepted=Vec::new();
                    let mut rejected=Vec::new();
                    for (position,node) in candidates.into_iter().enumerate() {
                        let mut probe=Settings::default();
                        probe.mode="system".into();
                        let mut sub=next.subscriptions[index].clone();sub.servers=vec![node.clone()];
                        probe.subscriptions=vec![sub];
                        if self.core.validate(&probe).is_ok() {accepted.push(node);}
                        else {rejected.push(format!("Узел {}: конфигурация отклонена встроенным ядром",position+1));}
                    }
                    if accepted.is_empty() || rejected.is_empty() {return Err(batch_error);}
                    skipped+=rejected.len();
                    next.subscriptions[index].servers=accepted;
                    let warning=rejected.join("; ");
                    next.subscriptions[index].error=Some(match next.subscriptions[index].error.take() {
                        Some(previous)=>format!("{previous}; {warning}"),None=>warning,
                    });
                    if !next.servers().iter().any(|s|s["name"]==next.selected) && !["AUTO","FAILOVER"].contains(&next.selected.as_str()) {next.selected="AUTO".into();}
                    validation=next.clone();
                        self.core.validate(&validation)?;
                }
                if let Some(sub)=next.subscriptions.iter_mut().find(|s|s.id==id) {
                    if let Some(details)=sub.error.take() {sub.error=Some(format!("Импортировано: {}. Пропущено: {skipped}. {details}",sub.servers.len()));}
                }
                if let Some(entry) = entry { entry.set_password(&url).map_err(|_| "Не удалось сохранить ссылку в хранилище Windows")?; }
                self.save(next)?;
                self.log("INFO", "Подписка обновлена и проверена Mihomo");
        Ok(())
    }
    fn save(&mut self, mut next: Settings) -> Result<(), String> {
        model::repository::normalize(&mut next);
        next.reconcile_selection();
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
        if !next.servers().is_empty() {
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
        if !self.baseline_ready {return Err("Atlas ещё восстанавливает сетевое состояние".into());}
        if self.settings.servers().is_empty() {return Err("Нет серверов. Добавьте подписку.".into());}
        if self.status == "CleanupError" {
            if self.core.running() {
                incident_history::record("recovery_reattached", json!({"previousState":self.status,
                    "previousError":self.error,"nextState":"Connected","reason":"LiveServiceSessionConfirmed"}), &[]);
                self.status = "Connected".into();
                self.error = None;
                return Ok(());
            }
            // The last exception is history, not evidence that the adapter is
            // still held. Reconcile again before deciding whether retry is safe.
            incident_history::record("recovery_retry", json!({"operationId":uuid::Uuid::new_v4(),
                "previousState":self.status,"previousError":self.error,
                "nextState":"InspectActualSystemState","tun":network_guard::tun_diagnostic(),
                "serviceProcess":service::process_snapshot().ok()}), &[]);
            let directory = self.core.directory.clone();
            let inspected = cleanup_network_session(Some(&mut self.core), &directory, false);
            recovery::complete_retry(&mut self.status, &mut self.error, inspected)?;
            incident_history::record("recovery_direct_connectivity", json!({
                "nextState":"ValidateDirectConnectivity","evidence":support_probes::recovery_direct(),
                "meaning":"A failed Internet probe does not imply remaining Atlas-owned network state"}), &[]);
        }
        if self.status == "Connected" && self.core.running() {
            return Ok(());
        }
        self.status = "Connecting".into();
        self.revision = self.revision.wrapping_add(1);
        self.error = None;
        // Reattaching to an existing service must not tear down its session
        // merely to prioritize a settings write.
        let preemptible = !self.core.service_reachable() && !service::is_running();
        let (token, _connecting) = self.cancellation.connecting(preemptible);
        self.core.continue_running = Some(token);
        self.snapshot();
        if !self.reconnect.load(std::sync::atomic::Ordering::SeqCst) {
            self.status = "Disconnected".into();
            self.core.continue_running = None;
            return Err("Подключение отменено".into());
        }
        self.settings.mode = "tun".into();
        let result = windows::restore(&self.core.directory.join("proxy-restore.json"))
            .and_then(|_| self.core.start(&self.settings));
        if result.is_ok() { _connecting.complete(); } else { drop(_connecting); }
        match result {
            Ok(()) => {
                if !self.reconnect.load(std::sync::atomic::Ordering::SeqCst) {
                    self.stop_session(false)?;
                    return Err("Подключение отменено".into());
                }
                self.settings.was_connected = true;
                self.settings.user_disconnected = false;
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
                if self.core.service_status().is_some_and(|status| status["state"] == "ProtectedPause") {
                    self.core.continue_running = None;
                    self.status = "ProtectedPause".into();
                    let error = format!("Служба Atlas продолжает восстановление существующей VPN-сессии: {e}");
                    self.error = Some(error.clone());
                    self.log("WARN", &error);
                    return Err(error);
                }
                // A service may have accepted this desktop connection while
                // keeping an already healthy session. A failed reattach or
                // configuration reconciliation must not tear that session
                // down as generic start-failure cleanup.
                if self.core.running() {
                    self.core.continue_running = None;
                    self.status = "Connected".into();
                    let error = format!("Существующее VPN-подключение продолжает работать; новые настройки не применены: {e}");
                    self.error = Some(error.clone());
                    self.log("WARN", &error);
                    return Err(error);
                }
                let directory = self.core.directory.clone();
                let cleanup = cleanup_network_session(Some(&mut self.core), &directory, false);
                let cleanup_failed = cleanup.is_err();
                let yielded = !cleanup_failed && self.cancellation.settings_pending()
                    && self.reconnect.load(std::sync::atomic::Ordering::SeqCst)
                    && matches!(e.as_str(), "Подключение отменено" | "Запуск Xray отменён");
                self.core.continue_running = None;
                let error = match cleanup {
                    Ok(()) => e,
                    Err(cleanup) => format!("{e}; {cleanup}"),
                };
                self.status = if cleanup_failed { "CleanupError" }
                    else if self.reconnect.load(std::sync::atomic::Ordering::SeqCst) { "Error" }
                    else { "Disconnected" }.into();
                if yielded {
                    self.error = None;
                    self.log("INFO", "Подключение продолжится после сохранения настроек");
                    return Ok(());
                }
                self.error = Some(error.clone());
                self.log("ERROR", &error);
                Err(error)
            }
        }
    }
    fn disconnect(&mut self) -> Result<(), String> {
        self.stop_session(true)
    }
    fn stop_session(&mut self, explicit: bool) -> Result<(), String> {
        let intent_error=if explicit {
            self.settings.was_connected=false;self.settings.user_disconnected=true;
            self.store.save(&self.settings).err()
        } else {None};
        self.revision = self.revision.wrapping_add(1);
        self.status = "Stopping".into();
        self.reconnect
            .store(false, std::sync::atomic::Ordering::SeqCst);
        self.cancellation.cancel();
        let directory = self.core.directory.clone();
        let cleanup = cleanup_network_session(Some(&mut self.core), &directory, false);
        self.core.continue_running = None;
        if let Err(error) = cleanup {
            let error=match intent_error {Some(persist)=>format!("{error}; не удалось сохранить отключение: {persist}"),None=>error};
            self.status = "CleanupError".into();
            self.error = Some(error.clone());
            self.log("ERROR", &error);
            return Err(error);
        }
        if let Err(error) = self.store.save(&self.settings) {
            self.status = "Disconnected".into();
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
    fn persist_visibility(&mut self,hidden:bool)->Result<(),String> {
        self.settings.last_window_hidden=hidden;
        self.store.save(&self.settings)
    }
    fn prepare_restart(&mut self) -> Result<(), String> {
        self.stop_session(false)?;
        self.log(
            "INFO",
            "Приложение перезапускается; сетевые настройки восстановлены",
        );
        Ok(())
    }
    fn dispatch(
        &mut self,
        _app: &tauri::AppHandle,
        action: &str,
        p: Value,
    ) -> Result<Value, String> {
        if update_health::pending() && !matches!(action,"snapshot"|"quit") {return Err("Atlas проверяет обновление; изменения доступны после завершения проверки".into());}
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
            "select" => {
                let reference=p["nodeId"].as_str().or_else(||p["name"].as_str()).ok_or("Не выбран сервер")?;
                let mut next=self.settings.clone();next.select_node(reference)?;self.save(next)?;
            }
            "connect" => self.connect()?,
            "disconnect" => self.disconnect()?,
            "save" => {
                let mut next: Settings =
                    serde_json::from_value(p).map_err(|_| "Некорректные настройки")?;
                next.subscriptions = self.settings.subscriptions.clone();
                next.was_connected=self.settings.was_connected;
                next.user_disconnected=self.settings.user_disconnected;
                next.last_window_hidden=self.settings.last_window_hidden;
                let enabled = next.startup.launch_with_windows;
                let previous = self.settings.startup.launch_with_windows;
                self.save(next)?;
                let result = startup::configure(enabled,enabled!=previous);
                if let Err(e) = result {
                    let mut back = self.settings.clone();
                    back.startup.launch_with_windows = previous;
                    self.save(back)?;
                    return Err(format!("Автозапуск Windows: {e}"));
                }
            }
            "subscription_user_agent" => {
                let id = p["id"].as_str().ok_or("Не выбрана подписка")?;
                let agent = p["userAgent"].as_str().ok_or("Не указан User-Agent")?.trim();
                if !agent.is_empty() { subscription_options::validate_agent(agent)?; }
                let mut next = self.settings.clone();
                let sub = next.subscriptions.iter_mut().find(|s| s.id == id)
                    .ok_or("Подписка не найдена")?;
                sub.options.user_agent_override = (!agent.is_empty()).then(||agent.to_owned());
                // Download metadata does not alter the active network configuration.
                self.store.save(&next)?;
                self.settings = next;
            }
            "subscription_delete" => {
                let id = p["id"].as_str().ok_or("Нет ID")?;
                if self.core.running() {
                    return Err("Отключитесь перед удалением подписки".into());
                }
                let mut next = self.settings.clone();
                next.subscriptions.retain(|s| s.id != id);
                model::repository::reconcile_references(&self.settings,&mut next);
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
                s.selected_node_id = "AUTO".into();
                s.favorites.clear();
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
    let operation_id = uuid::Uuid::new_v4().to_string();
    let previous_stage = std::cell::RefCell::new("InspectActualSystemState".to_owned());
    let stage = |name: &str, operation: &mut dyn FnMut() -> Result<(), String>| {
        let started = std::time::Instant::now();
        let next = match name {
            "core" | "service" => "StopOrReconnectServiceIfNeeded",
            "proxy_restore" => "ReconcileProxyState",
            "legacy_filters" => "ReconcileOwnedRules",
            "tun_release" => "ValidateAtlasBaseline",
            _ => name,
        };
        let previous = previous_stage.replace(next.to_owned());
        incident_history::record("shutdown_stage_started", json!({"operationId":operation_id,"stage":name,
            "previousState":previous,"nextState":next}), &[]);
        let _ = incident_history::flush(&directory.join("incident-history.ndjson"));
        let result = operation();
        incident_history::record("shutdown_stage_completed", json!({"operationId":operation_id,"stage":name,
            "elapsedMs":started.elapsed().as_millis(),"error":result.as_ref().err()}), &[]);
        let _ = incident_history::flush(&directory.join("incident-history.ndjson"));
        result
    };
    let mut core = core;
    let core_had_broker = core.as_deref().is_some_and(|c| c.has_broker());
    let owned_core = stage("core", &mut || core.as_deref_mut().map(|c|c.stop()).unwrap_or(Ok(())));
    // Core::stop already waits for its attached on-demand service. Avoid a
    // second full service timeout; still stop any service without an attached broker.
    let service = if core_had_broker { Ok(()) }
        else { stage("service", &mut || service::stop_and_wait_timeout(std::time::Duration::from_secs(1))) };
    let proxy = stage("proxy_restore", &mut || windows::restore(&directory.join("proxy-restore.json")));
    let legacy = stage("legacy_filters", &mut || if clear_legacy_filters { network_guard::clear() } else { Ok(()) });
    let tun = stage("tun_release", &mut || network_guard::wait_for_clean_baseline(std::time::Duration::from_secs(1)));
    let observed = network_guard::tun_diagnostic();
    let process = service::process_snapshot();
    let released = core.as_deref().is_none_or(|c| c.owned_processes_released());
    let reconciled = released && process.as_ref().is_ok_and(|p| matches!(p.state,"Stopped"|"Missing"))
        && observed["baselineReady"] == true && proxy.is_ok() && legacy.is_ok() && tun.is_ok();
    incident_history::record("recovery_baseline", json!({"operationId":operation_id,
        "serviceProcess":process.as_ref().ok(),"serviceQueryError":process.as_ref().err(),"tun":observed,
        "serviceChannelState":if core.as_deref().is_some_and(|c|c.has_broker()) {"Unknown"} else {"Disconnected"},
        "historicalStopError":owned_core.as_ref().err(),"historicalServiceError":service.as_ref().err(),
        "proxyState":if proxy.is_ok() {"RestoredOrNotOwned"} else {"RestoreFailed"},
        "dnsState":"AtlasDoesNotChangeInterfaceDnsServers",
        "cleanupState":if reconciled {"ReadyForRetry"} else {"CleanupRequired"}}), &[]);
    let _ = incident_history::flush(&directory.join("incident-history.ndjson"));
    let errors: Vec<_> = [owned_core, service, proxy, legacy, tun].into_iter()
        .filter_map(Result::err).collect();
    if reconciled {
        let _ = std::fs::remove_file(directory.join("tun-guard.active"));
        Ok(())
    } else {
        Err(format!("Очистка Atlas не завершена: {}", if errors.is_empty() {
            "фактическое освобождение службы или адаптера не подтверждено".into()
        } else { errors.join("; ") }))
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
    if update_health::pending() && !matches!(action.as_str(),"snapshot"|"frontend_diagnostic"|"quit") {return Err("Atlas проверяет обновление; изменения доступны после завершения проверки".into());}
    let shared = state.inner().clone();
    if action=="update_install" {
        app.state::<StartupState>().lock().unwrap_or_else(|e|e.into_inner()).cancel();
        let visibility=app.state::<WindowVisibility>().0.clone();let state=shared.clone();
        tauri::async_runtime::spawn_blocking(move || {
            state.lock().map_err(|_|"Состояние Atlas недоступно")?.persist_visibility(visibility.load(std::sync::atomic::Ordering::SeqCst))
        }).await.map_err(|e|e.to_string())??;
        let payload=payload.unwrap_or(Value::Null);
        update_frontend::start(app,payload["version"].as_str().ok_or("Не указана версия обновления")?)?;
        return Ok(json!({"started":true}));
    }
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
        app.state::<StartupState>().lock().unwrap_or_else(|e|e.into_inner()).cancel();
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
        let tun_interface = network_guard::tun_diagnostic();
        let (context, client, secrets) = match shared.try_lock() {
            Ok(a) => {
                let mut secrets = Vec::new();
                support_report::collect_secrets(&serde_json::to_value(&a.settings).map_err(|e| e.to_string())?, &mut secrets);
                let context = json!({"capturedAt":model::now(),"revision":a.revision,"status":a.status,"mode":a.settings.mode,
                    "routingMode":a.settings.routing_mode,"tunStack":a.settings.tun_stack,
                    "selected":a.settings.selected,"autoTestIntervalSeconds":a.settings.auto_test_interval_seconds,
                    "activeCheckIntervalSeconds":10,"autoSearchPingMs":a.settings.auto_search_ping_ms,
                    "error":a.error,"logs":a.logs,"tunInterface":tun_interface,
                    "competingVpn":network_guard::competing_route_diagnostic(),
                    "connectionConfiguration":support_report::configuration_evidence(&a.settings),
                    "uiPoolHealth":payload.as_ref().and_then(|v|v.get("poolHealth"))});
                (serde_json::to_string_pretty(&context).map_err(|e| e.to_string())?, Some(a.core.client()), secrets)
            }
            Err(_) => match app.state::<support_report::Access>().get() {
                Some(cached) => (json!({"stateRead":"cached: application lock busy","capturedAt":cached.captured_at,
                    "revision":cached.revision,"connectionConfiguration":cached.configuration,
                    "publishedState":app.state::<published_state::PublishedState>().get(),
                    "tunInterface":tun_interface}).to_string(),Some(cached.client),cached.secrets),
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
                // A recovery marker does not prohibit isolated URL probes.
                // The probe uses the same WFP-allowed Core/Xray executables,
                // random loopback ports, and never changes the TUN or guard.
                (a.settings.clone(), connected.then(|| a.client.clone()),
                    a.binary.clone(), a.directory.clone(),
                    a.settings.servers().iter().filter_map(|node| node["name"].as_str().map(str::to_owned)).collect::<Vec<_>>(),
                    a.revision)
            };
            let identities: std::collections::HashMap<_,_> = settings.servers().iter()
                .filter_map(|node| node["name"].as_str().map(|name| (name.to_owned(), latency::node_identity(node)))).collect();
            let progress = |name: &str, result: &Value| {
                let _ = app.emit("latency-result", json!({
                    "batchId":batch_id,"revision":revision,"name":name,"probeId":identities.get(name),"result":result
                }));
            };
            let results = if let Some(client) = client { latency::batch_stream(client, &names, &progress)? }
                else { latency::offline_batch_stream(settings, binary, directory, &names, &progress)? };
            Ok(json!({"revision":revision,"identities":identities,"results":results}))
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
    if matches!(action.as_str(), "subscription_add" | "subscription_refresh") {
        let captured = app.state::<published_state::ReadState>().get()?;
        return tauri::async_runtime::spawn_blocking(move || {
            let p = payload.unwrap_or(Value::Null);
            let id = p["id"].as_str().map(str::to_owned).unwrap_or_else(||uuid::Uuid::new_v4().to_string());
            let _lease = subscription_refresh::Lease::take(&id).ok_or("Эта подписка уже обновляется")?;
            let entry = keyring::Entry::new("AtlasVPN", &id).map_err(|_|"Хранилище Windows недоступно")?;
            let adding = action == "subscription_add";
            let source=if adding {
                serde_json::from_value(p["source"].clone()).map_err(|_|"Укажите источник подписки")?
            } else {captured.settings.subscriptions.iter().find(|s|s.id==id).ok_or("Подписка не найдена")?.source};
            let url = if adding { p["url"].as_str().ok_or("Введите HTTPS URL")?.to_owned() }
                else { entry.get_password().map_err(|_|"Ссылка подписки отсутствует в хранилище Windows")? };
            if source==SubscriptionSource::Url {subscription_options::https_url(&url)?;}
            else if !url.trim().starts_with("vless://") {return Err("Введите один VLESS-ключ".into());}
            // Download never owns the mutation lock: Quit must not wait for HTTPS.
            let options=captured.settings.subscriptions.iter().find(|s|s.id==id)
                .map(|s|s.options.clone()).unwrap_or_default();
            let downloaded = subscriptions::download(&url, captured.status == "Connected", &options);
            let mut a = shared.lock().map_err(|_|"Состояние Atlas недоступно")?;
            if app.state::<ShuttingDown>().load(std::sync::atomic::Ordering::SeqCst) {
                return Err("Atlas завершает работу".into());
            }
            if !adding && !subscriptions::refresh_is_current(
                captured.settings.subscriptions.iter().find(|s|s.id == id),
                a.settings.subscriptions.iter().find(|s|s.id == id)) {
                return Err("Подписка уже изменена; результат загрузки не применён".into());
            }
            match downloaded {
                Ok(nodes) => a.install_subscription(id, url, nodes, p["name"].as_str().map(str::to_owned), adding.then_some(entry),source)?,
                Err(error) => {
                    if let Some(sub) = a.settings.subscriptions.iter_mut().find(|s|s.id == id) { sub.error=Some(error.clone()); }
                    let settings = a.settings.clone();
                    a.store.save(&settings)?;
                    a.snapshot();
                    return Err(error);
                }
            }
            Ok(a.snapshot())
        }).await.map_err(|e|e.to_string())?;
    }
    let settings_change = if action == "save" {
        // Reject malformed requests before yielding a connection attempt.
        serde_json::from_value::<Settings>(payload.clone().unwrap_or(Value::Null))
            .map_err(|_| "Некорректные настройки")?;
        Some(app.state::<cancellation::Cancellation>().settings_change())
    } else { None };
    tauri::async_runtime::spawn_blocking(move || {
        let _settings_change = settings_change;
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
fn persist_window_hidden(app:&tauri::AppHandle,hidden:bool) {
    let visibility=app.state::<WindowVisibility>().0.clone();
    visibility.store(hidden,std::sync::atomic::Ordering::SeqCst);
    let shared=app.state::<Shared>().inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        if let Ok(mut a)=shared.lock() {
            a.settings.last_window_hidden=visibility.load(std::sync::atomic::Ordering::SeqCst);
            let settings=a.settings.clone();let _=a.store.save(&settings);
        }
    });
}
fn show_main(app:&tauri::AppHandle) {
    if let Some(w)=app.get_webview_window("main") {
        let _=w.show();let _=w.unminimize();let _=w.set_focus();
        if app.try_state::<Shared>().is_some() {persist_window_hidden(app,false);}
    }
}
pub fn run() {
    tauri::Builder::default()
        .runtime(tauri_runtime_cef::Cef::default())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_single_instance::init(|app, args, _| {
            if startup_coordinator::LaunchReason::from_args(&args)==startup_coordinator::LaunchReason::ManualLaunch {
                show_main(app);
            }
        }))
        .setup(|app| {
            let _startup_update_guard=if update_health::challenge().is_none(){Some(update_lock::UpdateLock::admit_session().map_err(std::io::Error::other)?)}else{None};
            let dir = app.path().app_local_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            let store =
                storage::Store::open(&dir.join("atlas.db")).map_err(std::io::Error::other)?;
            let mut settings = store.load().map_err(std::io::Error::other)?;
            settings.mode = "tun".into();
            // Repair stale startup registration to match the saved user preference.
            if update_health::challenge().is_none() {
                if let Err(error)=startup::configure(settings.startup.launch_with_windows,false) {
                    eprintln!("Atlas startup registration failed: {error}");
                }
                if !settings.startup.launch_with_windows && std::env::args().any(|arg| arg == "--autostart") {
                    app.handle().exit(0);
                    return Ok(());
                }
            }
            let binary = app.path().resource_dir()?.join("resources/Atlas.Core.exe");
            let history_path = dir.join("incident-history.ndjson");
            incident_history::load(&history_path);
            incident_history::record("application_start", json!({"version":env!("CARGO_PKG_VERSION"),"buildId":env!("ATLAS_BUILD_ID")}), &[]);
            let core = core::Core::new(binary, dir.clone());
            let reason=startup_coordinator::LaunchReason::from_args(&std::env::args().collect::<Vec<_>>());
            let coordinator=startup_coordinator::StartupCoordinator::new(reason,&settings.startup,
                settings.was_connected,settings.user_disconnected,settings.last_window_hidden);
            let show_initial=coordinator.should_show_window();
            // The coordinator captured the preceding session. This session has
            // not established a VPN yet; only connect success records it again.
            settings.was_connected=false;
            settings.last_window_hidden=!show_initial;
            app.manage(WindowVisibility(Arc::new(std::sync::atomic::AtomicBool::new(!show_initial))));
            let coordinator:StartupState=Arc::new(Mutex::new(coordinator));
            app.manage(coordinator.clone());
            let intent = Arc::new(std::sync::atomic::AtomicBool::new(false));
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
                baseline_ready: false,
                revision: 0,
                published,
                reads,
                diagnostic_access,
                cancellation,
                settings,
                store,
                core,
                status: "Initializing".into(),
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
            tauri::tray::TrayIconBuilder::with_id("atlas")
                .icon(tray_icon)
                .tooltip("Atlas VPN")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_tray_icon_event(|tray,event| {
                    if matches!(event,tauri::tray::TrayIconEvent::Click {button:tauri::tray::MouseButton::Left,button_state:tauri::tray::MouseButtonState::Up,..}) {
                        show_main(tray.app_handle());
                    }
                })
                .on_menu_event(|app, event| {
                    if app.state::<ShuttingDown>().load(std::sync::atomic::Ordering::SeqCst) { return; }
                    let id = event.id.as_ref();
                    if update_health::pending() && !matches!(id,"show"|"quit") {return;}
                    if id == "show" {
                        show_main(app);
                    } else {
                        let handle = app.clone();
                        let action = id.to_owned();
                        if matches!(action.as_str(),"connect"|"disconnect"|"restart"|"quit") {
                            app.state::<StartupState>().lock().unwrap_or_else(|e|e.into_inner()).cancel();
                        }
                        if action == "connect" || action == "disconnect" {
                            app.state::<ConnectionIntent>().0.store(action == "connect", std::sync::atomic::Ordering::SeqCst);
                        }
                        if action == "disconnect" {
                            app.state::<cancellation::Cancellation>().cancel();
                        }
                        if action == "quit" {
                            let shutdown = app.state::<ShuttingDown>().inner().clone();
                            if shutdown.swap(true, std::sync::atomic::Ordering::SeqCst) { return; }
                            let shutdown_deadline = shutdown.clone();
                            std::thread::spawn(move || {
                                // Keep a final hard ceiling for the independent
                                // service/route cleanup observer.
                                // Normal shutdown is still immediate. This is only
                                // the hard ceiling: allow the bounded core, service
                                // and TUN stages to finish instead of killing the UI
                                // halfway through tun_release and leaving Atlas-TUN
                                // behind for the next launch.
                                std::thread::sleep(std::time::Duration::from_millis(6000));
                                if shutdown_deadline.load(std::sync::atomic::Ordering::SeqCst) {
                                    // Exit must not leave a hidden GUI process if cleanup stalls.
                                    std::process::exit(0);
                                }
                            });
                            app.state::<ConnectionIntent>().0.store(false, std::sync::atomic::Ordering::SeqCst);
                            app.state::<cancellation::Cancellation>().cancel();
                            tauri::async_runtime::spawn_blocking(move || {
                                let result = handle.state::<Shared>().lock()
                                    .map_err(|_| "Состояние Atlas недоступно".to_owned())
                                    .and_then(|mut state| {
                                        let saved=state.persist_visibility(handle.state::<WindowVisibility>().0.load(std::sync::atomic::Ordering::SeqCst));
                                        let outcome = state.stop_session(false).and(saved);
                                        state.snapshot();
                                        outcome
                                    });
                                match result {
                                    Ok(()) => handle.exit(0),
                                    Err(error) => {
                                        incident_history::record("quit_cleanup_failed", json!({"error":error}), &[]);
                                        let directory = handle.path().app_local_data_dir().ok();
                                        if let Some(directory) = directory {
                                            let _ = incident_history::flush(&directory.join("incident-history.ndjson"));
                                        }
                                        // Exit means exit. The system-service observer owns
                                        // bounded cleanup if graceful shutdown could not finish.
                                        handle.exit(1);
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
                                    let saved=a.persist_visibility(handle.state::<WindowVisibility>().0.load(std::sync::atomic::Ordering::SeqCst));
                                    if saved.and_then(|_|a.prepare_restart()).is_ok() {
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
            persist_window_hidden(app.handle(),!show_initial);
            if show_initial {show_main(app.handle());}
            // Cleanup runs away from the window thread. No connect path is ready
            // until the baseline has completed (including the updater challenge).
            {
                let shared=shared.clone();let dir=dir.clone();let shutdown=shutdown.clone();
                std::thread::spawn(move || {
                    let Ok(mut a)=shared.lock() else {return;};
                    if shutdown.load(std::sync::atomic::Ordering::SeqCst) {return;}
            let startup_cleanup = (|| -> Result<(),String> {
                if update_health::challenge().is_some() {
                    // The supervisor already quiesced the previous session and
                    // started the candidate service. Health must inspect that
                    // service, not stop it as a stale desktop session.
                    return network_guard::wait_for_clean_baseline(std::time::Duration::from_secs(3));
                }
                let _ = network_guard::tun_identity();
                if service::is_running() { service::stop_and_wait()?; }
                windows::restore(&dir.join("proxy-restore.json"))?;
                network_guard::wait_for_tun_release(std::time::Duration::from_secs(3))?;
                network_guard::clear_routes_for_reusable_tun()?;
                let _ = std::fs::remove_file(dir.join("tun-guard.active"));
                Ok(())
            })();
                    {
                        a.baseline_ready=true;
                        match startup_cleanup {
                            Ok(())=>{a.status="Disconnected".into();a.error=None;},
                            Err(error)=>{a.status="CleanupError".into();a.error=Some(error);}
                        }
                        a.snapshot();
                    }
                });
            }
            {
                let shared=shared.clone();let shutdown=shutdown.clone();let intent=intent.clone();
                tauri::async_runtime::spawn(async move {
                    loop {
                        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                        if shutdown.load(std::sync::atomic::Ordering::SeqCst)
                            || coordinator.lock().unwrap_or_else(|e|e.into_inner()).is_finished() {break;}
                        let shared=shared.clone();let coordinator=coordinator.clone();let intent=intent.clone();let shutdown=shutdown.clone();
                        tauri::async_runtime::spawn_blocking(move || {
                            let Ok(mut a)=shared.try_lock() else {return;};
                            let ready=a.baseline_ready && a.status=="Disconnected" && !a.settings.servers().is_empty()
                                && !a.cancellation.settings_pending() && !update_health::pending()
                                && !shutdown.load(std::sync::atomic::Ordering::SeqCst);
                            // Keep coordinator locked until intent is committed; a
                            // manual action cancels policy before changing intent.
                            let mut policy=coordinator.lock().unwrap_or_else(|e|e.into_inner());
                            let due=policy.poll(std::time::Instant::now(),ready,&a.settings.startup);
                            if due {intent.store(true,std::sync::atomic::Ordering::SeqCst);}
                            drop(policy);
                            if due {let _=a.connect();a.snapshot();}
                        }).await.ok();
                    }
                });
            }
            let refresh_done = Arc::new(std::sync::atomic::AtomicBool::new(false));
            subscription_refresh::start(shared.clone(), app.state::<published_state::ReadState>().inner().clone(), shutdown.clone(), refresh_done.clone());
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                let mut reconnect_retry = connection_retry::ConnectionRetry::default();
                let mut cleanup_retry = connection_retry::ConnectionRetry::default();
                loop {
                    std::thread::sleep(std::time::Duration::from_secs(2));
                    if shutdown.load(std::sync::atomic::Ordering::SeqCst)
                        || handle.state::<ShuttingDown>().load(std::sync::atomic::Ordering::SeqCst) { break; }
                    if update_health::pending() {continue;}
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
                        incident_history::record("session_unavailable", json!({
                            "serviceProcess":service::process_snapshot().ok(),
                            "channelState":if a.core.service_reachable() {"Connected"} else {"Disconnected"},
                            "nextState":a.status,"reason":"PathNotConfirmed"}), &[]);
                        let _ = handle.emit("core-session-unavailable", ());
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
                    if a.status == "Connected" && service_running == Some(true) {
                        reconnect_retry.reset();
                    }
                    let wants_connection = intent.load(std::sync::atomic::Ordering::SeqCst);
                    if !wants_connection && a.status == "CleanupError"
                        && cleanup_retry.due(std::time::Instant::now(), true, true) {
                        match a.stop_session(false) {
                            Ok(()) => cleanup_retry.reset(),
                            Err(_) => cleanup_retry.failed(std::time::Instant::now()),
                        }
                    }
                    if !a.cancellation.settings_pending()
                        && reconnect_retry.due(std::time::Instant::now(), wants_connection, matches!(a.status.as_str(), "Error" | "CleanupError")) {
                        a.log("WARN", "Служба Atlas недоступна; выполняется повторное подключение с ограниченной задержкой");
                        match a.connect() {
                            Ok(()) => reconnect_retry.reset(),
                            Err(error) => {
                                if a.cancellation.settings_pending() { reconnect_retry.reset(); }
                                else { reconnect_retry.failed(std::time::Instant::now()); }
                                a.log("WARN", &format!("Повторное подключение не удалось; следующая попытка будет позже: {error}"));
                            }
                        }
                    }
                    a.snapshot();
                    if let Some(tray)=handle.tray_by_id("atlas") {let _=tray.set_tooltip(Some(format!("Atlas — {}",a.status)));}
                }
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if !window.state::<ShuttingDown>().load(std::sync::atomic::Ordering::SeqCst) {
                    api.prevent_close();
                    let _ = window.hide();
                    persist_window_hidden(window.app_handle(),true);
                }
            }
        })
        .on_page_load(|webview,_| {
            if update_health::challenge().is_some() {
                let _=webview.eval(r#"(() => { const timer=setInterval(() => { if(document.querySelector('main') && document.querySelectorAll('button').length>=5) { clearInterval(timer); window.__TAURI_INTERNALS__.invoke('update_health_ready'); } },100); })()"#);
            }
        })
        .invoke_handler(tauri::generate_handler![request,update_health_ready])
        .run(tauri::generate_context!())
        .expect("Atlas failed to initialize");
}

pub fn network_service() -> Result<(), String> {
    service::run()
}

#[tauri::command]
async fn update_health_ready(state:tauri::State<'_,Arc<Mutex<App>>>)->Result<(),String> {
    let (transaction_id,nonce)=update_health::challenge().ok_or("No updater health challenge")?;
    for _ in 0..200 {
        let initializing=state.lock().map_err(|_|"Application state unavailable")?.status=="Initializing";
        if !initializing {break;}
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    {
        let app=state.lock().map_err(|_|"Application state unavailable")?;
        if app.status!="Disconnected" || app.error.is_some() {return Err("Candidate startup baseline is not clean".into());}
        let _=app.store.load()?;
    }
    tauri::async_runtime::spawn_blocking(move||{
        let executable=std::env::current_exe().map_err(|_|"Candidate executable unavailable")?;
        let protocol=update_process::json(&executable.with_file_name("AtlasMaintenance.exe"),&["--protocol"],std::time::Duration::from_secs(5))?;
        if protocol["protocol"]!=1 || protocol["version"]!=env!("CARGO_PKG_VERSION") || protocol["build"]!=env!("ATLAS_BUILD_ID") {return Err("Candidate helper compatibility failed".into());}
        let health_connection=probe_network_service()?;
        let report=update_health::Report{transaction_id,nonce,version:env!("CARGO_PKG_VERSION").into(),build:env!("ATLAS_BUILD_ID").into(),pid:std::process::id(),service_pid:service::process_snapshot()?.pid,
            ui_ready:true,settings_readable:true,subscriptions_readable:true,service_ready:true,helper_protocol:1};
        use std::io::Write;
        {
            let mut out=std::io::stdout().lock();write!(out,"ATLAS_HEALTH_V1:").map_err(|_|"Health IPC write failed")?;serde_json::to_writer(&mut out,&report).map_err(|_|"Health IPC serialization failed")?;writeln!(out).map_err(|_|"Health IPC write failed")?;out.flush().map_err(|_|"Health IPC flush failed".to_owned())?;
        }
        // The on-demand service exits after three seconds without an IPC client.
        // Keep the authenticated connection through the supervisor's stability
        // window and durable commit, rather than mistaking normal idle shutdown
        // for an activation failure. No VPN session is started by this probe.
        let deadline=std::time::Instant::now()+std::time::Duration::from_secs(60);
        while update_health::pending() {
            if !health_connection.alive() {return Err("Candidate service health connection closed before commit".into());}
            if std::time::Instant::now()>=deadline {return Err("Candidate update commit timed out".into());}
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        drop(health_connection);
        Ok(())
    }).await.map_err(|_|"Health worker failed")?
}

pub fn watch_network_session(pid: u32, desktop_pid: u32, directory: &std::path::Path) -> Result<(), String> {
    session_cleanup::run(pid, desktop_pid, directory)
}

/// Exercises the installed service handshake without starting a network core.
pub fn check_network_service() -> Result<(), String> {
    probe_network_service().map(drop)
}
fn probe_network_service() -> Result<broker::Broker, String> {
    let broker = broker::Broker::launch()?;
    if service::verify_server_pid(std::process::id()).is_ok() {
        return Err("Проверка службы приняла посторонний PID".into());
    }
    let status = broker.call("status", Value::Null)?;
    if status["running"] != false || status["guard"] != false {
        return Err("Проверка ожидала службу без активной VPN-сессии".into());
    }
    let before = service::process_snapshot()?;
    broker.close_channel()?;
    broker.reconnect()?;
    let after = service::process_snapshot()?;
    if before.pid == 0 || before.pid != after.pid {
        return Err("Переподключение IPC неожиданно перезапустило процесс службы".into());
    }
    let restored = broker.call("status", Value::Null)?;
    if restored["networkEpoch"] != status["networkEpoch"] || restored["running"] != false {
        return Err("Восстановление IPC изменило сетевую сессию".into());
    }
    Ok(broker)
}

pub fn install_network_service() -> Result<(), String> {
    service::install()
}

pub fn uninstall_network_service() -> Result<(), String> {
    service::uninstall()
}
