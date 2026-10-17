use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use base64::Engine;
use etus::auth::Tokens;
use etus::auth::store::{FileStore, SecretStore};
use etus::config::Paths;
use etus::mcp::{McpClient, TokenSource};
use serde_json::{Value, json};

#[derive(Debug, Clone)]
struct Recorded {
    target: String,
    authorization: String,
    body: Value,
}

struct FakeHub {
    url: String,
    requests: Arc<Mutex<Vec<Recorded>>>,
}

fn token_for(sub: &str) -> String {
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json!({"sub": sub}).to_string());
    format!("eyJhbGciOiJub25lIn0.{payload}.sig")
}

fn tools() -> Value {
    json!([
        {"name": "hub__whoami", "title": "Quem sou eu", "description": "ETUS Platforms: who is connected.", "inputSchema": {"type": "object", "properties": {}}, "annotations": {"readOnlyHint": true, "destructiveHint": false}},
        {"name": "tasks__list_tasks", "title": "Listar tarefas", "description": "Tarefas: Lists tasks of one view.", "inputSchema": {"type": "object", "properties": {"view": {"type": "string", "enum": ["inbox", "today"]}, "limit": {"type": "integer"}}, "required": ["view"]}, "annotations": {"readOnlyHint": true, "destructiveHint": false}},
        {"name": "tasks__create_task", "title": "Criar tarefa", "description": "Tarefas: Creates a task as the person.", "inputSchema": {"type": "object", "properties": {"title": {"type": "string"}, "priority": {"anyOf": [{"const": 1, "type": "number"}, {"const": 4, "type": "number"}]}}, "required": ["title"]}, "annotations": {"readOnlyHint": false, "destructiveHint": false}},
        {"name": "tasks__broken", "title": "Quebrada", "description": "Tarefas: Always fails.", "inputSchema": {"type": "object", "properties": {}}, "annotations": {"readOnlyHint": true}},
        {"name": "tasks__revoked", "title": "Revogada", "description": "Tarefas: Listed but no longer allowed.", "inputSchema": {"type": "object", "properties": {}}, "annotations": {"readOnlyHint": true}},
        {"name": "tasks__agenda", "title": "Ler agenda", "description": "Tarefas: Needs the Google connector.", "inputSchema": {"type": "object", "properties": {}}, "annotations": {"readOnlyHint": true}},
        {"name": "tasks__consent_once", "title": "Autorizar uma vez", "description": "Tarefas: Needs consent on the first call.", "inputSchema": {"type": "object", "properties": {}}, "annotations": {"readOnlyHint": true}}
    ])
}

static CONSENT_CALLS: AtomicUsize = AtomicUsize::new(0);

fn action_required(kind: &str, url: &str) -> Value {
    let structured = json!({"status": "action_required", "action": {"kind": kind, "title": "Conectar o Google", "message": "Conecte sua conta Google para ler a agenda.", "url": url, "missing": ["google"]}});
    json!({"content": [{"type": "text", "text": format!("Para continuar, abra {url}")}], "structuredContent": structured})
}

