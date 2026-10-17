pub mod protocol;
pub mod service;
pub mod state;
pub mod tunnel;

use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use serde_json::{Value, json};

use crate::auth::store::StoreKind;
use crate::catalog::now_secs;
use crate::config::DEFAULT_HUB;
use crate::context::Ctx;
use crate::error::{Error, Result};
use crate::router_key::{self, Origin, RouterKey};
use protocol::{Backoff, Targets, Wait};
use service::{Kind, Launch, Service};
use state::{Reporter, Snapshot, State};

pub const DEVICE_KEY_PREFIX: &str = "sk-etd-";
const NO_KEY_WAIT: Duration = Duration::from_secs(600);

const NO_KEY: &str =
    "Nenhuma chave do AI Router neste computador. Rode `etus login` e `etus router setup` (sem --chave).";
const MANUAL_KEY: &str = "A saída pela casa usa a chave deste computador, que vem pelo login (sk-etd-...), e a chave guardada foi informada à mão (--chave). Rode `etus login` e `etus router setup` sem --chave, e tente de novo.";
const UNSUPPORTED: &str =
    "A saída pela casa ainda não tem suporte neste sistema (só macOS e Linux). Nada foi alterado.";
const KEYCHAIN: &str = "A chave do AI Router está no chaveiro do macOS, que o agente em segundo plano não consegue ler sem terminal. Rode `etus config armazenamento arquivo` e `etus router setup`, e tente de novo.";

pub fn usable_key(stored: Option<RouterKey>) -> Result<RouterKey> {
    let key = stored.ok_or_else(|| Error::Auth(NO_KEY.into()))?;
    if key.origin != Origin::Login || !key.key.starts_with(DEVICE_KEY_PREFIX) {
        return Err(Error::Usage(MANUAL_KEY.into()));
    }
    Ok(key)
}

pub fn load_device_key(ctx: &Ctx) -> Result<RouterKey> {
    usable_key(router_key::vault(ctx).load()?)
}

fn supported_service() -> Result<Service> {
    Service::from_env()?.ok_or_else(|| Error::Usage(UNSUPPORTED.into()))
}

fn agent_program() -> Result<String> {
    let resolved = crate::router::resolve_etus_bin();
    let unquoted = resolved.trim_matches('"');
    if Path::new(unquoted).is_absolute() {
        return Ok(unquoted.to_string());
    }
    std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .map_err(|e| Error::Other(format!("Não achei o caminho do etus: {e}")))
}

pub fn launch_spec(ctx: &Ctx, program: String) -> Launch {
    let mut args = Vec::new();
    if ctx.settings.hub != DEFAULT_HUB {
        args.extend(["--hub".to_string(), ctx.settings.hub.clone()]);
    }
    args.extend(["router".to_string(), "tunel".to_string()]);
    let mut env = Vec::new();
    if std::env::var_os("ETUS_CONFIG_DIR").is_some_and(|d| !d.is_empty()) {
        env.push((
            "ETUS_CONFIG_DIR".to_string(),
            ctx.settings.paths.config_dir.to_string_lossy().into_owned(),
        ));
    }
    if ctx.settings.token_store_from_env {
        let kind = match ctx.settings.token_store {
            StoreKind::File => "file",
            StoreKind::Keychain => "keychain",
        };
        env.push(("ETUS_TOKEN_STORE".to_string(), kind.to_string()));
    }
    Launch { program, args, env }
}

#[derive(Debug, Clone)]
pub struct Enabled {
    pub kind: Kind,
    pub file: PathBuf,
    pub replaced: bool,
    pub managed: bool,
}

impl Enabled {
    pub fn json(&self) -> Value {
        json!({
            "enabled": true,
            "agent": self.kind.label(),
            "agentFile": self.file.display().to_string(),
            "reinstalled": self.replaced,
            "serviceManaged": self.managed,
        })
    }
}

