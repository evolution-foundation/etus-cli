use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::catalog::now_secs;
use crate::config::write_private;
use crate::error::Result;

pub const STATE_FILE: &str = "saida-casa.json";
pub const LOG_FILE: &str = "saida-casa.log";
pub const LOG_LIMIT: u64 = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    #[default]
    Connecting,
    Connected,
    WaitingAdmin,
    Refused,
    ServerOff,
    Replaced,
    Offline,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub pid: u32,
    pub state: State,
    pub since: String,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub person_name: Option<String>,
    #[serde(default)]
    pub egress_ip: Option<String>,
    #[serde(default)]
    pub egress_ip_at: Option<String>,
    pub updated_at: String,
}

pub fn state_path(config_dir: &Path) -> PathBuf {
    config_dir.join(STATE_FILE)
}

pub fn log_path(config_dir: &Path) -> PathBuf {
    config_dir.join(LOG_FILE)
}

pub fn load(config_dir: &Path) -> Option<Snapshot> {
    let text = fs::read_to_string(state_path(config_dir)).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn remove(config_dir: &Path) {
    let _ = fs::remove_file(state_path(config_dir));
}

fn short(text: &str) -> String {
    let clean: String = text.chars().filter(|c| !c.is_control()).take(200).collect();
    clean.trim().to_string()
}

/// Keeps the state file and the short log of one tunnel process. Writes happen only on events, never on a timer of its own.
pub struct Reporter {
    dir: PathBuf,
    snapshot: Snapshot,
    last_line: Option<String>,
    echo: bool,
}

impl Reporter {
    pub fn new(dir: &Path, echo: bool) -> Reporter {
        let now = iso8601(now_secs());
        Reporter {
            dir: dir.to_path_buf(),
            snapshot: Snapshot {
                pid: std::process::id(),
                state: State::Connecting,
                since: now.clone(),
                updated_at: now,
                ..Snapshot::default()
            },
            last_line: None,
            echo,
        }
    }

    pub fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }

    pub fn set_state(&mut self, state: State, error: Option<&str>) {
        if self.snapshot.state != state {
            self.snapshot.state = state;
            self.snapshot.since = iso8601(now_secs());
        }
        self.snapshot.last_error = error.map(short);
        self.save();
    }

    pub fn set_person(&mut self, name: Option<String>) {
        self.snapshot.person_name = name.map(|n| short(&n));
        self.save();
    }

    pub fn set_egress(&mut self, ip: &str, at: Option<String>) {
        self.snapshot.egress_ip = Some(short(ip));
        self.snapshot.egress_ip_at = Some(at.map(|a| short(&a)).unwrap_or_else(|| iso8601(now_secs())));
        self.save();
    }

    pub fn touch(&mut self) {
        self.save();
    }

    fn save(&mut self) {
        self.snapshot.updated_at = iso8601(now_secs());
        if let Ok(text) = serde_json::to_vec_pretty(&self.snapshot) {
            let _ = write_private(&state_path(&self.dir), &text);
        }
    }

    /// One line per relevant event; a repeat of the previous line is skipped so a long outage does not fill the file.
    pub fn log(&mut self, line: &str) {
        let line = short(line);
        if self.last_line.as_deref() == Some(line.as_str()) {
            return;
        }
        if self.echo {
            eprintln!("etus: saída pela casa: {line}");
        }
        let _ = append_log(&log_path(&self.dir), &format!("{} {line}\n", iso8601(now_secs())));
        self.last_line = Some(line);
    }
}

pub fn append_log(path: &Path, line: &str) -> Result<()> {
    if fs::metadata(path).map(|m| m.len()).unwrap_or(0) + line.len() as u64 > LOG_LIMIT {
        let mut rotated = path.as_os_str().to_owned();
        rotated.push(".1");
        fs::rename(path, PathBuf::from(rotated))?;
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut options = fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)?.write_all(line.as_bytes())?;
    Ok(())
}

fn days_to_civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

fn civil_to_days(year: i64, month: u32, day: u32) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let yoe = year.rem_euclid(400);
    let month = i64::from(month);
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

pub fn iso8601(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rest = secs % 86_400;
    let (year, month, day) = days_to_civil(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        (rest % 3600) / 60,
        rest % 60
    )
}

pub fn parse_iso8601(text: &str) -> Option<u64> {
    let text = text.trim();
    let (date, time) = text.split_once('T')?;
    let mut date_parts = date.splitn(3, '-');
    let year: i64 = date_parts.next()?.parse().ok()?;
    let month: u32 = date_parts.next()?.parse().ok()?;
    let day: u32 = date_parts.next()?.parse().ok()?;
    let time = time.strip_suffix('Z')?;
    let time = time.split_once('.').map(|(t, _)| t).unwrap_or(time);
    let mut time_parts = time.splitn(3, ':');
    let hour: u64 = time_parts.next()?.parse().ok()?;
    let minute: u64 = time_parts.next()?.parse().ok()?;
    let second: u64 = time_parts.next()?.parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let days = civil_to_days(year, month, day);
    u64::try_from(days)
        .ok()
        .map(|d| d * 86_400 + hour * 3600 + minute * 60 + second)
}

