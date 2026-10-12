use std::{path::Path, time::{Duration,Instant}};
use windows_sys::Win32::{Foundation::*,UI::{Shell::*,WindowsAndMessaging::*}};
#[path="../../../src-tauri/src/update_user.rs"] mod update_user;
mod update_transaction {
    pub fn digest(_: &std::path::Path)->Result<String,String>{Err("Not available in the tray diagnostic fixture".into())}
}
fn root(parent:u32)->std::path::PathBuf {
    update_user::database().unwrap().parent().unwrap().parent().unwrap().join(format!("AtlasTrayRootProbe-{parent}"))
}
unsafe fn query(window:usize,id:u32)->serde_json::Value {
    let nid=NOTIFYICONIDENTIFIER{cbSize:std::mem::size_of::<NOTIFYICONIDENTIFIER>() as u32,hWnd:window as _,uID:id,..std::mem::zeroed()};
    let mut rect=std::mem::zeroed();SetLastError(0);
    let hr=Shell_NotifyIconGetRect(&nid,&mut rect);
    serde_json::json!({"hr":format!("0x{:08X}",hr as u32),"error":GetLastError(),"rect":[rect.left,rect.top,rect.right,rect.bottom]})
}
fn probe(directory:&Path,label:&str) {
    let icon=tray_icon::Icon::from_rgba(vec![64,128,255,255].repeat(256),16,16).unwrap();
    let tray=tray_icon::TrayIconBuilder::new().with_icon(icon).with_tooltip("Atlas isolated root probe").build().unwrap();
    let window=tray.window_handle() as usize;
    let (added,error,rect)=unsafe{
        let mut data=NOTIFYICONDATAW{cbSize:std::mem::size_of::<NOTIFYICONDATAW>() as u32,hWnd:window as _,uID:101,uFlags:NIF_ICON|NIF_MESSAGE|NIF_TIP,uCallbackMessage:6002,hIcon:LoadIconW(std::ptr::null_mut(),IDI_APPLICATION),..std::mem::zeroed()};
        for (to,from) in data.szTip.iter_mut().zip("Atlas isolated native registration probe".encode_utf16()){*to=from;}
        SetLastError(0);let added=Shell_NotifyIconW(NIM_ADD,&data);let error=GetLastError();
        (added,error,query(window,101))
    };
    let report=serde_json::json!({"label":label,"token":update_user::report().unwrap(),"hwnd":window,"id":101,"notifyAdd":added,"notifyError":error,"nativeRect":rect,"libraryRect":format!("{:?}",tray.rect())});
    std::fs::write(directory.join(format!("{label}.json")),serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    let deadline=Instant::now()+Duration::from_secs(20);
    while !directory.join(format!("{label}.stop")).exists() && Instant::now()<deadline {
        unsafe {let mut msg=std::mem::zeroed();while PeekMessageW(&mut msg,std::ptr::null_mut(),0,0,PM_REMOVE)!=0 {TranslateMessage(&msg);DispatchMessageW(&msg);}}
        std::thread::sleep(Duration::from_millis(20));
    }
    unsafe {let data=NOTIFYICONDATAW{cbSize:std::mem::size_of::<NOTIFYICONDATAW>() as u32,hWnd:window as _,uID:101,..std::mem::zeroed()};Shell_NotifyIconW(NIM_DELETE,&data);}
}
fn main(){
    if std::env::args().any(|a|a=="--shell-linked-bootstrap") {
        let child=update_user::diagnostic_launch_linked_shell().unwrap();println!("LINKED_SHELL_STARTED={}",child.pid);return;
    }
    if std::env::args().any(|a|a=="--shell-bootstrap" || a=="--shell-owner-bootstrap") {
        assert_eq!(std::env::var("GITHUB_ACTIONS").unwrap_or_default(),"true","Shell experiment only on a disposable GitHub runner");
        let child=update_user::diagnostic_launch_shell(std::env::args().any(|a|a=="--shell-owner-bootstrap")).unwrap();
        use std::os::windows::io::AsRawHandle;
        let deadline=Instant::now()+Duration::from_secs(15);
        while Instant::now()<deadline {
            let mut pid=0;unsafe{GetWindowThreadProcessId(GetShellWindow(),&mut pid);}
            let waited=unsafe{windows_sys::Win32::System::Threading::WaitForSingleObject(child.as_raw_handle(),0)};
            if pid==child.pid {println!("MEDIUM_SHELL_READY={pid}");return;}
            if waited==WAIT_OBJECT_0 {
                let mut code=0;unsafe{windows_sys::Win32::System::Threading::GetExitCodeProcess(child.as_raw_handle(),&mut code);}
                println!("MEDIUM_SHELL_EXIT=0x{code:08X}");return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        println!("MEDIUM_SHELL_DEADLINE; pid={}",child.pid);child.terminate();return;
    }
    if let Some(parent)=std::env::args().find_map(|a|a.strip_prefix("--parent=").map(|v|v.parse::<u32>().unwrap())) {
        probe(&root(parent),"medium");return;
    }
    let directory=root(std::process::id());std::fs::create_dir_all(&directory).unwrap();
    // Parent and child are the same executable and same original user/session.
    // The production updater's actual token creation is included unchanged.
    let parent=std::process::id();let worker=std::thread::spawn(move||{
        let dir=root(parent);std::fs::write(dir.join("parent.stop"),b"").unwrap();probe(&dir,"parent");
    });
    worker.join().unwrap();
    let arg=format!("--parent={parent}");
    let exe=std::env::current_exe().unwrap();
    match update_user::limited(&exe,&[&arg,"--user-context-report"]).unwrap(){
        Some((child,_pipe))=>{
            let deadline=Instant::now()+Duration::from_secs(15);
            while !directory.join("medium.json").exists() && Instant::now()<deadline{std::thread::sleep(Duration::from_millis(50));}
            let data:serde_json::Value=serde_json::from_slice(&std::fs::read(directory.join("medium.json")).expect("Medium diagnostic child must report")).unwrap();
            let rect=unsafe{query(data["hwnd"].as_u64().unwrap() as usize,101)};
            std::fs::write(directory.join("parent-query-medium.json"),serde_json::to_vec_pretty(&rect).unwrap()).unwrap();
            std::fs::write(directory.join("medium.stop"),b"").unwrap();
            use std::os::windows::io::AsRawHandle;
            assert_eq!(unsafe{windows_sys::Win32::System::Threading::WaitForSingleObject(child.as_raw_handle(),5000)},WAIT_OBJECT_0);
        }
        None=>println!("Parent already medium; no elevated-to-medium transition"),
    }
    println!("DIAGNOSTIC_ROOT={}",directory.display());
    for file in std::fs::read_dir(&directory).unwrap(){let path=file.unwrap().path();if path.extension().is_some_and(|v|v=="json"){println!("{}\n{}",path.display(),std::fs::read_to_string(&path).unwrap());}}
}
