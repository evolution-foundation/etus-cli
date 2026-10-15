use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::Arc;

use etus::update::{asset_name, sha256_hex};
use serde_json::Value;

const PAYLOAD: &[u8] = b"#!/bin/sh\necho etus 99.0.0\n";

fn serve(stream: &mut TcpStream, files: &HashMap<String, Vec<u8>>) {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 1024];
    while !buffer.windows(4).any(|w| w == b"\r\n\r\n") {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => buffer.extend_from_slice(&chunk[..n]),
        }
    }
    let head = String::from_utf8_lossy(&buffer).to_string();
    let path = head.split_whitespace().nth(1).unwrap_or("").to_string();
    let (status, body) = match files.get(&path) {
        Some(body) => ("200 OK", body.clone()),
        None => ("404 Not Found", b"not found".to_vec()),
    };
    let header = format!(
        "HTTP/1.1 {status}\r\ncontent-type: application/octet-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(&body);
}

fn start(files: HashMap<String, Vec<u8>>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://127.0.0.1:{}/dl/", listener.local_addr().unwrap().port());
    let files = Arc::new(files);
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            serve(&mut stream, &files);
        }
    });
    url
}

fn release(version: &str, sum: &str) -> HashMap<String, Vec<u8>> {
    let name = asset_name();
    HashMap::from([
        ("/dl/VERSION".to_string(), format!("{version}\n").into_bytes()),
        ("/dl/SHA256SUMS".to_string(), format!("{sum}  {name}\n").into_bytes()),
        (format!("/dl/{name}"), PAYLOAD.to_vec()),
    ])
}

fn installed_copy(name: &str) -> (PathBuf, Vec<u8>) {
    let dir = std::env::temp_dir().join(format!("etus-update-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let exe = dir.join("etus");
    std::fs::copy(env!("CARGO_BIN_EXE_etus"), &exe).unwrap();
    let original = std::fs::read(&exe).unwrap();
    (exe, original)
}

fn update(exe: &Path, base: &str, args: &[&str]) -> Output {
    Command::new(exe)
        .arg("atualizar-cli")
        .args(args)
        .env("ETUS_CONFIG_DIR", exe.parent().unwrap())
        .env("ETUS_CLI_DOWNLOAD_BASE", base)
        .env("NO_PROXY", "127.0.0.1")
        .env_remove("GH_TOKEN")
        .env_remove("GITHUB_TOKEN")
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn json_out(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or(Value::Null)
}

#[test]
fn updates_from_the_public_base_without_a_token() {
    let base = start(release("99.0.0", &sha256_hex(PAYLOAD)));
    let (exe, _) = installed_copy("ok");

    let check = update(&exe, &base, &["--verificar", "--json"]);
    assert!(check.status.success(), "{check:?}");
    let report = json_out(&check);
    assert_eq!(report["latest"], "99.0.0");
    assert_eq!(report["updateAvailable"], true);

    let done = update(&exe, &base, &["--json"]);
    assert!(done.status.success(), "{done:?}");
    assert_eq!(json_out(&done)["current"], "99.0.0");
    assert_eq!(std::fs::read(&exe).unwrap(), PAYLOAD);
}

#[test]
fn refuses_a_binary_that_does_not_match_the_sums() {
    let base = start(release("99.0.0", &"0".repeat(64)));
    let (exe, original) = installed_copy("bad-sum");
    let output = update(&exe, &base, &["--json"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("sha256"));
    assert_eq!(std::fs::read(&exe).unwrap(), original);
}

#[test]
fn keeps_the_binary_when_the_published_version_is_not_newer() {
    let base = start(release("0.0.1", &sha256_hex(PAYLOAD)));
    let (exe, original) = installed_copy("older");
    let output = update(&exe, &base, &["--json"]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(json_out(&output)["updateAvailable"], false);
    assert_eq!(std::fs::read(&exe).unwrap(), original);
}

#[test]
fn explicit_base_without_files_is_an_error() {
    let base = start(HashMap::new());
    let (exe, original) = installed_copy("missing");
    let output = update(&exe, &base, &[]);
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("ETUS_CLI_DOWNLOAD_BASE"));
    assert_eq!(std::fs::read(&exe).unwrap(), original);
}
