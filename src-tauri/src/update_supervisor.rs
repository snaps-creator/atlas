//! Crash-resumable orchestration. Platform effects are acknowledged only after
//! inspection; journal advancement never substitutes for an SCM/network check.
use crate::update_transaction::{Stage, Transaction, Version};
type Result<T> = std::result::Result<T,String>;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Point { BeforeQuiesce, AfterQuiesce, BeforeSwitch, AfterServiceSwitch, AfterPointerSwitch, BeforeHealth, AfterHealth, BeforeCommit, BeforeRollback, AfterRollbackService, AfterRollbackPointer }
pub trait Platform {
    fn checkpoint(&mut self, point:Point)->Result<()>;
    fn snapshot_data(&mut self,transaction:&Transaction)->Result<()>;
    fn quiesce(&mut self,version:&Version)->Result<()>;
    fn inspect_network_baseline(&mut self)->Result<()>;
    fn activate_service(&mut self,version:&Version)->Result<()>;
    fn inspect_service(&mut self,version:&Version)->Result<()>;
    /// Resume the verified on-demand service without switching or stopping it.
    fn resume_service(&mut self,version:&Version)->Result<()>;
    fn health(&mut self,transaction:&Transaction)->Result<()>;
    fn restore_data(&mut self,transaction:&Transaction)->Result<()>;
    fn launch_previous(&mut self,version:&Version)->Result<()>;
    /// Called only after the authoritative commit is durable. Infallible local
    /// ownership transfer: dropping the supervisor must no longer kill the UI.
    fn committed(&mut self);
}
pub fn activate(tx:&mut Transaction, host:&mut impl Platform)->Result<()> {
    if tx.journal.stage!=Stage::Prepared {return Err("Candidate is not prepared".into());}
    tx.journal.previous.verify(&tx.version_path(&tx.journal.previous))?;
    tx.journal.candidate.verify(&tx.version_path(&tx.journal.candidate))?;
    let result=(|| {
        tx.advance(Stage::Quiescing)?;host.checkpoint(Point::BeforeQuiesce)?;
        host.quiesce(&tx.journal.previous)?;host.inspect_network_baseline()?;
        tx.journal.network_state="baseline-verified".into();
        tx.advance(Stage::Quiesced)?;host.checkpoint(Point::AfterQuiesce)?;
        // Final desktop writes must be included. Taking this snapshot before
        // quiescence would silently lose settings changed during shutdown.
        host.snapshot_data(tx)?;
        tx.journal.migration_state="snapshot-captured".into();
        // Re-inspect immediately before the switch while retaining the global lock.
        host.inspect_network_baseline()?;tx.advance(Stage::SwitchIntent)?;
        host.checkpoint(Point::BeforeSwitch)?;
        host.activate_service(&tx.journal.candidate)?;host.inspect_service(&tx.journal.candidate)?;
        host.checkpoint(Point::AfterServiceSwitch)?;tx.advance(Stage::Activated)?;
        host.checkpoint(Point::AfterPointerSwitch)?;tx.advance(Stage::HealthPending)?;
        host.checkpoint(Point::BeforeHealth)?;host.health(tx)?;
        host.inspect_service(&tx.journal.candidate)?;host.inspect_network_baseline()?;
        host.checkpoint(Point::AfterHealth)?;
        tx.journal.health_state="verified".into();
        tx.journal.candidate.verify(&tx.version_path(&tx.journal.candidate))?;
        host.checkpoint(Point::BeforeCommit)?;tx.advance(Stage::Committed)
    })();
    if let Err(reason)=result {
        tx.record(crate::update_log::Operation::Rollback,Some(&reason));
        return match recover(tx,host) {Ok(())=>Err(format!("Update failed; previous restored: {reason}")),Err(rollback)=>Err(format!("RecoveryRequired: {rollback}; update: {reason}"))};
    }
    host.committed();Ok(())
}
pub fn recover(tx:&mut Transaction,host:&mut impl Platform)->Result<()> {
    if tx.journal.stage==Stage::Committed {
        tx.journal.candidate.verify(&tx.version_path(&tx.journal.candidate))?;
        return host.resume_service(&tx.journal.candidate);
    }
    tx.journal.previous.verify(&tx.version_path(&tx.journal.previous))?;
    if matches!(tx.journal.stage,Stage::Created|Stage::Downloading|Stage::Downloaded|Stage::Verified|Stage::Prepared|Stage::Failed) {return Ok(());}
    if tx.journal.stage!=Stage::RolledBack {
        tx.advance(Stage::RollbackPending)?;host.checkpoint(Point::BeforeRollback)?;
        host.quiesce(&tx.journal.candidate)?;host.inspect_network_baseline()?;
        host.restore_data(tx)?;
        host.activate_service(&tx.journal.previous)?;host.inspect_service(&tx.journal.previous)?;
        host.checkpoint(Point::AfterRollbackService)?;
        // Restore and inspect machine state before admitting a normal desktop.
        // Launching it under RollbackPending either trips its admission fence or
        // lets a legacy auto-connect race the final clean-network inspection.
        host.inspect_network_baseline()?;
        tx.journal.network_state="restored".into();
        tx.journal.migration_state="restored".into();
        tx.advance(Stage::RolledBack)?;host.checkpoint(Point::AfterRollbackPointer)?;
    }
    // Resume the on-demand service without replaying rollback or user data.
    host.resume_service(&tx.journal.previous)?;
    host.inspect_network_baseline()?;
    // Idempotent: the existing desktop's single-instance IPC handles retries.
    // A launch failure leaves a durable, usable previous installation and can
    // be retried after reboot without restoring settings a second time.
    host.launch_previous(&tx.journal.previous)
}

