use std::io::{BufRead, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc;
use std::time::Duration;

use base64::Engine;
use reqwest::Url;
use reqwest::blocking::Client;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::auth::manager::Refresher;
use crate::auth::store::Tokens;
use crate::catalog::now_secs;
use crate::error::{Error, Result};

pub const CLIENT_NAME: &str = "Etus CLI";
pub const CALLBACK_PATH: &str = "/callback";
pub const LOOPBACK_REDIRECT: &str = "http://127.0.0.1/callback";

#[derive(Debug, Clone, PartialEq)]
pub struct Discovery {
    pub resource: String,
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub registration_endpoint: Option<String>,
    pub revocation_endpoint: Option<String>,
    pub client_id_metadata_document_supported: bool,
}

#[derive(Deserialize)]
struct ResourceMetadata {
    resource: String,
    #[serde(default)]
    authorization_servers: Vec<String>,
}

#[derive(Deserialize)]
struct ServerMetadata {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
    #[serde(default)]
    registration_endpoint: Option<String>,
    #[serde(default)]
    revocation_endpoint: Option<String>,
    #[serde(default)]
    code_challenge_methods_supported: Vec<String>,
    #[serde(default)]
    client_id_metadata_document_supported: bool,
}

fn get_json<T: for<'de> Deserialize<'de>>(http: &Client, url: &str) -> Result<T> {
    let response = http.get(url).header("accept", "application/json").send()?;
    if !response.status().is_success() {
        return Err(Error::Network(format!(
            "O hub não respondeu à descoberta OAuth ({url}: HTTP {}).",
            response.status().as_u16()
        )));
    }
    response
        .json()
        .map_err(|_| Error::Network(format!("Resposta de descoberta inválida em {url}.")))
}

pub fn discover(http: &Client, hub: &str) -> Result<Discovery> {
    let resource: ResourceMetadata = get_json(http, &format!("{hub}/.well-known/oauth-protected-resource/mcp"))?;
    let server = resource
        .authorization_servers
        .first()
        .cloned()
        .unwrap_or_else(|| hub.to_string());
    let server = server.trim_end_matches('/');
    let meta: ServerMetadata = get_json(http, &format!("{server}/.well-known/oauth-authorization-server"))?;
    if !meta.code_challenge_methods_supported.iter().any(|m| m == "S256") {
        return Err(Error::Other("O servidor de autorização não suporta PKCE S256.".into()));
    }
    Ok(Discovery {
        resource: resource.resource,
        issuer: meta.issuer,
        authorization_endpoint: meta.authorization_endpoint,
        token_endpoint: meta.token_endpoint,
        registration_endpoint: meta.registration_endpoint,
        revocation_endpoint: meta.revocation_endpoint,
        client_id_metadata_document_supported: meta.client_id_metadata_document_supported,
    })
}

pub fn registration_body() -> Value {
    json!({
        "client_name": CLIENT_NAME,
        "redirect_uris": [LOOPBACK_REDIRECT],
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "token_endpoint_auth_method": "none",
        "software_id": "etus-cli",
        "software_version": env!("CARGO_PKG_VERSION"),
    })
}

pub fn register_client(http: &Client, discovery: &Discovery) -> Result<String> {
    let endpoint = discovery
        .registration_endpoint
        .as_deref()
        .ok_or_else(|| Error::Other("O hub não oferece registro dinâmico de clientes.".into()))?;
    let response = http.post(endpoint).json(&registration_body()).send()?;
    let status = response.status().as_u16();
    let body = response.text().unwrap_or_default();
    if status != 200 && status != 201 {
        return Err(oauth_error(status, &body));
    }
    let value: Value = serde_json::from_str(&body)?;
    value
        .get("client_id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| Error::Other("O registro do cliente não devolveu client_id.".into()))
}

pub fn random_token(bytes: usize) -> Result<String> {
    let mut buffer = vec![0u8; bytes];
    getrandom::fill(&mut buffer).map_err(|e| Error::Other(format!("Sem fonte de aleatoriedade: {e}")))?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buffer))
}

pub fn pkce_challenge(verifier: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

pub struct AuthorizationRequest<'a> {
    pub client_id: &'a str,
    pub redirect_uri: &'a str,
    pub challenge: &'a str,
    pub state: &'a str,
    pub scope: &'a str,
}

