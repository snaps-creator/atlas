mod applications;
mod broker;
mod config;
mod core;
mod country;
#[path = "network_diagnostics.rs"]
mod diagnostics;
mod job;
mod latency;
mod model;
mod network_guard;
mod portable;
mod rule_probe;
mod rules;
mod service;
mod storage;
mod subscriptions;
mod windows;
use model::*;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tauri::{Emitter, Manager};
use tauri_plugin_autostart::ManagerExt;
struct App {
    settings: Settings,
    store: storage::Store,
    core: core::Core,
    status: String,
    error: Option<String>,
    logs: Vec<Value>,
}
impl App {
    fn log(&mut self, level: &str, message: &str) {
        self.logs
            .push(json!({"time":model::now(),"level":level,"message":message}));
        if self.logs.len() > 1000 {
            self.logs.remove(0);
        }
    }
    fn snapshot(&mut self) -> Value {
        let running = self.core.running();
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
        json!({"settings":s,"status":self.status,"running":running,"guardActive":self.core.directory.join("tun-guard.active").exists(),"error":self.error,"duration":self.core.started.map(|t|t.elapsed().as_secs()).unwrap_or(0),"logs":self.logs})
    }
    fn save(&mut self, mut next: Settings) -> Result<(), String> {
        next.mode = "tun".into();
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
        if !next.subscriptions.is_empty() {
            self.core.apply(&next)?;
        }
        let previous = self.settings.clone();
        if let Err(e) = self.store.save(&next) {
            if self.core.running() {
                let _ = self.core.apply(&previous);
            }
            return Err(e);
        }
        self.settings = next;
        Ok(())
    }
    fn connect(&mut self) -> Result<(), String> {
        if self.status == "Connected" && self.core.running() {
            return Ok(());
        }
        self.status = "Connecting".into();
        self.error = None;
        self.settings.mode = "tun".into();
        let result = windows::restore(&self.core.directory.join("proxy-restore.json"))
            .and_then(|_| self.core.start(&self.settings));
        match result {
            Ok(()) => {
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
                let _ = windows::restore(&self.core.directory.join("proxy-restore.json"));
                self.status = "Error".into();
                self.error = Some(e.clone());
                self.log("ERROR", &e);
                Err(e)
            }
        }
    }
    fn disconnect(&mut self) -> Result<(), String> {
        let stopped = self.core.stop();
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
    if action == "rule_probe" {
        let payload = payload.ok_or("Нет правила")?;
        let key = payload["key"].as_str().ok_or("Нет ключа правила")?;
        let (settings, client, rule, route) = {
            let a = shared.lock().map_err(|_| "Ошибка состояния")?;
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
            let a = shared.lock().map_err(|_| "Ошибка состояния")?;
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
            let mut a = shared.lock().map_err(|_| "Ошибка состояния")?;
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
    if action == "latency" {
        let name = payload
            .as_ref()
            .and_then(|p| p["name"].as_str())
            .ok_or("Не выбран сервер")?
            .to_owned();
        let client = {
            let a = shared.lock().map_err(|_| "Ошибка состояния")?;
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
        let mut a = shared.lock().map_err(|_| "Ошибка внутреннего состояния")?;
        a.dispatch(&app, &action, payload.unwrap_or(Value::Null))
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
            let core = core::Core::new(binary, dir);
            let auto = settings.startup.auto_connect
                || (settings.startup.restore_connection && settings.was_connected);
            let delay = settings.startup.delay_seconds.min(300);
            if settings.startup.start_in_tray {
                if let Some(w) = app.get_webview_window("main") {
                    w.hide()?
                }
            }
            let shared = Arc::new(Mutex::new(App {
                settings,
                store,
                core,
                status: "Disconnected".into(),
                error: None,
                logs: vec![],
            }));
            app.manage(shared.clone());
            use tauri::menu::{Menu, MenuItem};
            let show = MenuItem::with_id(app, "show", "Открыть Атлас", true, None::<&str>)?;
            let connect = MenuItem::with_id(app, "connect", "Подключить", true, None::<&str>)?;
            let disconnect = MenuItem::with_id(app, "disconnect", "Отключить", true, None::<&str>)?;
            let restart = MenuItem::with_id(app, "restart", "Перезагрузить", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Выйти", true, None::<&str>)?;
            let menu =
                Menu::with_items(app, &[&show, &connect, &disconnect, &restart, &quit])?;
            let pixels: Vec<u8> = (0..32 * 32)
                .flat_map(|i| {
                    let x = i % 32;
                    let y = i / 32;
                    if (x as i32 - 16).pow(2) + (y as i32 - 16).pow(2) < 196 {
                        [0, 113, 227, 255]
                    } else {
                        [0, 0, 0, 0]
                    }
                })
                .collect();
            tauri::tray::TrayIconBuilder::new()
                .icon(tauri::image::Image::new_owned(pixels, 32, 32))
                .tooltip("Atlas VPN")
                .menu(&menu)
                .on_menu_event(|app, event| {
                    let id = event.id.as_ref();
                    if id == "show" {
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                    } else {
                        let handle = app.clone();
                        let action = id.to_owned();
                        tauri::async_runtime::spawn_blocking(move || {
                            let state = handle.state::<Shared>();
                            if let Ok(mut a) = state.lock() {
                                if action == "quit" {
                                    let _ = a.disconnect();
                                    drop(a);
                                    handle.exit(0);
                                } else if action == "restart" {
                                    if a.prepare_restart().is_ok() {
                                        drop(a);
                                        handle.restart()
                                    }
                                } else {
                                    let _ = a.dispatch(&handle, &action, Value::Null);
                                }
                            };
                        });
                    }
                })
                .build(app)?;
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                if auto {
                    std::thread::sleep(std::time::Duration::from_secs(delay));
                    for wait in [3, 5, 10, 20] {
                        let mut a = shared.lock().unwrap();
                        if a.connect().is_ok() {
                            break;
                        }
                        if a.settings.mode == "tun" { break; }
                        drop(a);
                        std::thread::sleep(std::time::Duration::from_secs(wait));
                    }
                }
                loop {
                    std::thread::sleep(std::time::Duration::from_secs(2));
                    let mut a = shared.lock().unwrap();
                    if a.status == "Connected" && !a.core.running() {
                        let _ = windows::restore(&a.core.directory.join("proxy-restore.json"));
                        a.status = "Error".into();
                        let message = "Ядро завершилось. Сетевая служба освобождает TUN и временные защитные фильтры; соединение VPN потеряно.";
                        a.error = Some(message.into());
                        a.log("ERROR",message);
                        let _ = handle.emit("core-crashed", ());
                    }
                }
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![request])
        .run(tauri::generate_context!())
        .expect("Atlas failed to initialize");
}

pub fn network_helper(parent: u32, pipe: &str) -> Result<(), String> {
    broker::serve(parent, pipe)
}

pub fn network_service() -> Result<(), String> {
    service::run()
}

pub fn install_network_service() -> Result<(), String> {
    service::install()
}

pub fn uninstall_network_service() -> Result<(), String> {
    service::uninstall()
}
