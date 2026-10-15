use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::Result;
use crate::mcp::ToolOutcome;

pub const STATUS: &str = "action_required";
pub const DEFAULT_WAIT_SECS: u64 = 300;
pub const MAX_WAIT_SECS: u64 = 1800;
const FIRST_POLL: Duration = Duration::from_secs(2);
const POLL_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Action {
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub message: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub missing: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
}

fn from_payload(value: &Value) -> Option<Action> {
    if value.get("status").and_then(Value::as_str) != Some(STATUS) {
        return None;
    }
    let action: Action = serde_json::from_value(value.get("action")?.clone()).ok()?;
    safe_url(&action.url).then_some(action)
}

pub fn safe_url(url: &str) -> bool {
    let Ok(parsed) = reqwest::Url::parse(url) else {
        return false;
    };
    match parsed.scheme() {
        "https" => parsed.host_str().is_some(),
        "http" => matches!(parsed.host_str(), Some("localhost" | "127.0.0.1" | "[::1]")),
        _ => false,
    }
}

pub fn detect(result: &Value) -> Option<Action> {
    if let Some(action) = result.get("structuredContent").and_then(from_payload) {
        return Some(action);
    }
    result
        .get("content")?
        .as_array()?
        .iter()
        .filter(|c| c.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|c| c.get("text").and_then(Value::as_str))
        .filter_map(|text| serde_json::from_str::<Value>(text).ok())
        .find_map(|value| from_payload(&value))
}

pub fn notice(action: &Action) -> String {
    let title = if action.title.trim().is_empty() {
        "Ação necessária"
    } else {
        action.title.trim()
    };
    let mut text = format!("Ação necessária: {title}");
    if !action.message.trim().is_empty() {
        text.push('\n');
        text.push_str(action.message.trim());
    }
    if !action.missing.is_empty() {
        text.push_str(&format!("\nFalta: {}", action.missing.join(", ")));
    }
    text.push_str(&format!("\nAbra: {}", action.url));
    if let Some(expires) = &action.expires_at {
        text.push_str(&format!("\nO link vale até {expires}."));
    }
    text
}

pub fn wait_limit(requested: u64) -> Duration {
    Duration::from_secs(requested.clamp(1, MAX_WAIT_SECS))
}

pub fn poll(
    limit: Duration,
    mut call: impl FnMut() -> Result<ToolOutcome>,
    mut sleep: impl FnMut(Duration),
    mut elapsed: impl FnMut() -> Duration,
) -> Result<ToolOutcome> {
    let mut delay = FIRST_POLL;
    loop {
        sleep(delay.min(limit.saturating_sub(elapsed())));
        let outcome = call()?;
        if detect(&outcome.result).is_none() || elapsed() >= limit {
            return Ok(outcome);
        }
        delay = POLL_INTERVAL;
    }
}

pub fn poll_for_real(limit: Duration, call: impl FnMut() -> Result<ToolOutcome>) -> Result<ToolOutcome> {
    let start = Instant::now();
    poll(limit, call, std::thread::sleep, || start.elapsed())
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use serde_json::json;

    use super::*;

    fn required(url: &str) -> Value {
        json!({"status": "action_required", "action": {"kind": "connect", "title": "Conectar o Google", "message": "Conecte sua conta Google para ler a agenda.", "url": url, "missing": ["google"]}})
    }

    #[test]
    fn detects_structured_content() {
        let result = json!({"content": [{"type": "text", "text": "Abra https://apps.etus.io/x"}], "structuredContent": required("https://apps.etus.io/x")});
        let action = detect(&result).unwrap();
        assert_eq!(action.kind, "connect");
        assert_eq!(action.url, "https://apps.etus.io/x");
        assert_eq!(action.missing, vec!["google".to_string()]);
    }

    #[test]
    fn detects_json_inside_text_content() {
        let result = json!({"content": [{"type": "text", "text": "aviso"}, {"type": "text", "text": required("https://apps.etus.io/y").to_string()}]});
        assert_eq!(detect(&result).unwrap().url, "https://apps.etus.io/y");
    }

    #[test]
    fn ignores_other_results_and_unsafe_urls() {
        assert!(detect(&json!({"structuredContent": {"status": "ok"}})).is_none());
        assert!(detect(&json!({"content": [{"type": "text", "text": "ok"}]})).is_none());
        assert!(detect(&json!({"structuredContent": required("javascript:alert(1)")})).is_none());
        assert!(detect(&json!({"structuredContent": required("http://apps.etus.io/x")})).is_none());
        assert!(detect(&json!({"structuredContent": required("http://localhost:5283/x")})).is_some());
        assert!(detect(&json!({"structuredContent": {"status": "action_required"}})).is_none());
    }

    #[test]
    fn notice_shows_title_message_and_url() {
        let action = detect(&json!({"structuredContent": required("https://apps.etus.io/x")})).unwrap();
        let text = notice(&action);
        assert!(text.starts_with("Ação necessária: Conectar o Google\n"));
        assert!(text.contains("Conecte sua conta Google"));
        assert!(text.contains("Falta: google"));
        assert!(text.ends_with("Abra: https://apps.etus.io/x"));
        assert!(!text.contains('\u{2014}') && !text.contains('\u{2013}'));
    }

    fn outcome(result: Value) -> ToolOutcome {
        ToolOutcome {
            result,
            is_error: false,
        }
    }

    #[test]
    fn poll_stops_when_the_action_is_done() {
        let clock = Cell::new(Duration::ZERO);
        let calls = Cell::new(0);
        let done = poll(
            Duration::from_secs(60),
            || {
                calls.set(calls.get() + 1);
                Ok(outcome(if calls.get() < 3 {
                    json!({"structuredContent": required("https://apps.etus.io/x")})
                } else {
                    json!({"structuredContent": {"ok": true}})
                }))
            },
            |d| clock.set(clock.get() + d),
            || clock.get(),
        )
        .unwrap();
        assert_eq!(done.result["structuredContent"]["ok"], true);
        assert_eq!(calls.get(), 3);
        assert_eq!(clock.get(), Duration::from_secs(12));
    }

    #[test]
    fn poll_is_bounded() {
        let clock = Cell::new(Duration::ZERO);
        let calls = Cell::new(0);
        let last = poll(
            Duration::from_secs(11),
            || {
                calls.set(calls.get() + 1);
                Ok(outcome(
                    json!({"structuredContent": required("https://apps.etus.io/x")}),
                ))
            },
            |d| clock.set(clock.get() + d),
            || clock.get(),
        )
        .unwrap();
        assert!(detect(&last.result).is_some());
        assert_eq!(clock.get(), Duration::from_secs(11));
        assert_eq!(calls.get(), 3);
    }

    #[test]
    fn wait_limit_is_clamped() {
        assert_eq!(wait_limit(0), Duration::from_secs(1));
        assert_eq!(wait_limit(99_999), Duration::from_secs(MAX_WAIT_SECS));
    }
}
