//! Opt-in debug acceptance. No App, database, startup registration, service or
//! network initialization. All identifiers/cache/shortcuts belong to this run.
use std::{path::Path, time::{Duration, Instant}};
use tauri::Manager;

pub fn run(directory: &Path) -> Result<(), String> {
    let token=directory.file_name().and_then(|v|v.to_str()).ok_or("Missing fixture directory")?;
    uuid::Uuid::parse_str(token).map_err(|_|"Fixture directory must be a fresh UUID")?;
    if !directory.is_absolute() || !directory.is_dir() { return Err("Fixture directory must exist".into()); }
    let id=format!("net.atlasvpn.lifecycle.{token}");
    let mut admission=match crate::window_activation::admit(&id)? {Some(g)=>Some(g),None=>return Ok(())};
    let mut context=tauri::generate_context!();
    context.config_mut().identifier=id;
    context.config_mut().app.windows[0].title="Atlas isolated lifecycle fixture".into();
    context.config_mut().app.windows[0].visible=false;
    context.config_mut().app.windows[0].url=tauri::WebviewUrl::External("about:blank".parse().unwrap());
    let root=directory.to_path_buf();
    tauri::Builder::default()
        .runtime(tauri_runtime_cef::Cef::default().root_cache_path(directory.join("cef")))
        .manage(crate::window_activation::Activation::default())
        .plugin(tauri_plugin_single_instance::init(|app,_,_|crate::window_activation::request(app)))
        .setup(|app| {
            let window=app.get_webview_window("main").unwrap();
            crate::shell_identity::window_with_id(window.hwnd()?.0 as isize,&std::env::current_exe()?,&app.config().identifier).map_err(std::io::Error::other)?;
            // An activation arriving before readiness must survive startup hide.
            crate::window_activation::request(app.handle());
            window.hide()?;
            Ok(())
        })
        .on_window_event(|window,event| {
            if let tauri::WindowEvent::CloseRequested{api,..}=event {api.prevent_close();let _=window.hide();}
        })
        .build(context).map_err(|e|e.to_string())?
        .run(move |app,event| {
            if matches!(event,tauri::RunEvent::Ready) {
                crate::window_activation::ready(app);
                drop(admission.take());
                let app=app.clone();let root=root.clone();
                std::thread::spawn(move || {
                    let result=validate(&app,&root);
                    let record=match &result {Ok(value)=>value.clone(),Err(error)=>serde_json::json!({"passed":false,"error":error})};
                    let _=std::fs::write(root.join("report.json"),serde_json::to_vec_pretty(&record).unwrap());
                    app.exit(if result.is_ok(){0}else{1});
                });
            }
        });
    Ok(())
}
fn until(mut check: impl FnMut()->bool) -> Result<(),String> {
    let deadline=Instant::now()+Duration::from_secs(8);
    while !check() {
        if Instant::now()>=deadline {return Err("Isolated lifecycle assertion timed out".into());}
        std::thread::sleep(Duration::from_millis(20)); // test observation only
    }
    Ok(())
}
fn validate(app:&tauri::AppHandle,root:&Path)->Result<serde_json::Value,String> {
    use std::os::windows::process::CommandExt;
    let window=app.get_webview_window("main").ok_or("Missing fixture window")?;
    let visible=||window.is_visible().unwrap_or(false) && !window.is_minimized().unwrap_or(true);
    until(visible)?;
    let exe=std::env::current_exe().map_err(|e|e.to_string())?;
    let shell=verify_shell(&window,root,&exe,&app.config().identifier)?;
    let launch=||std::process::Command::new(&exe).creation_flags(0x08000000)
        .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
        .spawn().map_err(|e|e.to_string());
    let wait=|child:&mut std::process::Child|->Result<(),String>{
        let mut status=None;
        let result=until(||{status=child.try_wait().ok().flatten();status.is_some()});
        if result.is_err(){let _=child.kill();let _=child.wait();}
        result?;
        if !status.unwrap().success(){return Err("Secondary fixture exited unsuccessfully".into());}Ok(())
    };
    let mut scenarios=Vec::new();
    scenarios.push("activation queued before main window readiness");
    wait(&mut launch()?)?;until(visible)?;scenarios.push("second launch while visible");
    window.close().map_err(|e|e.to_string())?;
    until(||!window.is_visible().unwrap_or(true))?;
    wait(&mut launch()?)?;until(visible)?;scenarios.push("close to tray then second launch");
    window.hide().map_err(|e|e.to_string())?;until(||!window.is_visible().unwrap_or(true))?;
    launch_shortcut(&root.join("fixture.lnk"))?;until(visible)?;
    scenarios.push("real ShellExecute of isolated .lnk restores hidden window");
    window.minimize().map_err(|e|e.to_string())?;until(||window.is_minimized().unwrap_or(false))?;
    window.hide().map_err(|e|e.to_string())?;until(||!window.is_visible().unwrap_or(true))?;
    wait(&mut launch()?)?;until(visible)?;scenarios.push("hidden and minimized then second launch");
    window.hide().map_err(|e|e.to_string())?;until(||!window.is_visible().unwrap_or(true))?;
    crate::window_activation::request(app);until(visible)?;scenarios.push("shared tray restore handler");
    window.hide().map_err(|e|e.to_string())?;until(||!window.is_visible().unwrap_or(true))?;
    let mut children=Vec::new();
    for _ in 0..8 {children.push(launch()?);}
    for child in &mut children {wait(child)?;}
    until(visible)?;scenarios.push("eight rapid second launches");
    if app.webview_windows().len()!=1 {return Err("Duplicate fixture windows".into());}
    Ok(serde_json::json!({"passed":true,"scenarios":scenarios,"pid":std::process::id(),
        "oneWindow":true,"focused":window.is_focused().unwrap_or(false),"shell":shell,"productionSetupExecuted":false}))
}

