use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use etus::auth::Tokens;
use etus::auth::store::{FileStore, SecretStore};
use etus::config::Paths;
use serde_json::{Value, json};

const ACCESS: &str = "hub-access-token";
const FORWARD: &str = "hub-forward-token";

#[derive(Debug, Clone)]
struct Recorded {
    method: String,
    target: String,
    authorization: String,
    body: Value,
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Grant,
    NoAiUse,
}

struct Fake {
    url: String,
    requests: Arc<Mutex<Vec<Recorded>>>,
}

impl Fake {
    fn find(&self, method: &str, target: &str) -> Vec<Recorded> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.method == method && r.target == target)
            .cloned()
            .collect()
    }
}

fn device_key(n: usize) -> String {
    format!(
        "sk-etd-{}",
        format!("{n}").repeat(43).chars().take(43).collect::<String>()
    )
}

fn reply(mode: Mode, issued: &AtomicUsize, request: &Recorded) -> (&'static str, String) {
    let bearer = |token: &str| request.authorization == format!("Bearer {token}");
    match (request.method.as_str(), request.target.as_str()) {
        ("POST", "/api/v1/mcp/forward-tokens") if bearer(ACCESS) => (
            "200 OK",
            json!({"token": FORWARD, "tokenType": "Bearer", "expiresIn": 60}).to_string(),
        ),
        ("POST", "/admin/api/device-keys") if bearer(FORWARD) && mode == Mode::NoAiUse => (
            "403 Forbidden",
            json!({"error": "permission_required", "permission": "ai.use", "status": "action_required", "action": {"kind": "permission", "title": "Pedir acesso a Usar a IA", "message": "Peça a um admin a permissão Usar a IA.", "url": "https://apps.etus.io/access-requests/new?app=router-panel&permission=ai.use&company=org_1", "missing": ["router-panel:ai.use"]}}).to_string(),
        ),
        ("POST", "/admin/api/device-keys") if bearer(FORWARD) => {
            let n = issued.fetch_add(1, Ordering::SeqCst) + 1;
            (
                "201 Created",
                json!({"key": device_key(n), "device": {"id": format!("dev_{n}"), "name": request.body["name"], "keyPrefix": "sk-etd-xxxx", "createdAt": "2026-10-16T12:00:00Z", "lastUsedAt": null, "revokedAt": null}}).to_string(),
            )
        }
        ("DELETE", "/admin/api/device-keys/current") => ("204 No Content", String::new()),
        _ => ("401 Unauthorized", json!({"error": "unauthenticated"}).to_string()),
    }
}

fn handle(mut stream: TcpStream, mode: Mode, issued: &AtomicUsize, requests: &Mutex<Vec<Recorded>>) {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        let n = stream.read(&mut chunk).unwrap_or(0);
        if n == 0 {
            return;
        }
        buffer.extend_from_slice(&chunk[..n]);
        if let Some(pos) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos + 4;
        }
    };
    let head = String::from_utf8_lossy(&buffer[..header_end]).to_string();
    let header = |name: &str| {
        head.lines()
            .find_map(|l| {
                l.split_once(':')
                    .filter(|(k, _)| k.trim().eq_ignore_ascii_case(name))
                    .map(|(_, v)| v.trim().to_string())
            })
            .unwrap_or_default()
    };
    let length: usize = header("content-length").parse().unwrap_or(0);
    while buffer.len() < header_end + length {
        let n = stream.read(&mut chunk).unwrap_or(0);
        if n == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..n]);
    }
    let mut first = head.lines().next().unwrap_or("").split_whitespace();
    let request = Recorded {
        method: first.next().unwrap_or("").to_string(),
        target: first.next().unwrap_or("").to_string(),
        authorization: header("authorization"),
        body: serde_json::from_slice(&buffer[header_end..]).unwrap_or(Value::Null),
    };
    let (status, payload) = reply(mode, issued, &request);
    requests.lock().unwrap().push(request);
    let response = format!(
        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{payload}",
        payload.len()
    );
    let _ = stream.write_all(response.as_bytes());
}

