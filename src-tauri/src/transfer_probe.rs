//! Explicit support-export diagnostic. Never used by background recovery.
use serde_json::{json,Value};
use std::{io::Read, time::{Duration,Instant}};
const BYTES:usize=1_048_576;
const ENDPOINT:&str="https://speed.cloudflare.com/__down?bytes=1048576";

pub(crate) fn run(proxy:&str, deadline:Instant)->Value {
    let remaining=deadline.saturating_duration_since(Instant::now());
    if remaining<Duration::from_secs(1) {return json!({"outcome":"skipped","reason":"export_deadline"});}
    measure(ENDPOINT,proxy,BYTES,remaining.min(Duration::from_secs(8)))
}
fn measure(endpoint:&str,proxy:&str,expected:usize,timeout:Duration)->Value {
    let started=Instant::now();
    let mut bytes=0usize;let mut first_byte=None;let mut status=None;
    let result=(|| ->Result<(),&'static str> {
        let proxy=reqwest::Proxy::all(proxy).map_err(|_|"invalid_proxy")?;
        let client=reqwest::blocking::Client::builder().no_proxy().proxy(proxy)
            .connect_timeout(timeout.min(Duration::from_secs(3))).timeout(timeout)
            .redirect(reqwest::redirect::Policy::none()).build().map_err(|_|"client_error")?;
        let mut response=client.get(endpoint).header("Accept-Encoding","identity").send().map_err(|_|"request_failed")?;
        status=Some(response.status().as_u16());
        if status!=Some(200) {return Err("unexpected_status");}
        if response.headers().get("content-encoding").is_some_and(|v|v!="identity") {return Err("encoded_body");}
        if response.content_length().is_some_and(|n|n!=expected as u64) {return Err("unexpected_length");}
        let mut first=[0u8;1];
        if response.read(&mut first).map_err(|_|"body_incomplete")?!=1 {return Err("body_incomplete");}
        bytes=1;first_byte=Some(started.elapsed());
        let mut buffer=[0u8;16_384];
        loop {
            if started.elapsed()>=timeout {return Err("body_incomplete");}
            let limit=buffer.len().min(expected+1-bytes);
            let n=response.read(&mut buffer[..limit]).map_err(|_|"body_incomplete")?;
            if n==0 {break;}bytes+=n;
            if bytes>expected {return Err("oversized_body");}
        }
        if bytes!=expected {return Err("body_incomplete");}Ok(())
    })();
    let elapsed=started.elapsed();
    let data_time=first_byte.map(|at|elapsed.saturating_sub(at).as_secs_f64());
    let mbps=if result.is_ok() {data_time.filter(|t|*t>0.0).map(|t|(bytes.saturating_sub(1) as f64*8.0)/(t*1_000_000.0))}else{None};
    json!({"endpoint":endpoint,"path":"local_mixed_proxy_current_rules","outcome":if result.is_ok(){"complete"}else{"not_measured"},
        "reason":result.err(),"status":status,"bytes":bytes,"expectedBytes":expected,"firstByteMs":first_byte.map(|t|t.as_millis()),
        "totalMs":elapsed.as_millis(),"transferMbps":mbps,"budgetMs":timeout.as_millis(),
        "scope":"One small download through current rules; may be DIRECT. Not maximum bandwidth, account availability or proof of censorship."})
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{net::TcpListener,io::Write};
    fn fixture(reply:Vec<u8>)->(String,std::thread::JoinHandle<()>) {
        let listener=TcpListener::bind("127.0.0.1:0").unwrap();let addr=listener.local_addr().unwrap();
        let worker=std::thread::spawn(move || {
            let(mut socket,_)=listener.accept().unwrap();socket.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            let mut request=[0;4096];let n=socket.read(&mut request).unwrap();
            assert!(String::from_utf8_lossy(&request[..n]).starts_with("GET http://transfer.invalid/"));
            let _=socket.write_all(&reply);
        });(format!("http://{addr}"),worker)
    }
    #[test] fn complete_body_reports_rate_but_partial_and_rejected_bodies_do_not() {
        for (reply,outcome) in [
            (b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\n1234".to_vec(),"complete"),
            (b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\n12".to_vec(),"not_measured"),
            (b"HTTP/1.1 403 Forbidden\r\nContent-Length: 4\r\nConnection: close\r\n\r\n1234".to_vec(),"not_measured"),
            (b"HTTP/1.1 302 Found\r\nLocation: http://never-follow.invalid/\r\nContent-Length: 0\r\n\r\n".to_vec(),"not_measured"),
            (b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n123456789".to_vec(),"not_measured")
        ] {
            let(proxy,worker)=fixture(reply);let r=measure("http://transfer.invalid/",&proxy,4,Duration::from_secs(1));worker.join().unwrap();
            assert_eq!(r["outcome"],outcome);assert_eq!(r["transferMbps"].is_number(),outcome=="complete");assert!(r["bytes"].as_u64().unwrap()<=5);
        }
    }
    #[test] fn deadline_and_stalled_proxy_are_bounded_without_direct_fallback() {
        assert_eq!(run("invalid",Instant::now())["outcome"],"skipped");
        let listener=TcpListener::bind("127.0.0.1:0").unwrap();let at=Instant::now();
        let r=measure("http://transfer.invalid/",&format!("http://{}",listener.local_addr().unwrap()),4,Duration::from_millis(120));
        assert_eq!(r["outcome"],"not_measured");assert!(r["transferMbps"].is_null());assert!(at.elapsed()<Duration::from_secs(2));
    }
    #[test] fn fast_headers_with_stalled_body_are_not_reported_as_fast_transfer() {
        let listener=TcpListener::bind("127.0.0.1:0").unwrap();let proxy=format!("http://{}",listener.local_addr().unwrap());
        let(release,wait)=std::sync::mpsc::channel();
        let worker=std::thread::spawn(move || {
            let(mut socket,_)=listener.accept().unwrap();socket.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            let mut request=[0;4096];socket.read(&mut request).unwrap();
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\n1").unwrap();
            let _=wait.recv_timeout(Duration::from_secs(2));
        });
        let at=Instant::now();let r=measure("http://transfer.invalid/",&proxy,4,Duration::from_millis(150));
        let _=release.send(());worker.join().unwrap();
        assert_eq!(r["reason"],"body_incomplete");assert!(r["firstByteMs"].is_number());assert!(r["transferMbps"].is_null());
        assert_eq!(r["bytes"],1);assert!(at.elapsed()<Duration::from_secs(2));
    }
}
