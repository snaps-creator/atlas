//! Manual RTT measurements to the authenticated destination, over its current
//! Atlas route. HTTP/2 PING/ACK (RFC 9113 6.7), never local TCP connect or HTTP TTFB.
use serde::Serialize;
use std::{sync::Arc, time::{Duration, Instant}};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_rustls::{rustls, TlsConnector};

const DEADLINE: Duration = Duration::from_secs(20);
const WARMUP_DEADLINE: Duration = Duration::from_secs(5);
const PING_DEADLINE: Duration = Duration::from_secs(3);

#[derive(Default, Serialize)]
#[serde(rename_all="camelCase")]
pub(crate) struct Timing {
    stage: &'static str,
    proxy_ms: Option<u64>,
    tunnel_ms: Option<u64>,
    tls_ms: Option<u64>,
    h2_ms: Option<u64>,
    samples_ms: Vec<u64>,
}
#[derive(Serialize)]
#[serde(rename_all="camelCase")]
pub(crate) struct ResultView {
    ok: bool,
    ms: Option<u64>,
    status: Option<u16>,
    metric: &'static str,
    outcome: &'static str,
    detail: &'static str,
    revision: u64,
    timing: Timing,
}

#[derive(Debug)]
enum Failure { Network, Unsupported, Timeout, Cancelled }
fn elapsed(start: Instant) -> u64 { start.elapsed().as_millis() as u64 }
fn median(samples: &[u64]) -> Option<u64> {
    if samples.len()!=3 {return None;}
    let mut values=samples.to_vec();values.sort_unstable();Some(values[1])
}

pub(crate) fn run(url: &str, port: u16, revision: u64, current: impl Fn()->bool) -> ResultView {
    let mut timing=Timing::default();
    let result=(|| {
        let host=url::Url::parse(url).ok().filter(|u|u.scheme()=="https")
            .and_then(|u|u.host_str().map(str::to_owned)).ok_or(Failure::Network)?;
        if !current() {return Err(Failure::Cancelled);}
        let runtime=tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|_|Failure::Network)?;
        runtime.block_on(async {
            tokio::select! {
                outcome=tokio::time::timeout(DEADLINE, measure(&host,port,&mut timing)) => outcome.unwrap_or(Err(Failure::Timeout)),
                _=async {while current() {tokio::time::sleep(Duration::from_millis(50)).await;}} => Err(Failure::Cancelled),
            }
        })
        // Dropping this private runtime cancels its H2 driver and closes its
        // socket on success, timeout, cancellation and every error path.
    })();
    let result=if current(){result}else{Err(Failure::Cancelled)};
    let (ms,outcome,detail)=match result {
        Ok(ms)=>(Some(ms),"rtt","Пинг HTTP/2: медиана трёх PING/ACK через текущие правила. Подготовка соединения не входит в число; это не ICMP и не проверка работы сервиса."),
        Err(Failure::Unsupported)=>(None,"unsupported","Сайт не поддерживает измерение HTTP/2 PING. HTTP-ответ не подставляется вместо пинга."),
        Err(Failure::Timeout)=>(None,"timeout","Не удалось завершить отдельное измерение PING за отведённое время. Это не означает, что сайт или открытый чат недоступен."),
        Err(Failure::Cancelled)=>(None,"cancelled","Проверка отменена или подключение изменилось."),
        Err(Failure::Network)=>(None,"network_error","Не удалось измерить пинг через текущее подключение."),
    };
    ResultView{ok:ms.is_some(),ms,status:None,metric:"h2_ping_rtt",outcome,detail,revision,timing}
}

async fn measure(host: &str, port: u16, timing: &mut Timing) -> Result<u64,Failure> {
    timing.stage="proxy";
    let start=Instant::now();
    let mut stream=tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST,port)).await.map_err(|_|Failure::Network)?;
    stream.set_nodelay(true).map_err(|_|Failure::Network)?;
    timing.proxy_ms=Some(elapsed(start));
    timing.stage="tunnel";
    let start=Instant::now();
    open_tunnel(&mut stream,host).await?;
    timing.tunnel_ms=Some(elapsed(start));
    timing.stage="tls";
    let start=Instant::now();
    let roots=rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let mut config=rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions().map_err(|_|Failure::Network)?
        .with_root_certificates(roots).with_no_client_auth();
    config.alpn_protocols=vec![b"h2".to_vec()];
    let name=rustls::pki_types::ServerName::try_from(host.to_owned()).map_err(|_|Failure::Network)?;
    let tls=TlsConnector::from(Arc::new(config)).connect(name,stream).await.map_err(|_|Failure::Network)?;
    timing.tls_ms=Some(elapsed(start));
    if tls.get_ref().1.alpn_protocol()!=Some(b"h2") {return Err(Failure::Unsupported);}
    timing.stage="h2";
    let start=Instant::now();
    let (_sender,mut connection)=h2::client::handshake(tls).await.map_err(|_|Failure::Network)?;
    let mut ping=connection.ping_pong().ok_or(Failure::Unsupported)?;
    let driver=tokio::spawn(async move {let _=connection.await;});
    let result=sample(&mut ping,timing,start).await;
    driver.abort();let _=driver.await;
    result
}

