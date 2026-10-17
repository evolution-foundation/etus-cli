use std::time::Duration;

use serde_json::Value;

use super::state::State;
use crate::error::{Error, Result};

pub const SUBPROTOCOL: &str = "etus-home-egress.v1";
pub const PATH: &str = "/etus/home-egress";
pub const URL_ENV: &str = "ETUS_HOME_EGRESS_URL";

pub const OPEN: u8 = 0x01;
pub const OPENED: u8 = 0x02;
pub const REFUSED: u8 = 0x03;
pub const DATA: u8 = 0x04;
pub const END: u8 = 0x05;
pub const RESET: u8 = 0x06;

pub const HEADER_LEN: usize = 5;
pub const MAX_DATA: usize = 65536;
pub const MAX_STREAMS: usize = 64;

pub const ALLOWED_HOSTS: [&str; 4] = [
    "api.anthropic.com",
    "console.anthropic.com",
    "platform.claude.com",
    "claude.ai",
];
pub const ALLOWED_PORT: u16 = 443;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame<'a> {
    pub kind: u8,
    pub stream: u32,
    pub payload: &'a [u8],
}

pub fn parse(bytes: &[u8]) -> Option<Frame<'_>> {
    let (head, payload) = bytes.split_first_chunk::<HEADER_LEN>()?;
    Some(Frame {
        kind: head[0],
        stream: u32::from_be_bytes([head[1], head[2], head[3], head[4]]),
        payload,
    })
}

pub fn encode(kind: u8, stream: u32, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_LEN + payload.len());
    out.push(kind);
    out.extend_from_slice(&stream.to_be_bytes());
    out.extend_from_slice(payload);
    out
}

/// The closed list of destinations. Exact, case-sensitive match on purpose: anything else is refused before any DNS lookup.
#[derive(Debug, Clone, Default)]
pub struct Targets {
    #[cfg(any(test, debug_assertions))]
    pub extra: Option<String>,
}

#[cfg(any(test, debug_assertions))]
pub const TEST_TARGET_ENV: &str = "ETUS_HOME_EGRESS_TEST_TARGET";

#[cfg(any(test, debug_assertions))]
fn test_target_from_env() -> Option<String> {
    if !cfg!(debug_assertions) {
        return None;
    }
    std::env::var(TEST_TARGET_ENV).ok().filter(|v| !v.trim().is_empty())
}

impl Targets {
    pub fn from_env() -> Targets {
        Targets {
            #[cfg(any(test, debug_assertions))]
            extra: test_target_from_env(),
        }
    }

    pub fn resolve(&self, target: &str) -> Option<(String, u16)> {
        #[cfg(any(test, debug_assertions))]
        if let Some(extra) = self.extra.as_deref()
            && extra == target
            && let Some((host, port)) = target.rsplit_once(':')
            && let Ok(port) = port.parse::<u16>()
        {
            return Some((host.to_string(), port));
        }
        let (host, port) = target.rsplit_once(':')?;
        if port != "443" {
            return None;
        }
        ALLOWED_HOSTS
            .iter()
            .find(|allowed| **allowed == host)
            .map(|allowed| (allowed.to_string(), ALLOWED_PORT))
    }
}

pub fn tunnel_url(router_url: &str) -> Result<String> {
    let base = router_url.trim().trim_end_matches('/');
    let rest = if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        return Err(Error::Usage(format!("Endereço do AI Router inválido: {router_url}")));
    };
    Ok(format!("{rest}{PATH}"))
}

