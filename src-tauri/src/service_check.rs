//! Optional, user-initiated HTTP diagnostics. Never changes network configuration.
use serde::Serialize;
use std::{
    io::Read,
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

pub(crate) const BODY_LIMIT: usize = 256 * 1024;
pub(crate) const TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Default)]
pub(crate) struct Coordinator {
    generation: AtomicU64,
    epoch: AtomicU64,
    active: AtomicUsize,
}
pub(crate) struct Permit(Arc<Coordinator>);
impl Drop for Permit {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::SeqCst);
    }
}
impl Coordinator {
    pub(crate) fn epoch(&self) -> u64 {
        self.epoch.load(Ordering::SeqCst)
    }
    pub(crate) fn invalidate(&self) {
        self.epoch.fetch_add(1, Ordering::SeqCst);
        self.begin();
    }
    pub(crate) fn begin(&self) -> u64 {
        self.generation.fetch_add(1, Ordering::SeqCst) + 1
    }
    pub(crate) fn cancel(&self, generation: u64) {
        let _ = self.generation.compare_exchange(
            generation,
            generation.wrapping_add(1),
            Ordering::SeqCst,
            Ordering::SeqCst,
        );
    }
    pub(crate) fn current(&self, generation: u64) -> bool {
        self.generation.load(Ordering::SeqCst) == generation
    }
    // Admission happens BEFORE spawn_blocking; no waiting workers or unbounded queue.
    pub(crate) fn acquire(self: &Arc<Self>, generation: u64) -> Result<Permit, String> {
        if !self.current(generation) {
            return Err("Проверка отменена".into());
        }
        self.active.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| (n < 2).then_some(n + 1))
            .map_err(|_| "Предыдущие HTTP-запросы ещё завершаются. Повторите проверку через несколько секунд.".to_owned())?;
        let permit = Permit(self.clone());
        if !self.current(generation) {
            return Err("Проверка отменена".into());
        }
        Ok(permit)
    }
}

pub(crate) fn endpoint(id: &str) -> Option<&'static str> {
    Some(match id {
        "chatgpt" => "https://chatgpt.com",
        "grok" => "https://grok.com",
        "gemini" => "https://gemini.google.com",
        "youtube" => "https://www.youtube.com",
        "telegram" => "https://web.telegram.org",
        _ => return None,
    })
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Outcome {
    HttpResponse,
    RegionalHint,
    AuthRequired,
    HttpError,
    Timeout,
    NetworkError,
    Cancelled,
}

