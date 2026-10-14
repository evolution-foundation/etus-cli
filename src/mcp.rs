use std::sync::atomic::{AtomicU64, Ordering};

use reqwest::blocking::Client;
use serde_json::{Value, json};

use crate::catalog::Tool;
use crate::error::{Error, Result};

pub const PROTOCOL_VERSION: &str = "2025-06-18";
const UNKNOWN_TOOL_HINT: &str = "A ferramenta não está disponível para você (sem permissão, desligada pelo admin ou conexão só de leitura). Rode `etus atualizar`.";

pub trait TokenSource {
    fn access_token(&self, rejected: Option<&str>) -> Result<String>;
}

pub struct McpClient<'a> {
    pub http: &'a Client,
    pub url: String,
    pub tokens: &'a dyn TokenSource,
    next_id: AtomicU64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolOutcome {
    pub result: Value,
    pub is_error: bool,
}

pub fn classify_status(status: u16, body: &str) -> Option<Error> {
    if (200..300).contains(&status) {
        return None;
    }
    let code = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v.get("error").and_then(Value::as_str).map(str::to_string))
        .unwrap_or_default();
    Some(match status {
        401 => Error::Auth("O hub não aceitou a sessão. Rode `etus login`.".into()),
        403 if code == "agents_not_allowed" => Error::Permission(
            "Sua conta não tem a permissão \"Conectar agentes externos\" na empresa ativa. Peça a um admin no hub."
                .into(),
        ),
        403 => Error::Permission(format!(
            "O hub recusou o acesso ({}).",
            if code.is_empty() { "403" } else { &code }
        )),
        429 => Error::Network("Limite de chamadas atingido (120 por minuto). Espere um pouco e tente de novo.".into()),
        _ => Error::Network(format!("O hub respondeu HTTP {status}.")),
    })
}

pub fn rpc_error(error: &Value) -> Error {
    let code = error.get("code").and_then(Value::as_i64).unwrap_or(0);
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("erro sem mensagem");
    if code == -32602 && message.contains("Unknown tool") {
        return Error::Permission(UNKNOWN_TOOL_HINT.into());
    }
    Error::Tool(format!("{message} (código {code})"))
}

pub fn refusal(result: &Value) -> Option<Error> {
    let text = first_text(result)?;
    let permission = text.contains("only for platform admins") || text.contains("connection is read-only");
    permission.then(|| Error::Permission(text.to_string()))
}

pub fn first_text(result: &Value) -> Option<&str> {
    result
        .get("content")?
        .as_array()?
        .iter()
        .find_map(|c| (c.get("type")?.as_str()? == "text").then(|| c.get("text")?.as_str())?)
}

pub fn parse_body(content_type: &str, body: &str, id: u64) -> Result<Value> {
    let invalid = || Error::Network("O hub respondeu algo que não é JSON-RPC.".into());
    if !content_type.contains("text/event-stream") {
        return serde_json::from_str(body).map_err(|_| invalid());
    }
    for event in body.split("\n\n").flat_map(|e| e.split("\r\n\r\n")) {
        let data: Vec<&str> = event
            .lines()
            .filter_map(|line| line.strip_prefix("data:"))
            .map(str::trim_start)
            .collect();
        if data.is_empty() {
            continue;
        }
        let Ok(message) = serde_json::from_str::<Value>(&data.join("\n")) else {
            continue;
        };
        if message.get("id").and_then(Value::as_u64) == Some(id) {
            return Ok(message);
        }
    }
    Err(invalid())
}

impl<'a> McpClient<'a> {
    pub fn new(http: &'a Client, url: String, tokens: &'a dyn TokenSource) -> Self {
        McpClient {
            http,
            url,
            tokens,
            next_id: AtomicU64::new(1),
        }
    }

    fn send(&self, token: &str, body: &Value) -> Result<(u16, String, String)> {
        let response = self
            .http
            .post(&self.url)
            .bearer_auth(token)
            .header("accept", "application/json, text/event-stream")
            .header("mcp-protocol-version", PROTOCOL_VERSION)
            .json(body)
            .send()?;
        let status = response.status().as_u16();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        Ok((status, content_type, response.text()?))
    }

    pub fn request(&self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let body = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let mut token = self.tokens.access_token(None)?;
        let mut reply = self.send(&token, &body)?;
        if reply.0 == 401 {
            token = self.tokens.access_token(Some(&token))?;
            reply = self.send(&token, &body)?;
        }
        let (status, content_type, text) = reply;
        if let Some(error) = classify_status(status, &text) {
            return Err(error);
        }
        let message = parse_body(&content_type, &text, id)?;
        if let Some(error) = message.get("error") {
            return Err(rpc_error(error));
        }
        message
            .get("result")
            .cloned()
            .ok_or_else(|| Error::Network("Resposta JSON-RPC sem resultado.".into()))
    }

    pub fn list_tools(&self) -> Result<Vec<Tool>> {
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..50 {
            let params = match &cursor {
                Some(c) => json!({"cursor": c}),
                None => json!({}),
            };
            let result = self.request("tools/list", params)?;
            let page: Vec<Tool> = serde_json::from_value(result.get("tools").cloned().unwrap_or(json!([])))?;
            tools.extend(page);
            cursor = result.get("nextCursor").and_then(Value::as_str).map(str::to_string);
            if cursor.is_none() {
                break;
            }
        }
        Ok(tools)
    }

    pub fn call_tool(&self, name: &str, arguments: &Value) -> Result<ToolOutcome> {
        let result = self.request("tools/call", json!({"name": name, "arguments": arguments}))?;
        let is_error = result.get("isError").and_then(Value::as_bool).unwrap_or(false);
        Ok(ToolOutcome { result, is_error })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_status_maps_to_exit_codes() {
        assert!(classify_status(200, "").is_none());
        assert_eq!(classify_status(401, "{}").unwrap().exit_code(), 3);
        let denied = classify_status(403, r#"{"error":"agents_not_allowed"}"#).unwrap();
        assert_eq!(denied.exit_code(), 4);
        assert!(denied.message().contains("Conectar agentes"));
        assert_eq!(classify_status(429, "").unwrap().exit_code(), 1);
        assert_eq!(classify_status(502, "").unwrap().exit_code(), 1);
    }

    #[test]
    fn rpc_errors_map_to_exit_codes() {
        assert_eq!(
            rpc_error(&json!({"code": -32602, "message": "Unknown tool: x__y"})).exit_code(),
            4
        );
        assert_eq!(rpc_error(&json!({"code": -32603, "message": "boom"})).exit_code(), 1);
    }

    #[test]
    fn hub_refusals_are_permission_errors() {
        let result = json!({"content": [{"type": "text", "text": "hub__app_create is only for platform admins of ETUS Platforms."}], "isError": true});
        assert_eq!(refusal(&result).unwrap().exit_code(), 4);
        assert!(refusal(&json!({"content": [{"type": "text", "text": "Task not found"}]})).is_none());
    }

    #[test]
    fn parses_json_and_sse_bodies() {
        let json_body = r#"{"jsonrpc":"2.0","id":3,"result":{}}"#;
        assert_eq!(parse_body("application/json", json_body, 3).unwrap()["id"], 3);
        let sse = "event: message\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\"}\n\nevent: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":7,\"result\":{\"ok\":true}}\n\n";
        assert_eq!(parse_body("text/event-stream", sse, 7).unwrap()["result"]["ok"], true);
        assert!(parse_body("text/event-stream", sse, 8).is_err());
    }
}