#[cfg(test)] mod tests {
    use super::*;use crate::update_transaction::{Payload,digest};use std::{fs,path::PathBuf};
    struct Fixture{root:PathBuf,tx:Transaction}
    impl Fixture {fn new()->Self {
        let root=std::env::temp_dir().join(format!("atlas-supervisor-test-{}",uuid::Uuid::new_v4()));fs::create_dir_all(root.join("versions/old")).unwrap();fs::create_dir_all(root.join("source")).unwrap();
        let version=|id:&str,dir:&std::path::Path|{fs::write(dir.join("Atlas.exe"),id).unwrap();fs::write(dir.join("runtime.dll"),format!("runtime-{id}")).unwrap();Version{absent:false,id:id.into(),version:"2.4.1".into(),build:id.into(),files:["Atlas.exe","runtime.dll"].into_iter().map(|p|{let(size,sha256)=digest(&dir.join(p)).unwrap();Payload{path:p.into(),size,sha256}}).collect()}};
        let old=version("old",&root.join("versions/old"));let new=version("new",&root.join("source"));let mut tx=Transaction::create(&root,old,new,"f".repeat(64),"old".into()).unwrap();
        for s in [Stage::Downloading,Stage::Downloaded,Stage::Verified]{tx.advance(s).unwrap();}tx.stage_payload(&root.join("source")).unwrap();Self{root,tx}
    }}
    impl Drop for Fixture{fn drop(&mut self){let _=fs::remove_dir_all(&self.root);}}
    // Controllable platform failure boundary. Filesystem/journal are real; this
    // does not claim that SCM, CEF or Windows networking have been exercised.
    struct Host {service:String,running:bool,starts:usize,restores:usize,fail:Option<Point>,health_bad:bool,network_bad:bool}
    impl Host {fn new()->Self{Self{service:"old".into(),running:true,starts:0,restores:0,fail:None,health_bad:false,network_bad:false}}}
    impl Platform for Host {
        fn checkpoint(&mut self,p:Point)->Result<()> {if self.fail==Some(p){self.fail=None;Err(format!("Injected {p:?}"))}else{Ok(())}}
        fn snapshot_data(&mut self,_:&Transaction)->Result<()>{Ok(())}
        fn quiesce(&mut self,_:&Version)->Result<()>{Ok(())}
        fn inspect_network_baseline(&mut self)->Result<()>{if self.network_bad{Err("network not restored".into())}else{Ok(())}}
        fn activate_service(&mut self,v:&Version)->Result<()>{self.service=v.id.clone();self.running=true;self.starts+=1;Ok(())}
        fn inspect_service(&mut self,v:&Version)->Result<()>{if self.service!=v.id{Err("SCM identity mismatch".into())}else if !self.running{Err("service stopped while idle".into())}else{Ok(())}}
        fn resume_service(&mut self,v:&Version)->Result<()>{
            if self.service!=v.id{return Err("SCM identity mismatch".into());}
            if !self.running{self.running=true;self.starts+=1;}
            self.inspect_service(v)
        }
        fn health(&mut self,_:&Transaction)->Result<()>{if self.health_bad{Err("candidate health failed".into())}else{Ok(())}}
        fn restore_data(&mut self,_:&Transaction)->Result<()>{self.restores+=1;Ok(())}
        fn launch_previous(&mut self,v:&Version)->Result<()>{self.inspect_service(v)}
        fn committed(&mut self){}
    }
    #[test] fn successful_commit_retains_verified_previous(){let mut f=Fixture::new();let mut h=Host::new();activate(&mut f.tx,&mut h).unwrap();assert_eq!(f.tx.journal.stage,Stage::Committed);f.tx.journal.previous.verify(&f.tx.version_path(&f.tx.journal.previous)).unwrap();}
    #[test] fn failed_first_install_restores_explicit_absence_without_fake_payload(){
        let f=Fixture::new();
        let fresh=f.root.join("fresh");fs::create_dir(&fresh).unwrap();
        let mut tx=Transaction::create(&fresh,Version::absent(),f.tx.journal.candidate.clone(),"f".repeat(64),String::new()).unwrap();
        for stage in [Stage::Downloading,Stage::Downloaded,Stage::Verified]{tx.advance(stage).unwrap();}
        tx.stage_payload(&f.root.join("source")).unwrap();
        let mut host=Host::new();host.health_bad=true;
        assert!(activate(&mut tx,&mut host).is_err());
        assert_eq!(tx.journal.stage,Stage::RolledBack);assert!(tx.journal.active.absent);
        assert!(!tx.version_path(&Version::absent()).exists());
        assert!(Transaction::open(&fresh).unwrap().journal.active.absent);
    }
    #[test] fn failure_at_every_activation_boundary_restores_complete_old(){for p in [Point::BeforeQuiesce,Point::AfterQuiesce,Point::BeforeSwitch,Point::AfterServiceSwitch,Point::AfterPointerSwitch,Point::BeforeHealth,Point::AfterHealth,Point::BeforeCommit]{let mut f=Fixture::new();let mut h=Host::new();h.fail=Some(p);assert!(activate(&mut f.tx,&mut h).is_err());assert_eq!(f.tx.journal.stage,Stage::RolledBack,"{p:?}");assert_eq!(h.service,"old");f.tx.journal.previous.verify(&f.tx.version_path(&f.tx.journal.previous)).unwrap();}}
    #[test] fn bad_health_rolls_back_but_network_failure_never_commits(){let mut f=Fixture::new();let mut h=Host::new();h.health_bad=true;assert!(activate(&mut f.tx,&mut h).is_err());assert_eq!(f.tx.journal.stage,Stage::RolledBack);let mut f=Fixture::new();let mut h=Host::new();h.network_bad=true;assert!(activate(&mut f.tx,&mut h).unwrap_err().contains("RecoveryRequired"));assert_eq!(f.tx.journal.stage,Stage::RollbackPending);h.network_bad=false;recover(&mut f.tx,&mut h).unwrap();assert_eq!(f.tx.journal.stage,Stage::RolledBack);}
    #[test] fn interrupted_rollback_resumes_from_actual_service(){for point in [Point::BeforeRollback,Point::AfterRollbackService,Point::AfterRollbackPointer]{let mut f=Fixture::new();let mut h=Host::new();for s in [Stage::Quiescing,Stage::Quiesced,Stage::SwitchIntent,Stage::Activated,Stage::HealthPending]{f.tx.advance(s).unwrap();}h.service="new".into();h.fail=Some(point);assert!(recover(&mut f.tx,&mut h).is_err());let mut reopened=Transaction::open(&f.root).unwrap();recover(&mut reopened,&mut h).unwrap();recover(&mut reopened,&mut h).unwrap();assert_eq!(h.service,"old");assert_eq!(reopened.journal.stage,Stage::RolledBack);}}

    #[test] fn recovery_resumes_idle_service_without_replaying_rollback() {
        for committed in [false,true] {
            let mut f=Fixture::new();let mut h=Host::new();
            if committed {activate(&mut f.tx,&mut h).unwrap();}
            else {for s in [Stage::Quiescing,Stage::Quiesced,Stage::SwitchIntent,Stage::Activated,Stage::HealthPending]{f.tx.advance(s).unwrap();}recover(&mut f.tx,&mut h).unwrap();}
            let stage=f.tx.journal.stage;let sequence=f.tx.journal.sequence;
            let restores=h.restores;let starts=h.starts;h.running=false;
            let mut reopened=Transaction::open(&f.root).unwrap();
            recover(&mut reopened,&mut h).unwrap();
            assert!(h.running);assert_eq!(h.starts,starts+1);assert_eq!(h.restores,restores);
            assert_eq!(reopened.journal.stage,stage);assert_eq!(reopened.journal.sequence,sequence);
            recover(&mut reopened,&mut h).unwrap();
            assert_eq!(h.starts,starts+1);assert_eq!(h.restores,restores);
        }
    }

    #[test] fn terminal_recovery_rejects_changed_identity_and_payload_before_start() {
        for committed in [false,true] {
            for corrupt_payload in [false,true] {
                let mut f=Fixture::new();let mut h=Host::new();
                if committed {activate(&mut f.tx,&mut h).unwrap();}
                else {h.health_bad=true;assert!(activate(&mut f.tx,&mut h).is_err());}
                let starts=h.starts;let restores=h.restores;h.running=false;
                if corrupt_payload {fs::write(f.tx.version_path(&f.tx.journal.active).join("Atlas.exe"),"tampered").unwrap();}
                else {h.service="foreign".into();}
                assert!(recover(&mut f.tx,&mut h).is_err());
                assert!(!h.running);assert_eq!(h.starts,starts);assert_eq!(h.restores,restores);
            }
        }
    }
}