/// Installs and starts the system agent that keeps the tunnel up. The key must already be in the vault.
pub fn install(ctx: &Ctx, key: &RouterKey) -> Result<Enabled> {
    let service = supported_service()?;
    usable_key(Some(key.clone()))?;
    if cfg!(target_os = "macos") && ctx.settings.token_store == StoreKind::Keychain {
        return Err(Error::Usage(KEYCHAIN.into()));
    }
    let launch = launch_spec(ctx, agent_program()?);
    let installed = service.install(&launch)?;
    Ok(Enabled {
        kind: service.kind,
        file: installed.file,
        replaced: installed.replaced,
        managed: service.manage,
    })
}

pub fn enable(ctx: &Ctx) -> Result<()> {
    supported_service()?;
    let key = load_device_key(ctx)?;
    let enabled = install(ctx, &key)?;
    let text = format!(
        "Saída pela casa ligada neste computador.\n\n  Agente:        {} ({})\n  Arquivo:       {}\n  Estado em:     {}\n  Log:           {}\n\nO admin também precisa ligar a saída pela casa para você no painel do AI Router. Acompanhe com `etus router saida-casa status`.",
        enabled.kind.label(),
        if enabled.replaced { "reinstalado" } else { "instalado" },
        enabled.file.display(),
        state::state_path(&ctx.settings.paths.config_dir).display(),
        state::log_path(&ctx.settings.paths.config_dir).display(),
    );
    ctx.out.print_human(&text, || enabled.json());
    Ok(())
}

pub fn disable(ctx: &Ctx) -> Result<()> {
    let Some(service) = Service::from_env()? else {
        ctx.out.print_human(
            "A saída pela casa não tem suporte neste sistema; nada a desligar.",
            || json!({"disabled": false, "supported": false}),
        );
        return Ok(());
    };
    let existed = service.uninstall()?;
    state::remove(&ctx.settings.paths.config_dir);
    let text = if existed {
        format!(
            "Saída pela casa desligada neste computador: o agente parou e {} foi apagado.",
            service.file().display()
        )
    } else {
        "A saída pela casa já estava desligada neste computador.".to_string()
    };
    ctx.out.print_human(
        &text,
        || json!({"disabled": true, "wasInstalled": existed, "agentFile": service.file().display().to_string()}),
    );
    Ok(())
}

/// Logout erases the computer key, so the agent would only be refused: stop it too. Best effort.
pub fn stop_for_logout(ctx: &Ctx) -> bool {
    let Ok(Some(service)) = Service::from_env() else {
        return false;
    };
    if !service.installed() {
        return false;
    }
    match service.uninstall() {
        Ok(existed) => {
            state::remove(&ctx.settings.paths.config_dir);
            existed
        }
        Err(error) => {
            eprintln!("etus: aviso: não consegui desligar a saída pela casa: {error}");
            false
        }
    }
}

/// After the computer key changes, the running agent reconnects with the new one.
pub fn restart_if_installed() {
    if let Ok(Some(service)) = Service::from_env()
        && service.installed()
        && let Err(error) = service.restart()
    {
        eprintln!("etus: aviso: não consegui reiniciar a saída pela casa: {error}");
    }
}

fn pid_alive(pid: u32) -> bool {
    pid != 0
        && Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
}

#[derive(Debug, Clone)]
pub struct Report {
    pub kind: Option<Kind>,
    pub installed: bool,
    pub running: Option<bool>,
    pub agent_file: Option<PathBuf>,
    pub snapshot: Option<Snapshot>,
    pub state_path: PathBuf,
    pub log_path: PathBuf,
}

pub fn report(ctx: &Ctx) -> Report {
    let dir = &ctx.settings.paths.config_dir;
    let service = Service::from_env().ok().flatten();
    let snapshot = state::load(dir);
    let installed = service.as_ref().is_some_and(Service::installed);
    let running = match service.as_ref().and_then(Service::running) {
        Some(running) => Some(running),
        None => snapshot.as_ref().map(|s| pid_alive(s.pid)),
    };
    Report {
        kind: service.as_ref().map(|s| s.kind),
        installed,
        running,
        agent_file: service.as_ref().map(Service::file),
        snapshot,
        state_path: state::state_path(dir),
        log_path: state::log_path(dir),
    }
}