async fn open_tunnel(stream: &mut (impl tokio::io::AsyncRead+tokio::io::AsyncWrite+Unpin), host: &str) -> Result<(),Failure> {
    // host comes only from the static endpoint allow-list; validate here as well.
    if host.is_empty() || !host.bytes().all(|b|b.is_ascii_alphanumeric() || b==b'.' || b==b'-') {return Err(Failure::Network);}
    stream.write_all(format!("CONNECT {host}:443 HTTP/1.1\r\nHost: {host}:443\r\n\r\n").as_bytes()).await.map_err(|_|Failure::Network)?;
    let mut header=Vec::new();
    while !header.ends_with(b"\r\n\r\n") {
        if header.len()>=16384 {return Err(Failure::Network);}
        header.push(stream.read_u8().await.map_err(|_|Failure::Network)?);
    }
    let status=std::str::from_utf8(&header).ok().and_then(|h|h.lines().next()).map(|s|s.split_whitespace().collect::<Vec<_>>());
    if !status.is_some_and(|s|s.len()>=2 && matches!(s[0],"HTTP/1.0"|"HTTP/1.1") && s[1]=="200") {return Err(Failure::Network);}
    Ok(())
}

async fn sample(ping: &mut h2::PingPong, timing: &mut Timing, setup: Instant) -> Result<u64,Failure> {
    // Warmup proves the remote H2 connection is ready; exclude it and all setup
    // from the reported RTT. No page request, cookie or auth is sent.
    tokio::time::timeout(WARMUP_DEADLINE,ping.ping(h2::Ping::opaque())).await
        .map_err(|_|Failure::Timeout)?.map_err(|_|Failure::Unsupported)?;
    timing.h2_ms=Some(elapsed(setup));
    timing.stage="ping";
    for _ in 0..3 {
        let start=Instant::now();
        tokio::time::timeout(PING_DEADLINE,ping.ping(h2::Ping::opaque())).await
            .map_err(|_|Failure::Timeout)?.map_err(|_|Failure::Network)?;
        timing.samples_ms.push(elapsed(start).max(1));
        tokio::time::sleep(Duration::from_millis(75)).await;
    }
    timing.stage="complete";
    median(&timing.samples_ms).ok_or(Failure::Network)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore="Explicit broad live comparison through a supplied loopback proxy; no cookies or login"]
    fn live_service_matrix() {
        use serde_json::json;
        let port: u16=std::env::var("ATLAS_DIAGNOSTIC_PROXY_PORT").expect("explicit proxy port").parse().unwrap();
        let endpoints=[
            ("chatgpt","current","https://chatgpt.com/"),
            ("grok","current","https://grok.com/"),
            ("gemini","current","https://gemini.google.com/"),
            ("youtube","current","https://www.youtube.com/"),
            ("telegram","current","https://web.telegram.org/"),
            ("claude","expanded","https://claude.ai/"),
            ("discord","expanded","https://discord.com/"),
            ("instagram","expanded","https://www.instagram.com/"),
            ("facebook","expanded","https://www.facebook.com/"),
            ("x","expanded","https://x.com/"),
            ("linkedin","expanded","https://www.linkedin.com/"),
            ("spotify","expanded","https://open.spotify.com/"),
            ("twitch","expanded","https://www.twitch.tv/"),
            ("reddit","expanded","https://www.reddit.com/"),
            ("pinterest","expanded","https://www.pinterest.com/"),
            ("proton","expanded","https://proton.me/"),
            ("notion","expanded","https://www.notion.so/"),
            ("cloudflare","control","https://www.cloudflare.com/"),
            ("microsoft","control","https://www.microsoft.com/"),
            ("wikipedia","control","https://www.wikipedia.org/"),
            ("yandex","control","https://ya.ru/"),
        ];
        // Two workers, as in the UI. Header requests run after RTT, never during
        // that endpoint's samples; no body or logged-in account is inspected.
        std::thread::scope(|scope| {
            for worker in 0..2 {
                let endpoints=&endpoints;
                scope.spawn(move || for (id,pool,url) in endpoints.iter().skip(worker).step_by(2) {
                    let at=crate::model::now();let network=run(url,port,0,||true);
                    let client=reqwest::blocking::Client::builder().no_proxy()
                        .proxy(reqwest::Proxy::all(format!("http://127.0.0.1:{port}")).unwrap())
                        .timeout(Duration::from_secs(12)).redirect(reqwest::redirect::Policy::none()).build().unwrap();
                    let start=Instant::now();
                    let http=match client.get(*url).send() {
                        Ok(reply)=>json!({"status":reply.status().as_u16(),"headersMs":elapsed(start),
                            "server":reply.headers().get("server").and_then(|v|v.to_str().ok()),
                            "challenge":reply.headers().get("cf-mitigated").and_then(|v|v.to_str().ok()),
                            "pop":reply.headers().get("cf-ray").and_then(|v|v.to_str().ok()).and_then(|v|v.rsplit('-').next())}),
                        Err(error)=>json!({"status":null,"headersMs":elapsed(start),"errorKind":if error.is_timeout(){"timeout"}else if error.is_connect(){"connect"}else{"other"}}),
                    };
                    println!("SERVICE_MATRIX {}",json!({"at":at,"service":id,"pool":pool,"host":url::Url::parse(url).unwrap().host_str(),"network":network,"http":http}));
                });
            }
        });
    }
    #[test]
    #[ignore="Explicit live diagnostic, only through an already running loopback Atlas proxy"]
    fn live_five_sites() {
        let port=std::env::var("ATLAS_DIAGNOSTIC_PROXY_PORT").expect("explicit proxy port").parse().unwrap();
        for id in ["chatgpt","grok","gemini","youtube","telegram"] {
            let result=run(crate::service_check::endpoint(id).unwrap(),port,0,||true);
            println!("NETWORK_PING {}",serde_json::json!({"service":id,"result":result}));
        }
    }
    #[test]
    fn incomplete_samples_never_become_positive_rtt() {
        assert_eq!(median(&[]),None);assert_eq!(median(&[15,22]),None);
        assert_eq!(median(&[700,42,40]),Some(42));
    }
    #[test]
    fn cancelled_probe_never_connects() {
        let result=run("https://example.com",0,3,||false);
        assert_eq!(result.outcome,"cancelled");assert_eq!(result.ms,None);
        assert_eq!(result.timing.proxy_ms,None);
    }
    #[test]
    fn setup_delay_is_excluded_but_remote_ping_delay_is_measured() {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
            let (client,mut server)=tokio::io::duplex(8192);
            let remote=tokio::spawn(async move {
                let mut preface=[0u8;24];server.read_exact(&mut preface).await.unwrap();
                assert_eq!(&preface,b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n");
                tokio::time::sleep(Duration::from_millis(500)).await;
                server.write_all(&[0,0,0,4,0,0,0,0,0]).await.unwrap();
                let mut count=0;
                while count<4 {
                    let mut header=[0u8;9];server.read_exact(&mut header).await.unwrap();
                    let len=((header[0] as usize)<<16)|((header[1] as usize)<<8)|header[2] as usize;
                    assert!(len<=16384);
                    let mut payload=vec![0;len];server.read_exact(&mut payload).await.unwrap();
                    if header[3]==4 && header[4]&1==0 {
                        server.write_all(&[0,0,0,4,1,0,0,0,0]).await.unwrap();
                    } else if header[3]==6 && header[4]&1==0 {
                        assert_eq!(len,8);
                        tokio::time::sleep(Duration::from_millis(30)).await;
                        header[4]=1;server.write_all(&header).await.unwrap();server.write_all(&payload).await.unwrap();count+=1;
                    } else {assert_eq!(header[3],4,"No HTTP page request is permitted");}
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            });
            let (_sender,mut conn)=h2::client::handshake(client).await.unwrap();
            let mut ping=conn.ping_pong().unwrap();
            let driver=tokio::spawn(async move {let _=conn.await;});
            let mut timing=Timing::default();
            let ms=sample(&mut ping,&mut timing,Instant::now()).await.unwrap();
            assert!(timing.h2_ms.unwrap()>=500);
            assert!((25..250).contains(&ms),"remote RTT {ms}, setup excluded");
            assert_eq!(timing.samples_ms.len(),3);
            driver.abort();let _=driver.await;remote.await.unwrap();
        });
    }
    #[test]
    fn cancellation_closes_a_stalled_proxy_without_waiting_for_deadline() {
        use std::io::Read;
        let listener=std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).unwrap();
        let port=listener.local_addr().unwrap().port();
        let server=std::thread::spawn(move || {
            let (mut stream,_)=listener.accept().unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            let mut buffer=[0;512];
            while stream.read(&mut buffer).unwrap()>0 {}
        });
        let start=Instant::now();
        let result=run("https://example.com",port,9,||start.elapsed()<Duration::from_millis(150));
        assert_eq!(result.outcome,"cancelled");assert_eq!(result.ms,None);
        assert!(start.elapsed()<Duration::from_secs(2));server.join().unwrap();
    }
}
