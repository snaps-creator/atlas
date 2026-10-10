//! Bounded, redacted flight recorder. No probes and no I/O on the connection lock.
use serde_json::{json, Value};
use std::{collections::VecDeque, path::Path, sync::{Mutex, OnceLock}};
const MAX_ENTRIES: usize = 4096;
const MAX_BYTES: usize = 32 * 1024 * 1024;
#[derive(Default)]
struct History { entries: VecDeque<Value>, bytes: usize, sequence: u64, saved: u64, dropped: u64, rotate: bool }
impl History {
    fn push(&mut self, value: Value) {
        if matches!(value["kind"].as_str(), Some("service_operation_failed" | "request_failed")) {
            if let Some(last)=self.entries.back_mut().filter(|last|last["kind"]==value["kind"] && last["sessionId"]==value["sessionId"] && last["evidence"]==value["evidence"]) {
                self.bytes-=last.to_string().len();
                last["repeatCount"]=json!(last["repeatCount"].as_u64().unwrap_or(1).saturating_add(1));
                last["lastAt"]=value["at"].clone();
                last["lastMonotonicMs"]=value["monotonicMs"].clone();
                self.bytes+=last.to_string().len();
                self.sequence+=1; self.rotate=true;
                while self.bytes>MAX_BYTES {
                    if let Some(old)=self.entries.pop_front() {self.bytes-=old.to_string().len();self.dropped+=1;}
                }
                return;
            }
        }
        // Large snapshots have their own quota; they must not evict an hour of
        // lightweight state merely because a failing app retries continuously.
        if value["kind"] == "automatic_incident" && self.entries.iter().filter(|v|v["kind"] == "automatic_incident").count() >= 8 {
            if let Some(index) = self.entries.iter().position(|v|v["kind"] == "automatic_incident") {
                if let Some(old) = self.entries.remove(index) { self.bytes -= old.to_string().len(); self.dropped += 1; self.rotate = true; }
            }
        }
        let size = value.to_string().len();
        if size > MAX_BYTES / 4 { self.dropped += 1; return; }
        self.bytes += size;
        self.sequence += 1;
        self.entries.push_back(value);
        while self.entries.len() > MAX_ENTRIES || self.bytes > MAX_BYTES {
            if let Some(old) = self.entries.pop_front() { self.bytes -= old.to_string().len(); self.dropped += 1; }
        }
    }
}
static HISTORY: OnceLock<Mutex<History>> = OnceLock::new();
fn history() -> &'static Mutex<History> { HISTORY.get_or_init(Default::default) }
pub fn record(kind: &str, value: Value, secrets: &[String]) {
    static SESSION: OnceLock<String> = OnceLock::new();
    static START: OnceLock<std::time::Instant> = OnceLock::new();
    let safe = crate::support_report::redact_value(&value, secrets).to_string();
    // Text intentionally remains text: redaction is allowed to change JSON syntax.
    let item = json!({"at":crate::model::now(),"kind":kind,"evidence":safe,
        "sessionId":SESSION.get_or_init(||uuid::Uuid::new_v4().to_string()),
        "buildId":env!("ATLAS_BUILD_ID"),"version":env!("CARGO_PKG_VERSION"),
        "monotonicMs":START.get_or_init(std::time::Instant::now).elapsed().as_millis()});
    if let Ok(mut h) = history().lock() { h.push(item); }
}
pub fn snapshot() -> Value {
    history().lock().map(|h| json!({"maxEntries":MAX_ENTRIES,"maxBytes":MAX_BYTES,
        "retainedBytes":h.bytes,"droppedEntries":h.dropped,
        "firstAt":h.entries.front().and_then(|v|v.get("at")),"lastAt":h.entries.back().and_then(|v|v.get("lastAt").or_else(||v.get("at"))),
        "meaning":"Compact periodic state plus deduplicated logs; explicit coverage, not packet capture",
        "entries":h.entries})).unwrap_or_else(|_| json!({"error":"History lock poisoned"}))
}

fn report_entry(value: &Value) -> Value {
    if value["kind"] != "automatic_incident" { return value.clone(); }
    let evidence = value["evidence"].as_str().unwrap_or_default();
    let parsed = serde_json::from_str::<Value>(evidence).unwrap_or(Value::Null);
    // A fresh incident snapshot is collected below the history section of every
    // exported report. Keeping eight older multi-megabyte Windows/proxy dumps made
    // the text file look frozen and obscured the timeline. The adjacent
    // incident_summary entries already retain failures and active probe results;
    // keep only identity/timing/error fields here so previous incidents remain
    // correlatable without repeating their complete inventories.
    json!({
        "at": value["at"],
        "kind": value["kind"],
        "sessionId":value["sessionId"],"buildId":value["buildId"],"version":value["version"],"monotonicMs":value["monotonicMs"],
        "evidence": {
            "revision": parsed["revision"],
            "before": {
                "startedAt": parsed["before"]["startedAt"],
                "completedAt": parsed["before"]["completedAt"],
                "logs": parsed["before"]["logs"]["evidence"]
            },
            "after": {
                "startedAt": parsed["after"]["startedAt"],
                "completedAt": parsed["after"]["completedAt"],
                "logs": parsed["after"]["logs"]["evidence"]
            },
            "activeErrors": parsed["active"].get("errors").cloned().unwrap_or(Value::Null),
            "originalBytes": evidence.len(),
            "fullEvidence": "Current export contains a fresh full incident snapshot; prior details are retained by the neighboring incident_summary entry"
        }
    })
}