impl Report {
    fn agent_text(&self) -> String {
        let Some(kind) = self.kind else {
            return "sem suporte neste sistema (só macOS e Linux)".into();
        };
        match (self.installed, self.running) {
            (false, _) => "não instalado (ligue com `etus router saida-casa ligar`)".into(),
            (true, Some(true)) => format!("instalado e rodando ({})", kind.label()),
            (true, Some(false)) => format!("instalado, mas parado ({})", kind.label()),
            (true, None) => format!("instalado ({})", kind.label()),
        }
    }

    pub fn text(&self, now: u64) -> String {
        let mut lines = vec![
            "Saída pela casa:".to_string(),
            format!("  Agente:        {}", self.agent_text()),
        ];
        if let Some(snapshot) = &self.snapshot {
            let mut status = state::describe(snapshot, now);
            if let Some(name) = snapshot
                .person_name
                .as_deref()
                .filter(|_| snapshot.state == State::Connected)
            {
                status.push_str(&format!(", como {name}"));
            }
            if self.running == Some(false) {
                status.push_str(" (último estado conhecido; o agente não está rodando)");
            }
            lines.push(format!("  Estado:        {status}"));
            if let Some(ip) = &snapshot.egress_ip {
                let when = snapshot
                    .egress_ip_at
                    .as_deref()
                    .and_then(|at| state::ago_from(at, now))
                    .map(|a| format!(", medido {a}"))
                    .unwrap_or_default();
                lines.push(format!("  IP de saída:   {ip} (visto pela Anthropic{when})"));
            }
            if let Some(ago) = state::ago_from(&snapshot.updated_at, now) {
                lines.push(format!("  Atualizado:    {ago}"));
            }
        }
        if self.installed || self.snapshot.is_some() {
            lines.push(format!("  Log:           {}", self.log_path.display()));
        }
        lines.join("\n")
    }

    pub fn json(&self, now: u64) -> Value {
        let snapshot = self.snapshot.as_ref();
        json!({
            "supported": self.kind.is_some(),
            "installed": self.installed,
            "running": self.running,
            "agentFile": self.agent_file.as_ref().map(|p| p.display().to_string()),
            "state": snapshot.map(|s| s.state),
            "description": snapshot.map(|s| state::describe(s, now)),
            "since": snapshot.map(|s| s.since.clone()),
            "lastError": snapshot.and_then(|s| s.last_error.clone()),
            "personName": snapshot.and_then(|s| s.person_name.clone()),
            "egressIp": snapshot.and_then(|s| s.egress_ip.clone()),
            "egressIpAt": snapshot.and_then(|s| s.egress_ip_at.clone()),
            "updatedAt": snapshot.map(|s| s.updated_at.clone()),
            "statePath": self.state_path.display().to_string(),
            "logPath": self.log_path.display().to_string(),
        })
    }
}

pub fn status(ctx: &Ctx) -> Result<()> {
    let report = report(ctx);
    let now = now_secs();
    ctx.out.print_human(&report.text(now), || report.json(now));
    Ok(())
}

