//! Separate bounded admission for URL tests and operational controller reads.
use serde_json::Value;
use std::{collections::{HashMap,HashSet}, sync::{atomic::{AtomicBool,Ordering},Arc,Mutex,mpsc}, time::{Duration, Instant}};
type Reply = Result<Value, String>;
const PER_CLASS: usize = 16;
struct WorkState { subscribers: Vec<mpsc::Sender<Reply>>, completed: Option<Reply> }
struct Work { cancelled: Arc<AtomicBool>, state: Mutex<WorkState> }
struct Job { started: Instant, delay: bool, key: Option<String>, work: Arc<Work>, rx: mpsc::Receiver<Reply> }
#[derive(Default)]
pub struct QueryJobs {
    jobs: HashMap<String, Job>,
    epoch: u64,
}
impl QueryJobs {
    /// A selector change invalidates route observations, not URL tests of
    /// concrete nodes. Group tests depend on the selection and are cancelled.
    pub fn selection_changed(&mut self, node_paths: &HashSet<String>) {
        self.jobs.retain(|_, job| {
            let keep = job.delay && job.key.as_ref().is_some_and(|key|
                key.split_once('?').is_some_and(|(path, _)| node_paths.contains(path)));
            if !keep { job.work.cancelled.store(true, Ordering::SeqCst); }
            keep
        });
    }
    /// Invalidate replies from an earlier network configuration. Workers have
    /// bounded API deadlines; dropping their receivers prevents stale results
    /// from being published into the new session.
    pub fn invalidate(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
        for job in self.jobs.values() {
            job.work.cancelled.store(true, Ordering::SeqCst);
        }
        self.jobs.clear();
    }
    pub fn start(&mut self, delay: bool, query: impl FnOnce(Arc<AtomicBool>) -> Reply + Send + 'static) -> Result<String, String> {
        self.start_keyed(delay, None, query)
    }
    pub fn start_keyed(&mut self, delay: bool, key: Option<String>, query: impl FnOnce(Arc<AtomicBool>) -> Reply + Send + 'static) -> Result<String, String> {
        self.jobs.retain(|_, job| {
            let keep = job.started.elapsed() < Duration::from_secs(45);
            if !keep { job.work.cancelled.store(true, Ordering::SeqCst); }
            keep
        });
        let shared = key.as_ref().and_then(|key| self.jobs.values()
            .find(|job| job.delay == delay && job.key.as_ref() == Some(key)
                && job.work.state.lock().is_ok_and(|state| state.completed.is_none()))
            .map(|job| (job.started, job.work.clone())));
        let id = format!("{}:{}", self.epoch, uuid::Uuid::new_v4());
        let (tx, rx) = mpsc::channel();
        let (started, work) = if let Some((started, work)) = shared {
            let mut state = work.state.lock().map_err(|_| "Очередь проверок недоступна")?;
            if let Some(result) = &state.completed { let _ = tx.send(result.clone()); }
            else { state.subscribers.push(tx); }
            drop(state);
            (started, work)
        } else {
            let distinct: HashSet<_> = self.jobs.values().filter(|job| job.delay == delay)
                .map(|job| Arc::as_ptr(&job.work)).collect();
            if distinct.len() >= PER_CLASS {
                return Err(if delay { "Очередь проверок серверов заполнена" } else { "Очередь чтения состояния заполнена" }.into());
            }
            let work = Arc::new(Work { cancelled: Arc::new(AtomicBool::new(false)),
                state: Mutex::new(WorkState { subscribers: vec![tx], completed: None }) });
            let worker = work.clone();
            std::thread::Builder::new().name(if delay { "atlas-delay" } else { "atlas-query" }.into())
                .spawn(move || {
                    if worker.cancelled.load(Ordering::SeqCst) { return; }
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| query(worker.cancelled.clone())))
                        .unwrap_or_else(|_| Err("Запрос прерван".into()));
                    if !worker.cancelled.load(Ordering::SeqCst) {
                        if let Ok(mut state) = worker.state.lock() {
                            state.completed = Some(result.clone());
                            for subscriber in state.subscribers.drain(..) { let _ = subscriber.send(result.clone()); }
                        }
                    }
                }).map_err(|e| e.to_string())?;
            (Instant::now(), work)
        };
        self.jobs.insert(id.clone(), Job { started, delay, key, work, rx });
        Ok(id)
    }
    pub fn poll(&mut self, id: &str) -> Result<Option<Value>, String> {
        if id.split_once(':').and_then(|(epoch, _)| epoch.parse::<u64>().ok()) != Some(self.epoch) {
            return Err("CANCELLED: конфигурация сети изменилась".into());
        }
        let job = self.jobs.get(id).ok_or("Запрос истёк")?;
        let result = match job.rx.try_recv() {
            Ok(result) => result.map(Some),
            Err(mpsc::TryRecvError::Empty) => return Ok(None),
            Err(_) => Err("Запрос прерван".into()),
        };
        self.jobs.remove(id);
        result
    }
}
impl Drop for QueryJobs {
    fn drop(&mut self) { self.invalidate(); }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    #[test]
    fn selecting_route_keeps_node_probe_but_cancels_group_and_route_reads() {
        let mut jobs = QueryJobs::default();
        let (resume, waiting) = mpsc::channel();
        let node = jobs.start_keyed(true, Some("/proxies/Berlin/delay?timeout=10000".into()), move |cancelled| {
            waiting.recv().unwrap();
            assert!(!cancelled.load(Ordering::SeqCst));
            Ok(serde_json::json!({"delay":42}))
        }).unwrap();
        let group = jobs.start_keyed(true, Some("/proxies/ATLAS/delay?timeout=10000".into()), |_| Ok(Value::Null)).unwrap();
        let route = jobs.start(false, |_| Ok(Value::Null)).unwrap();
        jobs.selection_changed(&HashSet::from(["/proxies/Berlin/delay".into()]));
        assert!(jobs.poll(&group).is_err());
        assert!(jobs.poll(&route).is_err());
        resume.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(result) = jobs.poll(&node).unwrap() { assert_eq!(result["delay"],42); break; }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        // An old timeout is never reused as the result of a new request.
        let fresh = jobs.start_keyed(true, Some("/proxies/Berlin/delay?timeout=10000".into()), |_| Ok(Value::Null)).unwrap();
        assert_ne!(fresh,node);
    }
    #[test]
    fn stalled_latency_jobs_cannot_consume_operational_capacity() {
        let mut jobs = QueryJobs::default();
        let mut release: Vec<mpsc::Sender<()>> = Vec::new();
        for _ in 0..PER_CLASS {
            let (tx, rx) = mpsc::channel();
            release.push(tx);
            jobs.start(true, move |_| { let _ = rx.recv(); Ok(Value::Null) }).unwrap();
        }
        assert!(jobs.start(true, |_| panic!("unbounded admission")).is_err());
        let id = jobs.start(false, |_| Ok(serde_json::json!({"ready":true}))).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(value) = jobs.poll(&id).unwrap() { assert_eq!(value["ready"], true); break; }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(jobs.poll(&id).is_err());
        drop(release);
    }
    #[test]
    fn previous_session_reply_is_cancelled_even_when_worker_finishes() {
        let mut jobs = QueryJobs::default();
        let id = jobs.start(true, |_| Ok(serde_json::json!({"delay":42}))).unwrap();
        jobs.invalidate();
        assert!(jobs.poll(&id).unwrap_err().starts_with("CANCELLED"));
        let current = jobs.start(true, |_| Ok(serde_json::json!({"delay":10}))).unwrap();
        assert_ne!(id, current);
    }
    #[test]
    fn invalidation_signals_in_flight_workers_before_they_can_use_a_network_slot() {
        let mut jobs = QueryJobs::default();
        let (started, entered) = mpsc::channel();
        let (resume, waiting) = mpsc::channel();
        let id = jobs.start(true, move |cancelled| {
            started.send(()).unwrap();
            waiting.recv().unwrap();
            crate::query_admission::acquire_cancellable(true, Some(&cancelled))
                .map(|_| Value::Null)
        }).unwrap();
        entered.recv_timeout(Duration::from_secs(2)).unwrap();
        jobs.invalidate();
        resume.send(()).unwrap();
        assert!(jobs.poll(&id).unwrap_err().starts_with("CANCELLED"));
        let fresh = jobs.start(true, |_| Ok(Value::Null)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if jobs.poll(&fresh).unwrap().is_some() { break; }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    #[test]
    fn identical_probes_share_one_worker_and_both_callers_receive_its_reply() {
        let mut jobs = QueryJobs::default();
        let starts = Arc::new(AtomicUsize::new(0));
        let counter = starts.clone();
        let (release, waiting) = mpsc::channel();
        let first = jobs.start_keyed(true, Some("node:cloudflare".into()), move |_| {
            counter.fetch_add(1, Ordering::SeqCst);
            waiting.recv().unwrap();
            Ok(serde_json::json!({"delay":42}))
        }).unwrap();
        let second = jobs.start_keyed(true, Some("node:cloudflare".into()), |_| panic!("duplicate worker"))
            .unwrap();
        release.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        for id in [first, second] {
            loop {
                if let Some(value) = jobs.poll(&id).unwrap() { assert_eq!(value["delay"], 42); break; }
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        assert_eq!(starts.load(Ordering::SeqCst), 1);
    }
}
