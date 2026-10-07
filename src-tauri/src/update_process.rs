use std::{io::Read,path::Path,process::{Command,Stdio},time::{Duration,Instant}};
use std::os::windows::{io::AsRawHandle,process::CommandExt};
use windows_sys::Win32::{System::{Pipes::PeekNamedPipe,Threading::WaitForSingleObject},Foundation::WAIT_OBJECT_0};
pub fn status(executable:&Path,args:&[&str],timeout:Duration)->Result<(),String>{
    let mut child=Command::new(executable).args(args).current_dir(executable.parent().ok_or("Missing helper directory")?)
        .creation_flags(0x08000000).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()
        .map_err(|e|format!("Cannot start installation helper: {e}"))?;
    if unsafe{WaitForSingleObject(child.as_raw_handle(),timeout.as_millis().min(u32::MAX as u128) as u32)}!=WAIT_OBJECT_0 {
        let _=child.kill();unsafe{WaitForSingleObject(child.as_raw_handle(),5000);}
        return Err("Installation helper exceeded its deadline".into());
    }
    let result=child.wait().map_err(|e|e.to_string())?;
    if result.success(){Ok(())}else{Err(format!("Installation helper failed: {result}"))}
}
pub fn json(executable:&Path,args:&[&str],timeout:Duration)->Result<serde_json::Value,String> {
    let mut child=Command::new(executable).args(args).creation_flags(0x08000000).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().map_err(|_|"Update helper launch failed")?;
    let mut pipe=child.stdout.take().ok_or("Update helper stdout missing")?;let deadline=Instant::now()+timeout;let mut bytes=Vec::new();
    let result=(||{
        loop {
            let mut available=0u32;
            let alive=unsafe{WaitForSingleObject(child.as_raw_handle(),0)}!=WAIT_OBJECT_0;
            let pipe_ok=unsafe{PeekNamedPipe(pipe.as_raw_handle(),std::ptr::null_mut(),0,std::ptr::null_mut(),&mut available,std::ptr::null_mut())}!=0;
            if available>0 {
                if bytes.len()+available as usize>65536{return Err("Update helper output limit exceeded".into());}
                let mut buffer=vec![0;available as usize];pipe.read_exact(&mut buffer).map_err(|_|"Update helper IPC read failed")?;bytes.extend(buffer);
            } else if !alive || !pipe_ok {break;}
            if Instant::now()>=deadline{return Err("Update helper timeout".into());}
            unsafe{WaitForSingleObject(child.as_raw_handle(),25);}
        }
        let status=child.try_wait().map_err(|_|"Update helper status failed")?.ok_or("Update helper closed IPC before exit")?;
        if !status.success(){return Err("Update helper returned failure".into());}
        serde_json::from_slice(&bytes).map_err(|_|"Invalid update helper reply".into())
    })();
    if result.is_err(){
        let _=child.kill();
        if unsafe{WaitForSingleObject(child.as_raw_handle(),5000)}==WAIT_OBJECT_0 {let _=child.wait();}
    }result
}
