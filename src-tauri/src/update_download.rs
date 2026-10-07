//! Bounded disk streaming with strong-validator resume. Does not touch Atlas,
//! SCM, routes or TUN. no_proxy disables proxy discovery, not Windows routing.
use std::{fs::{self,OpenOptions},io::Write,path::Path,sync::atomic::{AtomicBool,AtomicU64,Ordering},time::{Duration,Instant}};
use serde::{Serialize,Deserialize};
use reqwest::header::{ETAG,RANGE,IF_RANGE,CONTENT_RANGE,RETRY_AFTER};
type Result<T> = std::result::Result<T,String>;
pub struct Asset {pub url:String,pub size:u64,pub sha256:String}
#[derive(Serialize,Deserialize)]
struct Partial {url:String,size:u64,sha256:String,etag:String}
pub fn client()->Result<reqwest::Client> {
    reqwest::Client::builder().no_proxy().https_only(true).connect_timeout(Duration::from_secs(10))
        .read_timeout(Duration::from_secs(15)).redirect(reqwest::redirect::Policy::custom(|attempt|{
            let u=attempt.url();let host=u.host_str().unwrap_or("");
            if attempt.previous().len()>5 || u.scheme()!="https" || !u.username().is_empty() || u.password().is_some() ||
                !(host=="github.com" || host.ends_with(".githubusercontent.com")) {attempt.error("Untrusted update redirect")}else{attempt.follow()}
        })).build().map_err(|_|"Update HTTP client initialization failed".into())
}
fn etag(value:&str)->bool {value.len()>=2 && value.len()<512 && value.starts_with('"') && value.ends_with('"') && !value.contains(['\r','\n'])}
fn range(value:&str,start:u64,total:u64)->bool {
    let Some((span,size))=value.strip_prefix("bytes ").and_then(|s|s.split_once('/')) else{return false;};
    let Some((a,b))=span.split_once('-') else{return false;};
    a.parse::<u64>()==Ok(start) && size.parse::<u64>()==Ok(total) && b.parse::<u64>().is_ok_and(|b|b>=start && b<total)
}
async fn pause(duration:Duration,cancel:&AtomicBool,deadline:Instant)->Result<()> {
    let until=Instant::now()+duration;
    while Instant::now()<until {if cancel.load(Ordering::SeqCst){return Err("Download cancelled".into());}if Instant::now()>=deadline{return Err("Download overall timeout".into());}tokio::time::sleep((until-Instant::now()).min(Duration::from_millis(100))).await;}Ok(())
}
pub async fn download(asset:&Asset,part:&Path,cancel:&AtomicBool,network_generation:&AtomicU64)->Result<()> {
    let url=url::Url::parse(&asset.url).map_err(|_|"Invalid update URL")?;
    if url.scheme()!="https" || url.host_str()!=Some("github.com") || url.query().is_some() || url.fragment().is_some() || !url.username().is_empty() || url.password().is_some() {return Err("Invalid immutable update URL".into());}
    transfer(&client()?,asset,part,cancel,network_generation,Duration::from_secs(600),8).await
}
async fn transfer(client:&reqwest::Client,asset:&Asset,part:&Path,cancel:&AtomicBool,generation:&AtomicU64,overall:Duration,attempts:u32)->Result<()> {
    if asset.size==0 || asset.sha256.len()!=64 || !asset.sha256.bytes().all(|b|b.is_ascii_hexdigit()) {return Err("Invalid update size/hash".into());}
    let metadata=part.with_extension("part.json");let deadline=Instant::now()+overall;
    for attempt in 0..attempts {
        if cancel.load(Ordering::SeqCst){return Err("Download cancelled".into());}
        if Instant::now()>=deadline{return Err("Download overall timeout".into());}
        let start_generation=generation.load(Ordering::SeqCst);
        let saved=crate::update_integrity::read_bounded(&metadata,8192).ok().and_then(|b|serde_json::from_slice::<Partial>(&b).ok())
            .filter(|p|p.url==asset.url && p.size==asset.size && p.sha256==asset.sha256 && etag(&p.etag));
        let mut offset=if saved.is_some(){fs::metadata(part).map(|m|m.len()).unwrap_or(0)}else{0};
        if offset>asset.size {offset=0;}
        if offset==asset.size {
            let actual=crate::update_transaction::digest(part)?;
            if actual==(asset.size,asset.sha256.clone()){return Ok(());}offset=0;
        }
        let mut request=client.get(&asset.url).timeout(deadline.saturating_duration_since(Instant::now()));
        if offset>0 {request=request.header(RANGE,format!("bytes={offset}-")).header(IF_RANGE,&saved.as_ref().unwrap().etag);}
        // Keep the same request future while polling cancellation. Dropping and
        // reissuing it every tick would prevent slow TLS handshakes completing.
        let mut pending=Box::pin(request.send());
        let response=loop {
            if cancel.load(Ordering::SeqCst){return Err("Download cancelled".into());}
            if Instant::now()>=deadline{return Err("Download overall timeout".into());}
            if generation.load(Ordering::SeqCst)!=start_generation{break None;}
            match tokio::time::timeout(Duration::from_millis(100),pending.as_mut()).await {
                Ok(result)=>break Some(result),Err(_)=>continue,
            }
        };
        let mut retry_delay=Duration::from_millis((250u64<<attempt.min(4))+(uuid::Uuid::new_v4().as_u128()%251) as u64);
        if let Some(Ok(mut response))=response {
            let status=response.status().as_u16();
            if status==429 || status==503 {
                if let Some(seconds)=response.headers().get(RETRY_AFTER).and_then(|v|v.to_str().ok()).and_then(|s|s.parse::<u64>().ok()) {retry_delay=Duration::from_secs(seconds.min(60));}
            } else if status==200 || status==206 {
                let tag=response.headers().get(ETAG).and_then(|v|v.to_str().ok()).filter(|s|etag(s)).map(str::to_owned);
                if status==206 && (offset==0 || response.headers().get(CONTENT_RANGE).and_then(|v|v.to_str().ok()).is_none_or(|s|!range(s,offset,asset.size)) || tag.as_deref()!=saved.as_ref().map(|s|s.etag.as_str())) {
                    return Err("Unsafe partial update response".into());
                }
                if status==200 {offset=0;}
                let mut file=OpenOptions::new().write(true).create(true).truncate(offset==0).append(offset>0).open(part).map_err(|_|"Cannot open update partial file")?;
                if let Some(tag)=tag {
                    let value=Partial{url:asset.url.clone(),size:asset.size,sha256:asset.sha256.clone(),etag:tag};
                    crate::update_transaction::durable_write(&metadata,&serde_json::to_vec(&value).map_err(|_|"Partial metadata invalid")?)?;
                } else {let _=fs::remove_file(&metadata);}
                let mut completed=false;let mut last_byte=Instant::now();
                loop {
                    if cancel.load(Ordering::SeqCst){file.sync_all().map_err(|_|"Cannot flush cancelled download")?;return Err("Download cancelled".into());}
                    if generation.load(Ordering::SeqCst)!=start_generation || Instant::now()>=deadline || last_byte.elapsed()>=Duration::from_secs(15) {break;}
                    match tokio::time::timeout(Duration::from_millis(250),response.chunk()).await {
                        Err(_)=>continue,
                        Ok(Err(_))=>break,
                        Ok(Ok(None))=>{completed=true;break;},
                        Ok(Ok(Some(bytes)))=>{
                            last_byte=Instant::now();
                            offset=offset.checked_add(bytes.len() as u64).ok_or("Download size overflow")?;
                            if offset>asset.size {return Err("Update exceeds signed size".into());}
                            file.write_all(&bytes).map_err(|_|"Cannot write update partial (disk full or access denied)")?;
                        }
                    }
                }
                file.sync_all().map_err(|_|"Cannot flush update partial")?;drop(file);
                if completed && offset==asset.size {
                    let actual=crate::update_transaction::digest(part)?;
                    if actual==(asset.size,asset.sha256.clone()){return Ok(());}
                    return Err("Downloaded update hash mismatch".into());
                }
            } else if !matches!(status,408|500|502|504) {return Err(format!("Update HTTP status {status}"));}
        }
        if attempt+1<attempts {pause(retry_delay,cancel,deadline).await?;}
    }
    Err("Update download retry budget exhausted".into())
}
#[cfg(test)]mod tests {
    use super::*;use std::{io::{Read,Write},net::TcpListener};use sha2::{Digest,Sha256};
    fn run(response:&'static [u8],expected:&[u8])->Result<Vec<u8>> {
        let listener=TcpListener::bind("127.0.0.1:0").unwrap();let address=listener.local_addr().unwrap();
        let server=std::thread::spawn(move||{let(mut s,_)=listener.accept().unwrap();let mut buf=[0;2048];let _=s.read(&mut buf);s.write_all(response).unwrap();});
        let dir=std::env::temp_dir().join(format!("atlas-download-test-{}",uuid::Uuid::new_v4()));fs::create_dir(&dir).unwrap();let part=dir.join("package.part");
        let asset=Asset{url:format!("http://{address}/immutable"),size:expected.len() as u64,sha256:format!("{:x}",Sha256::digest(expected))};
        let rt=tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let result=rt.block_on(transfer(&reqwest::Client::builder().no_proxy().build().unwrap(),&asset,&part,&AtomicBool::new(false),&AtomicU64::new(0),Duration::from_secs(2),1));
        server.join().unwrap();let bytes=result.map(|_|fs::read(&part).unwrap());fs::remove_dir_all(dir).unwrap();bytes
    }
    #[test]fn real_http_stream_checks_signed_size_and_hash(){assert_eq!(run(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nETag: \"v1\"\r\n\r\nnew",b"new").unwrap(),b"new");assert!(run(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nbad",b"new").is_err());assert!(run(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\nmore",b"new").is_err());}
    #[test]fn unexpected_partial_response_is_rejected(){assert!(run(b"HTTP/1.1 206 Partial Content\r\nContent-Length: 3\r\nContent-Range: bytes 0-2/3\r\n\r\nnew",b"new").is_err());}
    #[test]fn resume_range_and_etag_are_strict(){assert!(range("bytes 10-19/20",10,20));for s in ["bytes 0-19/20","bytes 10-20/20","bytes 10-19/*","bytes 10-19/21"]{assert!(!range(s,10,20));}assert!(!etag("W/\"x\""));assert!(!etag("\"x\r\n\""));}
    #[test]fn actual_interrupted_response_resumes_only_with_matching_validator(){
        for changed in [false,true] {
            let listener=TcpListener::bind("127.0.0.1:0").unwrap();let address=listener.local_addr().unwrap();
            let server=std::thread::spawn(move||{
                let(mut s,_)=listener.accept().unwrap();let mut b=[0;4096];let _=s.read(&mut b).unwrap();
                s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\nETag: \"v1\"\r\nConnection: close\r\n\r\nabc").unwrap();drop(s);
                let(mut s,_)=listener.accept().unwrap();let n=s.read(&mut b).unwrap();let request=String::from_utf8_lossy(&b[..n]).to_lowercase();
                assert!(request.contains("range: bytes=3-"),"{request}");assert!(request.contains("if-range: \"v1\""));
                let tag=if changed{"v2"}else{"v1"};let _=s.write_all(format!("HTTP/1.1 206 Partial Content\r\nContent-Length: 3\r\nContent-Range: bytes 3-5/6\r\nETag: \"{tag}\"\r\nConnection: close\r\n\r\ndef").as_bytes());
            });
            let dir=std::env::temp_dir().join(format!("atlas-resume-{}",uuid::Uuid::new_v4()));fs::create_dir(&dir).unwrap();
            let asset=Asset{url:format!("http://{address}/immutable"),size:6,sha256:format!("{:x}",Sha256::digest(b"abcdef"))};
            let rt=tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();let client=reqwest::Client::builder().no_proxy().build().unwrap();
            let result=rt.block_on(transfer(&client,&asset,&dir.join("package.part"),&AtomicBool::new(false),&AtomicU64::new(0),Duration::from_secs(5),2));
            server.join().unwrap();assert_eq!(result.is_ok(),!changed,"{result:?}");fs::remove_dir_all(dir).unwrap();
        }
    }
    #[test]fn cancellation_interrupts_wait_for_response_headers(){
        let listener=TcpListener::bind("127.0.0.1:0").unwrap();let address=listener.local_addr().unwrap();
        let cancel=std::sync::Arc::new(AtomicBool::new(false));let signal=cancel.clone();
        let (finish,done)=std::sync::mpsc::channel();
        let server=std::thread::spawn(move||{let(mut s,_)=listener.accept().unwrap();let mut b=[0;2048];let _=s.read(&mut b);signal.store(true,Ordering::SeqCst);let _=done.recv_timeout(Duration::from_secs(3));});
        let dir=std::env::temp_dir().join(format!("atlas-cancel-{}",uuid::Uuid::new_v4()));fs::create_dir(&dir).unwrap();
        let asset=Asset{url:format!("http://{address}/immutable"),size:3,sha256:format!("{:x}",Sha256::digest(b"new"))};
        let rt=tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();let client=reqwest::Client::builder().no_proxy().build().unwrap();let start=Instant::now();
        let result=rt.block_on(transfer(&client,&asset,&dir.join("package.part"),&cancel,&AtomicU64::new(0),Duration::from_secs(5),2));
        let _=finish.send(());drop(client);server.join().unwrap();assert_eq!(result.unwrap_err(),"Download cancelled");assert!(start.elapsed()<Duration::from_secs(2));assert!(!dir.join("package.part").exists());fs::remove_dir_all(dir).unwrap();
    }
    #[test]fn retry_after_and_server_ignoring_range_do_not_duplicate_bytes(){
        let listener=TcpListener::bind("127.0.0.1:0").unwrap();let address=listener.local_addr().unwrap();
        let server=std::thread::spawn(move||{
            for response in [b"HTTP/1.1 429 Too Many Requests\r\nContent-Length: 0\r\nRetry-After: 0\r\nConnection: close\r\n\r\n".as_slice(),b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nETag: \"v2\"\r\nConnection: close\r\n\r\nnew"] {
                let(mut s,_)=listener.accept().unwrap();let mut b=[0;2048];let n=s.read(&mut b).unwrap();assert!(String::from_utf8_lossy(&b[..n]).to_lowercase().contains("range: bytes=1-"));s.write_all(response).unwrap();
            }
        });
        let dir=std::env::temp_dir().join(format!("atlas-retry-{}",uuid::Uuid::new_v4()));fs::create_dir(&dir).unwrap();let part=dir.join("package.part");fs::write(&part,b"n").unwrap();
        let asset=Asset{url:format!("http://{address}/immutable"),size:3,sha256:format!("{:x}",Sha256::digest(b"new"))};
        fs::write(part.with_extension("part.json"),serde_json::to_vec(&Partial{url:asset.url.clone(),size:asset.size,sha256:asset.sha256.clone(),etag:"\"v1\"".into()}).unwrap()).unwrap();
        let rt=tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();let client=reqwest::Client::builder().no_proxy().build().unwrap();
        rt.block_on(transfer(&client,&asset,&part,&AtomicBool::new(false),&AtomicU64::new(0),Duration::from_secs(5),2)).unwrap();server.join().unwrap();assert_eq!(fs::read(part).unwrap(),b"new");fs::remove_dir_all(dir).unwrap();
    }
}