pub fn endpoint(router_url: &str, override_url: Option<String>) -> Result<String> {
    match override_url.map(|u| u.trim().to_string()).filter(|u| !u.is_empty()) {
        Some(url) if url.starts_with("ws://") || url.starts_with("wss://") => Ok(url),
        Some(url) => Err(Error::Usage(format!(
            "{URL_ENV} precisa começar com ws:// ou wss:// (recebi {url})."
        ))),
        None => tunnel_url(router_url),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Wait {
    Fixed(Duration),
    Backoff,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Retry {
    pub state: State,
    pub wait: Wait,
    pub reason: String,
}

impl Retry {
    pub fn backoff(reason: impl Into<String>) -> Retry {
        Retry {
            state: State::Offline,
            wait: Wait::Backoff,
            reason: reason.into(),
        }
    }

    fn fixed(state: State, minutes: u64, reason: &str) -> Retry {
        Retry {
            state,
            wait: Wait::Fixed(Duration::from_secs(minutes * 60)),
            reason: reason.into(),
        }
    }
}

pub const REFUSED_KEY: &str = "chave do computador recusada (AUTH_002): rode `etus router setup`";

fn error_code(body: &[u8]) -> String {
    serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|v| {
            let error = v.get("error")?;
            error
                .get("code")
                .and_then(Value::as_str)
                .or_else(|| error.as_str())
                .map(str::to_string)
        })
        .unwrap_or_default()
}

pub fn classify_http(status: u16, body: &[u8], retry_after: Option<&str>) -> Retry {
    let code = error_code(body);
    match (status, code.as_str()) {
        (401, _) => Retry::fixed(State::Refused, 10, REFUSED_KEY),
        (403, "no_router_key") => Retry::fixed(
            State::Refused,
            10,
            "sua chave do AI Router está desativada (no_router_key): fale com um admin",
        ),
        (403, "home_egress_off") => Retry::fixed(
            State::WaitingAdmin,
            5,
            "o admin ainda não ligou a saída pela casa para você no painel",
        ),
        (403, other) => Retry::fixed(
            State::Refused,
            10,
            &format!(
                "o roteador recusou a conexão ({})",
                if other.is_empty() { "403" } else { other }
            ),
        ),
        (404, _) => Retry::fixed(
            State::ServerOff,
            30,
            "o AI Router ainda não tem a saída pela casa ligada",
        ),
        (429, _) => {
            let secs = retry_after
                .and_then(|v| v.trim().parse::<u64>().ok())
                .filter(|s| *s > 0)
                .unwrap_or(60)
                .min(3600);
            Retry {
                state: State::Offline,
                wait: Wait::Fixed(Duration::from_secs(secs)),
                reason: "tentativas demais (too_many_attempts)".into(),
            }
        }
        (status, code) => Retry::backoff(format!(
            "o roteador respondeu HTTP {status}{}",
            if code.is_empty() {
                String::new()
            } else {
                format!(" ({code})")
            }
        )),
    }
}

pub fn classify_close(code: Option<u16>) -> Retry {
    match code {
        Some(4001) => Retry::fixed(
            State::WaitingAdmin,
            5,
            "o admin desligou a saída pela casa para você no painel",
        ),
        Some(4002) => Retry::fixed(
            State::Replaced,
            10,
            "outra conexão sua assumiu (outro computador com a saída pela casa ligada)",
        ),
        Some(code) => Retry::backoff(format!("o roteador fechou a conexão (código {code})")),
        None => Retry::backoff("o roteador fechou a conexão"),
    }
}

const BACKOFF_MIN_SECS: u64 = 2;
const BACKOFF_MAX_SECS: u64 = 60;

#[derive(Debug, Default)]
pub struct Backoff {
    attempt: u32,
}

impl Backoff {
    pub fn reset(&mut self) {
        self.attempt = 0;
    }

    pub fn next_delay(&mut self, unit: f64) -> Duration {
        let base = (BACKOFF_MIN_SECS << self.attempt.min(6)).min(BACKOFF_MAX_SECS) as f64;
        self.attempt = self.attempt.saturating_add(1);
        Duration::from_secs_f64(base * (0.5 + 0.5 * unit.clamp(0.0, 1.0)))
    }
}

pub fn jitter_unit() -> f64 {
    let mut bytes = [0u8; 4];
    if getrandom::fill(&mut bytes).is_err() {
        return 1.0;
    }
    f64::from(u32::from_le_bytes(bytes)) / f64::from(u32::MAX)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Control {
    Hello { person_name: Option<String> },
    Egress { ip: String, at: Option<String> },
    Other,
}

pub fn parse_control(text: &str) -> Control {
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return Control::Other;
    };
    let field = |name: &str| value.get(name).and_then(Value::as_str).map(str::to_string);
    match value.get("type").and_then(Value::as_str) {
        Some("hello") => Control::Hello {
            person_name: field("personName"),
        },
        Some("egress") => match field("ip").filter(|ip| !ip.is_empty() && ip.len() <= 64) {
            Some(ip) => Control::Egress { ip, at: field("at") },
            None => Control::Other,
        },
        _ => Control::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip_and_short_ones_are_malformed() {
        let bytes = encode(DATA, 0x0102_0304, b"abc");
        assert_eq!(bytes, vec![4, 1, 2, 3, 4, b'a', b'b', b'c']);
        assert_eq!(
            parse(&bytes),
            Some(Frame {
                kind: DATA,
                stream: 0x0102_0304,
                payload: b"abc"
            })
        );
        let empty = encode(END, 7, &[]);
        assert_eq!(parse(&empty).unwrap().payload, b"");
        assert_eq!(parse(&empty).unwrap().stream, 7);
        assert_eq!(parse(&[1, 0, 0, 0]), None);
        assert_eq!(parse(&[]), None);
    }

    #[test]
    fn only_the_closed_list_on_443_is_allowed() {
        let targets = Targets::default();
        for host in ALLOWED_HOSTS {
            assert_eq!(targets.resolve(&format!("{host}:443")), Some((host.to_string(), 443)));
        }
        for refused in [
            "API.anthropic.com:443",
            "Claude.AI:443",
            "api.anthropic.com.:443",
            "claude.ai.:443",
            "api.anthropic.com:80",
            "api.anthropic.com:0443",
            "api.anthropic.com:8443",
            "api.anthropic.com",
            "api.anthropic.com:",
            ":443",
            "evil.api.anthropic.com:443",
            "api.anthropic.com.evil.io:443",
            "anthropic.com:443",
            "160.79.104.10:443",
            "[2607:6bc0::10]:443",
            "127.0.0.1:443",
            "localhost:443",
            " api.anthropic.com:443",
            "api.anthropic.com:443 ",
            "api.anthropic.com:443:443",
        ] {
            assert_eq!(targets.resolve(refused), None, "{refused}");
        }
    }

    #[test]
    fn the_test_target_only_matches_exactly() {
        let targets = Targets {
            extra: Some("127.0.0.1:4567".into()),
        };
        assert_eq!(targets.resolve("127.0.0.1:4567"), Some(("127.0.0.1".into(), 4567)));
        assert_eq!(targets.resolve("127.0.0.1:4568"), None);
        assert_eq!(targets.resolve("claude.ai:443"), Some(("claude.ai".into(), 443)));
    }

    #[test]
    fn tunnel_url_follows_the_router_url() {
        assert_eq!(
            tunnel_url("https://router.etus.io").unwrap(),
            "wss://router.etus.io/etus/home-egress"
        );
        assert_eq!(
            tunnel_url("https://router.etus.io/").unwrap(),
            "wss://router.etus.io/etus/home-egress"
        );
        assert_eq!(
            tunnel_url("http://localhost:20128").unwrap(),
            "ws://localhost:20128/etus/home-egress"
        );
        assert!(tunnel_url("ftp://router").is_err());
        assert_eq!(
            endpoint(
                "https://router.etus.io",
                Some(" ws://127.0.0.1:20140/etus/home-egress ".into())
            )
            .unwrap(),
            "ws://127.0.0.1:20140/etus/home-egress"
        );
        assert_eq!(
            endpoint("https://router.etus.io", Some(String::new())).unwrap(),
            "wss://router.etus.io/etus/home-egress"
        );
        assert!(endpoint("https://router.etus.io", Some("http://x".into())).is_err());
    }

    #[test]
    fn refusals_map_to_states_and_waits() {
        let minutes = |m: u64| Wait::Fixed(Duration::from_secs(m * 60));
        let r = classify_http(401, br#"{"error":{"code":"AUTH_002","message":"x"}}"#, None);
        assert_eq!((r.state, r.wait), (State::Refused, minutes(10)));
        assert!(r.reason.contains("etus router setup"));
        let r = classify_http(403, br#"{"error":{"code":"no_router_key"}}"#, None);
        assert_eq!((r.state, r.wait), (State::Refused, minutes(10)));
        let r = classify_http(403, br#"{"error":{"code":"home_egress_off"}}"#, None);
        assert_eq!((r.state, r.wait), (State::WaitingAdmin, minutes(5)));
        let r = classify_http(404, b"not json", None);
        assert_eq!((r.state, r.wait), (State::ServerOff, minutes(30)));
        let r = classify_http(429, br#"{"error":{"code":"too_many_attempts"}}"#, Some("17"));
        assert_eq!(r.wait, Wait::Fixed(Duration::from_secs(17)));
        let r = classify_http(429, b"", None);
        assert_eq!(r.wait, Wait::Fixed(Duration::from_secs(60)));
        let r = classify_http(503, br#"{"error":{"code":"panel_unavailable"}}"#, None);
        assert_eq!((r.state, r.wait), (State::Offline, Wait::Backoff));
        assert!(r.reason.contains("panel_unavailable"));
        assert_eq!(classify_close(Some(4001)).state, State::WaitingAdmin);
        assert_eq!(classify_close(Some(4002)).state, State::Replaced);
        assert_eq!(classify_close(Some(4002)).wait, minutes(10));
        assert_eq!(classify_close(Some(1001)).wait, Wait::Backoff);
        assert_eq!(classify_close(None).wait, Wait::Backoff);
    }

    #[test]
    fn backoff_grows_to_a_minute_and_resets() {
        let mut backoff = Backoff::default();
        let delays: Vec<u64> = (0..8).map(|_| backoff.next_delay(1.0).as_secs()).collect();
        assert_eq!(delays, vec![2, 4, 8, 16, 32, 60, 60, 60]);
        assert_eq!(backoff.next_delay(0.0).as_secs(), 30);
        backoff.reset();
        assert_eq!(backoff.next_delay(0.0), Duration::from_secs(1));
        let unit = jitter_unit();
        assert!((0.0..=1.0).contains(&unit));
    }

    #[test]
    fn control_messages_are_parsed() {
        assert_eq!(
            parse_control(r#"{"type":"hello","personName":"Ana","hosts":[],"pingSeconds":20}"#),
            Control::Hello {
                person_name: Some("Ana".into())
            }
        );
        assert_eq!(
            parse_control(r#"{"type":"egress","ip":"203.0.113.7","at":"2026-10-17T12:00:00.000Z"}"#),
            Control::Egress {
                ip: "203.0.113.7".into(),
                at: Some("2026-10-17T12:00:00.000Z".into())
            }
        );
        assert_eq!(parse_control(r#"{"type":"egress"}"#), Control::Other);
        assert_eq!(parse_control("nope"), Control::Other);
    }
}
