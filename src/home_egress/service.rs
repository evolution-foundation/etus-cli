use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::config::write_private;
use crate::error::{Error, Result};

pub const LABEL: &str = "io.etus.saida-casa";
pub const UNIT: &str = "etus-saida-casa.service";
pub const DIR_ENV: &str = "ETUS_SAIDA_CASA_AGENT_DIR";
pub const NO_SERVICE_ENV: &str = "ETUS_SAIDA_CASA_NO_SERVICE";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Launchd,
    Systemd,
}

impl Kind {
    pub fn current() -> Option<Kind> {
        if cfg!(target_os = "macos") {
            Some(Kind::Launchd)
        } else if cfg!(target_os = "linux") {
            Some(Kind::Systemd)
        } else {
            None
        }
    }

    pub fn file_name(self) -> String {
        match self {
            Kind::Launchd => format!("{LABEL}.plist"),
            Kind::Systemd => UNIT.to_string(),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::Launchd => "LaunchAgent do usuário (launchd)",
            Kind::Systemd => "serviço systemd do usuário",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

impl Launch {
    pub fn render(&self, kind: Kind) -> String {
        match kind {
            Kind::Launchd => plist(self),
            Kind::Systemd => systemd_unit(self),
        }
    }
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

pub fn plist(launch: &Launch) -> String {
    let mut args = format!("        <string>{}</string>\n", xml_escape(&launch.program));
    for arg in &launch.args {
        args.push_str(&format!("        <string>{}</string>\n", xml_escape(arg)));
    }
    let env = if launch.env.is_empty() {
        String::new()
    } else {
        let mut entries = String::from("    <key>EnvironmentVariables</key>\n    <dict>\n");
        for (key, value) in &launch.env {
            entries.push_str(&format!(
                "        <key>{}</key>\n        <string>{}</string>\n",
                xml_escape(key),
                xml_escape(value)
            ));
        }
        entries.push_str("    </dict>\n");
        entries
    };
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LABEL}</string>
    <key>ProgramArguments</key>
    <array>
{args}    </array>
{env}    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>ProcessType</key>
    <string>Background</string>
    <key>StandardOutPath</key>
    <string>/dev/null</string>
    <key>StandardErrorPath</key>
    <string>/dev/null</string>
</dict>
</plist>
"#
    )
}

fn systemd_quote(text: &str) -> String {
    let escaped = text.replace('\\', "\\\\").replace('"', "\\\"").replace('%', "%%");
    format!("\"{escaped}\"")
}

pub fn systemd_unit(launch: &Launch) -> String {
    let exec = std::iter::once(&launch.program)
        .chain(launch.args.iter())
        .map(|a| systemd_quote(a))
        .collect::<Vec<_>>()
        .join(" ");
    let env: String = launch
        .env
        .iter()
        .map(|(k, v)| format!("Environment={}\n", systemd_quote(&format!("{k}={v}"))))
        .collect();
    format!(
        "[Unit]\nDescription=Etus: saida pela casa do AI Router\nAfter=network-online.target\nWants=network-online.target\n\n[Service]\nType=simple\nExecStart={exec}\n{env}Restart=always\nRestartSec=5\nStandardOutput=null\nStandardError=null\n\n[Install]\nWantedBy=default.target\n"
    )
}

/// Where the agent file lives and whether launchctl/systemctl may be called. Tests point `dir` at a temp folder and set `manage` to false.
#[derive(Debug, Clone)]
pub struct Service {
    pub kind: Kind,
    pub dir: PathBuf,
    pub manage: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub file: PathBuf,
    pub replaced: bool,
}

fn default_dir(kind: Kind) -> Result<PathBuf> {
    let base = directories::BaseDirs::new()
        .ok_or_else(|| Error::Other("Não foi possível achar a pasta do usuário.".into()))?;
    Ok(match kind {
        Kind::Launchd => base.home_dir().join("Library/LaunchAgents"),
        Kind::Systemd => base.config_dir().join("systemd/user"),
    })
}

impl Service {
    pub fn from_env() -> Result<Option<Service>> {
        let Some(kind) = Kind::current() else {
            return Ok(None);
        };
        let dir = match std::env::var_os(DIR_ENV).filter(|d| !d.is_empty()) {
            Some(dir) => PathBuf::from(dir),
            None => default_dir(kind)?,
        };
        let manage = std::env::var(NO_SERVICE_ENV).map(|v| v.trim() != "1").unwrap_or(true);
        Ok(Some(Service { kind, dir, manage }))
    }

    pub fn file(&self) -> PathBuf {
        self.dir.join(self.kind.file_name())
    }

    pub fn installed(&self) -> bool {
        self.file().exists()
    }

    pub fn install(&self, launch: &Launch) -> Result<Installed> {
        let file = self.file();
        let content = launch.render(self.kind);
        let previous = fs::read_to_string(&file).ok();
        let replaced = previous.is_some();
        if previous.as_deref() != Some(content.as_str()) {
            write_private(&file, content.as_bytes())?;
        }
        if self.manage {
            match self.kind {
                Kind::Launchd => self.launchd_start(&file, previous.as_deref() == Some(content.as_str()))?,
                Kind::Systemd => {
                    run("systemctl", &["--user", "daemon-reload"])?;
                    run("systemctl", &["--user", "enable", "--now", UNIT])?;
                    if replaced {
                        run("systemctl", &["--user", "restart", UNIT])?;
                    }
                }
            }
        }
        Ok(Installed { file, replaced })
    }

    fn launchd_start(&self, file: &Path, unchanged: bool) -> Result<()> {
        let domain = gui_domain()?;
        let target = format!("{domain}/{LABEL}");
        let loaded = quiet("launchctl", &["print", &target]);
        if loaded && unchanged {
            return run("launchctl", &["kickstart", "-k", &target]);
        }
        if loaded {
            let _ = quiet("launchctl", &["bootout", &target]);
        }
        let file = file.to_string_lossy();
        let mut last = Ok(());
        for _ in 0..10 {
            last = run("launchctl", &["bootstrap", &domain, &file]);
            if last.is_ok() {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(300));
        }
        last
    }

    /// Stops the agent and removes its file. Idempotent: returns false when nothing was installed.
    pub fn uninstall(&self) -> Result<bool> {
        let file = self.file();
        let existed = file.exists();
        if self.manage {
            match self.kind {
                Kind::Launchd => {
                    let target = format!("{}/{LABEL}", gui_domain()?);
                    if quiet("launchctl", &["print", &target]) {
                        let _ = quiet("launchctl", &["bootout", &target]);
                    }
                }
                Kind::Systemd if existed => {
                    let _ = quiet("systemctl", &["--user", "disable", "--now", UNIT]);
                }
                Kind::Systemd => {}
            }
        }
        if existed {
            fs::remove_file(&file)?;
            if self.manage && self.kind == Kind::Systemd {
                let _ = quiet("systemctl", &["--user", "daemon-reload"]);
            }
        }
        Ok(existed)
    }

    pub fn restart(&self) -> Result<()> {
        if !self.manage || !self.installed() {
            return Ok(());
        }
        match self.kind {
            Kind::Launchd => run("launchctl", &["kickstart", "-k", &format!("{}/{LABEL}", gui_domain()?)]),
            Kind::Systemd => run("systemctl", &["--user", "restart", UNIT]),
        }
    }

    /// None when the service manager is not consulted (tests) or cannot answer.
    pub fn running(&self) -> Option<bool> {
        if !self.manage {
            return None;
        }
        match self.kind {
            Kind::Launchd => {
                let target = format!("{}/{LABEL}", gui_domain().ok()?);
                let output = Command::new("launchctl")
                    .args(["print", &target])
                    .stdin(Stdio::null())
                    .stderr(Stdio::null())
                    .output()
                    .ok()?;
                Some(output.status.success() && String::from_utf8_lossy(&output.stdout).contains("state = running"))
            }
            Kind::Systemd => {
                let output = Command::new("systemctl")
                    .args(["--user", "is-active", UNIT])
                    .stdin(Stdio::null())
                    .stderr(Stdio::null())
                    .output()
                    .ok()?;
                Some(String::from_utf8_lossy(&output.stdout).trim() == "active")
            }
        }
    }
}

fn gui_domain() -> Result<String> {
    let output = Command::new("id")
        .arg("-u")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|e| Error::Other(format!("Não consegui ler o id do usuário: {e}")))?;
    let uid = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if uid.is_empty() || !uid.chars().all(|c| c.is_ascii_digit()) {
        return Err(Error::Other("Não consegui ler o id do usuário.".into()));
    }
    Ok(format!("gui/{uid}"))
}

fn quiet(program: &str, args: &[&str]) -> bool {
    Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn run(program: &str, args: &[&str]) -> Result<()> {
    let output = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| Error::Other(format!("Não consegui rodar {program}: {e}")))?;
    if output.status.success() {
        return Ok(());
    }
    let detail: String = String::from_utf8_lossy(&output.stderr)
        .trim()
        .chars()
        .take(300)
        .collect();
    Err(Error::Other(format!(
        "`{program} {}` falhou{}",
        args.join(" "),
        if detail.is_empty() {
            String::new()
        } else {
            format!(": {detail}")
        }
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::temp_dir;

    fn sample() -> Launch {
        Launch {
            program: "/Users/ana b/.local/bin/etus".into(),
            args: vec!["router".into(), "tunel".into()],
            env: vec![("ETUS_CONFIG_DIR".into(), "/tmp/a&b%c".into())],
        }
    }

    #[test]
    fn plist_keeps_the_agent_alive_in_the_background() {
        let text = plist(&sample());
        assert!(text.contains("<key>Label</key>\n    <string>io.etus.saida-casa</string>"));
        assert!(text.contains(
            "<array>\n        <string>/Users/ana b/.local/bin/etus</string>\n        <string>router</string>\n        <string>tunel</string>\n    </array>"
        ));
        assert!(text.contains("<key>RunAtLoad</key>\n    <true/>"));
        assert!(text.contains("<key>KeepAlive</key>\n    <true/>"));
        assert!(text.contains("<key>ProcessType</key>\n    <string>Background</string>"));
        assert!(text.contains("<key>StandardOutPath</key>\n    <string>/dev/null</string>"));
        assert!(text.contains("<key>StandardErrorPath</key>\n    <string>/dev/null</string>"));
        assert!(text.contains("<key>ETUS_CONFIG_DIR</key>\n        <string>/tmp/a&amp;b%c</string>"));
        let plain = plist(&Launch {
            env: Vec::new(),
            ..sample()
        });
        assert!(!plain.contains("EnvironmentVariables"));
    }

    #[test]
    fn systemd_unit_restarts_and_quotes() {
        let text = systemd_unit(&sample());
        assert!(text.contains("ExecStart=\"/Users/ana b/.local/bin/etus\" \"router\" \"tunel\"\n"));
        assert!(text.contains("Environment=\"ETUS_CONFIG_DIR=/tmp/a&b%%c\"\n"));
        assert!(text.contains("Restart=always\nRestartSec=5\n"));
        assert!(text.contains("StandardOutput=null\nStandardError=null\n"));
        assert!(text.contains("[Install]\nWantedBy=default.target\n"));
    }

    #[test]
    fn install_and_uninstall_only_touch_the_given_folder() {
        for kind in [Kind::Launchd, Kind::Systemd] {
            let dir = temp_dir("egress-agent");
            let service = Service {
                kind,
                dir: dir.join("agents"),
                manage: false,
            };
            assert!(!service.installed());
            assert!(!service.uninstall().unwrap());
            let first = service.install(&sample()).unwrap();
            assert!(!first.replaced);
            assert_eq!(fs::read_to_string(service.file()).unwrap(), sample().render(kind));
            assert!(service.install(&sample()).unwrap().replaced);
            assert_eq!(service.running(), None);
            service.restart().unwrap();
            assert!(service.uninstall().unwrap());
            assert!(!service.installed());
            assert!(!service.uninstall().unwrap());
        }
    }
}