fn verify_shell(window:&tauri::WebviewWindow,root:&Path,exe:&Path,id:&str)->Result<serde_json::Value,String> {
    use windows_api::{core::{Interface,PCWSTR},Win32::{Foundation::HWND,
        Storage::EnhancedStorage::{PKEY_AppUserModel_ID,PKEY_AppUserModel_RelaunchCommand},
        System::Com::{CoInitializeEx,CoUninitialize,CoCreateInstance,COINIT_APARTMENTTHREADED,CLSCTX_INPROC_SERVER,IPersistFile},
        UI::Shell::{ShellLink,IShellLinkW,PropertiesSystem::{SHGetPropertyStoreForWindow,IPropertyStore}}}};
    use std::os::windows::ffi::OsStrExt;
    unsafe {
        CoInitializeEx(None,COINIT_APARTMENTTHREADED).ok().map_err(|e|e.to_string())?;
        let result=(||->Result<serde_json::Value,String>{
            let store:IPropertyStore=SHGetPropertyStoreForWindow(HWND(window.hwnd().map_err(|e|e.to_string())?.0 as _)).map_err(|e|e.to_string())?;
            let actual_id=store.GetValue(&PKEY_AppUserModel_ID).map_err(|e|e.to_string())?.to_string();
            let actual_command=store.GetValue(&PKEY_AppUserModel_RelaunchCommand).map_err(|e|e.to_string())?.to_string();
            if actual_id!=id || actual_command!=format!("\"{}\"",crate::shell_identity::launcher(exe)?.display()) {
                return Err("Window Shell identity/relaunch properties differ".into());
            }
            let link:IShellLinkW=CoCreateInstance(&ShellLink,None,CLSCTX_INPROC_SERVER).map_err(|e|e.to_string())?;
            let target:Vec<u16>=exe.as_os_str().encode_wide().chain(Some(0)).collect();
            link.SetPath(PCWSTR(target.as_ptr())).map_err(|e|e.to_string())?;
            let file:IPersistFile=link.cast().map_err(|e|e.to_string())?;
            let path:Vec<u16>=root.join("fixture.lnk").as_os_str().encode_wide().chain(Some(0)).collect();
            file.Save(PCWSTR(path.as_ptr()),true).map_err(|e|e.to_string())?;
            Ok(serde_json::json!({"id":actual_id,"command":actual_command,"readBack":true}))
        })();
        CoUninitialize();result
    }
}
fn launch_shortcut(path:&Path)->Result<(),String>{
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::{UI::{Shell::*,WindowsAndMessaging::SW_HIDE},Foundation::*,System::Threading::*};
    let path:Vec<u16>=path.as_os_str().encode_wide().chain(Some(0)).collect();
    unsafe {
        let mut launch:SHELLEXECUTEINFOW=std::mem::zeroed();
        launch.cbSize=std::mem::size_of_val(&launch) as u32;launch.fMask=SEE_MASK_NOCLOSEPROCESS|SEE_MASK_NOASYNC;
        launch.lpFile=path.as_ptr();launch.nShow=SW_HIDE;
        if ShellExecuteExW(&mut launch)==0 {return Err(format!("Fixture shortcut launch: {}",std::io::Error::last_os_error()));}
        if launch.hProcess.is_null(){return Err("Missing fixture shortcut child handle".into());}
        let wait=WaitForSingleObject(launch.hProcess,10_000);
        let mut code=1;GetExitCodeProcess(launch.hProcess,&mut code);
        if wait!=WAIT_OBJECT_0 {TerminateProcess(launch.hProcess,1);WaitForSingleObject(launch.hProcess,5000);}
        CloseHandle(launch.hProcess);
        if wait!=WAIT_OBJECT_0 || code!=0{return Err("Fixture shortcut child failed".into());}
        Ok(())
    }
}