// Deliberately narrow evidence: scripts/translation dictionaries are NOT rendered text.
// No browser engine, JS execution, cookies, credentials, or service-specific routing.
fn visible_text(html: &str) -> String {
    use std::sync::OnceLock;
    static HIDDEN: OnceLock<regex::Regex> = OnceLock::new();
    static TAGS: OnceLock<regex::Regex> = OnceLock::new();
    static HIDDEN_ATTR: OnceLock<regex::Regex> = OnceLock::new();
    let hidden = HIDDEN.get_or_init(|| regex::Regex::new(
        r"(?is)<!--.*?-->|<script\b[^>]*>.*?</script\s*>|<style\b[^>]*>.*?</style\s*>"
    ).unwrap());
    let tags = TAGS.get_or_init(|| regex::Regex::new(r"(?s)<[^>]*>").unwrap());
    let hidden_attr = HIDDEN_ATTR.get_or_init(|| regex::Regex::new(
        r#"(?i)(?:\s)hidden(?:\s|=|/|>)|aria-hidden\s*=\s*["']?true|display\s*:\s*none|visibility\s*:\s*hidden"#
    ).unwrap());
    let text = hidden.replace_all(html, " ");
    if text.contains("<!--") {
        return String::new();
    }
    let mut rendered = String::new();
    let mut stack: Vec<(String, bool)> = Vec::new();
    let mut offset = 0;
    for tag in tags.find_iter(&text) {
        if !stack.last().is_some_and(|(_, hidden)| *hidden) {
            rendered.push_str(&text[offset..tag.start()]);
            rendered.push(' ');
        }
        let raw = tag.as_str().to_ascii_lowercase();
        let name: String = raw
            .trim_start_matches(['<', '/'])
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect();
        if raw.starts_with("</") {
            if let Some(index) = stack.iter().rposition(|(n, _)| n == &name) {
                stack.truncate(index);
            }
        } else if !name.is_empty()
            && !raw.ends_with("/>")
            && ![
                "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta",
                "param", "source", "track", "wbr",
            ]
            .contains(&name.as_str())
        {
            let suppressed = stack.last().is_some_and(|(_, h)| *h)
                || hidden_attr.is_match(&raw)
                || ["script", "style", "head", "template", "noscript"].contains(&name.as_str());
            stack.push((name, suppressed));
            if stack.len() > 128 {
                return String::new();
            }
        }
        offset = tag.end();
    }
    if !stack.last().is_some_and(|(_, hidden)| *hidden) {
        rendered.push_str(&text[offset..]);
    }
    rendered
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .replace("&apos;", "'")
        .replace('’', "'")
        .replace("&nbsp;", " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

pub(crate) fn classify(status: u16, final_host: &str, html: Option<&str>) -> Outcome {
    if status == 401 || ((200..400).contains(&status) && final_host == "accounts.google.com") {
        return Outcome::AuthRequired;
    }
    if let Some(html) = html.filter(|_| final_host == "gemini.google.com") {
        let text = visible_text(html);
        if [
            "gemini isn't currently supported in your country",
            "gemini is not currently supported in your country",
            "gemini is not available in your country",
            "gemini недоступен в вашей стране",
            "gemini пока недоступен в вашей стране",
        ]
        .iter()
        .any(|phrase| text.contains(phrase))
        {
            return Outcome::RegionalHint;
        }
    }
    if (200..400).contains(&status) {
        Outcome::HttpResponse
    } else {
        Outcome::HttpError
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ResultView {
    pub ok: bool, // Compatibility: HTTP status only, never functional service health.
    pub ms: Option<u64>,
    pub status: Option<u16>,
    pub outcome: Outcome,
    pub detail: String,
    pub revision: u64,
    pub body_limited: bool,
}

pub(crate) fn description(outcome: &Outcome) -> &'static str {
    match outcome {
        Outcome::HttpResponse => "Сайт ответил. Работа сервиса и доступ в вашем аккаунте не подтверждены.",
        Outcome::RegionalHint => "В HTTP-ответе найден текст об ограничении региона. Проверка без входа в аккаунт; это не проверка вашей вкладки.",
        Outcome::AuthRequired => "Ответ требует входа или авторизации. Доступ в вашем аккаунте не проверялся.",
        Outcome::HttpError => "Сайт вернул HTTP-ошибку. Код 403/429 сам по себе не доказывает региональную блокировку.",
        Outcome::Timeout => "Время HTTP-запроса истекло. Причина недоступности сервиса не установлена.",
        Outcome::NetworkError => "Не удалось завершить HTTPS-запрос. Причина недоступности сервиса не установлена.",
        Outcome::Cancelled => "Проверка отменена или настройки подключения изменились. Результат не используется.",
    }
}

pub(crate) fn run(
    url: &str,
    proxy_port: u16,
    revision: u64,
    current: impl Fn() -> bool,
) -> ResultView {
    let mut out = ResultView {
        ok: false,
        ms: None,
        status: None,
        outcome: Outcome::Cancelled,
        detail: String::new(),
        revision,
        body_limited: false,
    };
    if !current() {
        out.detail = description(&out.outcome).into();
        return out;
    }
    let started = Instant::now();
    let response = reqwest::blocking::Client::builder()
        .no_proxy()
        .proxy(
            reqwest::Proxy::all(format!("http://127.0.0.1:{proxy_port}"))
                .expect("fixed loopback proxy"),
        )
        .timeout(TIMEOUT)
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        // Preserve the original probe's request headers. An identifying bot UA
        // changed ChatGPT's response from 200 to an anti-bot 403 in paired tests.
        .and_then(|client| client.get(url).send());
    match response {
        Ok(mut response) => {
            out.ms = Some(started.elapsed().as_millis() as u64);
            let status = response.status().as_u16();
            out.status = Some(status);
            out.ok = response.status().is_success();
            let host = response.url().host_str().unwrap_or("").to_owned();
            let html = response
                .headers()
                .get("content-type")
                .and_then(|h| h.to_str().ok())
                .is_some_and(|h| h.to_ascii_lowercase().starts_with("text/html"));
            let encoded = response
                .headers()
                .get("content-encoding")
                .and_then(|h| h.to_str().ok())
                .is_some_and(|h| h != "identity");
            let mut body = Vec::new();
            // Only Gemini has a body-based regional hint classifier. Other
            // probes finish at headers, as before; their HTML is not needed.
            if host == "gemini.google.com" && html && !encoded {
                let mut buffer = [0; 8192];
                while body.len() < BODY_LIMIT && current() && started.elapsed() < TIMEOUT {
                    let room = buffer.len().min(BODY_LIMIT - body.len());
                    match response.read(&mut buffer[..room]) {
                        Ok(0) => break,
                        Ok(n) => body.extend_from_slice(&buffer[..n]),
                        Err(_) => {
                            out.body_limited = true;
                            break;
                        }
                    }
                }
                out.body_limited |= body.len() == BODY_LIMIT || started.elapsed() >= TIMEOUT;
            } else if host == "gemini.google.com" {
                out.body_limited = true;
            }
            out.outcome = classify(
                status,
                &host,
                std::str::from_utf8(&body).ok().filter(|_| html && !encoded),
            );
        }
        Err(error) => {
            out.outcome = if error.is_timeout() {
                Outcome::Timeout
            } else {
                Outcome::NetworkError
            }
        }
    }
    if !current() {
        out.outcome = Outcome::Cancelled;
        out.ok = false;
    }
    out.detail = description(&out.outcome).into();
    if out.body_limited {
        out.detail
            .push_str(" Содержимое ответа проверено не полностью.");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "Explicit verification of the corrected production probe through the running local proxy"]
    fn verify_corrected_probe_live() {
        let port=std::env::var("ATLAS_DIAGNOSTIC_PROXY_PORT").expect("explicit proxy port").parse::<u16>().unwrap();
        for round in 1..=2 {
            for id in ["chatgpt","gemini"] {
                let started=Instant::now();let result=run(endpoint(id).unwrap(),port,0,||true);
                println!("CORRECTED_PROBE {}",serde_json::json!({"round":round,"service":id,"result":result,"totalMs":started.elapsed().as_millis()}));
            }
        }
    }
    #[test]
    #[ignore = "Explicit read-only comparison through an already running local proxy; no settings changes"]
    fn compare_previous_and_current_http_headers() {
        let port=std::env::var("ATLAS_DIAGNOSTIC_PROXY_PORT").expect("explicit proxy port").parse::<u16>().unwrap();
        for id in ["chatgpt","gemini"] {
            for round in 1..=2 {
                for current_headers in [false,true] {
                    let client=reqwest::blocking::Client::builder()
                        .proxy(reqwest::Proxy::all(format!("http://127.0.0.1:{port}")).unwrap())
                        .timeout(TIMEOUT).redirect(reqwest::redirect::Policy::limited(5)).build().unwrap();
                    let request=client.get(endpoint(id).unwrap());
                    let request=if current_headers {request.header("User-Agent","Atlas-Service-Check/1.0").header("Accept-Encoding","identity")} else {request};
                    let started=Instant::now();
                    let evidence=match request.send() {
                        Ok(response)=>serde_json::json!({"service":id,"round":round,"currentHeaders":current_headers,
                            "status":response.status().as_u16(),"headersMs":started.elapsed().as_millis(),
                            "challenge":response.headers().get("cf-mitigated").and_then(|h|h.to_str().ok())==Some("challenge")}),
                        Err(error)=>serde_json::json!({"service":id,"round":round,"currentHeaders":current_headers,
                            "headersMs":started.elapsed().as_millis(),"timeout":error.is_timeout(),"connectError":error.is_connect()}),
                    };
                    println!("HEADER_COMPARISON {evidence}");
                }
            }
        }
    }
    #[test]
    fn http_success_never_means_service_works() {
        assert_eq!(
            classify(200, "gemini.google.com", Some("<main>Welcome</main>")),
            Outcome::HttpResponse
        );
        assert_eq!(
            classify(
                200,
                "gemini.google.com",
                Some("<p>Gemini isn’t currently supported in your country. Stay tuned!</p>")
            ),
            Outcome::RegionalHint
        );
    }
    #[test]
    fn translations_and_http_errors_are_not_regional_evidence() {
        for body in ["<script>Gemini isn't currently supported in your country</script>",
            "<script>const html = '</template>'; const message = \"Gemini isn't currently supported in your country\";</script>",
            "<template>Gemini isn't currently supported in your country</template>",
            "<template><template>nested</template>Gemini isn't currently supported in your country</template>",
            "<div hidden><p>Gemini isn't currently supported in your country</p></div>",
            "<div aria-hidden='true'><span>Gemini isn't currently supported in your country</span></div>",
            "<div style='display:none'>Gemini isn't currently supported in your country</div>",
            "<!-- Gemini isn't currently supported in your country -->"] {
            assert_eq!(classify(200,"gemini.google.com",Some(body)),Outcome::HttpResponse);
        }
        for code in [403, 429, 500] {
            assert_eq!(
                classify(code, "gemini.google.com", None),
                Outcome::HttpError
            );
        }
        assert_eq!(classify(401, "example.org", None), Outcome::AuthRequired);
        assert_eq!(
            classify(200, "accounts.google.com", None),
            Outcome::AuthRequired
        );
        assert_eq!(
            classify(503, "accounts.google.com", None),
            Outcome::HttpError
        );
        assert_eq!(
            classify(200, "accounts.google.com.example.org", None),
            Outcome::HttpResponse
        );
        assert_eq!(
            classify(
                200,
                "example.org",
                Some("Gemini isn't currently supported in your country")
            ),
            Outcome::HttpResponse
        );
    }
    #[test]
    fn admission_is_bounded_and_old_cancel_cannot_cancel_new_batch() {
        let state = Arc::new(Coordinator::default());
        let first = state.begin();
        let a = state.acquire(first).unwrap();
        let b = state.acquire(first).unwrap();
        assert!(state.acquire(first).is_err());
        let next = state.begin();
        state.cancel(first);
        assert!(state.current(next));
        assert!(!state.current(first));
        assert!(state.acquire(next).is_err());
        drop(a);
        drop(b);
        assert!(state.acquire(next).is_ok());
        state.cancel(next);
        assert!(state.acquire(next).is_err());
    }
    #[test]
    fn cancellation_before_start_performs_no_io() {
        let r = run("invalid URL", 0, 7, || false);
        assert_eq!(r.outcome, Outcome::Cancelled);
        assert_eq!(r.revision, 7);
        assert_eq!(r.status, None);
    }
    #[test]
    fn network_epoch_invalidates_only_diagnostics() {
        let state = Arc::new(Coordinator::default());
        let generation = state.begin();
        let old_epoch = state.epoch();
        state.invalidate();
        assert_ne!(state.epoch(), old_epoch);
        assert!(!state.current(generation));
    }
    fn fixture(body: String, status: u16) -> (u16, std::thread::JoinHandle<String>) {
        use std::{io::Write, net::TcpListener};
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let worker = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut request = Vec::new();
            let mut buf = [0; 1024];
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = socket.read(&mut buf).unwrap();
                if n == 0 {
                    break;
                }
                request.extend_from_slice(&buf[..n]);
            }
            let _ = write!(socket, "HTTP/1.1 {status} Fixture\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            String::from_utf8(request).unwrap()
        });
        (port, worker)
    }
    #[test]
    fn actual_http_reader_obeys_body_cap_and_does_not_send_account_credentials() {
        let body = format!(
            "{}<p>Gemini isn't currently supported in your country</p>",
            "x".repeat(BODY_LIMIT)
        );
        let (port, worker) = fixture(body, 200);
        let result = run("http://gemini.google.com/", port, 7, || true);
        assert_eq!(result.outcome, Outcome::HttpResponse);
        assert!(result.body_limited);
        let request = worker.join().unwrap().to_lowercase();
        assert!(request.starts_with("get http://gemini.google.com/"));
        assert!(!request.contains("cookie:"));
        assert!(!request.contains("authorization:"));
        assert!(!request.contains("user-agent:"));
        assert!(!request.contains("accept-encoding:"));
    }
    #[test]
    fn other_sites_finish_at_headers_even_when_body_stalls() {
        use std::io::Write;
        let listener=std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port=listener.local_addr().unwrap().port();
        let (done_tx,done_rx)=std::sync::mpsc::channel();
        let worker=std::thread::spawn(move || {
            let (mut socket,_)=listener.accept().unwrap();
            socket.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            let mut request=Vec::new();let mut buf=[0;1024];
            while !request.windows(4).any(|w|w==b"\r\n\r\n") {
                let n=socket.read(&mut buf).unwrap();if n==0 {break;}request.extend_from_slice(&buf[..n]);
            }
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 1000000\r\n\r\n").unwrap();
            let _=done_rx.recv_timeout(Duration::from_secs(12));
        });
        let started=Instant::now();let result=run("http://chatgpt.com/",port,1,||true);
        let elapsed=started.elapsed();let _=done_tx.send(());worker.join().unwrap();
        assert_eq!(result.status,Some(200));assert!(!result.body_limited);
        assert!(elapsed<Duration::from_secs(2),"unused body delayed the check: {elapsed:?}");
    }
    #[test]
    fn actual_http_200_regional_page_is_a_hint_not_a_functional_success() {
        let (port, worker) = fixture(
            "<main>Gemini isn’t currently supported in your country. Stay tuned!</main>".into(),
            200,
        );
        let result = run("http://gemini.google.com/", port, 9, || true);
        assert_eq!(result.status, Some(200));
        assert_eq!(result.outcome, Outcome::RegionalHint);
        worker.join().unwrap();
    }
    #[test]
    fn response_arriving_after_cancellation_cannot_become_a_success() {
        let (port, worker) = fixture("<main>OK</main>".into(), 200);
        let observations = AtomicUsize::new(0);
        let result = run("http://atlas-fixture.invalid/", port, 9, || {
            observations.fetch_add(1, Ordering::SeqCst) == 0
        });
        assert_eq!(result.outcome, Outcome::Cancelled);
        assert!(!result.ok);
        worker.join().unwrap();
    }
    #[test]
    fn stalled_proxy_is_bounded_by_the_request_deadline() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let (_socket, _) = listener.accept().unwrap();
            let _ = done_rx.recv_timeout(Duration::from_secs(20));
        });
        let start = Instant::now();
        let result = run("http://atlas-fixture.invalid/", port, 1, || true);
        let elapsed = start.elapsed();
        let _ = done_tx.send(());
        worker.join().unwrap();
        assert_eq!(result.outcome, Outcome::Timeout);
        assert!(
            elapsed < TIMEOUT + Duration::from_secs(5),
            "request took {elapsed:?}"
        );
    }
}
