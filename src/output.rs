use std::io::Write;

use serde_json::{Value, json};

use crate::error::{Error, Result};
use crate::mcp::{ToolOutcome, refusal};

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Output {
    pub json: bool,
    pub quiet: bool,
}

pub fn payload(result: &Value) -> Value {
    if let Some(structured) = result.get("structuredContent").filter(|v| !v.is_null()) {
        return structured.clone();
    }
    let content = result.get("content").and_then(Value::as_array);
    if let Some([single]) = content.map(Vec::as_slice)
        && single.get("type").and_then(Value::as_str) == Some("text")
    {
        let text = single.get("text").and_then(Value::as_str).unwrap_or("");
        return serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.to_string()));
    }
    result.clone()
}

fn describe_content(item: &Value) -> String {
    match item.get("type").and_then(Value::as_str).unwrap_or("") {
        "text" => {
            let text = item.get("text").and_then(Value::as_str).unwrap_or("");
            match serde_json::from_str::<Value>(text) {
                Ok(value) if value.is_object() || value.is_array() => pretty(&value),
                _ => text.to_string(),
            }
        }
        "image" | "audio" => format!(
            "[{} {}]",
            item.get("type").and_then(Value::as_str).unwrap_or(""),
            item.get("mimeType").and_then(Value::as_str).unwrap_or("")
        ),
        "resource_link" | "resource" => {
            let uri = item
                .get("uri")
                .or_else(|| item.get("resource").and_then(|r| r.get("uri")))
                .and_then(Value::as_str)
                .unwrap_or("");
            format!("[recurso {uri}]")
        }
        _ => pretty(item),
    }
}

pub fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_default()
}

pub fn human_text(result: &Value) -> String {
    if let Some(structured) = result.get("structuredContent").filter(|v| !v.is_null()) {
        return pretty(structured);
    }
    match result.get("content").and_then(Value::as_array) {
        Some(items) => items.iter().map(describe_content).collect::<Vec<_>>().join("\n"),
        None => pretty(result),
    }
}

fn error_text(result: &Value) -> String {
    let text = result
        .get("content")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|c| c.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    if text.trim().is_empty() {
        "A ferramenta falhou sem mensagem.".into()
    } else {
        text
    }
}

impl Output {
    pub fn print_value(&self, value: &Value) {
        if self.quiet {
            return;
        }
        let text = if self.json { value.to_string() } else { pretty(value) };
        println_safe(&text);
    }

    pub fn print_human(&self, text: &str, value: impl FnOnce() -> Value) {
        if self.quiet {
            return;
        }
        if self.json {
            println_safe(&value().to_string());
        } else {
            println_safe(text);
        }
    }

    pub fn tool_result(&self, outcome: &ToolOutcome) -> Result<()> {
        if outcome.is_error {
            if let Some(error) = refusal(&outcome.result) {
                return Err(error);
            }
            return Err(Error::Tool(error_text(&outcome.result)));
        }
        if self.quiet {
            return Ok(());
        }
        if self.json {
            println_safe(&payload(&outcome.result).to_string());
        } else {
            println_safe(&human_text(&outcome.result));
        }
        Ok(())
    }

    pub fn error(&self, error: &Error) {
        if matches!(error, Error::Exit(_)) {
            return;
        }
        if self.json {
            let body =
                json!({"error": {"kind": error.kind(), "message": error.message(), "exitCode": error.exit_code()}});
            eprintln!("{body}");
        } else {
            eprintln!("etus: {}", error.message());
        }
    }
}

pub fn println_safe(text: &str) {
    let mut stdout = std::io::stdout().lock();
    let _ = writeln!(stdout, "{text}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_prefers_structured_content() {
        let result = json!({"content": [{"type": "text", "text": "{\"a\":1}"}], "structuredContent": {"a": 1}});
        assert_eq!(payload(&result), json!({"a": 1}));
        let text_json = json!({"content": [{"type": "text", "text": "[1,2]"}]});
        assert_eq!(payload(&text_json), json!([1, 2]));
        let plain = json!({"content": [{"type": "text", "text": "ok"}]});
        assert_eq!(payload(&plain), json!("ok"));
        let many = json!({"content": [{"type": "text", "text": "a"}, {"type": "text", "text": "b"}]});
        assert_eq!(payload(&many), many);
    }

    #[test]
    fn human_text_pretty_prints_json() {
        let result = json!({"content": [{"type": "text", "text": "{\"a\":1}"}, {"type": "image", "mimeType": "image/png", "data": "x"}]});
        assert_eq!(human_text(&result), "{\n  \"a\": 1\n}\n[image image/png]");
    }

    #[test]
    fn tool_errors_become_exit_codes() {
        let out = Output {
            json: true,
            quiet: false,
        };
        let failed = ToolOutcome {
            result: json!({"content": [{"type": "text", "text": "Task not found"}], "isError": true}),
            is_error: true,
        };
        assert_eq!(
            out.tool_result(&failed).unwrap_err(),
            Error::Tool("Task not found".into())
        );
        let refused = ToolOutcome {
            result: json!({"content": [{"type": "text", "text": "hub__set_company changes data and this connection is read-only (scope mcp:read)."}], "isError": true}),
            is_error: true,
        };
        assert_eq!(out.tool_result(&refused).unwrap_err().exit_code(), 4);
    }
}
