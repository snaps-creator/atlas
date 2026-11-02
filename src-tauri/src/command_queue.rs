//! Admission only: an in-flight IPC frame is always consumed before the next one.
use std::{collections::BTreeSet, sync::{Condvar, Mutex, atomic::{AtomicBool, Ordering}}, time::{Duration, Instant}};

#[derive(Default)]
struct State { active: bool, sequence: u64, waiting: BTreeSet<(u8,u64)> }
#[derive(Default)]
pub(crate) struct Queue { state: Mutex<State>, wake: Condvar }
pub(crate) struct Permit<'a>(&'a Queue);
fn priority(op: &str) -> u8 {
    match op {
        "stop" | "disconnect" | "quit" => 0,
        "start" | "connect" | "apply" | "select" | "save" => 1,
        "delay" | "delay_many" | "delay_results" => 3,
        _ => 2,
    }
}
impl Queue {
    pub fn acquire(&self, op: &str, timeout: Duration, running: Option<&AtomicBool>) -> Result<Permit<'_>, String> {
        let deadline=Instant::now()+timeout;
        let mut state=self.state.lock().unwrap_or_else(|e|e.into_inner());
        state.sequence+=1;
        let ticket=(priority(op),state.sequence);
        state.waiting.insert(ticket);
        loop {
            let cancelled=running.is_some_and(|v|!v.load(Ordering::SeqCst));
            if cancelled || Instant::now()>=deadline {
                state.waiting.remove(&ticket);self.wake.notify_all();
                return Err(if cancelled {"Подключение отменено"} else {"Сетевая служба занята. Повторите операцию."}.into());
            }
            if !state.active && state.waiting.first()==Some(&ticket) {
                state.waiting.remove(&ticket);state.active=true;return Ok(Permit(self));
            }
            state=self.wake.wait_timeout(state,Duration::from_millis(10)).unwrap_or_else(|e|e.into_inner()).0;
        }
    }
}
impl Drop for Permit<'_> {
    fn drop(&mut self) {
        self.0.state.lock().unwrap_or_else(|e|e.into_inner()).active=false;
        self.0.wake.notify_all();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stop_overtakes_64_queued_probes_without_interrupting_active_frame() {
        let queue=std::sync::Arc::new(Queue::default());
        let active=queue.acquire("query",Duration::from_secs(5),None).unwrap();
        let (tx,rx)=std::sync::mpsc::channel();
        let mut threads=Vec::new();
        for _ in 0..64 {
            let (queue,tx)=(queue.clone(),tx.clone());
            threads.push(std::thread::spawn(move || {let _p=queue.acquire("delay_results",Duration::from_secs(5),None).unwrap();tx.send("probe").unwrap();}));
        }
        let (q,t)=(queue.clone(),tx.clone());
        threads.push(std::thread::spawn(move || {let _p=q.acquire("stop",Duration::from_secs(5),None).unwrap();t.send("stop").unwrap();}));
        let deadline=Instant::now()+Duration::from_secs(3);
        while queue.state.lock().unwrap().waiting.len()!=65 {assert!(Instant::now()<deadline);std::thread::yield_now();}
        assert!(rx.try_recv().is_err());
        let start=Instant::now();drop(active);
        assert_eq!(rx.recv_timeout(Duration::from_secs(1)).unwrap(),"stop");
        assert!(start.elapsed()<Duration::from_secs(1));
        for t in threads {t.join().unwrap();}
    }
    #[test]
    fn cancelled_and_expired_waiters_do_not_block_later_commands() {
        let q=Queue::default();let p=q.acquire("query",Duration::from_secs(1),None).unwrap();
        assert!(q.acquire("stop",Duration::from_millis(10),None).is_err());
        assert!(q.acquire("stop",Duration::from_secs(1),Some(&AtomicBool::new(false))).is_err());
        drop(p);assert!(q.acquire("query",Duration::from_secs(1),None).is_ok());
    }
}
