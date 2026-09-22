//! Exercises the production exit deadline, parent watcher and Windows Job
//! together. All processes are this test binary; no installed service or TUN.
use std::os::windows::{
    io::{AsRawHandle, FromRawHandle, OwnedHandle},
    process::CommandExt,
};
use std::{
    fs,
    path::Path,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::WAIT_OBJECT_0,
    Storage::FileSystem::SYNCHRONIZE,
    System::Threading::{OpenProcess, TerminateProcess, WaitForSingleObject, PROCESS_TERMINATE},
};

fn spawn(role: &str, dir: &Path, parent: u32) -> Child {
    Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "shutdown::process_tests::process_fixture"])
        .env("ATLAS_EXIT_FIXTURE_ROLE", role)
        .env("ATLAS_EXIT_FIXTURE_DIR", dir)
        .env("ATLAS_EXIT_FIXTURE_PARENT", parent.to_string())
        .creation_flags(0x08000000)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}

fn wait_file(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !path.exists() {
        assert!(
            Instant::now() < deadline,
            "fixture did not create {}",
            path.display()
        );
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn process_fixture() {
    let Ok(role) = std::env::var("ATLAS_EXIT_FIXTURE_ROLE") else {
        return;
    };
    // Last-resort cleanup if the test runner itself dies or setup fails.
    thread::spawn(|| {
        thread::sleep(Duration::from_secs(25));
        std::process::exit(99);
    });
    let dir = std::path::PathBuf::from(std::env::var_os("ATLAS_EXIT_FIXTURE_DIR").unwrap());
    let parent = std::env::var("ATLAS_EXIT_FIXTURE_PARENT")
        .unwrap()
        .parse()
        .unwrap();
    let pid_file = dir.join(format!("{role}.pid.tmp"));
    fs::write(&pid_file, std::process::id().to_string()).unwrap();
    fs::rename(pid_file, dir.join(format!("{role}.pid"))).unwrap();
    if role == "desktop" {
        let _service = spawn("service", &dir, std::process::id());
        wait_file(&dir.join("go"));
        super::begin(
            Duration::from_millis(100),
            || loop {
                thread::park();
            },
            || std::process::exit(0),
        );
    } else if role == "service" {
        let core = spawn("core", &dir, std::process::id());
        let _job = crate::job::Job::attach(&core).unwrap();
        let _watch = crate::service::watch_session(parent).unwrap();
        let _observer = spawn("observer", &dir, std::process::id());
        wait_file(&dir.join("observer-ready"));
        fs::write(dir.join("ready"), []).unwrap();
        loop {
            thread::park();
        } // Simulate a permanently blocked command loop.
    } else if role == "observer" {
        let handle = unsafe { OpenProcess(SYNCHRONIZE, 0, parent) };
        assert!(!handle.is_null());
        let handle = unsafe { OwnedHandle::from_raw_handle(handle) };
        fs::write(dir.join("observer-ready"), []).unwrap();
        crate::session_cleanup::after_process_exit(&handle, || {
            crate::session_cleanup::flush_dns()?; // No-op in tests.
            fs::write(dir.join("cleaned"), []).map_err(|e| e.to_string())
        })
        .unwrap();
        std::process::exit(0);
    }
    loop {
        thread::park();
    }
}

struct Process(OwnedHandle);
impl Drop for Process {
    fn drop(&mut self) {
        // Only exact handles belonging to this test; never enumerate by name.
        unsafe {
            TerminateProcess(self.0.as_raw_handle(), 98);
        }
    }
}

#[test]
fn exit_terminates_desktop_blocked_service_core_and_cleanup_observer() {
    let dir = std::env::temp_dir().join(format!("atlas-exit-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    let mut desktop = spawn("desktop", &dir, 0);
    wait_file(&dir.join("ready"));
    let processes: Vec<_> = ["desktop", "service", "core", "observer"]
        .into_iter()
        .map(|role| {
            wait_file(&dir.join(format!("{role}.pid")));
            let pid = fs::read_to_string(dir.join(format!("{role}.pid")))
                .unwrap()
                .parse()
                .unwrap();
            let handle = unsafe { OpenProcess(SYNCHRONIZE | PROCESS_TERMINATE, 0, pid) };
            assert!(!handle.is_null());
            (
                role,
                Process(unsafe { OwnedHandle::from_raw_handle(handle) }),
            )
        })
        .collect();
    fs::write(dir.join("go"), []).unwrap();
    let deadline = Instant::now() + Duration::from_secs(9);
    for (role, process) in &processes {
        let remaining = deadline
            .saturating_duration_since(Instant::now())
            .as_millis() as u32;
        assert_eq!(
            unsafe { WaitForSingleObject(process.0.as_raw_handle(), remaining) },
            WAIT_OBJECT_0,
            "{role} survived the shutdown deadline"
        );
    }
    assert!(desktop.wait().unwrap().success());
    assert!(
        dir.join("cleaned").exists(),
        "observer skipped its cleanup callback"
    );
    drop(processes);
    fs::remove_dir_all(dir).unwrap();
}
