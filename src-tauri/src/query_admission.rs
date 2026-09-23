//! Bound producers before they enter the service queue; never hold the IPC lock while waiting.
use std::{sync::atomic::{AtomicUsize,Ordering},time::{Duration,Instant}};
static DELAYS: AtomicUsize=AtomicUsize::new(0);
static READS: AtomicUsize=AtomicUsize::new(0);
pub struct Permit(&'static AtomicUsize);
impl Drop for Permit { fn drop(&mut self) { self.0.fetch_sub(1,Ordering::SeqCst); } }
pub fn acquire(delay:bool)->Result<Permit,String> {
    let (count,limit)=if delay {(&DELAYS,12)} else {(&READS,4)};
    let deadline=Instant::now()+Duration::from_secs(20);
    loop {
        if count.fetch_update(Ordering::SeqCst,Ordering::SeqCst,|n|(n<limit).then_some(n+1)).is_ok() { return Ok(Permit(count)); }
        if Instant::now()>=deadline {return Err("Локальная очередь Atlas занята; проверка не выполнена".into());}
        std::thread::sleep(Duration::from_millis(25));
    }
}
pub fn retry_busy<T>(deadline:Instant,mut call:impl FnMut()->Result<T,String>)->Result<T,String> {
    loop {
        match call() {
            Err(e) if matches!(e.as_str(),"Сетевая служба занята. Повторите операцию."|"Очередь проверок серверов заполнена"|"Очередь чтения состояния заполнена") && Instant::now()<deadline => std::thread::sleep(Duration::from_millis(100)),
            result => return result,
        }
    }
}
#[cfg(test)] mod tests {
    use super::*;
    #[test] fn busy_poll_is_retried_but_remote_timeout_is_not() {
        let mut calls=0;
        let result=retry_busy(Instant::now()+Duration::from_secs(1),||{calls+=1;if calls==1 {Err("Сетевая служба занята. Повторите операцию.".into())}else{Ok(42)}});
        assert_eq!(result,Ok(42));assert_eq!(calls,2);
        let mut calls=0;
        let _:Result<(),_>=retry_busy(Instant::now()+Duration::from_secs(1),||{calls+=1;Err("Mihomo API: HTTP 504".into())});
        assert_eq!(calls,1);
    }
}