pub fn ago(secs: u64) -> String {
    match secs {
        0..=59 => "agora há pouco".into(),
        60..=3599 => format!("há {} min", secs / 60),
        3600..=86_399 => {
            let minutes = (secs % 3600) / 60;
            if minutes == 0 {
                format!("há {} h", secs / 3600)
            } else {
                format!("há {} h {minutes} min", secs / 3600)
            }
        }
        _ => format!("há {} dias", secs / 86_400),
    }
}

pub fn ago_from(iso: &str, now: u64) -> Option<String> {
    parse_iso8601(iso).map(|then| ago(now.saturating_sub(then)))
}

pub fn describe(snapshot: &Snapshot, now: u64) -> String {
    let since = ago_from(&snapshot.since, now).unwrap_or_else(|| "-".into());
    let error = snapshot.last_error.as_deref().unwrap_or("");
    match snapshot.state {
        State::Connecting => "conectando ao AI Router".into(),
        State::Connected => format!("conectado ({since})"),
        State::WaitingAdmin => "aguardando o admin ligar no painel do AI Router (tenta de novo a cada 5 min)".into(),
        State::Refused if error.is_empty() => "chave do computador recusada: rode `etus router setup`".into(),
        State::Refused => format!("recusado: {error}"),
        State::ServerOff => "o AI Router ainda não tem a saída pela casa ligada (tenta de novo a cada 30 min)".into(),
        State::Replaced => "outra conexão sua assumiu, de outro computador (tenta de novo a cada 10 min)".into(),
        State::Offline if error.is_empty() => format!("sem conexão com o AI Router ({since}); tentando de novo"),
        State::Offline => format!("sem conexão com o AI Router ({since}): {error}; tentando de novo"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::temp_dir;

    #[test]
    fn iso_dates_round_trip() {
        assert_eq!(iso8601(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso8601(1_792_238_400), "2026-10-17T12:00:00Z");
        assert_eq!(iso8601(951_825_600), "2000-02-29T12:00:00Z");
        assert_eq!(parse_iso8601("2026-10-17T12:00:00.000Z"), Some(1_792_238_400));
        assert_eq!(parse_iso8601("2026-10-17T12:00:00Z"), Some(1_792_238_400));
        for secs in [0, 86_399, 951_825_600, 1_792_238_400, 4_102_444_800] {
            assert_eq!(parse_iso8601(&iso8601(secs)), Some(secs));
        }
        assert_eq!(parse_iso8601("ontem"), None);
        assert_eq!(parse_iso8601("2026-13-01T00:00:00Z"), None);
        assert_eq!(ago(30), "agora há pouco");
        assert_eq!(ago(125), "há 2 min");
        assert_eq!(ago(7200), "há 2 h");
        assert_eq!(ago(7500), "há 2 h 5 min");
        assert_eq!(ago(3 * 86_400), "há 3 dias");
    }

    #[cfg(unix)]
    #[test]
    fn state_file_is_private_camel_case_and_round_trips() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_dir("egress-state");
        let mut reporter = Reporter::new(&dir, false);
        reporter.set_state(State::Connected, None);
        reporter.set_person(Some("Ana\nSouza".into()));
        reporter.set_egress("203.0.113.7", Some("2026-10-17T12:00:00.000Z".into()));
        let path = state_path(&dir);
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        let raw: serde_json::Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(raw["state"], "connected");
        assert_eq!(raw["personName"], "AnaSouza");
        assert_eq!(raw["egressIp"], "203.0.113.7");
        assert_eq!(raw["egressIpAt"], "2026-10-17T12:00:00.000Z");
        assert_eq!(raw["pid"], std::process::id());
        for key in ["since", "updatedAt", "lastError"] {
            assert!(raw.get(key).is_some(), "{key}");
        }
        assert_eq!(load(&dir).unwrap(), *reporter.snapshot());
        reporter.set_state(State::WaitingAdmin, Some("o admin ainda não ligou"));
        assert_eq!(load(&dir).unwrap().state, State::WaitingAdmin);
        assert_eq!(
            serde_json::to_value(State::ServerOff).unwrap(),
            serde_json::json!("server_off")
        );
        remove(&dir);
        assert!(load(&dir).is_none());
    }

    #[test]
    fn log_skips_repeats_and_rotates() {
        let dir = temp_dir("egress-log");
        let mut reporter = Reporter::new(&dir, false);
        reporter.log("conectado");
        reporter.log("conectado");
        reporter.log("caiu");
        let text = fs::read_to_string(log_path(&dir)).unwrap();
        assert_eq!(text.lines().count(), 2);
        let line = "x".repeat(1000) + "\n";
        for _ in 0..300 {
            append_log(&log_path(&dir), &line).unwrap();
        }
        assert!(fs::metadata(log_path(&dir)).unwrap().len() <= LOG_LIMIT);
        assert!(dir.join("saida-casa.log.1").exists());
    }

    #[test]
    fn descriptions_are_in_portuguese() {
        let now = 1_792_238_400;
        let snapshot = |state, error: Option<&str>| Snapshot {
            state,
            since: iso8601(now - 600),
            last_error: error.map(str::to_string),
            ..Snapshot::default()
        };
        assert_eq!(
            describe(&snapshot(State::Connected, None), now),
            "conectado (há 10 min)"
        );
        assert!(describe(&snapshot(State::WaitingAdmin, None), now).contains("aguardando o admin"));
        assert!(describe(&snapshot(State::Refused, None), now).contains("etus router setup"));
        assert!(describe(&snapshot(State::Offline, Some("timeout")), now).contains("timeout"));
    }
}
