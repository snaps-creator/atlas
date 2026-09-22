//! Bounded, redacted flight recorder. No probes and no I/O on the connection lock.
use serde_json::{json, Value};
use std::{collections::VecDeque, path::Path, sync::{Mutex, OnceLock}};
const MAX_ENTRIES: usize = 240;
const MAX_BYTES: usize = 2 * 1024 * 1024;
#[derive(Default)]
struct History { entries: VecDeque<Value>, bytes: usize, sequence: u64, saved: u64 }
impl History {
    fn push(&mut self, value: Value) {
        let size = value.to_string().len();
        if size > MAX_BYTES / 4 { return; }
        self.bytes += size;
        self.sequence += 1;
        self.entries.push_back(value);
        while self.entries.len() > MAX_ENTRIES || self.bytes > MAX_BYTES {
            if let Some(old) = self.entries.pop_front() { self.bytes -= old.to_string().len(); }
        }
    }
}
static HISTORY: OnceLock<Mutex<History>> = OnceLock::new();
fn history() -> &'static Mutex<History> { HISTORY.get_or_init(Default::default) }
pub fn record(kind: &str, value: Value, secrets: &[String]) {
    let safe = crate::support_report::redact(&value.to_string(), secrets);
    // Text intentionally remains text: redaction is allowed to change JSON syntax.
    let item = json!({"at":crate::model::now(),"kind":kind,"evidence":safe});
    if let Ok(mut h) = history().lock() { h.push(item); }
}
pub fn snapshot() -> Value {
    history().lock().map(|h| json!({"maxEntries":MAX_ENTRIES,"maxBytes":MAX_BYTES,
        "meaning":"Passive samples, not continuous packet capture; gaps and missed short events are possible",
        "entries":h.entries})).unwrap_or_else(|_| json!({"error":"History lock poisoned"}))
}
pub fn load(path: &Path) {
    if std::fs::metadata(path).is_ok_and(|m| m.len() <= MAX_BYTES as u64 + 65536) {
        if let Ok(data) = std::fs::read_to_string(path) {
            if let Ok(mut h) = history().lock() {
                for line in data.lines() { if let Ok(item) = serde_json::from_str(line) { h.push(item); } }
            }
        }
    }
}
pub fn flush(path: &Path) -> Result<(), String> {
    use std::io::Write;
    let (all, new, sequence, first) = {
        let h = history().lock().map_err(|_| "History unavailable")?;
        if h.saved == h.sequence { return Ok(()); }
        let lines: Vec<_> = h.entries.iter().map(|v|format!("{v}\n")).collect();
        let count = (h.sequence - h.saved).min(lines.len() as u64) as usize;
        (lines.concat(),lines[lines.len()-count..].concat(),h.sequence,h.saved == 0)
    };
    // Append only newly recorded samples. Rewrite the bounded ring on rotation,
    // not every sampling tick (avoids gigabytes/day of redundant disk writes).
    if first || std::fs::metadata(path).map_or(true,|m|m.len() + new.len() as u64 > MAX_BYTES as u64) {
        std::fs::write(path,all).map_err(|e|e.to_string())?;
    } else {
        std::fs::OpenOptions::new().append(true).open(path).and_then(|mut f|f.write_all(new.as_bytes())).map_err(|e|e.to_string())?;
    }
    if let Ok(mut h) = history().lock() { h.saved = sequence; }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_history_keeps_latest_evidence() {
        let mut h = History::default();
        for i in 0..MAX_ENTRIES + 10 { h.push(json!({"i":i})); }
        assert_eq!(h.entries.len(), MAX_ENTRIES);
        assert_eq!(h.entries.front().unwrap()["i"], 10);
        for _ in 0..20 { h.push(json!({"data":"x".repeat(MAX_BYTES / 8)})); }
        assert!(h.bytes <= MAX_BYTES);
    }
}
