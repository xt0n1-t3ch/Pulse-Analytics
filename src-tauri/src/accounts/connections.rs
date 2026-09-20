use super::{
    AccountSnapshot, ConnectAccountRequest,
    store::{self, AccountRecord},
};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Child, Stdio};
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

struct Connection {
    cancelled: Arc<AtomicBool>,
    child: Arc<Mutex<Option<Child>>>,
    status: String,
    auth_url: Option<String>,
    error: Option<String>,
}

static CONNECTIONS: OnceLock<Mutex<HashMap<String, Connection>>> = OnceLock::new();
fn connections() -> &'static Mutex<HashMap<String, Connection>> {
    CONNECTIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(super) fn overlay(snapshots: &mut [AccountSnapshot]) {
    if let Ok(states) = connections().lock() {
        for snapshot in snapshots {
            if let Some(state) = states.get(&snapshot.id) {
                snapshot.status = state.status.clone();
                snapshot.auth_url = state.auth_url.clone();
                snapshot.error = state.error.clone();
                snapshot.refreshing = false;
            }
        }
    }
}
pub(super) fn is_pending(id: &str) -> bool {
    connections()
        .lock()
        .is_ok_and(|states| states.contains_key(id))
}

pub(super) fn cancel(id: &str) -> Result<()> {
    store::validate_id(id)?;
    let mut states = connections()
        .lock()
        .map_err(|_| anyhow::anyhow!("Connection state is unavailable"))?;
    if let Some(state) = states.get_mut(id) {
        state.cancelled.store(true, Ordering::Release);
        if let Ok(mut child) = state.child.lock()
            && let Some(child) = child.as_mut()
        {
            let _ = child.kill();
            let _ = child.wait();
        }
        state.status = "cancelled".into();
        state.auth_url = None;
        state.error = None;
    }
    Ok(())
}

fn protect_directory(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(windows)]
    {
        let user = std::env::var("USERNAME").context("Windows account name is unavailable")?;
        let domain = std::env::var("USERDOMAIN").unwrap_or_default();
        let owner = if domain.is_empty() {
            user
        } else {
            format!("{domain}\\{user}")
        };
        let status = cc_discord_presence::codex::util::silent_command("icacls.exe")
            .arg(path)
            .args(["/inheritance:r", "/grant:r", &format!("{owner}:(OI)(CI)F")])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        if !status.success() {
            bail!("Could not protect the account profile");
        }
    }
    Ok(())
}

fn save_key(record: &AccountRecord, key: &str, cancelled: &AtomicBool) -> Result<()> {
    let states = connections()
        .lock()
        .map_err(|_| anyhow::anyhow!("Connection state is unavailable"))?;
    if cancelled.load(Ordering::Acquire)
        || !states
            .get(&record.id)
            .is_some_and(|state| std::ptr::eq(state.cancelled.as_ref(), cancelled))
    {
        bail!("Sign-in cancelled");
    }
    if key.trim().is_empty() || key.len() > 16384 || key.contains(['\r', '\n']) {
        bail!("Enter a valid provider key");
    }
    let mut file = tempfile::NamedTempFile::new_in(&record.source_path)?;
    serde_json::to_writer(file.as_file_mut(), &json!({"apiKey":key.trim()}))?;
    file.as_file_mut().sync_all()?;
    file.persist(record.source_path.join("auth.json"))
        .map_err(|_| anyhow::anyhow!("Could not save the protected provider key"))?;
    Ok(())
}