pub fn authorization_url(discovery: &Discovery, request: &AuthorizationRequest) -> Result<String> {
    let mut url = Url::parse(&discovery.authorization_endpoint)
        .map_err(|_| Error::Other("Endereço de autorização inválido.".into()))?;
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", request.client_id)
        .append_pair("redirect_uri", request.redirect_uri)
        .append_pair("code_challenge", request.challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", request.state)
        .append_pair("scope", request.scope)
        .append_pair("resource", &discovery.resource);
    Ok(url.to_string())
}

#[derive(Debug, Clone, PartialEq)]
pub enum Callback {
    Code {
        code: String,
        state: Option<String>,
        iss: Option<String>,
    },
    Denied {
        error: String,
        description: Option<String>,
        state: Option<String>,
    },
}

pub fn parse_callback(input: &str) -> Option<Callback> {
    let input = input.trim();
    let query = match input.split_once('?') {
        Some((_, q)) => q,
        None => input,
    };
    let query = query.split('#').next().unwrap_or("");
    let pairs: Vec<(String, String)> = url_pairs(query);
    let get = |key: &str| pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone());
    if let Some(code) = get("code") {
        return Some(Callback::Code {
            code,
            state: get("state"),
            iss: get("iss"),
        });
    }
    get("error").map(|error| Callback::Denied {
        error,
        description: get("error_description"),
        state: get("state"),
    })
}

fn url_pairs(query: &str) -> Vec<(String, String)> {
    Url::parse(&format!("http://x/?{query}"))
        .map(|u| u.query_pairs().map(|(k, v)| (k.into_owned(), v.into_owned())).collect())
        .unwrap_or_default()
}

pub fn accept_callback(callback: Callback, expected_state: &str, issuer: &str) -> Result<String> {
    match callback {
        Callback::Denied {
            error,
            description,
            state,
        } => {
            if state.as_deref() != Some(expected_state) {
                return Err(Error::Auth("Resposta de autorização com state inválido.".into()));
            }
            if error == "access_denied" {
                return Err(Error::Permission("Acesso recusado no navegador.".into()));
            }
            Err(Error::Auth(format!(
                "O hub recusou a autorização ({error}{}).",
                description.map(|d| format!(": {d}")).unwrap_or_default()
            )))
        }
        Callback::Code { code, state, iss } => {
            if state.as_deref() != Some(expected_state) {
                return Err(Error::Auth("Resposta de autorização com state inválido.".into()));
            }
            if let Some(iss) = iss
                && iss.trim_end_matches('/') != issuer.trim_end_matches('/')
            {
                return Err(Error::Auth("A resposta veio de outro emissor (iss).".into()));
            }
            Ok(code)
        }
    }
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
    #[serde(default)]
    scope: Option<String>,
}

pub fn oauth_error(status: u16, body: &str) -> Error {
    let parsed: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let code = parsed.get("error").and_then(Value::as_str).unwrap_or("");
    let description = parsed.get("error_description").and_then(Value::as_str);
    match (status, code) {
        (_, "invalid_grant") => Error::Auth("A sessão expirou ou foi revogada. Rode `etus login`.".into()),
        (_, "invalid_client") => {
            Error::Auth("O hub não reconhece mais o cliente desta CLI. Rode `etus login --novo-cliente`.".into())
        }
        (429, _) | (_, "too_many_requests") => {
            Error::Network("Muitas tentativas seguidas no hub. Espere um minuto e tente de novo.".into())
        }
        (_, "") => Error::Other(format!("O hub respondeu HTTP {status}.")),
        (_, code) => Error::Other(format!(
            "O hub recusou o pedido ({code}{}).",
            description.map(|d| format!(": {d}")).unwrap_or_default()
        )),
    }
}

fn post_token(http: &Client, endpoint: &str, form: &[(&str, &str)]) -> Result<TokenResponse> {
    let response = http
        .post(endpoint)
        .header("accept", "application/json")
        .form(form)
        .send()?;
    let status = response.status().as_u16();
    let body = response.text().unwrap_or_default();
    if status != 200 {
        return Err(oauth_error(status, &body));
    }
    serde_json::from_str(&body).map_err(|_| Error::Other("Resposta de token inválida.".into()))
}

fn tokens_from(response: TokenResponse, client_id: &str, discovery_like: (&str, Option<&str>, &str)) -> Tokens {
    let (token_endpoint, revocation_endpoint, resource) = discovery_like;
    Tokens {
        access_token: response.access_token,
        refresh_token: response.refresh_token,
        expires_at: now_secs() + response.expires_in.unwrap_or(3600),
        scope: response.scope,
        client_id: client_id.to_string(),
        token_endpoint: token_endpoint.to_string(),
        revocation_endpoint: revocation_endpoint.map(str::to_string),
        resource: resource.to_string(),
    }
}

