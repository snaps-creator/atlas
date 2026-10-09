use crate::model::Settings;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use std::path::Path;
#[repr(C)]
struct Blob {
    len: u32,
    data: *mut u8,
}
#[link(name = "Crypt32")]
extern "system" {
    fn CryptProtectData(
        a: *const Blob,
        b: *const u16,
        c: *const Blob,
        d: *mut std::ffi::c_void,
        e: *mut std::ffi::c_void,
        f: u32,
        g: *mut Blob,
    ) -> i32;
    fn CryptUnprotectData(
        a: *const Blob,
        b: *mut *mut u16,
        c: *const Blob,
        d: *mut std::ffi::c_void,
        e: *mut std::ffi::c_void,
        f: u32,
        g: *mut Blob,
    ) -> i32;
}
#[link(name = "Kernel32")]
extern "system" {
    fn LocalFree(p: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
}
pub(crate) fn crypt(input: &[u8], encrypt: bool) -> Result<Vec<u8>, String> {
    let data = Blob {
        len: input.len().try_into().map_err(|_| "Слишком большой файл")?,
        data: input.as_ptr() as *mut u8,
    };
    let mut out = Blob {
        len: 0,
        data: std::ptr::null_mut(),
    };
    unsafe {
        let ok = if encrypt {
            CryptProtectData(
                &data,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                1,
                &mut out,
            )
        } else {
            CryptUnprotectData(
                &data,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                1,
                &mut out,
            )
        };
        if ok == 0 {
            return Err("Windows DPAPI: данные недоступны для текущего пользователя".into());
        }
        let result = std::slice::from_raw_parts(out.data, out.len as usize).to_vec();
        LocalFree(out.data.cast());
        Ok(result)
    }
}
// Shared by current state and rotating legacy backups. The ingestion source
// only helps disambiguate old name references; it never filters the new pool.
fn decode_settings(bytes: &[u8]) -> Result<Settings, String> {
    use crate::model::repository;
    use serde_json::Value;
    let invalid = || "Повреждены сохранённые настройки".to_string();
    let mut value: Value = serde_json::from_slice(bytes).map_err(|_| invalid())?;
    let fields = value.as_object_mut().ok_or_else(invalid)?;
    let active = fields
        .get("activeSource")
        .cloned()
        .unwrap_or(Value::String("URL".into()));
    if !fields.contains_key("selected") || fields["selected"].is_null() {
        let selected = active
            .as_str()
            .and_then(|source| fields.get("sourceSelections")?.get(source)?.as_str())
            .unwrap_or("AUTO")
            .to_owned();
        fields.insert("selected".into(), Value::String(selected));
    }
    // An automatic default must not mask the old explicit selection.
    if !fields.get("selectedNodeId").is_some_and(Value::is_string) {
        fields.insert("selectedNodeId".into(), Value::String(String::new()));
    }
    // Legacy state had no recorded visibility; retain its tray preference on
    // updater restart without overriding an explicitly recorded visible window.
    if !fields.contains_key("lastWindowHidden") {
        let hidden = fields
            .get("startup")
            .and_then(|startup| startup.get("startInTray"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        fields.insert("lastWindowHidden".into(), Value::Bool(hidden));
    }
    fields.remove("activeSource");
    fields.remove("sourceSelections");
    let mut settings: Settings = serde_json::from_value(value).map_err(|_| invalid())?;
    let mut references = Vec::new();
    for source in &settings.subscriptions {
        for node in &source.servers {
            if !node.is_object() {
                return Err(invalid());
            }
            if let Some(name) = node["name"].as_str() {
                references.push((
                    name.to_owned(),
                    format!("{}:{}", source.id, repository::identity(node)),
                    serde_json::to_value(source.source).map_err(|_| invalid())? == active,
                ));
            }
        }
    }
    let resolve = |name: &str| {
        references
            .iter()
            .find(|(_, id, _)| id == name)
            .or_else(|| {
                references
                    .iter()
                    .find(|(old, _, preferred)| old == name && *preferred)
            })
            .or_else(|| references.iter().find(|(old, _, _)| old == name))
            .map(|(_, id, _)| id.clone())
    };
    repository::normalize(&mut settings);
    if settings.selected_node_id.is_empty()
        && !["AUTO", "FAILOVER"].contains(&settings.selected.as_str())
    {
        if let Some(id) = resolve(&settings.selected) {
            settings.selected_node_id = id;
        }
    }
    settings.favorites = settings
        .favorites
        .iter()
        .flat_map(|name| {
            // An existing canonical ID is exact. Legacy names applied in every
            // source mode, so retain every matching node before reconciliation.
            if references.iter().any(|(_, id, _)| id == name) {
                return vec![name.clone()];
            }
            references
                .iter()
                .filter(|(old, _, _)| old == name)
                .map(|(_, id, _)| id.clone())
                .collect::<Vec<_>>()
        })
        .collect();
    settings.reconcile_selection();
    Ok(settings)
}

pub struct Store {
    db: Connection,
}
impl Store {
    pub fn open(path: &Path) -> Result<Self, String> {
        Self::from_connection(Connection::open(path).map_err(|e| e.to_string())?)
    }
    fn from_connection(mut db: Connection) -> Result<Self, String> {
        let schema: i64 = db
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        if !(0..=3).contains(&schema) {
            return Err("Версия базы настроек новее поддерживаемой; база не изменена".into());
        }
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;")
            .map_err(|e| e.to_string())?;
        Self::migrate(&mut db)?;
        Ok(Self { db })
    }
    fn migrate(db: &mut Connection) -> Result<(), String> {
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        // Read the version under the write lock, so simultaneous opens cannot
        // migrate an already migrated payload or overwrite its original backup.
        let schema: i64 = tx
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        if !(0..=3).contains(&schema) {
            return Err("Версия базы настроек новее поддерживаемой; база не изменена".into());
        }
        tx.execute_batch("CREATE TABLE IF NOT EXISTS state(id INTEGER PRIMARY KEY CHECK(id=1), payload BLOB NOT NULL);
            CREATE TABLE IF NOT EXISTS backups(id INTEGER PRIMARY KEY AUTOINCREMENT, created INTEGER NOT NULL, payload BLOB NOT NULL);")
            .map_err(|e| e.to_string())?;
        if schema < 3 {
            let original: Option<Vec<u8>> = tx
                .query_row("SELECT payload FROM state WHERE id=1", [], |r| r.get(0))
                .optional()
                .map_err(|e| e.to_string())?;
            // Decode and encrypt before changing state. Any failure rolls back
            // the backup, payload and version together; no plaintext is stored.
            let migrated = original
                .as_ref()
                .map(|payload| {
                    let settings = decode_settings(&crypt(payload, false)?)?;
                    crypt(
                        &serde_json::to_vec(&settings).map_err(|e| e.to_string())?,
                        true,
                    )
                })
                .transpose()?;
            if schema < 2 {
                tx.execute_batch("CREATE TABLE IF NOT EXISTS migration_backups(version INTEGER PRIMARY KEY, payload BLOB NOT NULL);
                    INSERT OR IGNORE INTO migration_backups(version,payload) SELECT 2,payload FROM state WHERE id=1;")
                    .map_err(|e| e.to_string())?;
            }
            tx.execute_batch("CREATE TABLE IF NOT EXISTS migration_backups3(version INTEGER PRIMARY KEY, payload BLOB NOT NULL);
                INSERT OR IGNORE INTO migration_backups3(version,payload) SELECT 3,payload FROM state WHERE id=1;")
                .map_err(|e| e.to_string())?;
            if let Some(payload) = migrated {
                tx.execute("UPDATE state SET payload=?1 WHERE id=1", params![payload])
                    .map_err(|e| e.to_string())?;
            }
            tx.pragma_update(None, "user_version", 3)
                .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())
    }
    pub fn load(&self) -> Result<Settings, String> {
        let blob: Option<Vec<u8>> = self
            .db
            .query_row("SELECT payload FROM state WHERE id=1", [], |r| r.get(0))
            .optional()
            .map_err(|e| e.to_string())?;
        match blob {
            None => Ok(Settings::default()),
            Some(b) => decode_settings(&crypt(&b, false)?),
        }
    }
    /// SQLite takes a consistent snapshot including WAL contents. Copying only
    /// atlas.db would silently omit recently committed subscriptions/settings.
    #[cfg(test)]
    pub fn update_snapshot(&self, destination: &Path) -> Result<(), String> {
        if destination.exists() {
            return Err("Snapshot destination already exists".into());
        }
        let destination = destination.to_str().ok_or("Invalid snapshot path")?;
        self.db
            .execute("VACUUM INTO ?1", params![destination])
            .map_err(|e| e.to_string())?;
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(destination)
            .and_then(|f| f.sync_all())
            .map_err(|e| e.to_string())?;
        let snapshot =
            Connection::open_with_flags(destination, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .map_err(|e| e.to_string())?;
        let check: String = snapshot
            .query_row("PRAGMA integrity_check", [], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        if check != "ok" {
            return Err("Settings snapshot integrity failed".into());
        }
        Ok(())
    }
    pub fn save(&mut self, s: &Settings) -> Result<(), String> {
        let data = crypt(&serde_json::to_vec(s).map_err(|e| e.to_string())?, true)?;
        let tx = self.db.transaction().map_err(|e| e.to_string())?;
        tx.execute(
            "INSERT INTO backups(created,payload) SELECT ?1,payload FROM state WHERE id=1",
            params![crate::model::now()],
        )
        .map_err(|e| e.to_string())?;
        tx.execute("INSERT INTO state(id,payload) VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET payload=excluded.payload",params![data]).map_err(|e|e.to_string())?;
        tx.execute("DELETE FROM backups WHERE id NOT IN (SELECT id FROM backups ORDER BY id DESC LIMIT 20)",[]).map_err(|e|e.to_string())?;
        tx.commit().map_err(|e| e.to_string())
    }
    /// Durable runtime fields must not consume a user configuration checkpoint.
    pub fn save_runtime(&mut self, settings: &Settings) -> Result<(), String> {
        let mut persisted = self.load()?;
        persisted.last_window_hidden = settings.last_window_hidden;
        persisted.was_connected = settings.was_connected;
        persisted.user_disconnected = settings.user_disconnected;
        let data = crypt(&serde_json::to_vec(&persisted).map_err(|e|e.to_string())?, true)?;
        self.db.execute("INSERT INTO state(id,payload) VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET payload=excluded.payload",params![data]).map_err(|e|e.to_string())?;
        Ok(())
    }
    pub fn previous(&self) -> Result<Settings, String> {
        let data: Vec<u8> = self
            .db
            .query_row(
                "SELECT payload FROM backups ORDER BY id DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .map_err(|_| "Нет резервной копии")?;
        decode_settings(&crypt(&data, false)?).map_err(|_| "Резервная копия повреждена".into())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn legacy_fixture() -> serde_json::Value {
        use serde_json::json;
        let mut value = serde_json::to_value(Settings::default()).unwrap();
        let fields = value.as_object_mut().unwrap();
        fields.remove("selectedNodeId");
        fields.remove("userDisconnected");
        fields.remove("lastWindowHidden");
        value["activeSource"] = json!("VLESS");
        value["sourceSelections"] = json!({"URL":"same", "VLESS":"same"});
        value["selected"] = json!("same");
        value["favorites"] = json!(["same", "url-only"]);
        value["subscriptions"] = json!([
            {"id":"url-keyring-id", "name":"Provider", "source":"URL", "maskedUrl":"https://example.test/***",
             "updatedAt":12345, "error":"fixture refresh error", "options":{"userAgent":"Fixture/1", "userAgentOverride":"Custom/2", "updateIntervalHours":24, "providerId":"provider", "parameters":{"retained":"yes"}},
             "servers":[{"name":"same", "type":"vless", "server":"192.0.2.1", "port":443, "uuid":"fixture-a"},
                        {"name":"url-only", "type":"vless", "server":"192.0.2.2", "port":443, "uuid":"fixture-b"}]},
            {"id":"vless-keyring-id", "name":"Key", "source":"VLESS", "maskedUrl":"vless://***",
             "updatedAt":67890, "error":null, "options":{"userAgent":"Fixture/3"},
             "servers":[{"name":"same", "type":"vless", "server":"192.0.2.3", "port":443, "uuid":"fixture-c"}]}
        ]);
        value
    }
    fn fixture_db(payload: &[u8], schema: i64) -> Connection {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE state(id INTEGER PRIMARY KEY CHECK(id=1), payload BLOB NOT NULL);
            CREATE TABLE backups(id INTEGER PRIMARY KEY AUTOINCREMENT, created INTEGER NOT NULL, payload BLOB NOT NULL);").unwrap();
        db.pragma_update(None, "user_version", schema).unwrap();
        db.execute("INSERT INTO state VALUES(1,?1)", params![payload])
            .unwrap();
        db
    }
    fn payload(db: &Connection) -> Vec<u8> {
        db.query_row("SELECT payload FROM state WHERE id=1", [], |r| r.get(0))
            .unwrap()
    }
    #[test]
    fn window_visibility_does_not_evict_configuration_history() {
        let mut store=Store::from_connection(Connection::open_in_memory().unwrap()).unwrap();
        let mut settings=Settings::default();store.save(&settings).unwrap();
        settings.theme="dark".into();store.save(&settings).unwrap();
        for index in 0..30 {settings.last_window_hidden=index%2==0;store.save_runtime(&settings).unwrap();}
        assert_eq!(store.previous().unwrap().theme,"system");
        assert_eq!(store.load().unwrap().theme,"dark");
        assert_eq!(store.load().unwrap().last_window_hidden,settings.last_window_hidden);
        assert_eq!(store.db.query_row("SELECT COUNT(*) FROM backups",[],|r|r.get::<_,i64>(0)).unwrap(),1);
    }
    #[test]
    fn delete_source_then_rollback_refuses_missing_credential_without_mutation() {
        struct Credentials(Vec<keyring::Entry>);
        impl Drop for Credentials {
            fn drop(&mut self) {
                for entry in &self.0 { let _=entry.delete_credential(); }
            }
        }
        let mut fixture=legacy_fixture();
        let mut credentials=Credentials(Vec::new());
        for source in fixture["subscriptions"].as_array_mut().unwrap() {
            // Real WinCred entries, isolated from every user source even if an
            // assertion fails. No existing Atlas credentials are read or changed.
            let id=format!("atlas-rollback-test-{}",uuid::Uuid::new_v4());
            source["id"]=serde_json::json!(id);
            let entry=keyring::Entry::new("AtlasVPN",&id).unwrap();
            credentials.0.push(entry);
            credentials.0.last().unwrap().set_password("https://example.test/rollback-fixture").unwrap();
        }
        let mut store=Store::from_connection(Connection::open_in_memory().unwrap()).unwrap();
        let before=decode_settings(&serde_json::to_vec(&fixture).unwrap()).unwrap();
        store.save(&before).unwrap();
        let mut after=before.clone(); after.subscriptions.remove(0);
        crate::model::repository::reconcile_references(&before,&mut after);
        store.save(&after).unwrap();
        credentials.0[0].delete_credential().unwrap();
        assert!(matches!(credentials.0[0].get_password(),Err(keyring::Error::NoEntry)));
        let encrypted=payload(&store.db);
        let backup=store.previous().unwrap();
        let exists=|id:&str| keyring::Entry::new("AtlasVPN",id).unwrap().get_password().is_ok();
        let result=crate::settings_write::require_credentials(&backup,exists);
        assert!(result.is_err());
        assert_eq!(payload(&store.db),encrypted);
        assert_eq!(store.load().unwrap().subscriptions.len(),after.subscriptions.len());
        assert_eq!(store.load().unwrap().selected_node_id,after.selected_node_id);
        assert_eq!(store.load().unwrap().favorites,after.favorites);
        // Only a real replacement credential makes the previous source usable.
        credentials.0[0].set_password("https://example.test/reentered-fixture").unwrap();
        crate::settings_write::require_credentials(&backup,exists).unwrap();
        store.save(&backup).unwrap();
        assert_eq!(store.load().unwrap().subscriptions.len(),before.subscriptions.len());
        assert_eq!(credentials.0[0].get_password().unwrap(),"https://example.test/reentered-fixture");
    }

    #[test]
    fn schema3_migrates_all_sources_and_preserves_encrypted_original() {
        let fixture = legacy_fixture();
        let original = crypt(&serde_json::to_vec(&fixture).unwrap(), true).unwrap();
        let mut db = fixture_db(&original, 2);
        Store::migrate(&mut db).unwrap();
        assert_eq!(
            db.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            3
        );
        let backup: Vec<u8> = db
            .query_row(
                "SELECT payload FROM migration_backups3 WHERE version=3",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(backup, original);
        let migrated = payload(&db);
        assert_ne!(migrated, original);
        let stored: serde_json::Value =
            serde_json::from_slice(&crypt(&migrated, false).unwrap()).unwrap();
        assert!(stored.get("activeSource").is_none());
        assert!(stored.get("sourceSelections").is_none());
        let settings = decode_settings(&crypt(&migrated, false).unwrap()).unwrap();
        assert_eq!(settings.servers().len(), 3);
        assert!(settings.selected_node_id.starts_with("vless-keyring-id:"));
        assert_eq!(
            settings.selected,
            settings.subscriptions[1].servers[0]["name"]
                .as_str()
                .unwrap()
        );
        assert!(settings
            .favorites
            .iter()
            .any(|id| id.starts_with("vless-keyring-id:")));
        assert!(settings
            .favorites
            .iter()
            .any(|id| id.starts_with("url-keyring-id:")));
        assert!(!settings.user_disconnected);
        assert!(!settings.last_window_hidden);
        for (index, source) in settings.subscriptions.iter().enumerate() {
            let serialized = serde_json::to_value(source).unwrap();
            for field in ["id", "name", "source", "maskedUrl", "updatedAt", "error"] {
                assert_eq!(serialized[field], fixture["subscriptions"][index][field]);
            }
            let original_options: crate::subscription_options::Options =
                serde_json::from_value(fixture["subscriptions"][index]["options"].clone()).unwrap();
            assert_eq!(source.options, original_options);
        }
        Store::migrate(&mut db).unwrap();
        assert_eq!(payload(&db), migrated);
        assert_eq!(
            db.query_row(
                "SELECT payload FROM migration_backups3 WHERE version=3",
                [],
                |r| r.get::<_, Vec<u8>>(0)
            )
            .unwrap(),
            original
        );
        assert_eq!(
            db.query_row("SELECT COUNT(*) FROM backups", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
        let mut store = Store { db };
        for index in 0..25 {
            let mut next = settings.clone();
            next.theme = format!("fixture-{index}");
            store.save(&next).unwrap();
        }
        assert_eq!(
            store
                .db
                .query_row("SELECT COUNT(*) FROM backups", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            20
        );
        assert_eq!(
            store
                .db
                .query_row(
                    "SELECT payload FROM migration_backups3 WHERE version=3",
                    [],
                    |r| r.get::<_, Vec<u8>>(0)
                )
                .unwrap(),
            original
        );
    }
    #[test]
    fn legacy_favorite_collisions_expand_but_canonical_ids_remain_exact() {
        let mut fixture = legacy_fixture();
        let url_id = format!(
            "url-keyring-id:{}",
            crate::model::repository::identity(&fixture["subscriptions"][0]["servers"][0])
        );
        let vless_id = format!(
            "vless-keyring-id:{}",
            crate::model::repository::identity(&fixture["subscriptions"][1]["servers"][0])
        );
        for (favorites, expected) in [
            (
                serde_json::json!(["same", "same"]),
                vec![url_id.clone(), vless_id.clone()],
            ),
            (serde_json::json!([vless_id]), vec![vless_id.clone()]),
        ] {
            fixture["favorites"] = favorites;
            let original = crypt(&serde_json::to_vec(&fixture).unwrap(), true).unwrap();
            let mut db = fixture_db(&original, 2);
            Store::migrate(&mut db).unwrap();
            let store = Store { db };
            let settings = store.load().unwrap();
            assert_eq!(settings.favorites, expected);
            assert_eq!(settings.selected_node_id, vless_id);
            let roundtrip = decode_settings(&serde_json::to_vec(&settings).unwrap()).unwrap();
            assert_eq!(roundtrip.favorites, expected);
        }
    }
    #[test]
    fn legacy_visibility_inherits_tray_preference_only_when_missing() {
        for (tray, explicit, expected) in [
            (true, None, true),
            (false, None, false),
            (true, Some(false), false),
            (false, Some(true), true),
        ] {
            for was_connected in [false, true] {
                let mut fixture = legacy_fixture();
                fixture["startup"]["startInTray"] = serde_json::json!(tray);
                fixture["wasConnected"] = serde_json::json!(was_connected);
                if let Some(hidden) = explicit {
                    fixture["lastWindowHidden"] = serde_json::json!(hidden);
                }
                let original = crypt(&serde_json::to_vec(&fixture).unwrap(), true).unwrap();
                let mut db = fixture_db(&original, 2);
                Store::migrate(&mut db).unwrap();
                let stored: serde_json::Value =
                    serde_json::from_slice(&crypt(&payload(&db), false).unwrap()).unwrap();
                assert_eq!(stored["lastWindowHidden"], serde_json::json!(expected));
                let settings = Store { db }.load().unwrap();
                assert_eq!(settings.last_window_hidden, expected);
                assert!(!settings.user_disconnected);
                assert_eq!(settings.was_connected, was_connected);
            }
        }
    }
    #[test]
    fn legacy_selection_fallback_only_applies_when_selected_is_absent() {
        let mut fixture = legacy_fixture();
        fixture["selected"] = serde_json::json!("url-only");
        let decode = |value: &serde_json::Value| {
            decode_settings(&serde_json::to_vec(value).unwrap()).unwrap()
        };
        assert!(decode(&fixture)
            .selected_node_id
            .starts_with("url-keyring-id:"));
        fixture.as_object_mut().unwrap().remove("selected");
        assert!(decode(&fixture)
            .selected_node_id
            .starts_with("vless-keyring-id:"));
        fixture["selected"] = serde_json::json!("FAILOVER");
        assert_eq!(decode(&fixture).selected_node_id, "FAILOVER");
        fixture["selected"] = serde_json::json!("AUTO");
        assert_eq!(decode(&fixture).selected_node_id, "AUTO");
        fixture["selected"] = serde_json::json!("removed-node");
        assert_eq!(decode(&fixture).selected_node_id, "AUTO");
        fixture.as_object_mut().unwrap().remove("selected");
        fixture.as_object_mut().unwrap().remove("sourceSelections");
        assert_eq!(decode(&fixture).selected_node_id, "AUTO");
    }
    #[test]
    fn load_and_previous_normalize_legacy_without_rewriting_backups() {
        let original = crypt(&serde_json::to_vec(&legacy_fixture()).unwrap(), true).unwrap();
        let db = fixture_db(&original, 3);
        db.execute(
            "INSERT INTO backups(created,payload) VALUES(99,?1)",
            params![original],
        )
        .unwrap();
        let store = Store { db };
        let loaded = store.load().unwrap();
        let previous = store.previous().unwrap();
        assert_eq!(loaded.selected_node_id, previous.selected_node_id);
        assert_eq!(loaded.favorites, previous.favorites);
        assert_eq!(previous.servers().len(), 3);
        assert_eq!(payload(&store.db), original);
        assert_eq!(
            store
                .db
                .query_row("SELECT payload FROM backups", [], |r| r
                    .get::<_, Vec<u8>>(0))
                .unwrap(),
            original
        );
    }
    #[test]
    fn corrupt_legacy_payload_and_write_failure_roll_back_migration() {
        let mut malformed = legacy_fixture();
        malformed["subscriptions"][0]["servers"] = serde_json::json!([42]);
        for original in [
            b"invalid DPAPI fixture".to_vec(),
            crypt(b"not JSON", true).unwrap(),
            crypt(&serde_json::to_vec(&malformed).unwrap(), true).unwrap(),
        ] {
            let mut db = fixture_db(&original, 2);
            assert!(Store::migrate(&mut db).is_err());
            assert_eq!(payload(&db), original);
            assert_eq!(
                db.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                    .unwrap(),
                2
            );
            assert_eq!(
                db.query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE name='migration_backups3'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
                0
            );
        }
        let original = crypt(&serde_json::to_vec(&legacy_fixture()).unwrap(), true).unwrap();
        let mut db = fixture_db(&original, 2);
        db.execute_batch("CREATE TRIGGER reject_migration BEFORE UPDATE ON state BEGIN SELECT RAISE(ABORT, 'fixture failure'); END;").unwrap();
        assert!(Store::migrate(&mut db).is_err());
        assert_eq!(payload(&db), original);
        assert_eq!(
            db.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            2
        );
        assert_eq!(
            db.query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name='migration_backups3'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
    }
    #[test]
    fn pre_schema2_and_empty_databases_upgrade_transactionally() {
        for schema in [0, 1] {
            let original = crypt(&serde_json::to_vec(&legacy_fixture()).unwrap(), true).unwrap();
            let mut db = fixture_db(&original, schema);
            Store::migrate(&mut db).unwrap();
            assert_eq!(
                db.query_row(
                    "SELECT payload FROM migration_backups WHERE version=2",
                    [],
                    |r| r.get::<_, Vec<u8>>(0)
                )
                .unwrap(),
                original
            );
            assert_eq!(
                db.query_row(
                    "SELECT payload FROM migration_backups3 WHERE version=3",
                    [],
                    |r| r.get::<_, Vec<u8>>(0)
                )
                .unwrap(),
                original
            );
        }
        let store = Store::from_connection(Connection::open_in_memory().unwrap()).unwrap();
        assert_eq!(store.load().unwrap().selected, "AUTO");
        assert_eq!(
            store
                .db
                .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            3
        );
        assert_eq!(
            store
                .db
                .query_row("SELECT COUNT(*) FROM migration_backups3", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
    #[test]
    fn newer_schema_is_rejected_without_rewriting_it() {
        let p = std::env::temp_dir().join(format!("atlas-schema-{}.db", uuid::Uuid::new_v4()));
        {
            let db = Connection::open(&p).unwrap();
            db.execute_batch("PRAGMA user_version=99; CREATE TABLE future(value TEXT); INSERT INTO future VALUES('unchanged');").unwrap();
        }
        let before = std::fs::read(&p).unwrap();
        assert!(Store::open(&p).is_err());
        assert_eq!(std::fs::read(&p).unwrap(), before);
        std::fs::remove_file(p).unwrap();
    }
    #[test]
    fn update_snapshot_includes_wal_and_survives_candidate_changes() {
        let dir = std::env::temp_dir().join(format!("atlas-snapshot-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        {
            let mut db = Store::open(&dir.join("atlas.db")).unwrap();
            let mut settings = Settings::default();
            settings.theme = "dark".into();
            db.save(&settings).unwrap();
            db.update_snapshot(&dir.join("previous.db")).unwrap();
            settings.theme = "light".into();
            db.save(&settings).unwrap();
            let snapshot = Store::open(&dir.join("previous.db")).unwrap();
            assert_eq!(snapshot.load().unwrap().theme, "dark");
            assert!(db.update_snapshot(&dir.join("previous.db")).is_err());
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn persistence_backup_and_encryption() {
        let p = std::env::temp_dir().join(format!("atlas-test-{}.db", uuid::Uuid::new_v4()));
        {
            let mut db = Store::open(&p).unwrap();
            let mut s = Settings::default();
            db.save(&s).unwrap();
            s.theme = "secret-theme".into();
            db.save(&s).unwrap();
            assert_eq!(db.load().unwrap().theme, "secret-theme");
            assert_eq!(db.previous().unwrap().theme, "system");
            let bytes: Vec<u8> = db
                .db
                .query_row("SELECT payload FROM state", [], |r| r.get(0))
                .unwrap();
            assert!(!bytes.windows(12).any(|w| w == b"secret-theme"));
        }
        let _ = std::fs::remove_file(p);
    }
}
