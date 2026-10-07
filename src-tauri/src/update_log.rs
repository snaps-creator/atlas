//! Deliberately narrow structured logging: arbitrary errors/URLs/configuration
//! never become serialized fields. This log is not recovery authority.
use serde::Serialize;
use std::{fs::OpenOptions,io::Write,path::Path,time::{Instant,SystemTime,UNIX_EPOCH}};
use crate::update_transaction::Journal;
#[derive(Serialize)]pub enum Operation {Download,Verify,Stage,Quiesce,Activate,Health,Commit,Rollback,Recovery}
#[derive(Serialize)]pub enum ErrorCategory {None,Network,Timeout,Integrity,Disk,Permission,Health,RecoveryRequired,Other}
#[derive(Serialize)]struct Event<'a> {
    timestamp_utc_ms:u128,elapsed_ms:u128,transaction_id:&'a str,sequence:u64,
    from_version:&'a str,to_version:&'a str,build:&'a str,phase:crate::update_transaction::Stage,
    operation:Operation,result:&'static str,error_category:ErrorCategory,
}
pub struct Log {started:Instant}
impl Log {
    pub fn new()->Self {Self{started:Instant::now()}}
    pub fn record(&self,path:&Path,journal:&Journal,operation:Operation,error:Option<&str>)->Result<(),String> {
        journal.previous.validate()?;journal.candidate.validate()?;
        let category=match error {
            None=>ErrorCategory::None,
            Some(e) if e.contains("timeout")=>ErrorCategory::Timeout,
            Some(e) if e.contains("hash")||e.contains("signature")=>ErrorCategory::Integrity,
            Some(e) if e.contains("disk")||e.contains("space")=>ErrorCategory::Disk,
            Some(e) if e.contains("denied")=>ErrorCategory::Permission,
            Some(e) if e.contains("health")=>ErrorCategory::Health,
            Some(e) if e.contains("RecoveryRequired")=>ErrorCategory::RecoveryRequired,
            Some(e) if e.contains("HTTP")||e.contains("network")=>ErrorCategory::Network,
            Some(_)=>ErrorCategory::Other,
        };
        let event=Event{timestamp_utc_ms:SystemTime::now().duration_since(UNIX_EPOCH).map_err(|_|"Invalid log clock")?.as_millis(),elapsed_ms:self.started.elapsed().as_millis(),transaction_id:&journal.transaction_id,sequence:journal.sequence,from_version:&journal.previous.version,to_version:&journal.candidate.version,build:&journal.candidate.build,phase:journal.stage,operation,result:if error.is_some(){"failed"}else{"ok"},error_category:category};
        let mut file=OpenOptions::new().create(true).append(true).open(path).map_err(|_|"Update log unavailable")?;
        serde_json::to_writer(&mut file,&event).map_err(|_|"Update log encoding failed")?;file.write_all(b"\n").map_err(|_|"Update log write failed")?;file.sync_data().map_err(|_|"Update log flush failed".into())
    }
}
#[cfg(test)]mod tests {
    use super::*;use crate::update_transaction::{Version,Payload,Stage};
    #[test]fn secret_canaries_never_enter_structured_update_log(){
        let path=std::env::temp_dir().join(format!("atlas-log-{}",uuid::Uuid::new_v4()));
        let version=Version{absent:false,id:"old".into(),version:"2.4.1".into(),build:"build".into(),files:vec![Payload{path:"Atlas.exe".into(),size:1,sha256:"a".repeat(64)}]};
        let j=Journal{schema:1,transaction_id:uuid::Uuid::new_v4().to_string(),sequence:1,stage:Stage::Downloading,started_at:1,updated_at:2,active:version.clone(),previous:version.clone(),candidate:version,expected_artifact_hash:"a".repeat(64),previous_service_path:"Atlas.Service.exe".into(),migration_state:"unchanged".into(),network_state:"unchanged".into(),health_state:"unconfirmed".into()};
        let secret="HTTP error vless://11111111-2222-3333-4444-555555555555@secret.example:443 https://subscription.example/private-token?Authorization=Bearer-secret password=very-private Cookie=session-secret";
        Log::new().record(&path,&j,Operation::Download,Some(secret)).unwrap();let log=std::fs::read_to_string(&path).unwrap();
        for value in ["vless://","11111111-2222-3333-4444-555555555555","secret.example","subscription.example","private-token","Bearer-secret","very-private","session-secret"]{assert!(!log.contains(value));}
        assert!(log.contains(&j.transaction_id));std::fs::remove_file(path).unwrap();
    }
}