pub fn exchange_code(
    http: &Client,
    discovery: &Discovery,
    client_id: &str,
    code: &str,
    redirect_uri: &str,
    verifier: &str,
) -> Result<Tokens> {
    let response = post_token(
        http,
        &discovery.token_endpoint,
        &[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("client_id", client_id),
            ("code_verifier", verifier),
            ("resource", &discovery.resource),
        ],
    )?;
    Ok(tokens_from(
        response,
        client_id,
        (
            &discovery.token_endpoint,
            discovery.revocation_endpoint.as_deref(),
            &discovery.resource,
        ),
    ))
}

pub struct HttpRefresher {
    pub http: Client,
}

impl Refresher for HttpRefresher {
    fn refresh(&self, tokens: &Tokens) -> Result<Tokens> {
        let refresh_token = tokens
            .refresh_token
            .as_deref()
            .ok_or_else(|| Error::Auth("A sessão expirou. Rode `etus login`.".into()))?;
        let response = post_token(
            &self.http,
            &tokens.token_endpoint,
            &[
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token),
                ("client_id", &tokens.client_id),
                ("resource", &tokens.resource),
            ],
        )?;
        let mut next = tokens_from(
            response,
            &tokens.client_id,
            (
                &tokens.token_endpoint,
                tokens.revocation_endpoint.as_deref(),
                &tokens.resource,
            ),
        );
        if next.scope.is_none() {
            next.scope = tokens.scope.clone();
        }
        Ok(next)
    }
}

pub fn revoke(http: &Client, tokens: &Tokens) -> Result<()> {
    let (Some(endpoint), Some(refresh)) = (&tokens.revocation_endpoint, &tokens.refresh_token) else {
        return Ok(());
    };
    let response = http
        .post(endpoint)
        .form(&[
            ("token", refresh.as_str()),
            ("token_type_hint", "refresh_token"),
            ("client_id", &tokens.client_id),
        ])
        .send()?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(Error::Other(format!(
            "O hub não confirmou a revogação (HTTP {}).",
            response.status().as_u16()
        )))
    }
}

pub struct Loopback {
    listener: TcpListener,
    pub redirect_uri: String,
}

pub fn bind_loopback() -> Result<Loopback> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    Ok(Loopback {
        listener,
        redirect_uri: format!("http://127.0.0.1:{port}{CALLBACK_PATH}"),
    })
}

const DONE_PAGE: &str = "<!doctype html><html lang=\"pt-BR\"><meta charset=\"utf-8\"><title>Etus CLI</title><body style=\"font-family:system-ui,sans-serif;max-width:32rem;margin:4rem auto;padding:0 1rem\"><h1>Pronto</h1><p>Pode fechar esta aba e voltar ao terminal.</p></body></html>";

fn read_request_target(stream: &mut TcpStream) -> Option<String> {
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    let mut buffer = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    while !buffer.windows(4).any(|w| w == b"\r\n\r\n") && buffer.len() < 16 * 1024 {
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..n]);
    }
    let text = String::from_utf8_lossy(&buffer);
    let line = text.lines().next()?;
    let mut parts = line.split_whitespace();
    (parts.next()? == "GET").then(|| parts.next().map(str::to_string))?
}

fn respond(stream: &mut TcpStream, status: &str, body: &str) {
    let response = format!(
        "HTTP/1.1 {status}\r\ncontent-type: text/html; charset=utf-8\r\ncontent-length: {}\r\ncache-control: no-store\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

impl Loopback {
    pub fn wait(self, timeout: Duration, read_pasted: bool) -> Result<Callback> {
        let (sender, receiver) = mpsc::channel::<Callback>();
        let listener = self.listener;
        let from_browser = sender.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let Some(target) = read_request_target(&mut stream) else {
                    continue;
                };
                if !target.starts_with(CALLBACK_PATH) {
                    respond(&mut stream, "404 Not Found", "");
                    continue;
                }
                match parse_callback(&target) {
                    Some(callback) => {
                        respond(&mut stream, "200 OK", DONE_PAGE);
                        if from_browser.send(callback).is_err() {
                            return;
                        }
                    }
                    None => respond(&mut stream, "400 Bad Request", ""),
                }
            }
        });
        if read_pasted {
            std::thread::spawn(move || {
                let stdin = std::io::stdin();
                for line in stdin.lock().lines() {
                    let Ok(line) = line else { return };
                    if let Some(callback) = parse_callback(&line)
                        && sender.send(callback).is_ok()
                    {
                        return;
                    }
                }
            });
        }
        receiver.recv_timeout(timeout).map_err(|_| {
            Error::Auth(
                "O login não terminou a tempo. Rode `etus login` de novo (se o navegador mostrou erro de cliente, use `etus login --novo-cliente`)."
                    .into(),
            )
        })
    }
}

