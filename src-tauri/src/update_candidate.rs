use std::{io::Read,os::windows::{io::{AsRawHandle,IntoRawHandle,FromRawHandle},process::CommandExt},path::Path,process::{Child,Command,Stdio},time::{Duration,Instant}};
use windows_sys::Win32::{Foundation::{WAIT_OBJECT_0,WAIT_TIMEOUT},System::{Pipes::PeekNamedPipe,Threading::WaitForSingleObject}};
use crate::update_health::Report;
enum Desktop {Standard(Child), Limited(crate::update_user::Process)}
impl AsRawHandle for Desktop {fn as_raw_handle(&self)->std::os::windows::io::RawHandle {match self {Self::Standard(child)=>child.as_raw_handle(),Self::Limited(child)=>child.as_raw_handle()}}}
impl Desktop {
    fn id(&self)->u32{match self{Self::Standard(child)=>child.id(),Self::Limited(child)=>child.pid}}
    fn terminate(&mut self){match self {
        Self::Standard(child)=>{let _=child.kill();if unsafe{WaitForSingleObject(child.as_raw_handle(),5000)}==WAIT_OBJECT_0 {let _=child.wait();}},
        Self::Limited(child)=>child.terminate(),
    }}
}
pub struct Candidate {child:Desktop,keep:bool,pub report:Report}
impl Drop for Candidate {fn drop(&mut self){if !self.keep{self.child.terminate();}}}
impl Candidate {
    pub fn commit(&mut self){self.keep=true;}
    pub fn launch(executable:&Path,transaction:&str,nonce:&str,version:&str,build:&str)->Result<Self,String>{
        if let Some((child,stdout))=crate::update_user::limited(executable,&["--update-health",transaction,nonce])? {
            return Self::from_parts(Desktop::Limited(child),stdout,transaction,nonce,version,build,Duration::from_secs(45));
        }
        let mut command=Command::new(executable);command.args(["--update-health",transaction,nonce]);
        command.current_dir(executable.parent().ok_or("Candidate installation directory missing")?);
        Self::from_command(command,transaction,nonce,version,build,Duration::from_secs(45))
    }
    fn from_command(mut command:Command,transaction:&str,nonce:&str,version:&str,build:&str,timeout:Duration)->Result<Self,String>{
        let mut child=command.creation_flags(0x08000000).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().map_err(|_|"Candidate launch failed")?;
        let stdout=child.stdout.take().ok_or("Candidate health channel unavailable")?;
        let stdout=unsafe{std::fs::File::from_raw_handle(stdout.into_raw_handle())};
        Self::from_parts(Desktop::Standard(child),stdout,transaction,nonce,version,build,timeout)
    }
    fn from_parts(mut child:Desktop,mut stdout:std::fs::File,transaction:&str,nonce:&str,version:&str,build:&str,timeout:Duration)->Result<Self,String>{
        let result=(||{
            let deadline=Instant::now()+timeout;let mut bytes=Vec::new();
            loop {
                if unsafe{WaitForSingleObject(child.as_raw_handle(),0)}==WAIT_OBJECT_0{return Err("Candidate exited before health completion".into());}
                let mut count=0;
                if unsafe{PeekNamedPipe(stdout.as_raw_handle(),std::ptr::null_mut(),0,std::ptr::null_mut(),&mut count,std::ptr::null_mut())}==0{return Err("Candidate health channel closed".into());}
                if count>0 {
                    if bytes.len()+count as usize>65536{return Err("Candidate health output exceeded limit".into());}
                    let mut chunk=vec![0;count as usize];stdout.read_exact(&mut chunk).map_err(|_|"Candidate health read failed")?;bytes.extend(chunk);
                    for line in bytes.split(|b|*b==b'\n').filter(|line|!line.is_empty()) {
                        if let Some(json)=line.strip_prefix(b"ATLAS_HEALTH_V1:") {
                            if let Ok(report)=serde_json::from_slice::<Report>(json) {
                                report.validate(transaction,nonce,version,build,child.id())?;
                                // A ready reply followed by an immediate crash is failure.
                                if unsafe{WaitForSingleObject(child.as_raw_handle(),5000)}!=WAIT_TIMEOUT{return Err("Candidate failed stability window".into());}
                                return Ok(report);
                            }
                        }
                    }
                }
                if Instant::now()>=deadline{return Err("Candidate health timeout".into());}
                unsafe{WaitForSingleObject(child.as_raw_handle(),25);}
            }
        })();
        match result {Ok(report)=>Ok(Self{child,keep:false,report}),Err(e)=>{child.terminate();Err(e)}}
    }
}
#[cfg(test)]mod tests {
    use super::*;
    #[test]fn candidate_child_driver(){
        let Ok(mode)=std::env::var("ATLAS_TEST_HEALTH_CHILD") else{return;};
        let report=Report{transaction_id:"tx".into(),nonce:if mode=="wrong-nonce"{"wrong"}else{"nonce"}.into(),version:"2.4.1".into(),build:"build".into(),pid:std::process::id(),service_pid:123,ui_ready:true,settings_readable:true,subscriptions_readable:true,service_ready:true,helper_protocol:1};
        // With one test thread libtest prints its test name without a newline.
        // The actual desktop emits a standalone protocol line as well.
        println!("\nATLAS_HEALTH_V1:{}",serde_json::to_string(&report).unwrap());
        use std::io::Write;std::io::stdout().flush().unwrap();if mode=="crash"{std::process::exit(7);}std::thread::park();
    }
    #[test]fn real_child_health_rejects_spoof_and_crash_and_holds_live_process(){
        for mode in ["wrong-nonce","crash","healthy"] {
            let mut command=Command::new(std::env::current_exe().unwrap());command.args(["--exact","update_candidate::tests::candidate_child_driver","--nocapture"]).env("ATLAS_TEST_HEALTH_CHILD",mode);
            let result=Candidate::from_command(command,"tx","nonce","2.4.1","build",Duration::from_secs(10));
            if mode=="healthy" {let c=result.unwrap();assert_eq!(unsafe{WaitForSingleObject(c.child.as_raw_handle(),0)},WAIT_TIMEOUT);drop(c);}else{assert!(result.is_err());}
        }
    }
}
