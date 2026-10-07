use serde::{Serialize,Deserialize};
#[derive(Clone,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Report {
    pub transaction_id:String,pub nonce:String,pub version:String,pub build:String,
    pub pid:u32,pub service_pid:u32,pub ui_ready:bool,pub settings_readable:bool,
    pub subscriptions_readable:bool,pub service_ready:bool,pub helper_protocol:u32,
}
pub fn challenge()->Option<(String,String)> {
    let args:Vec<String>=std::env::args().collect();let i=args.iter().position(|a|a=="--update-health")?;
    let transaction=args.get(i+1)?;let nonce=args.get(i+2)?;
    if uuid::Uuid::parse_str(transaction).is_err() || uuid::Uuid::parse_str(nonce).is_err(){return None;}
    Some((transaction.clone(),nonce.clone()))
}
/// A health candidate may render/read settings but cannot refresh subscriptions
/// or accept mutations until its own transaction is durably committed.
pub fn pending()->bool {
    let Some((transaction,_))=challenge() else{return false;};
    let committed=(||->Option<bool>{
        use std::io::Read;
        let exe=std::env::current_exe().ok()?;let directory=exe.parent()?;let versions=directory.parent()?;
        if versions.file_name()?.to_str()!=Some("versions"){return None;}
        let file=std::fs::File::open(versions.parent()?.join("current.json")).ok()?;let mut bytes=Vec::new();file.take(4*1024*1024+1).read_to_end(&mut bytes).ok()?;
        if bytes.len()>4*1024*1024{return None;}
        let value:serde_json::Value=serde_json::from_slice(&bytes).ok()?;
        Some(committed_for(&value,&transaction,directory.file_name()?.to_str()?))
    })();
    committed!=Some(true)
}
fn committed_for(value:&serde_json::Value,transaction:&str,directory:&str)->bool {
    value["schema"]==1 && value["transaction_id"]==transaction && value["stage"]=="Committed" &&
    value["active"]==value["candidate"] && value["active"]["id"]==directory &&
    value["active"]["version"]==env!("CARGO_PKG_VERSION") && value["active"]["build"]==env!("ATLAS_BUILD_ID")
}
impl Report {
    pub fn validate(&self,transaction:&str,nonce:&str,version:&str,build:&str,pid:u32)->Result<(),String> {
        if self.transaction_id!=transaction || self.nonce!=nonce || self.version!=version || self.build!=build || self.pid!=pid || pid==0 || self.service_pid==0 ||
            !self.ui_ready || !self.settings_readable || !self.subscriptions_readable || !self.service_ready || self.helper_protocol!=1 {return Err("Candidate health contract failed".into());}Ok(())
    }
}
#[cfg(test)]mod tests {
    use super::*;
    #[test]fn only_own_durable_commit_releases_candidate_mutations(){
        use serde_json::json;let active=json!({"id":"candidate","version":env!("CARGO_PKG_VERSION"),"build":env!("ATLAS_BUILD_ID")});
        let mut value=json!({"schema":1,"transaction_id":"tx","stage":"HealthPending","active":active,"candidate":active});
        assert!(!committed_for(&value,"tx","candidate"));value["stage"]=json!("Committed");assert!(committed_for(&value,"tx","candidate"));assert!(!committed_for(&value,"other","candidate"));assert!(!committed_for(&value,"tx","old"));value["active"]["build"]=json!("other");assert!(!committed_for(&value,"tx","candidate"));
    }
    #[test]fn health_binds_challenge_version_pid_and_all_local_checks(){
        let r=Report{transaction_id:"tx".into(),nonce:"nonce".into(),version:"2.4.1".into(),build:"build".into(),pid:123,service_pid:456,ui_ready:true,settings_readable:true,subscriptions_readable:true,service_ready:true,helper_protocol:1};
        assert!(r.validate("tx","nonce","2.4.1","build",123).is_ok());
        for (tx,nonce,version,build,pid) in [("other","nonce","2.4.1","build",123),("tx","stale","2.4.1","build",123),("tx","nonce","old","build",123),("tx","nonce","2.4.1","old",123),("tx","nonce","2.4.1","build",321)]{assert!(r.validate(tx,nonce,version,build,pid).is_err());}
        let mut bad=r.clone();bad.helper_protocol=2;assert!(bad.validate("tx","nonce","2.4.1","build",123).is_err());bad=r;bad.settings_readable=false;assert!(bad.validate("tx","nonce","2.4.1","build",123).is_err());
    }
}