fn rpc_reply(body: &Value) -> (String, String) {
    let id = body["id"].clone();
    let method = body["method"].as_str().unwrap_or("");
    let ok = |result: Value| json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string();
    let json_ct = "application/json".to_string();
    match method {
        "tools/list" => (json_ct, ok(json!({"tools": tools()}))),
        "tools/call" => {
            let name = body["params"]["name"].as_str().unwrap_or("");
            let args = body["params"]["arguments"].clone();
            match name {
                "hub__whoami" => {
                    let me = json!({"person": {"id": "usr_1", "name": "Ana", "email": "ana@etus.io", "platformAdmin": false}, "activeCompany": {"id": "org_1", "name": "Etus", "slug": "etus", "role": "member"}, "companies": [], "access": "read_write"});
                    (json_ct, ok(json!({"content": [{"type": "text", "text": me.to_string()}], "structuredContent": me})))
                }
                "tasks__list_tasks" => (json_ct, ok(json!({"content": [{"type": "text", "text": args.to_string()}]}))),
                "tasks__create_task" => {
                    let message = json!({"jsonrpc": "2.0", "id": id, "result": {"content": [{"type": "text", "text": json!({"created": args}).to_string()}]}});
                    ("text/event-stream".into(), format!("event: message\ndata: {message}\n\n"))
                }
                "tasks__agenda" => (
                    json_ct,
                    ok(action_required("connect", "https://apps.etus.io/extensoes/conectores/google")),
                ),
                "tasks__consent_once" => {
                    if CONSENT_CALLS.fetch_add(1, Ordering::SeqCst) == 0 {
                        (json_ct, ok(action_required("consent", "https://apps.etus.io/oauth/consent?app=tasks")))
                    } else {
                        (json_ct, ok(json!({"structuredContent": {"ok": true}})))
                    }
                }
                "tasks__broken" => (json_ct, ok(json!({"content": [{"type": "text", "text": "Task not found"}], "isError": true}))),
                _ => (
                    json_ct,
                    json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32602, "message": format!("Unknown tool: {name}")}}).to_string(),
                ),
            }
        }
        _ => (
            json_ct,
            json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": "Method not found"}}).to_string(),
        ),
    }
}

fn handle(mut stream: TcpStream, good: &str, requests: &Mutex<Vec<Recorded>>) {
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
    let target = head
        .lines()
        .next()
        .unwrap_or("")
        .split_whitespace()
        .nth(1)
        .unwrap_or("")
        .to_string();
    let body: Value = serde_json::from_slice(&buffer[header_end..]).unwrap_or(Value::Null);
    let authorization = header("authorization");
    requests.lock().unwrap().push(Recorded {
        target,
        authorization: authorization.clone(),
        body: body.clone(),
    });
    let (status, content_type, payload) = if authorization != format!("Bearer {good}") {
        (
            "401 Unauthorized",
            "application/json".to_string(),
            r#"{"error":"unauthenticated"}"#.to_string(),
        )
    } else {
        let (ct, text) = rpc_reply(&body);
        ("200 OK", ct, text)
    };
    let response = format!(
        "HTTP/1.1 {status}\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{payload}",
        payload.len()
    );
    let _ = stream.write_all(response.as_bytes());
}

fn start(good: String) -> FakeHub {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let shared = requests.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            handle(stream, &good, &shared);
        }
    });
    FakeHub { url, requests }
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("etus-it-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn login_as(dir: &Path, hub: &str, access_token: &str) {
    let tokens = Tokens {
        access_token: access_token.into(),
        refresh_token: None,
        expires_at: etus::catalog::now_secs() + 3600,
        scope: Some("mcp".into()),
        client_id: "mcp_test".into(),
        token_endpoint: format!("{hub}/oauth/token"),
        revocation_endpoint: None,
        resource: format!("{hub}/mcp"),
    };
    FileStore {
        path: Paths::at(dir).tokens_file(hub),
    }
    .save(&tokens)
    .unwrap();
}

fn etus(dir: &Path, hub: &str, args: &[&str]) -> Output {
    etus_with_store(dir, hub, args, Some("file"))
}

