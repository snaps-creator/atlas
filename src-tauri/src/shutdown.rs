use std::time::Duration;

#[cfg(test)]
#[path = "shutdown_process_tests.rs"]
mod process_tests;

/// The deadline must be independent of both the UI loop and the worker pool.
/// Keep it armed even after graceful cleanup: posting Exit can itself stall.
pub(crate) fn begin(
    deadline: Duration,
    graceful: impl FnOnce() + Send + 'static,
    force: impl FnOnce() + Send + 'static,
) {
    std::thread::spawn(move || {
        std::thread::sleep(deadline);
        force();
    });
    std::thread::spawn(graceful);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{mpsc, Arc, Mutex};

    #[test]
    fn blocked_application_lock_cannot_block_exit_deadline() {
        let state = Arc::new(Mutex::new(()));
        let lock = state.lock().unwrap();
        let worker = state.clone();
        let (forced, received) = mpsc::channel();
        let (done, completed) = mpsc::channel();
        begin(
            Duration::from_millis(100),
            move || {
                let _lock = worker.lock().unwrap();
                done.send(()).unwrap();
            },
            move || {
                forced.send(()).unwrap();
            },
        );
        received.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(completed.try_recv().is_err());
        drop(lock);
        completed.recv_timeout(Duration::from_secs(2)).unwrap();
    }

    #[test]
    fn exit_event_delivery_stall_keeps_deadline_armed() {
        let (forced, received) = mpsc::channel();
        begin(
            Duration::from_millis(50),
            || {},
            move || {
                forced.send(()).unwrap();
            },
        );
        received.recv_timeout(Duration::from_secs(2)).unwrap();
    }

    #[test]
    fn stalled_exit_process_fixture() {
        if std::env::var_os("ATLAS_TEST_STALLED_EXIT").is_none() {
            return;
        }
        begin(
            Duration::from_millis(100),
            || {
                std::thread::park();
            },
            || std::process::exit(73),
        );
        loop {
            std::thread::park();
        }
    }

    #[test]
    fn deadline_really_terminates_a_stalled_process() {
        use std::os::windows::process::CommandExt;
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "shutdown::tests::stalled_exit_process_fixture"])
            .env("ATLAS_TEST_STALLED_EXIT", "1")
            .creation_flags(0x08000000)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert_eq!(status.code(), Some(73));
                break;
            }
            if std::time::Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("shutdown deadline left the fixture running");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