pub(super) fn forget_secret(record: &AccountRecord) -> Result<()> {
    if record.origin != "managed" {
        return Ok(());
    }
    let expected = cc_discord_presence::storage::home()
        .join("accounts")
        .join(&record.id);
    if record.source_path != expected {
        bail!("Account profile ownership does not match");
    }
    for name in ["auth.json", ".credentials.json"] {
        let path = expected.join(name);
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

pub(super) fn connect(request: ConnectAccountRequest) -> Result<String> {
    if !matches!(request.method.as_str(), "browser" | "api_key") {
        bail!("Unsupported connection method");
    }
    if request.method == "api_key"
        && !matches!(request.provider.as_str(), "commandcode" | "opencode")
    {
        bail!("Use subscription sign-in for this provider");
    }
    if request.method == "browser" && request.provider == "opencode" {
        bail!("Connect OpenCode Go with a Go API key");
    }
    let existing = request
        .account_id
        .as_deref()
        .map(store::record)
        .transpose()?;
    if existing
        .as_ref()
        .is_some_and(|record| record.origin != "managed" || record.provider != request.provider)
    {
        bail!("Only a matching Pulse-owned connection can be reconnected");
    }
    let id = existing
        .as_ref()
        .map(|record| Ok(record.id.clone()))
        .unwrap_or_else(store::new_id)?;
    cancel(&id)?;
    let path = cc_discord_presence::storage::home()
        .join("accounts")
        .join(&id);
    if existing
        .as_ref()
        .is_some_and(|record| record.source_path != path)
    {
        bail!("Account profile ownership does not match");
    }
    protect_directory(&path)?;
    let record = AccountRecord {
        id: id.clone(),
        provider: request.provider,
        origin: "managed".into(),
        source_path: path,
        label: request
            .label
            .filter(|value| !value.trim().is_empty())
            .or_else(|| existing.map(|record| record.label))
            .unwrap_or_else(|| "Personal account".into()),
        removed: false,
    };
    if request.method == "api_key"
        && request.api_key.as_deref().is_none_or(|key| {
            key.trim().is_empty() || key.len() > 16384 || key.contains(['\r', '\n'])
        })
    {
        bail!("Enter a valid provider key");
    }
    store::add(&record, &format!("managed:{id}"), true)?;
    let cancelled = Arc::new(AtomicBool::new(false));
    let child = Arc::new(Mutex::new(None));
    connections()
        .lock()
        .map_err(|_| anyhow::anyhow!("Connection state is unavailable"))?
        .insert(
            id.clone(),
            Connection {
                cancelled: cancelled.clone(),
                child: child.clone(),
                status: "connecting".into(),
                auth_url: None,
                error: None,
            },
        );
    std::thread::spawn(move || {
        let result = (|| {
            super::wait_for_idle(&record.id, &cancelled)?;
            if cancelled.load(Ordering::Acquire) {
                bail!("Sign-in cancelled");
            }
            store::save(&AccountSnapshot::pending(&record))?;
            if request.method == "api_key" {
                return save_key(
                    &record,
                    request.api_key.as_deref().unwrap_or_default(),
                    &cancelled,
                );
            }
            match record.provider.as_str() {
                "codex" => login_codex(&record, &cancelled, &child),
                "claude" => login_claude(&record, &cancelled, &child),
                "commandcode" => login_commandcode(&record, &cancelled),
                _ => Err(anyhow::anyhow!("Unsupported browser connection")),
            }
        })();
        if let Ok(mut slot) = child.lock()
            && let Some(mut process) = slot.take()
        {
            let _ = process.kill();
            let _ = process.wait();
        }
        let Ok(mut states) = connections().lock() else {
            return;
        };
        let current = states
            .get(&record.id)
            .is_some_and(|state| Arc::ptr_eq(&state.cancelled, &cancelled));
        if !current || cancelled.load(Ordering::Acquire) {
            drop(states);
            if store::record(&record.id).is_err() {
                let _ = forget_secret(&record);
            }
            return;
        }
        match result {
            Ok(()) => {
                states.remove(&record.id);
                drop(states);
                super::invalidate(&record.id);
                let _ = super::schedule(&record.id, true);
            }
            Err(error) => {
                if let Some(state) = states.get_mut(&record.id) {
                    state.status = "error".into();
                    state.auth_url = None;
                    state.error = Some(error.to_string());
                }
            }
        }
    });
    super::start();
    Ok(id)
}

fn auth_url(id: &str, url: String, cancelled: &AtomicBool) -> Result<()> {
    if ![
        "https://auth.openai.com/",
        "https://chatgpt.com/",
        "https://claude.ai/",
        "https://console.anthropic.com/",
        "https://platform.claude.com/",
        "https://commandcode.ai/",
    ]
    .iter()
    .any(|prefix| url.starts_with(prefix))
    {
        bail!("Provider returned an unsupported sign-in address");
    }
    if let Ok(mut states) = connections().lock()
        && let Some(state) = states.get_mut(id)
    {
        if state.cancelled.load(Ordering::Acquire)
            || !std::ptr::eq(state.cancelled.as_ref(), cancelled)
        {
            bail!("Sign-in cancelled");
        }
        state.auth_url = Some(url);
        state.status = "awaiting_login".into();
    }
    Ok(())
}

fn login_codex(
    record: &AccountRecord,
    cancelled: &AtomicBool,
    holder: &Mutex<Option<Child>>,
) -> Result<()> {
    let mut process =
        cc_discord_presence::codex::account_usage::isolated_app_server_command(&record.source_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("Codex could not start its isolated sign-in service")?;
    let mut stdin = process
        .stdin
        .take()
        .context("Codex sign-in input is unavailable")?;
    let stdout = process
        .stdout
        .take()
        .context("Codex sign-in output is unavailable")?;
    *holder
        .lock()
        .map_err(|_| anyhow::anyhow!("Connection process state is unavailable"))? = Some(process);
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    writeln!(
        stdin,
        "{}",
        json!({"id":1,"method":"initialize","params":{"clientInfo":{"name":"pulse-accounts","version":env!("CARGO_PKG_VERSION")}}})
    )?;
    stdin.flush()?;
    let deadline = Instant::now() + Duration::from_secs(600);
    while Instant::now() < deadline && !cancelled.load(Ordering::Acquire) {
        let line = match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(line) => line,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(_) => bail!("Codex sign-in service stopped"),
        };
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if value["id"] == 1 {
            if value.get("error").is_some() {
                bail!("Codex rejected sign-in initialization");
            }
            writeln!(stdin, "{}", json!({"method":"initialized","params":{}}))?;
            writeln!(
                stdin,
                "{}",
                json!({"id":2,"method":"account/login/start","params":{"type":"chatgpt"}})
            )?;
            stdin.flush()?;
        } else if value["id"] == 2 {
            let url = value
                .pointer("/result/authUrl")
                .and_then(Value::as_str)
                .context("Codex could not start browser sign-in")?;
            auth_url(&record.id, url.into(), cancelled)?;
        } else if value["method"] == "account/login/completed" {
            if value.pointer("/params/success").and_then(Value::as_bool) == Some(true) {
                return Ok(());
            }
            bail!("Codex sign-in was not completed");
        }
    }
    bail!("Sign-in was cancelled or timed out")
}

fn login_claude(
    record: &AccountRecord,
    cancelled: &AtomicBool,
    holder: &Mutex<Option<Child>>,
) -> Result<()> {
    let mut command = cc_discord_presence::codex::util::silent_command("claude");
    command
        .args(["auth", "login", "--claudeai"])
        .env("CLAUDE_CONFIG_DIR", &record.source_path);
    for key in [
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "ANTHROPIC_PROFILE",
        "CLAUDE_CODE_USE_BEDROCK",
        "CLAUDE_CODE_USE_VERTEX",
        "CLAUDE_CODE_USE_FOUNDRY",
    ] {
        command.env_remove(key);
    }
    let mut process = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("Claude CLI is required for isolated subscription sign-in")?;
    let stdout = process
        .stdout
        .take()
        .context("Claude sign-in output is unavailable")?;
    *holder
        .lock()
        .map_err(|_| anyhow::anyhow!("Connection process state is unavailable"))? = Some(process);
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let deadline = Instant::now() + Duration::from_secs(600);
    while Instant::now() < deadline && !cancelled.load(Ordering::Acquire) {
        if let Ok(line) = rx.recv_timeout(Duration::from_millis(200))
            && let Some(start) = line.find("https://")
        {
            let url = line[start..]
                .split(|c: char| c.is_whitespace() || c == '\u{1b}')
                .next()
                .unwrap_or_default();
            let _ = auth_url(&record.id, url.to_owned(), cancelled);
        }
        let exited = holder.lock().ok().and_then(|mut slot| {
            slot.as_mut()
                .and_then(|child| child.try_wait().ok().flatten())
        });
        if let Some(status) = exited {
            if status.success() && record.source_path.join(".credentials.json").is_file() {
                return Ok(());
            }
            bail!("Claude sign-in did not complete. Retry with an available Claude CLI.");
        }
    }
    bail!("Sign-in was cancelled or timed out")
}

fn callback(stream: &mut TcpStream, state: &str) -> Result<Option<String>> {
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    stream.set_write_timeout(Some(Duration::from_secs(3)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.by_ref().take(8193).read_line(&mut line)?;
    if line.len() > 8192 {
        bail!("Invalid callback request");
    }
    let options = line.starts_with("OPTIONS /callback ");
    if !options && !line.starts_with("POST /callback ") {
        return respond(stream, 405, false).map(|_| None);
    }
    let mut length = None;
    let mut origin = false;
    let mut total = line.len();
    loop {
        line.clear();
        reader.by_ref().take(8193).read_line(&mut line)?;
        total += line.len();
        if total > 8192 || line.is_empty() {
            bail!("Invalid callback headers");
        }
        if line == "\r\n" || line == "\n" {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("origin") {
                origin = value.trim() == "https://commandcode.ai";
            }
            if name.eq_ignore_ascii_case("transfer-encoding") {
                bail!("Unsupported callback framing");
            }
            if name.eq_ignore_ascii_case("content-length") {
                if length.is_some() {
                    bail!("Ambiguous callback framing");
                }
                length = value.trim().parse::<usize>().ok();
            }
        }
    }
    if options {
        return respond(stream, if origin { 204 } else { 403 }, origin).map(|_| None);
    }
    let length = length
        .filter(|length| *length <= 16384)
        .context("Invalid callback body size")?;
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    if !origin {
        return respond(stream, 403, false).map(|_| None);
    }
    let value: Value =
        serde_json::from_slice(&body).map_err(|_| anyhow::anyhow!("Invalid callback body"))?;
    if value["state"].as_str() != Some(state) {
        return respond(stream, 403, false).map(|_| None);
    }
    let key = value["apiKey"]
        .as_str()
        .filter(|key| !key.is_empty() && !key.contains(['\r', '\n']));
    let Some(key) = key else {
        return respond(stream, 400, true).map(|_| None);
    };
    respond(stream, 200, true)?;
    Ok(Some(key.to_owned()))
}

fn respond(stream: &mut TcpStream, status: u16, cors: bool) -> Result<()> {
    let body = if status == 204 {
        ""
    } else if status == 200 {
        r#"{"success":true}"#
    } else {
        r#"{"success":false}"#
    };
    let headers = if cors {
        "Access-Control-Allow-Origin: https://commandcode.ai\r\nAccess-Control-Allow-Methods: POST, OPTIONS\r\nAccess-Control-Allow-Headers: Content-Type\r\nAccess-Control-Allow-Private-Network: true\r\n"
    } else {
        ""
    };
    write!(
        stream,
        "HTTP/1.1 {status} Response\r\n{headers}Content-Type: application/json\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )?;
    stream.flush()?;
    Ok(())
}

fn login_commandcode(record: &AccountRecord, cancelled: &AtomicBool) -> Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    listener.set_nonblocking(true)?;
    let state = store::new_id()?;
    let port = listener.local_addr()?.port();
    auth_url(
        &record.id,
        format!(
            "https://commandcode.ai/studio/auth/cli?callback=http%3A%2F%2Flocalhost%3A{port}%2Fcallback&state={state}"
        ),
        cancelled,
    )?;
    let deadline = Instant::now() + Duration::from_secs(600);
    while Instant::now() < deadline && !cancelled.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((mut stream, _)) => {
                if let Ok(Some(key)) = callback(&mut stream, &state) {
                    save_key(record, &key, cancelled)?;
                    return Ok(());
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(100))
            }
            Err(error) => return Err(error.into()),
        }
    }
    bail!("Sign-in was cancelled or timed out")
}

#[cfg(test)]
mod tests {
    use super::*;
    fn post_callback(origin: &str, supplied_state: &str, expected_state: &str) -> String {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let expected_state = expected_state.to_owned();
        let task = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            callback(&mut stream, &expected_state).unwrap()
        });
        let mut stream = TcpStream::connect(address).unwrap();
        let body = json!({"state":supplied_state,"apiKey":"test-key"}).to_string();
        write!(stream,"POST /callback HTTP/1.1\r\nHost: localhost\r\nOrigin: {origin}\r\nContent-Length: {}\r\n\r\n{body}",body.len()).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        let result = task.join().unwrap();
        assert_eq!(
            result.is_some(),
            origin == "https://commandcode.ai" && supplied_state == expected_state_for_test()
        );
        response
    }
    fn expected_state_for_test() -> &'static str {
        "expected"
    }
    #[test]
    fn callback_requires_both_provider_origin_and_current_random_state() {
        assert!(
            post_callback("https://commandcode.ai", "expected", "expected")
                .starts_with("HTTP/1.1 200")
        );
        assert!(
            post_callback("https://commandcode.ai", "wrong", "expected")
                .starts_with("HTTP/1.1 403")
        );
        assert!(
            post_callback("https://example.invalid", "expected", "expected")
                .starts_with("HTTP/1.1 403")
        );
    }
    #[test]
    fn forgetting_a_linked_profile_never_changes_its_credentials() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("auth.json");
        std::fs::write(&path, b"original").unwrap();
        let record = AccountRecord {
            id: "a".repeat(32),
            provider: "codex".into(),
            origin: "local".into(),
            source_path: temp.path().to_owned(),
            label: "Local".into(),
            removed: false,
        };
        forget_secret(&record).unwrap();
        assert_eq!(std::fs::read(path).unwrap(), b"original");
    }
    #[test]
    fn only_official_provider_sign_in_origins_are_accepted() {
        assert!(
            auth_url(
                "missing",
                "https://auth.openai.com/authorize?state=test".into(),
                &AtomicBool::new(false)
            )
            .is_ok()
        );
        assert!(
            auth_url(
                "missing",
                "https://auth.openai.com.attacker.invalid/authorize".into(),
                &AtomicBool::new(false)
            )
            .is_err()
        );
        assert!(
            auth_url(
                "missing",
                "http://commandcode.ai/studio/auth/cli".into(),
                &AtomicBool::new(false)
            )
            .is_err()
        );
    }
    #[test]
    fn cancelled_connection_cannot_recreate_a_removed_secret() {
        let temp = tempfile::tempdir().unwrap();
        let record = AccountRecord {
            id: "c".repeat(32),
            provider: "commandcode".into(),
            origin: "managed".into(),
            source_path: temp.path().to_owned(),
            label: "Test".into(),
            removed: true,
        };
        assert!(save_key(&record, "test-key", &AtomicBool::new(true)).is_err());
        assert!(!temp.path().join("auth.json").exists());
    }
    #[test]
    fn old_connection_generation_cannot_replace_a_new_key_or_sign_in_url() {
        let temp = tempfile::tempdir().unwrap();
        let record = AccountRecord {
            id: "d".repeat(32),
            provider: "commandcode".into(),
            origin: "managed".into(),
            source_path: temp.path().to_owned(),
            label: "Test".into(),
            removed: false,
        };
        std::fs::write(temp.path().join("auth.json"), b"new-key").unwrap();
        let current = Arc::new(AtomicBool::new(false));
        connections().lock().unwrap().insert(
            record.id.clone(),
            Connection {
                cancelled: current,
                child: Arc::new(Mutex::new(None)),
                status: "connecting".into(),
                auth_url: None,
                error: None,
            },
        );
        let previous = AtomicBool::new(false);
        assert!(save_key(&record, "old-key", &previous).is_err());
        assert!(
            auth_url(
                &record.id,
                "https://commandcode.ai/studio/auth/cli".into(),
                &previous
            )
            .is_err()
        );
        assert_eq!(
            std::fs::read(temp.path().join("auth.json")).unwrap(),
            b"new-key"
        );
        assert!(
            connections()
                .lock()
                .unwrap()
                .remove(&record.id)
                .unwrap()
                .auth_url
                .is_none()
        );
    }
}