fn etus_with_store(dir: &Path, hub: &str, args: &[&str], store: Option<&str>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_etus"));
    command
        .args(args)
        .env("ETUS_CONFIG_DIR", dir)
        .env("ETUS_HUB_URL", hub)
        .env("NO_PROXY", "127.0.0.1")
        .env_remove("ETUS_TOKEN_STORE")
        .stdin(Stdio::null());
    if let Some(store) = store {
        command.env("ETUS_TOKEN_STORE", store);
    }
    command.output().unwrap()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn calls(hub: &FakeHub, tool: &str) -> usize {
    hub.requests
        .lock()
        .unwrap()
        .iter()
        .filter(|r| r.body["method"] == "tools/call" && r.body["params"]["name"] == tool)
        .count()
}

struct Static(String);

impl TokenSource for Static {
    fn access_token(&self, _rejected: Option<&str>) -> etus::error::Result<String> {
        Ok(self.0.clone())
    }
}

#[test]
fn client_lists_and_calls_tools() {
    let token = token_for("u1");
    let hub = start(token.clone());
    let http = reqwest::blocking::Client::new();
    let source = Static(token.clone());
    let client = McpClient::new(&http, format!("{}/mcp", hub.url), &source);
    let listed = client.list_tools().unwrap();
    assert_eq!(listed.len(), 7);
    let outcome = client
        .call_tool("tasks__list_tasks", &json!({"view": "today"}))
        .unwrap();
    assert!(!outcome.is_error);
    assert_eq!(etus::output::payload(&outcome.result), json!({"view": "today"}));
    let streamed = client.call_tool("tasks__create_task", &json!({"title": "x"})).unwrap();
    assert_eq!(
        etus::output::payload(&streamed.result),
        json!({"created": {"title": "x"}})
    );
    let recorded = hub.requests.lock().unwrap().clone();
    assert!(recorded.iter().all(|r| r.authorization == format!("Bearer {token}")));
    assert!(recorded.iter().all(|r| r.target == "/mcp"));
}

#[test]
fn cli_end_to_end_against_fake_gateway() {
    let token = token_for("u1");
    let hub = start(token.clone());
    let dir = temp_dir("e2e");

    let anonymous = etus(&dir, &hub.url, &["apps"]);
    assert_eq!(anonymous.status.code(), Some(3));

    login_as(&dir, &hub.url, &token);

    let apps = etus(&dir, &hub.url, &["apps", "--json"]);
    assert_eq!(apps.status.code(), Some(0), "{}", String::from_utf8_lossy(&apps.stderr));
    let listed: Value = serde_json::from_str(&stdout(&apps)).unwrap();
    let tasks = listed
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["app"] == "tarefas")
        .unwrap();
    assert_eq!(tasks["name"], "Tarefas");
    assert_eq!(tasks["prefix"], "tasks");
    assert!(tasks["aliases"].as_array().unwrap().contains(&json!("tasks")));
    let create = tasks["commands"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["tool"] == "tasks__create_task")
        .unwrap();
    assert_eq!(create["name"], "criar-tarefa");
    assert_eq!(create["aliases"], json!(["create-task"]));

    let human = stdout(&etus(&dir, &hub.url, &["apps"]));
    let line = human.lines().find(|l| l.starts_with("tarefas")).unwrap();
    assert!(line.contains("Tarefas") && line.ends_with("também: tasks"), "{line}");

    let help = etus(&dir, &hub.url, &["--help"]);
    assert!(stdout(&help).contains("tarefas"));

    let app_help = etus(&dir, &hub.url, &["tarefas", "--help"]);
    assert_eq!(app_help.status.code(), Some(0));
    let app_help = stdout(&app_help);
    assert!(app_help.contains("Uso: etus tarefas <COMANDO>"), "{app_help}");
    assert!(app_help.contains("listar-tarefas") && app_help.contains("criar-tarefa"));

    let english_help = stdout(&etus(&dir, &hub.url, &["tasks", "--help"]));
    assert!(english_help.contains("Uso: etus tasks <COMANDO>"), "{english_help}");

    let tool_help = stdout(&etus(&dir, &hub.url, &["tarefas", "criar-tarefa", "--help"]));
    assert!(tool_help.contains("Uso: etus tarefas criar-tarefa"), "{tool_help}");
    assert!(tool_help.contains("Creates a task as the person."));

    let list = etus(
        &dir,
        &hub.url,
        &["tarefas", "list-tasks", "--view", "inbox", "--limit", "3", "--json"],
    );
    assert_eq!(list.status.code(), Some(0), "{}", String::from_utf8_lossy(&list.stderr));
    assert_eq!(
        serde_json::from_str::<Value>(&stdout(&list)).unwrap(),
        json!({"view": "inbox", "limit": 3})
    );

    for (app, command) in [
        ("tasks", "listar-tarefas"),
        ("Tarefas", "list_tasks"),
        ("tarefas", "list-tasks"),
    ] {
        let alias = etus(&dir, &hub.url, &[app, command, "--json", r#"{"view":"today"}"#]);
        assert_eq!(
            serde_json::from_str::<Value>(&stdout(&alias)).unwrap(),
            json!({"view": "today"}),
            "{app} {command}"
        );
    }

    let missing = etus(&dir, &hub.url, &["tasks", "list-tasks"]);
    assert_eq!(missing.status.code(), Some(2));

    let bad_enum = etus(&dir, &hub.url, &["tasks", "list-tasks", "--json", r#"{"view":"week"}"#]);
    assert_eq!(bad_enum.status.code(), Some(2));

    let refused = etus(&dir, &hub.url, &["tasks", "create-task", "--title", "Comprar café"]);
    assert_eq!(refused.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&refused.stderr).contains("--yes"));
    assert_eq!(calls(&hub, "tasks__create_task"), 0);

    let dry = etus(
        &dir,
        &hub.url,
        &[
            "tasks",
            "create-task",
            "--title",
            "Comprar café",
            "--priority",
            "1",
            "--dry-run",
            "--json",
        ],
    );
    assert_eq!(dry.status.code(), Some(0));
    let plan: Value = serde_json::from_str(&stdout(&dry)).unwrap();
    assert_eq!(plan["tool"], "tasks__create_task");
    assert_eq!(plan["arguments"], json!({"title": "Comprar café", "priority": 1}));
    assert_eq!(calls(&hub, "tasks__create_task"), 0);

    let created = etus(
        &dir,
        &hub.url,
        &["tasks", "create-task", "--title", "Comprar café", "--yes", "--json"],
    );
    assert_eq!(
        created.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&created.stderr)
    );
    assert_eq!(
        serde_json::from_str::<Value>(&stdout(&created)).unwrap(),
        json!({"created": {"title": "Comprar café"}})
    );
    assert_eq!(calls(&hub, "tasks__create_task"), 1);

    let broken = etus(&dir, &hub.url, &["tasks", "broken"]);
    assert_eq!(broken.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&broken.stderr).contains("Task not found"));

    let revoked = etus(&dir, &hub.url, &["tasks", "revoked", "--json"]);
    assert_eq!(revoked.status.code(), Some(4));
    let error: Value = serde_json::from_str(String::from_utf8_lossy(&revoked.stderr).trim()).unwrap();
    assert_eq!(error["error"]["exitCode"], 4);

    let unknown = etus(&dir, &hub.url, &["nada", "x"]);
    assert_eq!(unknown.status.code(), Some(2));

    let me = etus(&dir, &hub.url, &["whoami", "--json"]);
    assert_eq!(
        serde_json::from_str::<Value>(&stdout(&me)).unwrap()["activeCompany"]["id"],
        "org_1"
    );

    let quiet = etus(&dir, &hub.url, &["tasks", "list-tasks", "--view", "inbox", "-q"]);
    assert_eq!(quiet.status.code(), Some(0));
    assert_eq!(stdout(&quiet), "");

    let profiled = etus(
        &dir,
        &hub.url,
        &["--perfil", "prf_9", "tasks", "list-tasks", "--view", "inbox"],
    );
    assert_eq!(profiled.status.code(), Some(0));
    assert!(
        hub.requests
            .lock()
            .unwrap()
            .iter()
            .any(|r| r.target == "/mcp?perfil=prf_9")
    );

    let completions = etus(&dir, &hub.url, &["completions", "zsh"]);
    assert_eq!(completions.status.code(), Some(0));
    assert!(stdout(&completions).contains("criar-tarefa"));
}

#[test]
fn action_required_prints_the_link_and_exits_with_code_5() {
    let token = token_for("u1");
    let hub = start(token.clone());
    let dir = temp_dir("action");
    login_as(&dir, &hub.url, &token);

    let human = etus(&dir, &hub.url, &["tarefas", "agenda"]);
    assert_eq!(human.status.code(), Some(5));
    let stderr = String::from_utf8_lossy(&human.stderr);
    assert!(stderr.contains("Ação necessária: Conectar o Google"), "{stderr}");
    assert!(
        stderr.contains("Abra: https://apps.etus.io/extensoes/conectores/google"),
        "{stderr}"
    );
    assert!(stdout(&human).contains("\"status\": \"action_required\""));

    let agent = etus(&dir, &hub.url, &["tarefas", "agenda", "--json"]);
    assert_eq!(agent.status.code(), Some(5));
    let body: Value = serde_json::from_str(&stdout(&agent)).unwrap();
    assert_eq!(body["status"], "action_required");
    assert_eq!(
        body["action"]["url"],
        "https://apps.etus.io/extensoes/conectores/google"
    );
    let error: Value = serde_json::from_str(String::from_utf8_lossy(&agent.stderr).trim()).unwrap();
    assert_eq!(error["error"]["kind"], "action_required");
    assert_eq!(error["error"]["exitCode"], 5);
    assert_eq!(error["error"]["action"]["kind"], "connect");
}

#[test]
fn wait_repeats_the_call_after_the_person_acts() {
    let token = token_for("u1");
    let hub = start(token.clone());
    let dir = temp_dir("action-wait");
    login_as(&dir, &hub.url, &token);

    let output = etus(&dir, &hub.url, &["tarefas", "consent-once", "--wait=20", "--json"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_str::<Value>(&stdout(&output)).unwrap(),
        json!({"ok": true})
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("https://apps.etus.io/oauth/consent?app=tasks"));
    assert_eq!(calls(&hub, "tasks__consent_once"), 2);
}

#[test]
fn rejected_session_asks_for_login() {
    let hub = start(token_for("good"));
    let dir = temp_dir("rejected");
    login_as(&dir, &hub.url, &token_for("stale"));
    let output = etus(&dir, &hub.url, &["apps"]);
    assert_eq!(output.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&output.stderr).contains("etus login"));
}

#[test]
fn token_command_prints_a_valid_token_for_scripts() {
    let token = token_for("u1");
    let hub = start(token.clone());
    let dir = temp_dir("token");

    let anonymous = etus_with_store(&dir, &hub.url, &["token"], None);
    assert_eq!(anonymous.status.code(), Some(3));
    assert_eq!(stdout(&anonymous), "");

    login_as(&dir, &hub.url, &token);
    let plain = etus_with_store(&dir, &hub.url, &["token"], None);
    assert_eq!(
        plain.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&plain.stderr)
    );
    assert_eq!(stdout(&plain), token);
    assert!(!String::from_utf8_lossy(&plain.stderr).contains(&token));

    let json = etus_with_store(&dir, &hub.url, &["token", "--json"], None);
    let value: Value = serde_json::from_str(&stdout(&json)).unwrap();
    assert_eq!(value["accessToken"], token.as_str());
    assert!(value["expiresAt"].as_u64().unwrap() > etus::catalog::now_secs());

    let help = etus_with_store(&dir, &hub.url, &["token", "--help"], None);
    let help = stdout(&help);
    assert!(
        help.contains("segredo") && help.contains("MCP_TOKEN=$(etus token)"),
        "{help}"
    );
}