/// The agent itself: keeps one tunnel to the router up, forever, with the waits of the contract.
pub fn run_tunnel(ctx: &Ctx) -> Result<()> {
    let foreground = std::io::stderr().is_terminal();
    let dir = ctx.settings.paths.config_dir.clone();
    if foreground {
        load_device_key(ctx)?;
        if let Ok(Some(service)) = Service::from_env()
            && service.running() == Some(true)
        {
            eprintln!(
                "etus: aviso: o agente da saída pela casa já está rodando; as duas conexões vão se derrubar. Pare o agente com `etus router saida-casa desligar` para testar em primeiro plano."
            );
        }
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| Error::Other(format!("Não consegui iniciar o túnel: {e}")))?;
    let connector = tunnel::tls_connector();
    let targets = Targets::from_env();
    let mut backoff = Backoff::default();
    let mut report = Reporter::new(&dir, foreground);
    report.set_state(State::Connecting, None);
    report.log(&format!(
        "iniciado (etus-cli {}, pid {})",
        env!("CARGO_PKG_VERSION"),
        std::process::id()
    ));
    loop {
        let dial = load_device_key(ctx).and_then(|key| {
            let url = protocol::endpoint(&key.router_url, std::env::var(protocol::URL_ENV).ok())?;
            Ok(tunnel::Dial {
                url,
                device_name: key.device_name.clone().unwrap_or_else(router_key::machine_name),
                key: key.key,
                targets: targets.clone(),
            })
        });
        let wait = match dial {
            Err(error) => {
                report.set_state(State::Refused, Some(error.message()));
                report.log(&format!("sem chave utilizável: {}", error.message()));
                NO_KEY_WAIT
            }
            Ok(dial) => {
                let ended = runtime.block_on(tunnel::session(&dial, &connector, &mut report));
                if ended.greeted {
                    backoff.reset();
                } else {
                    report.log(&format!("não conectou: {}", ended.retry.reason));
                }
                report.set_state(ended.retry.state, Some(&ended.retry.reason));
                match ended.retry.wait {
                    Wait::Fixed(wait) => wait,
                    Wait::Backoff => backoff.next_delay(protocol::jitter_unit()),
                }
            }
        };
        runtime.block_on(async { tokio::time::sleep(wait).await });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(origin: Origin, value: &str) -> RouterKey {
        RouterKey {
            key: value.into(),
            origin,
            router_url: "https://router.etus.io".into(),
            device_id: None,
            device_name: None,
        }
    }

    #[test]
    fn only_a_login_device_key_opens_the_tunnel() {
        assert!(usable_key(Some(key(Origin::Login, "sk-etd-abc"))).is_ok());
        let manual = usable_key(Some(key(Origin::Manual, "sk-abc"))).unwrap_err();
        assert!(manual.message().contains("sem --chave"));
        assert!(usable_key(Some(key(Origin::Manual, "sk-etd-abc"))).is_err());
        assert!(usable_key(Some(key(Origin::Login, "sk-abc"))).is_err());
        assert_eq!(usable_key(None).unwrap_err().exit_code(), 3);
    }

    #[test]
    fn report_text_explains_each_state() {
        let now = 1_792_238_400;
        let base = Report {
            kind: Some(Kind::Launchd),
            installed: true,
            running: Some(true),
            agent_file: None,
            snapshot: Some(Snapshot {
                pid: 1,
                state: State::Connected,
                since: state::iso8601(now - 7200),
                last_error: None,
                person_name: Some("Ana".into()),
                egress_ip: Some("203.0.113.7".into()),
                egress_ip_at: Some(state::iso8601(now - 300)),
                updated_at: state::iso8601(now - 60),
            }),
            state_path: PathBuf::from("/tmp/x/saida-casa.json"),
            log_path: PathBuf::from("/tmp/x/saida-casa.log"),
        };
        let text = base.text(now);
        assert!(text.contains("instalado e rodando"));
        assert!(text.contains("conectado (há 2 h), como Ana"));
        assert!(text.contains("203.0.113.7 (visto pela Anthropic, medido há 5 min)"));
        assert!(text.contains("Atualizado:    há 1 min"));
        assert!(text.contains("/tmp/x/saida-casa.log"));
        let json = base.json(now);
        assert_eq!(json["state"], "connected");
        assert_eq!(json["egressIp"], "203.0.113.7");
        let off = Report {
            installed: false,
            running: None,
            snapshot: None,
            ..base.clone()
        };
        assert!(off.text(now).contains("etus router saida-casa ligar"));
        assert!(!off.text(now).contains("Log:"));
        let unsupported = Report { kind: None, ..off };
        assert!(unsupported.text(now).contains("sem suporte"));
    }
}