pub fn snapshot_for_report() -> Value {
    history().lock().map(|h| {
        let entries: Vec<_> = h.entries.iter().map(report_entry).collect();
        json!({"maxEntries":MAX_ENTRIES,"maxBytes":MAX_BYTES,
            "retainedBytes":h.bytes,"exportedBytes":entries.iter().map(|v|v.to_string().len()).sum::<usize>(),
            "droppedEntries":h.dropped,
            "firstAt":h.entries.front().and_then(|v|v.get("at")),"lastAt":h.entries.back().and_then(|v|v.get("lastAt").or_else(||v.get("at"))),
            "meaning":"Compact periodic state and prior incident summaries; the current incident has full evidence below",
            "entries":entries})
    }).unwrap_or_else(|_| json!({"error":"History lock poisoned"}))
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
    static WRITER: OnceLock<Mutex<()>> = OnceLock::new();
    let _writer = WRITER.get_or_init(Default::default).lock().map_err(|_| "History writer unavailable")?;
    use std::io::Write;
    let (contents, sequence, rewrite) = {
        let h = history().lock().map_err(|_| "History unavailable")?;
        if h.saved == h.sequence { return Ok(()); }
        let count = (h.sequence - h.saved).min(h.entries.len() as u64) as usize;
        let new:String=h.entries.iter().skip(h.entries.len()-count).map(|v|format!("{v}\n")).collect();
        let rewrite=h.saved==0 || h.rotate || std::fs::metadata(path).map_or(true,|m|m.len()+new.len() as u64>MAX_BYTES as u64);
        let contents=if rewrite {h.entries.iter().map(|v|format!("{v}\n")).collect()} else {new};
        (contents,h.sequence,rewrite)
    };
    // Append only newly recorded samples. Rewrite the bounded ring on rotation,
    // not every sampling tick (avoids gigabytes/day of redundant disk writes).
    if rewrite {
        std::fs::write(path,contents).map_err(|e|e.to_string())?;
    } else {
        std::fs::OpenOptions::new().append(true).open(path).and_then(|mut f|f.write_all(contents.as_bytes())).map_err(|e|e.to_string())?;
    }
    if let Ok(mut h) = history().lock() { h.saved = sequence; if h.sequence == sequence { h.rotate = false; } }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn repeated_error_storm_keeps_first_last_count_and_session_boundary() {
        let mut h=History::default();
        for at in 0..86400 { h.push(json!({"kind":"service_operation_failed","at":at,"sessionId":"a","evidence":"x".repeat(4096)})); }
        assert_eq!(h.entries.len(),1);
        let item=&h.entries[0];
        assert_eq!(item["at"],0); assert_eq!(item["lastAt"],86399); assert_eq!(item["repeatCount"],86400);
        assert!(h.bytes<8192);
        h.push(json!({"kind":"service_operation_failed","at":86400,"sessionId":"b","evidence":"x".repeat(4096)}));
        assert_eq!(h.entries.len(),2);
        h.push(json!({"kind":"service_operation_failed","at":86401,"sessionId":"b","evidence":"different"}));
        assert_eq!(h.entries.len(),3);
    }
    #[test]
    fn one_hour_survives_repeated_large_incidents() {
        let mut h=History::default();
        for tick in 0..240 {
            h.push(json!({"at":tick*15,"kind":"passive_sample","evidence":"x".repeat(6000)}));
            if tick%4 == 0 { h.push(json!({"kind":"automatic_incident","evidence":"x".repeat(256*1024)})); }
        }
        assert_eq!(h.entries.iter().filter(|v|v["kind"] == "passive_sample").count(),240);
        assert_eq!(h.entries.iter().filter(|v|v["kind"] == "automatic_incident").count(),8);
        assert_eq!(h.entries.front().unwrap()["at"],0);
    }
    #[test]
    fn bounded_history_keeps_latest_evidence() {
        let mut h = History::default();
        for i in 0..MAX_ENTRIES + 10 { h.push(json!({"i":i})); }
        assert_eq!(h.entries.len(), MAX_ENTRIES);
        assert_eq!(h.entries.front().unwrap()["i"], 10);
        for _ in 0..20 { h.push(json!({"data":"x".repeat(MAX_BYTES / 8)})); }
        assert!(h.bytes <= MAX_BYTES);
    }
    #[test]
    fn report_compacts_full_automatic_incident_but_keeps_identity() {
        let entry=json!({"at":7,"kind":"automatic_incident","evidence":json!({
            "revision":42,"before":{"startedAt":1,"completedAt":2,"logs":{"evidence":["dial failed"]},"windows":"x".repeat(512*1024)},
            "active":{"errors":["timeout"]},"after":{"startedAt":3,"completedAt":4,"logs":{"evidence":[]}}
        }).to_string()});
        let compact=report_entry(&entry);
        assert_eq!(compact["evidence"]["revision"],42);
        assert_eq!(compact["evidence"]["before"]["logs"][0],"dial failed");
        assert!(compact.to_string().len()<4096);
    }
}