#[test]
fn config_chooses_where_the_token_lives() {
    let token = token_for("u1");
    let hub = start(token.clone());
    let dir = temp_dir("store-config");
    login_as(&dir, &hub.url, &token);

    let shown = etus_with_store(&dir, &hub.url, &["config", "--json"], None);
    let value: Value = serde_json::from_str(&stdout(&shown)).unwrap();
    if cfg!(not(target_os = "windows")) {
        assert_eq!(value["tokenStore"], "arquivo");
        assert!(value["tokenLocation"].as_str().unwrap().starts_with("arquivo "));
    }

    let chosen = etus_with_store(&dir, &hub.url, &["config", "armazenamento", "arquivo", "--json"], None);
    assert_eq!(
        chosen.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&chosen.stderr)
    );
    let config: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("config.json")).unwrap()).unwrap();
    assert_eq!(config["token_store"], "file");

    let overridden = etus_with_store(&dir, &hub.url, &["config", "armazenamento", "chaveiro"], Some("file"));
    assert_eq!(overridden.status.code(), Some(2));

    let invalid = etus_with_store(&dir, &hub.url, &["apps"], Some("cofre"));
    assert_eq!(invalid.status.code(), Some(2));
}

#[test]
fn setup_fails_without_login() {
    let hub = start(token_for("u1"));
    let dir = temp_dir("setup-no-login");
    let output = etus(&dir, &hub.url, &["setup", "--target", "cursor"]);
    assert_eq!(output.status.code(), Some(3));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("login"), "{stderr}");
}