fn start(mode: Mode) -> Fake {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let shared = requests.clone();
    std::thread::spawn(move || {
        let issued = AtomicUsize::new(0);
        for stream in listener.incoming().flatten() {
            handle(stream, mode, &issued, &shared);
        }
    });
    Fake { url, requests }
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("etus-router-it-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("claude")).unwrap();
    dir
}

fn login(dir: &Path, hub: &str) {
    FileStore {
        path: Paths::at(dir).tokens_file(hub),
    }
    .save(&Tokens {
        access_token: ACCESS.into(),
        refresh_token: None,
        expires_at: etus::catalog::now_secs() + 3600,
        scope: Some("mcp".into()),
        client_id: "mcp_test".into(),
        token_endpoint: format!("{hub}/oauth/token"),
        revocation_endpoint: None,
        resource: format!("{hub}/mcp"),
    })
    .unwrap();
}

fn etus(dir: &Path, hub: &str, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_etus"))
        .args(args)
        .env("ETUS_CONFIG_DIR", dir)
        .env("ETUS_HUB_URL", hub)
        .env("ETUS_TOKEN_STORE", "file")
        .env("CLAUDE_CONFIG_DIR", dir.join("claude"))
        .env("NO_PROXY", "127.0.0.1")
        .env_remove("OMNIROUTE_API_KEY")
        .env_remove("ETUS_ROUTER_API_URL")
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn settings(dir: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(dir.join("claude/settings.json")).unwrap()).unwrap()
}

#[test]
fn setup_gets_this_computers_key_from_the_login_and_logout_revokes_it() {
    let fake = start(Mode::Grant);
    let dir = temp_dir("login");
    login(&dir, &fake.url);
    std::fs::write(
        dir.join("claude/settings.json"),
        json!({"env": {"ANTHROPIC_BASE_URL": fake.url, "ANTHROPIC_AUTH_TOKEN": "sk-plain-old-token-0000"}}).to_string(),
    )
    .unwrap();

    let setup = etus(&dir, &fake.url, &["--json", "router", "setup", "--url", &fake.url]);
    assert!(setup.status.success(), "{}", String::from_utf8_lossy(&setup.stderr));
    assert!(!String::from_utf8_lossy(&setup.stdout).contains(&device_key(1)));
    let written = settings(&dir);
    assert!(written["env"].get("ANTHROPIC_AUTH_TOKEN").is_none());
    assert!(written["apiKeyHelper"].as_str().unwrap().ends_with(" router chave"));
    assert!(!written.to_string().contains("sk-etd-"));
    let created = fake.find("POST", "/admin/api/device-keys");
    assert_eq!(created.len(), 1);
    assert!(created[0].body["name"].as_str().is_some_and(|n| !n.is_empty()));
    assert!(created[0].body["replaces"].is_null());
    assert_eq!(
        fake.find("POST", "/api/v1/mcp/forward-tokens")[0].body,
        json!({"appKey": "router-panel"})
    );

    let key = etus(&dir, &fake.url, &["router", "chave"]);
    assert!(key.status.success());
    assert_eq!(stdout(&key), device_key(1));

    let again = etus(&dir, &fake.url, &["router", "setup", "--url", &fake.url]);
    assert!(again.status.success(), "{}", String::from_utf8_lossy(&again.stderr));
    assert_eq!(fake.find("POST", "/admin/api/device-keys")[1].body["replaces"], "dev_1");
    assert_eq!(stdout(&etus(&dir, &fake.url, &["router", "chave"])), device_key(2));

    let status = etus(&dir, &fake.url, &["--json", "router", "status"]);
    let status: Value = serde_json::from_str(&stdout(&status)).unwrap();
    assert_eq!(status["keyOrigin"], "login");
    assert!(!status.to_string().contains(&device_key(2)));

    let logout = etus(&dir, &fake.url, &["logout"]);
    assert!(logout.status.success());
    let revoked = fake.find("DELETE", "/admin/api/device-keys/current");
    assert_eq!(revoked.len(), 1);
    assert_eq!(revoked[0].authorization, format!("Bearer {}", device_key(2)));
    assert_eq!(etus(&dir, &fake.url, &["router", "chave"]).status.code(), Some(3));
}

#[test]
fn without_ai_use_setup_shows_the_request_access_link() {
    let fake = start(Mode::NoAiUse);
    let dir = temp_dir("no-ai-use");
    login(&dir, &fake.url);
    let setup = etus(&dir, &fake.url, &["router", "setup", "--url", &fake.url]);
    assert_eq!(setup.status.code(), Some(5));
    let stderr = String::from_utf8_lossy(&setup.stderr);
    assert!(stderr.contains("access-requests/new?app=router-panel&permission=ai.use"));
    assert!(!dir.join("claude/settings.json").exists());
}

#[test]
fn without_login_a_plain_token_moves_to_the_vault_and_chave_is_plan_b() {
    let fake = start(Mode::Grant);
    let dir = temp_dir("legacy");
    std::fs::write(
        dir.join("claude/settings.json"),
        json!({"env": {"ANTHROPIC_BASE_URL": fake.url, "ANTHROPIC_AUTH_TOKEN": "sk-plain-old-token-0000"}}).to_string(),
    )
    .unwrap();
    let setup = etus(&dir, &fake.url, &["router", "setup", "--url", &fake.url]);
    assert!(setup.status.success(), "{}", String::from_utf8_lossy(&setup.stderr));
    assert!(settings(&dir)["env"].get("ANTHROPIC_AUTH_TOKEN").is_none());
    assert_eq!(
        stdout(&etus(&dir, &fake.url, &["router", "chave"])),
        "sk-plain-old-token-0000"
    );
    assert!(fake.find("POST", "/admin/api/device-keys").is_empty());

    let manual = etus(
        &dir,
        &fake.url,
        &["router", "setup", "sk-manual-key-000000000", "--url", &fake.url],
    );
    assert!(manual.status.success());
    assert_eq!(
        stdout(&etus(&dir, &fake.url, &["router", "chave"])),
        "sk-manual-key-000000000"
    );

    let fresh = temp_dir("nothing");
    let refused = etus(&fresh, &fake.url, &["router", "setup", "--url", &fake.url]);
    assert_eq!(refused.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&refused.stderr).contains("etus login"));
}