pub fn open_browser(url: &str) -> bool {
    let mut command = if cfg!(target_os = "macos") {
        let mut c = std::process::Command::new("open");
        c.arg(url);
        c
    } else if cfg!(target_os = "windows") {
        let mut c = std::process::Command::new("rundll32");
        c.args(["url.dll,FileProtocolHandler", url]);
        c
    } else {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(url);
        c
    };
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn discovery() -> Discovery {
        Discovery {
            resource: "https://apps.etus.io/mcp".into(),
            issuer: "https://apps.etus.io".into(),
            authorization_endpoint: "https://apps.etus.io/oauth/authorize".into(),
            token_endpoint: "https://apps.etus.io/oauth/token".into(),
            registration_endpoint: Some("https://apps.etus.io/oauth/register".into()),
            revocation_endpoint: None,
            client_id_metadata_document_supported: true,
        }
    }

    #[test]
    fn pkce_matches_rfc_7636_example() {
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        let verifier = random_token(32).unwrap();
        assert_eq!(verifier.len(), 43);
        assert_eq!(pkce_challenge(&verifier).len(), 43);
    }

    #[test]
    fn authorization_url_carries_pkce_and_resource() {
        let url = authorization_url(
            &discovery(),
            &AuthorizationRequest {
                client_id: "mcp_abc",
                redirect_uri: "http://127.0.0.1:5000/callback",
                challenge: "ch",
                state: "st",
                scope: "mcp",
            },
        )
        .unwrap();
        assert!(url.starts_with("https://apps.etus.io/oauth/authorize?response_type=code&client_id=mcp_abc"));
        assert!(url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A5000%2Fcallback"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("resource=https%3A%2F%2Fapps.etus.io%2Fmcp"));
    }

    #[test]
    fn callbacks_are_checked() {
        let ok = parse_callback("/callback?code=abc&state=s1&iss=https%3A%2F%2Fapps.etus.io").unwrap();
        assert_eq!(
            accept_callback(ok.clone(), "s1", "https://apps.etus.io").unwrap(),
            "abc"
        );
        assert!(accept_callback(ok.clone(), "other", "https://apps.etus.io").is_err());
        assert!(accept_callback(ok, "s1", "https://evil.example").is_err());
        let pasted = parse_callback("http://127.0.0.1:1234/callback?code=x&state=s").unwrap();
        assert!(matches!(pasted, Callback::Code { ref code, .. } if code == "x"));
        let denied = parse_callback("/callback?error=access_denied&state=s").unwrap();
        assert_eq!(accept_callback(denied, "s", "x").unwrap_err().exit_code(), 4);
        assert!(parse_callback("/callback").is_none());
    }

    #[test]
    fn oauth_errors_map_to_exit_codes() {
        assert_eq!(oauth_error(400, r#"{"error":"invalid_grant"}"#).exit_code(), 3);
        assert_eq!(oauth_error(401, r#"{"error":"invalid_client"}"#).exit_code(), 3);
        assert_eq!(oauth_error(429, "{}").exit_code(), 1);
        assert!(
            oauth_error(400, r#"{"error":"invalid_target","error_description":"x"}"#)
                .message()
                .contains("invalid_target")
        );
    }

    #[test]
    fn registration_uses_portless_loopback_redirect() {
        let body = registration_body();
        assert_eq!(body["redirect_uris"][0], "http://127.0.0.1/callback");
        assert_eq!(body["token_endpoint_auth_method"], "none");
    }

    #[test]
    fn loopback_receives_the_browser_redirect() {
        let loopback = bind_loopback().unwrap();
        let uri = loopback.redirect_uri.clone();
        let handle = std::thread::spawn(move || loopback.wait(Duration::from_secs(5), false));
        let addr = uri.trim_start_matches("http://").split('/').next().unwrap().to_string();
        let mut stream = TcpStream::connect(addr).unwrap();
        stream
            .write_all(b"GET /callback?code=c1&state=s1 HTTP/1.1\r\nHost: x\r\n\r\n")
            .unwrap();
        let mut reply = String::new();
        stream.read_to_string(&mut reply).unwrap();
        assert!(reply.starts_with("HTTP/1.1 200"));
        let callback = handle.join().unwrap().unwrap();
        assert_eq!(
            callback,
            Callback::Code {
                code: "c1".into(),
                state: Some("s1".into()),
                iss: None
            }
        );
    }
}