#[test]
fn setup_succeeds_with_login() {
    let token = token_for("u1");
    let hub = start(token.clone());
    let dir = temp_dir("setup-logged-in");
    login_as(&dir, &hub.url, &token);
    let work_dir = temp_dir("setup-project");
    let home = temp_dir("setup-home");

    let run = |args: &[&str]| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_etus"));
        command
            .args(args)
            .current_dir(&work_dir)
            .env("ETUS_CONFIG_DIR", &dir)
            .env("ETUS_HUB_URL", &hub.url)
            .env("HOME", &home)
            .env("CLAUDE_CONFIG_DIR", home.join(".claude"))
            .env("NO_PROXY", "127.0.0.1")
            .env_remove("ETUS_SKILLS_PROFILES")
            .stdin(Stdio::null());
        command.output().unwrap()
    };
    let output = run(&[
        "setup",
        "--target",
        "cursor",
        "--perfil",
        "codigo",
        "--projeto",
        "test",
        "--workspace",
        "etus",
    ]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(work_dir.join(".cursor/rules/etus.mdc").exists());
    assert!(
        !hub.requests
            .lock()
            .unwrap()
            .iter()
            .any(|r| r.target.contains("perfil=")),
        "the setup profile is not the hub access profile"
    );
    let mcp: Value =
        serde_json::from_str(&std::fs::read_to_string(work_dir.join(".cursor/mcp.json")).unwrap()).unwrap();
    assert_eq!(mcp["mcpServers"]["etus"]["url"], format!("{}/mcp?modo=quente", hub.url));
    let state: Value = serde_json::from_str(&std::fs::read_to_string(work_dir.join(".etus.json")).unwrap()).unwrap();
    assert_eq!(state["project"], "TEST");
    assert_eq!(state["perfil"], serde_json::json!(["codigo"]));
    assert_eq!(state["targets"], serde_json::json!(["cursor"]));

    let healthy = run(&["setup", "diagnosticar"]);
    assert_eq!(healthy.status.code(), Some(0), "{}", stdout(&healthy));

    std::fs::remove_file(work_dir.join(".cursor/rules/etus.mdc")).unwrap();
    let broken = run(&["setup", "doctor"]);
    assert_eq!(broken.status.code(), Some(1));
    assert!(stdout(&broken).contains("FALHA"));

    let synced = run(&["setup", "sincronizar", "--mcp-modo", "compacto"]);
    assert_eq!(
        synced.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&synced.stderr)
    );
    assert!(work_dir.join(".cursor/rules/etus.mdc").exists());
    assert!(
        std::fs::read_to_string(work_dir.join(".cursor/mcp.json"))
            .unwrap()
            .contains("modo=compacto")
    );
    assert_eq!(run(&["setup", "diagnosticar"]).status.code(), Some(0));
}

#[cfg(target_os = "macos")]
#[test]
fn keychain_store_fails_fast_without_a_terminal() {
    let token = token_for("u1");
    let hub = start(token.clone());
    let dir = temp_dir("failfast");
    login_as(&dir, &hub.url, &token);
    for args in [&["apps"][..], &["token"][..], &["logout"][..]] {
        let output = etus_with_store(&dir, &hub.url, args, Some("keychain"));
        assert_eq!(output.status.code(), Some(3), "{args:?}");
        assert!(String::from_utf8_lossy(&output.stderr).contains("chaveiro"));
        assert_eq!(stdout(&output), "");
    }
    assert!(Paths::at(&dir).tokens_file(&hub.url).exists());
}
