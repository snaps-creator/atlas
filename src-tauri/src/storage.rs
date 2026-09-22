use crate::model::Settings;
use rusqlite::{params, Connection};
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
pub struct Store {
    db: Connection,
}
impl Store {
    pub fn open(path: &Path) -> Result<Self, String> {
        let db = Connection::open(path).map_err(|e| e.to_string())?;
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; CREATE TABLE IF NOT EXISTS state(id INTEGER PRIMARY KEY CHECK(id=1), payload BLOB NOT NULL); CREATE TABLE IF NOT EXISTS backups(id INTEGER PRIMARY KEY AUTOINCREMENT, created INTEGER NOT NULL, payload BLOB NOT NULL);").map_err(|e|e.to_string())?;
        // Keep the original encrypted payload outside the rotating backup history.
        // The stored group schema remains compatible; normalization is performed on compilation.
        db.execute_batch("BEGIN IMMEDIATE; CREATE TABLE IF NOT EXISTS migration_backups(version INTEGER PRIMARY KEY, payload BLOB NOT NULL); INSERT OR IGNORE INTO migration_backups(version,payload) SELECT 2,payload FROM state WHERE id=1; PRAGMA user_version=2; COMMIT;").map_err(|e|e.to_string())?;
        Ok(Self { db })
    }
    pub fn load(&self) -> Result<Settings, String> {
        use rusqlite::OptionalExtension;
        let blob: Option<Vec<u8>> = self
            .db
            .query_row("SELECT payload FROM state WHERE id=1", [], |r| r.get(0))
            .optional()
            .map_err(|e| e.to_string())?;
        match blob {
            None => Ok(Settings::default()),
            Some(b) => serde_json::from_slice(&crypt(&b, false)?)
                .map_err(|_| "Повреждены сохранённые настройки".into()),
        }
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
    pub fn previous(&self) -> Result<Settings, String> {
        let data: Vec<u8> = self
            .db
            .query_row(
                "SELECT payload FROM backups ORDER BY id DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .map_err(|_| "Нет резервной копии")?;
        serde_json::from_slice(&crypt(&data, false)?)
            .map_err(|_| "Резервная копия повреждена".into())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn persistence_backup_and_encryption() {
        let p = std::env::temp_dir().join(format!("atlas-test-{}.db", uuid::Uuid::new_v4()));
        {
            let mut db = Store::open(&p).unwrap();
            let mut s = Settings::default();
            db.save(&s).unwrap();
            s.selected = "secret-node".into();
            db.save(&s).unwrap();
            assert_eq!(db.load().unwrap().selected, "secret-node");
            assert_eq!(db.previous().unwrap().selected, "AUTO");
            let bytes: Vec<u8> = db
                .db
                .query_row("SELECT payload FROM state", [], |r| r.get(0))
                .unwrap();
            assert!(!bytes.windows(11).any(|w| w == b"secret-node"));
        }
        let _ = std::fs::remove_file(p);
    }
}
