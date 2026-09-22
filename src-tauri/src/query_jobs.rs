//! Separate bounded admission for URL tests and operational controller reads.
use serde_json::Value;
use std::{collections::HashMap, sync::mpsc, time::{Duration, Instant}};
type Reply = Result<Value, String>;
const PER_CLASS: usize = 8;
#[derive(Default)]
pub struct QueryJobs {
    jobs: HashMap<String, (Instant, bool, mpsc::Receiver<Reply>)>,
}
impl QueryJobs {
    pub fn start(&mut self, delay: bool, query: impl FnOnce() -> Reply + Send + 'static) -> Result<String, String> {
        self.jobs.retain(|_, (started, _, _)| started.elapsed() < Duration::from_secs(30));
        if self.jobs.values().filter(|(_, class, _)| *class == delay).count() >= PER_CLASS {
            return Err(if delay { "Очередь проверок серверов заполнена" } else { "Очередь чтения состояния заполнена" }.into());
        }
        let id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new().name(if delay { "atlas-delay" } else { "atlas-query" }.into())
            .spawn(move || { let _ = tx.send(query()); }).map_err(|e| e.to_string())?;
        self.jobs.insert(id.clone(), (Instant::now(), delay, rx));
        Ok(id)
    }
    pub fn poll(&mut self, id: &str) -> Result<Option<Value>, String> {
        let (_, _, rx) = self.jobs.get(id).ok_or("Запрос истёк")?;
        let result = match rx.try_recv() {
            Ok(result) => result.map(Some),
            Err(mpsc::TryRecvError::Empty) => return Ok(None),
            Err(_) => Err("Запрос прерван".into()),
        };
        self.jobs.remove(id);
        result
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stalled_latency_jobs_cannot_consume_operational_capacity() {
        let mut jobs = QueryJobs::default();
        let mut release: Vec<mpsc::Sender<()>> = Vec::new();
        for _ in 0..PER_CLASS {
            let (tx, rx) = mpsc::channel();
            release.push(tx);
            jobs.start(true, move || { let _ = rx.recv(); Ok(Value::Null) }).unwrap();
        }
        assert!(jobs.start(true, || panic!("unbounded admission")).is_err());
        let id = jobs.start(false, || Ok(serde_json::json!({"ready":true}))).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(value) = jobs.poll(&id).unwrap() { assert_eq!(value["ready"], true); break; }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(jobs.poll(&id).is_err());
        drop(release);
    }
}
