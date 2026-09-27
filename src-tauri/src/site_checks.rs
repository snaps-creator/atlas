use std::sync::{Mutex, MutexGuard, TryLockError};
use std::time::{Duration, Instant};

pub fn wait_for_state<T>(state: &Mutex<T>, timeout: Duration) -> Result<MutexGuard<'_, T>, String> {
    let deadline = Instant::now() + timeout;
    loop {
        match state.try_lock() {
            Ok(value) => return Ok(value),
            Err(TryLockError::Poisoned(_)) => return Err("Состояние Atlas недоступно".into()),
            Err(TryLockError::WouldBlock) if Instant::now() >= deadline =>
                return Err("Atlas занят: проверка сервиса не началась за 2 секунды".into()),
            Err(TryLockError::WouldBlock) => std::thread::sleep(Duration::from_millis(10)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn concurrent_checks_wait_for_temporary_contention() {
        let state = Mutex::new(true);
        std::thread::scope(|scope| {
            let guard = state.lock().unwrap();
            let workers: Vec<_> = (0..5).map(|_| scope.spawn(|| {
                assert!(*wait_for_state(&state, Duration::from_secs(2)).unwrap());
            })).collect();
            std::thread::sleep(Duration::from_millis(50));
            drop(guard);
            for worker in workers { worker.join().unwrap(); }
        });
    }
    #[test]
    fn prolonged_contention_is_a_bounded_local_error() {
        let state = Mutex::new(());
        let _guard = state.lock().unwrap();
        assert!(wait_for_state(&state, Duration::from_millis(20)).unwrap_err().contains("не началась"));
    }
}
