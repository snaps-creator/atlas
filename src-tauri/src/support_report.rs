//! Manual offline support export. Never changes network state or runs a probe.
use serde_json::Value;
use std::{
    io::Read,
    os::windows::process::CommandExt,
    process::{Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

const LIMIT: u64 = 1024 * 1024;

pub(crate) fn collect_secrets(value: &Value, result: &mut Vec<String>) {
    match value {
        Value::Object(fields) => {
            for (key, value) in fields {
                let key = key.to_ascii_lowercase();
                if [
                    "password",
                    "uuid",
                    "token",
                    "secret",
                    "key",
                    "authorization",
                ]
                .iter()
                .any(|k| key.contains(k))
                {
                    if let Some(s) = value.as_str().filter(|s| !s.is_empty()) {
                        result.push(s.to_owned());
                    }
                }
                collect_secrets(value, result);
            }
        }
        Value::Array(values) => {
            for value in values {
                collect_secrets(value, result);
            }
        }
        _ => {}
    }
}

pub(crate) fn redact(text: &str, secrets: &[String]) -> String {
    let mut output = text.to_owned();
    let mut secrets = secrets.to_vec();
    secrets.sort_by_key(|s| std::cmp::Reverse(s.len()));
    for secret in secrets {
        if !secret.is_empty() {
            output = output.replace(&secret, "[REDACTED]");
        }
    }
    for (pattern, replacement) in [
        (r#"(?i)\b[a-z][a-z0-9+.-]*://[^\s<>\"']+"#, "[URL REDACTED]"),
        (
            r"(?i)\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b",
            "[UUID REDACTED]",
        ),
        (
            r#"(?i)(?:authorization|password|token|secret|api[_-]?key)\s*[\"']?\s*[:=]\s*(?:\"[^\"]*\"|'[^']*'|(?:Bearer\s+)?[^\s,;]+)"#,
            "[CREDENTIAL REDACTED]",
        ),
    ] {
        output = regex::Regex::new(pattern)
            .unwrap()
            .replace_all(&output, replacement)
            .into_owned();
    }
    if let Ok(profile) = std::env::var("USERPROFILE") {
        output = output.replace(&profile, "%USERPROFILE%");
    }
    output
}

fn reader(stream: impl Read + Send + 'static) -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut data = Vec::new();
        let _ = stream.take(LIMIT).read_to_end(&mut data);
        let mut text = String::from_utf8_lossy(&data).into_owned();
        if data.len() == LIMIT as usize {
            text.push_str("\n[Output truncated]\n");
        }
        let _ = tx.send(text);
    });
    rx
}

fn run_bounded(mut command: Command, timeout: Duration) -> Result<String, String> {
    let mut child = command
        .creation_flags(0x08000000)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let job = match crate::job::Job::attach(&child) {
        Ok(job) => job,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
    };
    let stdout = reader(child.stdout.take().unwrap());
    let stderr = reader(child.stderr.take().unwrap());
    let start = Instant::now();
    let result = loop {
        match child.try_wait() {
            Ok(Some(status)) => break format!("Collector exit: {status}"),
            Err(error) => break format!("Collector error: {error}"),
            _ if start.elapsed() >= timeout => {
                break "Collector timed out; partial report follows.".into()
            }
            _ => std::thread::sleep(Duration::from_millis(30)),
        }
    };
    drop(job); // Also close pipes held by descendants of this collector only.
    let _ = child.wait();
    Ok(format!(
        "{result}\n{}\n{}",
        stdout
            .recv_timeout(Duration::from_secs(1))
            .unwrap_or_default(),
        stderr
            .recv_timeout(Duration::from_secs(1))
            .unwrap_or_default()
    ))
}

pub(crate) fn save(
    context: String,
    client: Option<crate::core::ApiClient>,
    secrets: Vec<String>,
) -> Result<bool, String> {
    let path = rfd::FileDialog::new()
        .set_title("Сохранить диагностику Atlas")
        .set_file_name(format!("atlas-diagnostics-{}.txt", crate::model::now()))
        .add_filter("Текстовый отчёт", &["txt"])
        .save_file();
    let Some(path) = path else {
        return Ok(false);
    };
    let mut command = Command::new("powershell.exe");
    command.args([
        "-NoLogo",
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        include_str!("support_snapshot.ps1"),
    ]);
    let os = run_bounded(command, Duration::from_secs(25))
        .unwrap_or_else(|e| format!("Windows snapshot unavailable: {e}"));
    let logs = client
        .map(|c| c.logs().map(|lines| lines.join("\n")))
        .unwrap_or_else(|| Err("Atlas занят: журнал ядра недоступен".into()))
        .unwrap_or_else(|e| format!("Core logs unavailable: {e}"));
    let text = format!(
        "Atlas diagnostic report\nVersion: {}\nTimestamp (Unix UTC): {}\n\
        Read-only snapshot. Local IP addresses and adapter names are included.\n\
        No automatic network repair or internet probes were performed.\n\n\
        === Atlas state and logs ===\n{context}\n\n=== Core logs ===\n{logs}\n\n{os}\n",
        env!("CARGO_PKG_VERSION"),
        crate::model::now()
    );
    std::fs::write(path, format!("\u{feff}{}", redact(&text, &secrets)))
        .map_err(|e| format!("Не удалось сохранить отчёт: {e}"))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn report_hides_credentials_but_keeps_network_evidence() {
        let mut secrets = vec![];
        collect_secrets(
            &serde_json::json!({"nodes":[{"password":"my-password", "uuid":"credential-id", "reality-opts":{"private-key":"private-material"}}]}),
            &mut secrets,
        );
        let output = redact("my-password credential-id private-material https://host/sub?token=abc vless://secret@host Authorization: Bearer xyz 192.168.1.1 DHCP 68 67", &secrets);
        for secret in [
            "my-password",
            "credential-id",
            "private-material",
            "host/sub",
            "secret@host",
            "xyz",
        ] {
            assert!(!output.contains(secret), "{secret}");
        }
        assert!(output.contains("192.168.1.1 DHCP 68 67"));
        let json_log = redact(
            r#"{"authorization": "Bearer hidden-auth", "token":"hidden-token", "password": "a password with spaces"}"#,
            &[],
        );
        for secret in ["hidden-auth", "hidden-token", "a password with spaces"] {
            assert!(!json_log.contains(secret), "{json_log}");
        }
    }
    #[test]
    fn stalled_collector_returns_partial_output_without_hanging() {
        let mut command = Command::new("powershell.exe");
        command.args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "[Console]::WriteLine('fixture snapshot'); Start-Sleep -Seconds 60",
        ]);
        let started = Instant::now();
        let output = run_bounded(command, Duration::from_secs(2)).unwrap();
        assert!(output.contains("timed out"));
        assert!(output.contains("fixture snapshot"));
        assert!(started.elapsed() < Duration::from_secs(6));
    }

    #[test]
    fn snapshot_keeps_dhcp_and_service_data_when_a_section_fails() {
        // Replace every networking cmdlet. This test must never inspect the host.
        let stubs = r#"
function Get-CimInstance { param($ClassName,$Filter)
 if ($ClassName -eq 'Win32_Service') { [pscustomobject]@{Name='AtlasNetworkService'; State='Running'; ProcessId=123} }
 elseif ($ClassName -eq 'Win32_Process') { [pscustomobject]@{Name='mihomo.exe'; ProcessId=456; ParentProcessId=123} }
 else { [pscustomobject]@{Description='fixture'; DHCPEnabled=$true; DHCPServer='192.168.1.1'; DHCPLeaseObtained='2026-09-22T01:00:00'; DHCPLeaseExpires='2026-09-23T01:00:00'} }
}
function Get-NetAdapter { param([switch]$IncludeHidden) [pscustomobject]@{Name='fixture';Status='Up'} }
function Get-DnsClientServerAddress { [pscustomobject]@{InterfaceAlias='fixture';ServerAddresses=@('192.168.1.1')} }
function Get-NetRoute { throw 'fixture route unavailable' }
function Get-NetIPInterface { [pscustomobject]@{InterfaceAlias='fixture';Dhcp='Enabled'} }
function Get-WinEvent { param($FilterHashtable,$MaxEvents) [pscustomobject]@{Id=1001;Message='fixture DHCP event'} }
"#;
        let script = format!("{stubs}\n{}", include_str!("support_snapshot.ps1"));
        let mut command = Command::new("powershell.exe");
        command.args(["-NoProfile", "-NonInteractive", "-Command", &script]);
        let output = run_bounded(command, Duration::from_secs(8)).unwrap();
        for expected in [
            "AtlasNetworkService",
            "mihomo.exe",
            "2026-09-23T01:00:00",
            "192.168.1.1",
            "fixture route unavailable",
            "Interface metrics and DHCP state",
            "fixture DHCP event",
        ] {
            assert!(output.contains(expected), "missing {expected}: {output}");
        }
    }
}
