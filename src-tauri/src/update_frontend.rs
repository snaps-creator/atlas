//! Native update handoff never holds the application/network state mutex.
use std::{io::{BufRead,BufReader},process::{Command,Stdio},os::windows::process::CommandExt,sync::atomic::{AtomicBool,Ordering}};
use tauri::Emitter;
static ACTIVE:AtomicBool=AtomicBool::new(false);
pub fn start(app:tauri::AppHandle,version:&str)->Result<(),String>{
    if version.len()>40 || !version.bytes().all(|b|b.is_ascii_digit()||b==b'.') {return Err("Некорректная версия обновления".into());}
    if ACTIVE.swap(true,Ordering::SeqCst){return Err("Обновление уже выполняется".into());}
    let result:Result<(),String>=(||{
        let source=std::env::current_exe().map_err(|_|"Не найден каталог Atlas")?.with_file_name("AtlasUpdater.exe");
        // The installer terminates every owned updater under the installation.
        // Keep its authenticated download/pinned package outside that tree so
        // quiescence cannot kill the process waiting for the installer to finish.
        let work=std::env::temp_dir().join(format!("atlas-update-worker-{}",uuid::Uuid::new_v4()));
        std::fs::create_dir(&work).map_err(|_|"Не удалось создать каталог обновлятора")?;
        let executable=work.join("AtlasUpdater.exe");
        if std::fs::copy(source,&executable).is_err(){let _=std::fs::remove_dir_all(&work);return Err("Не удалось подготовить обновлятор Atlas".into());}
        let mut child=match Command::new(executable).args(["--download-install",version]).creation_flags(0x08000000)
            .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn(){
                Ok(child)=>child,
                Err(_)=>{let _=std::fs::remove_dir_all(&work);return Err("Не удалось запустить обновлятор Atlas".into());}
            };
        let output=child.stdout.take().ok_or("Канал обновлятора недоступен")?;
        std::thread::spawn(move||{
            let mut reported_error=false;
            // Each native message is bounded; never publish arbitrary process output.
            let mut reader=BufReader::new(output);let mut buffer=Vec::new();
            loop {
                buffer.clear();
                let read=std::io::Read::take(&mut reader,8193).read_until(b'\n',&mut buffer);
                if !matches!(read,Ok(n) if n>0) || buffer.len()>8192 {break;}
                if let Ok(value)=serde_json::from_slice::<serde_json::Value>(&buffer){
                    if ["downloading","installing","complete","error"].contains(&value["stage"].as_str().unwrap_or("")) {
                        reported_error|=value["stage"]=="error";
                        let _=app.emit("atlas-update-progress",value);
                    }
                }
            }
            if !child.wait().is_ok_and(|status|status.success()) && !reported_error {
                let _=app.emit("atlas-update-progress",serde_json::json!({"stage":"error","message":"Обновлятор завершился с ошибкой; текущая версия сохранена"}));
            }
            ACTIVE.store(false,Ordering::SeqCst);
            let _=std::fs::remove_dir_all(work);
        });
        Ok(())
    })();
    if result.is_err(){ACTIVE.store(false,Ordering::SeqCst);}result
}
