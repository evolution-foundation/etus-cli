use std::fs;
use std::io::{IsTerminal, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::cli::RouterSetupArgs;
use crate::config::write_private;
use crate::context::Ctx;
use crate::error::{Error, Result};
use crate::home_egress;
use crate::router_key::{self, Fetch, Origin, RouterKey};

pub const DEFAULT_ROUTER_URL: &str = "https://router.etus.io";
pub const DEFAULT_MODEL: &str = "dev-continuo";
pub const DEFAULT_FAST_MODEL: &str = "rapido";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouterSetupOptions {
    pub chave: Option<String>,
    pub modelo: String,
    pub fast_model: String,
    pub url: String,
    pub no_work_tags: bool,
    pub test: bool,
    pub home_egress: bool,
}

impl From<RouterSetupArgs> for RouterSetupOptions {
    fn from(args: RouterSetupArgs) -> Self {
        Self {
            chave: args.chave,
            modelo: args.modelo,
            fast_model: args.fast_model,
            url: args.url,
            no_work_tags: args.no_work_tags,
            test: args.test,
            home_egress: args.saida_casa,
        }
    }
}

pub fn claude_settings_path() -> Result<PathBuf> {
    let dir = if let Some(dir) = std::env::var_os("CLAUDE_CONFIG_DIR").filter(|d| !d.is_empty()) {
        PathBuf::from(dir)
    } else if let Some(base) = directories::BaseDirs::new() {
        base.home_dir().join(".claude")
    } else {
        return Err(Error::Other(
            "Não foi possível localizar o diretório do Claude Code.".into(),
        ));
    };
    Ok(dir.join("settings.json"))
}

pub fn check_claude_installed() -> bool {
    Command::new("claude")
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

pub fn format_timestamp() -> String {
    let duration = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = duration.as_secs();
    let days = secs / 86400;
    let day_secs = secs % 86400;
    let hour = day_secs / 3600;
    let min = (day_secs % 3600) / 60;
    let sec = day_secs % 60;

    let mut year = 1970;
    let mut d = days;
    loop {
        let leap = (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0);
        let days_in_year = if leap { 366 } else { 365 };
        if d < days_in_year {
            break;
        }
        d -= days_in_year;
        year += 1;
    }
    let leap = (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0);
    let days_in_months = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut month = 1;
    for &dim in &days_in_months {
        if d < dim {
            break;
        }
        d -= dim;
        month += 1;
    }
    let day = d + 1;
    format!("{year:04}{month:02}{day:02}{hour:02}{min:02}{sec:02}")
}

pub fn normalize_router_url(raw: &str) -> Result<String> {
    let trimmed = raw.trim().trim_end_matches('/');
    let url = reqwest::Url::parse(trimmed).map_err(|_| Error::Usage(format!("URL do AI Router inválida: {raw}")))?;
    let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    match url.scheme() {
        "https" => {}
        "http" if loopback => {}
        _ => {
            return Err(Error::Usage(
                "A URL do AI Router precisa ser https (http só em localhost).".into(),
            ));
        }
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(Error::Usage(
            "A URL do AI Router não pode conter parâmetros de query ou fragmento.".into(),
        ));
    }
    Ok(trimmed.to_string())
}

pub fn prune_backups(path: &Path, keep: usize) -> Result<()> {
    let parent = match path.parent() {
        Some(p) => p,
        None => return Ok(()),
    };
    let file_name = match path.file_name().and_then(|n| n.to_str()) {
        Some(n) => n,
        None => return Ok(()),
    };
    let prefix = format!("{file_name}.bak.");
    let mut backups: Vec<PathBuf> = Vec::new();
    if let Ok(entries) = fs::read_dir(parent) {
        for entry in entries.flatten() {
            if let Ok(name) = entry.file_name().into_string() {
                if !name.starts_with(&prefix) {
                    continue;
                }
                backups.push(entry.path());
            }
        }
    }
    backups.sort();
    if backups.len() > keep {
        let to_remove = backups.len() - keep;
        for old in &backups[..to_remove] {
            let _ = fs::remove_file(old);
        }
    }
    Ok(())
}

pub fn backup_settings(path: &Path) -> Result<Option<PathBuf>> {
    if !path.exists() {
        return Ok(None);
    }
    let content = fs::read(path)?;
    let timestamp = format_timestamp();
    let bak_path = PathBuf::from(format!("{}.bak.{timestamp}", path.display()));
    write_private(&bak_path, &content)?;
    let _ = prune_backups(path, 5);
    Ok(Some(bak_path))
}

pub fn is_etus_command(cmd: &str, sub: &str) -> bool {
    let trimmed = cmd.trim().trim_matches('"').trim();
    let parts: Vec<&str> = trimmed.split_whitespace().collect();
    if parts.len() < 2 {
        return false;
    }
    if parts.last() != Some(&sub) {
        return false;
    }
    let exe = parts[parts.len() - 2].trim_matches('"');
    let file_name = exe.rsplit(['/', '\\']).next().unwrap_or(exe);
    let stem = file_name.to_lowercase();
    stem == "etus" || stem.starts_with("etus.")
}

pub fn is_native_executable(path: &Path) -> bool {
    if let Ok(mut file) = fs::File::open(path) {
        let mut magic = [0u8; 4];
        if file.read_exact(&mut magic).is_ok() {
            // ELF (Linux: \x7fELF), PE (Windows: MZ), Mach-O (macOS: 0xfe 0xed 0xfa 0xce/0xcf, 0xce/0xcf 0xfa 0xed 0xfe)
            return magic.starts_with(b"\x7fELF")
                || magic.starts_with(b"MZ")
                || magic == [0xfe, 0xed, 0xfa, 0xce]
                || magic == [0xfe, 0xed, 0xfa, 0xcf]
                || magic == [0xce, 0xfa, 0xed, 0xfe]
                || magic == [0xcf, 0xfa, 0xed, 0xfe];
        }
    }
    false
}

pub fn supports_hook(path: &Path) -> bool {
    Command::new(path)
        .arg("hook")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

pub fn resolve_etus_bin() -> String {
    if let Ok(exe) = std::env::current_exe() {
        let path_str = exe.to_string_lossy();
        if !path_str.contains("/target/") && !path_str.contains("\\target\\") && exe.exists() {
            if path_str.contains(' ') {
                return format!("\"{path_str}\"");
            }
            return path_str.into_owned();
        }
    }

    if let Some(home) = directories::BaseDirs::new().map(|b| b.home_dir().to_path_buf()) {
        let local_bin = home.join(".local/bin/etus");
        if local_bin.exists() && is_native_executable(&local_bin) && supports_hook(&local_bin) {
            let path_str = local_bin.to_string_lossy();
            if path_str.contains(' ') {
                return format!("\"{path_str}\"");
            }
            return path_str.into_owned();
        }
    }

    if let Some(exe) = std::env::current_exe().ok().filter(|e| e.exists() && supports_hook(e)) {
        let path_str = exe.to_string_lossy();
        if path_str.contains(' ') {
            return format!("\"{path_str}\"");
        }
        return path_str.into_owned();
    }

    "etus".to_string()
}

pub fn configure_env(data: &mut Value, url: &str, model: &str, fast_model: &str) {
    if !data.is_object() {
        *data = json!({});
    }
    let env = match data.get_mut("env") {
        Some(val) if val.is_object() => val,
        _ => {
            data["env"] = json!({});
            data.get_mut("env").unwrap()
        }
    };

    let env_obj = env.as_object_mut().unwrap();
    env_obj.insert("ANTHROPIC_BASE_URL".into(), json!(url));
    env_obj.remove("ANTHROPIC_AUTH_TOKEN");
    env_obj.remove("ANTHROPIC_API_KEY");
    env_obj.insert("ANTHROPIC_MODEL".into(), json!(model));
    env_obj.insert("ANTHROPIC_DEFAULT_OPUS_MODEL".into(), json!(model));
    env_obj.insert("ANTHROPIC_DEFAULT_SONNET_MODEL".into(), json!(model));
    env_obj.insert("ANTHROPIC_DEFAULT_HAIKU_MODEL".into(), json!(fast_model));
    env_obj.insert("ANTHROPIC_SMALL_FAST_MODEL".into(), json!(fast_model));
    env_obj.remove("CLAUDE_CODE_SUBAGENT_MODEL");
    env_obj.insert(
        "CLAUDE_CODE_DISABLE_UNKNOWN_MODEL_WINDOW_ENFORCEMENT".into(),
        json!("1"),
    );
    env_obj.insert("CLAUDE_CODE_MAX_CONTEXT_TOKENS".into(), json!("1000000"));
}

pub fn configure_hooks(data: &mut Value, work_tags: bool, etus_bin: &str) {
    let events = ["SessionStart", "CwdChanged", "FileChanged", "UserPromptSubmit"];
    let hook_cmd = format!("{etus_bin} hook");

    let mut hooks_obj = match data.get("hooks").and_then(Value::as_object).cloned() {
        Some(obj) => obj,
        None if work_tags => serde_json::Map::new(),
        None => return,
    };

    for (_event, groups) in hooks_obj.iter_mut() {
        if let Some(group_list) = groups.as_array_mut() {
            group_list.retain_mut(|group| {
                if let Some(h_list) = group.get_mut("hooks").and_then(Value::as_array_mut) {
                    h_list.retain(|h| {
                        let cmd = h.get("command").and_then(Value::as_str).unwrap_or("");
                        !is_etus_command(cmd, "hook")
                    });
                    !h_list.is_empty()
                } else {
                    true
                }
            });
        }
    }

    if work_tags {
        for event in events {
            let entry = hooks_obj.entry(event).or_insert_with(|| json!([]));
            if let Some(list) = entry.as_array_mut() {
                list.push(json!({
                    "hooks": [
                        {
                            "type": "command",
                            "command": hook_cmd,
                            "timeout": 10
                        }
                    ]
                }));
            }
        }
    }

    hooks_obj.retain(|_event, groups| groups.as_array().is_some_and(|a| !a.is_empty()));

    if hooks_obj.is_empty() {
        if let Some(obj) = data.as_object_mut() {
            obj.remove("hooks");
        }
    } else {
        data["hooks"] = Value::Object(hooks_obj);
    }
}

pub fn configure_statusline(data: &mut Value, work_tags: bool, etus_bin: &str) {
    let status_cmd = format!("{etus_bin} statusline");
    let current_status = data.get("statusLine");
    let is_ours = current_status
        .and_then(|s| s.get("command"))
        .and_then(Value::as_str)
        .is_some_and(|cmd| is_etus_command(cmd, "statusline"));

    if work_tags {
        if current_status.is_none() || current_status.is_some_and(|s| s.is_null()) || is_ours {
            data["statusLine"] = json!({
                "type": "command",
                "command": status_cmd
            });
        }
    } else if is_ours {
        let _ = data.as_object_mut().and_then(|obj| obj.remove("statusLine"));
    }
}

pub fn key_helper_command(etus_bin: &str) -> String {
    format!("{etus_bin} router chave")
}

pub fn is_key_helper(cmd: &str) -> bool {
    cmd.trim()
        .strip_suffix("router chave")
        .is_some_and(|exe| is_etus_command(&format!("{} chave", exe.trim()), "chave"))
}

pub fn configure_key_helper(data: &mut Value, etus_bin: &str) {
    data["apiKeyHelper"] = json!(key_helper_command(etus_bin));
}

pub fn explicit_key(explicit: Option<String>, env_token: Option<String>) -> Option<String> {
    [explicit, env_token]
        .into_iter()
        .flatten()
        .map(|k| k.trim().to_string())
        .find(|k| !k.is_empty())
}

fn prompt_key() -> Result<String> {
    use dialoguer::Password;
    eprintln!("Para obter sua chave individual, acesse: https://router.etus.io/admin/my-key\n");
    match Password::new().with_prompt("Chave do AI Router (sk-...)").interact() {
        Ok(key) if !key.trim().is_empty() => Ok(key.trim().to_string()),
        _ => Err(Error::Usage("Chave não informada.".into())),
    }
}

const LOGIN_HINT: &str = "Para obter a chave deste computador sem colar nada, entre com `etus login` e rode `etus router setup` de novo. Ou informe a chave: `etus router setup --chave <sk-...>`.";

pub fn resolve_key(
    ctx: &Ctx,
    opts: &RouterSetupOptions,
    env_token: Option<String>,
    stored: Option<RouterKey>,
    legacy: Option<String>,
    is_interactive: bool,
) -> Result<(RouterKey, &'static str)> {
    let manual = |key: String| RouterKey {
        key,
        origin: Origin::Manual,
        router_url: opts.url.clone(),
        device_id: None,
        device_name: None,
    };
    if let Some(key) = explicit_key(opts.chave.clone(), env_token) {
        return Ok((manual(key), "informada"));
    }
    let (reason, needs_login) = match router_key::obtain(ctx, &opts.url, stored.as_ref()) {
        Ok(key) => return Ok((key, "obtida pelo login")),
        Err(Fetch::Failed(error)) => return Err(error),
        Err(Fetch::NeedsLogin(reason)) => (reason, true),
        Err(Fetch::Unavailable(reason)) => (reason, false),
    };
    if let Some(previous) = stored.filter(|s| s.router_url == opts.url) {
        return Ok((previous, "mantida"));
    }
    if let Some(token) = legacy.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()) {
        eprintln!("etus: {reason} {LOGIN_HINT}");
        return Ok((manual(token), "migrada do settings.json"));
    }
    if needs_login {
        return Err(Error::Auth(format!("{reason} {LOGIN_HINT}")));
    }
    if is_interactive {
        eprintln!("etus: {reason}");
        return Ok((manual(prompt_key()?), "digitada"));
    }
    Err(Error::Usage(format!("{reason}\n\n{LOGIN_HINT}")))
}

pub fn mask_token(token: &str) -> String {
    let trimmed = token.trim();
    let chars: Vec<char> = trimmed.chars().collect();
    if chars.len() < 20 {
        "***".to_string()
    } else {
        let prefix: String = chars.iter().take(7).collect();
        let suffix: String = chars.iter().rev().take(8).rev().collect();
        format!("{prefix}...{suffix}")
    }
}

pub fn run_test_with_timeout(timeout: Duration) -> Result<String> {
    let mut child = Command::new("claude")
        .args(["-p", "responda apenas OK"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| Error::Other(format!("Falha ao iniciar o Claude Code para teste: {e}")))?;

    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut stdout_buf = Vec::new();
                let mut stderr_buf = Vec::new();
                if let Some(mut out) = child.stdout.take() {
                    let _ = out.read_to_end(&mut stdout_buf);
                }
                if let Some(mut err) = child.stderr.take() {
                    let _ = err.read_to_end(&mut stderr_buf);
                }
                let text = String::from_utf8_lossy(&stdout_buf).trim().to_string();
                if status.success() {
                    return Ok(text);
                } else {
                    let err = String::from_utf8_lossy(&stderr_buf).trim().to_string();
                    return Err(Error::Other(format!(
                        "Teste falhou (código {:?}): {}\n{}",
                        status.code(),
                        err,
                        text
                    )));
                }
            }
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(Error::Other(format!(
                        "Teste excedeu o tempo limite de {} s aguardando resposta do Claude Code.",
                        timeout.as_secs()
                    )));
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => {
                let _ = child.kill();
                return Err(Error::Other(format!("Erro ao monitorar o teste: {e}")));
            }
        }
    }
}

const HOME_EGRESS_MANUAL: &str = "A saída pela casa usa a chave deste computador, que vem pelo login. Rode `etus login` e `etus router setup --saida-casa` sem --chave (e sem OMNIROUTE_API_KEY).";

pub fn setup(ctx: &Ctx, mut opts: RouterSetupOptions) -> Result<()> {
    opts.url = normalize_router_url(&opts.url)?;
    let env_token = std::env::var("OMNIROUTE_API_KEY").ok();
    if opts.home_egress && explicit_key(opts.chave.clone(), env_token.clone()).is_some() {
        return Err(Error::Usage(HOME_EGRESS_MANUAL.into()));
    }

    let settings_path = claude_settings_path()?;
    if let Some(parent) = settings_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let claude_installed = check_claude_installed();

    let mut data: Value = if settings_path.exists() {
        let content = fs::read_to_string(&settings_path)?;
        if content.trim().is_empty() {
            json!({})
        } else {
            serde_json::from_str(&content).map_err(|e| {
                Error::Other(format!(
                    "Configuração {} não é um JSON válido: {e}",
                    settings_path.display()
                ))
            })?
        }
    } else {
        json!({})
    };

    if !data.is_object() {
        return Err(Error::Other(format!(
            "{} deveria conter um objeto JSON.",
            settings_path.display()
        )));
    }

    let existing_base_url = data
        .get("env")
        .and_then(|e| e.get("ANTHROPIC_BASE_URL"))
        .and_then(Value::as_str);

    // Só reaproveita a chave se o ANTHROPIC_BASE_URL atual coincidir com a URL configurada
    let existing_token = if existing_base_url.is_some_and(|u| u.trim_end_matches('/') == opts.url.trim_end_matches('/'))
    {
        data.get("env")
            .and_then(|e| e.get("ANTHROPIC_AUTH_TOKEN"))
            .and_then(Value::as_str)
    } else {
        None
    };

    let is_interactive = std::io::stdin().is_terminal() && std::io::stderr().is_terminal();
    let vault = router_key::vault(ctx);
    let stored = vault.load()?;
    let (key, key_status) = resolve_key(
        ctx,
        &opts,
        env_token,
        stored,
        existing_token.map(str::to_string),
        is_interactive,
    )?;
    if opts.home_egress && home_egress::usable_key(Some(key.clone())).is_err() {
        return Err(Error::Usage(format!(
            "A chave do AI Router não veio pelo login ({key_status}). {HOME_EGRESS_MANUAL}"
        )));
    }
    vault.save(&key)?;

    let bak_path = backup_settings(&settings_path)?;

    let custom_statusline_preserved = data
        .get("statusLine")
        .and_then(|s| s.get("command"))
        .and_then(Value::as_str)
        .is_some_and(|cmd| !is_etus_command(cmd, "statusline"));

    let etus_bin = resolve_etus_bin();
    configure_env(&mut data, &opts.url, &opts.modelo, &opts.fast_model);
    configure_key_helper(&mut data, &etus_bin);
    configure_hooks(&mut data, !opts.no_work_tags, &etus_bin);
    configure_statusline(&mut data, !opts.no_work_tags, &etus_bin);

    let formatted = serde_json::to_string_pretty(&data)? + "\n";
    write_private(&settings_path, formatted.as_bytes())?;

    let home_egress_enabled = if opts.home_egress {
        Some(home_egress::install(ctx, &key).map_err(|error| {
            Error::Other(format!(
                "O Claude Code foi configurado em {}, mas a saída pela casa não ligou: {}",
                settings_path.display(),
                error.message()
            ))
        })?)
    } else {
        home_egress::restart_if_installed();
        None
    };

    let masked = mask_token(&key.key);

    let mut test_result = None;
    if opts.test {
        if claude_installed {
            if !ctx.out.json {
                eprintln!("Testando conexão com o Claude Code...");
            }
            match run_test_with_timeout(Duration::from_secs(15)) {
                Ok(resp) => {
                    test_result = Some(resp);
                }
                Err(err) => {
                    let backup_info = bak_path
                        .as_ref()
                        .map(|b| format!("\nBackup salvo em: {}", b.display()))
                        .unwrap_or_default();
                    return Err(Error::Other(format!(
                        "A configuração foi gravada em {}{},\nmas o teste de conexão falhou:\n{err}",
                        settings_path.display(),
                        backup_info
                    )));
                }
            }
        } else if !ctx.out.json {
            eprintln!("Aviso: Teste não pôde ser executado pois 'claude' não foi encontrado no PATH.");
        }
    }

    let statusline_desc = if opts.no_work_tags {
        "desativada"
    } else if custom_statusline_preserved {
        "personalizada (mantida)"
    } else {
        "ativada (statusline)"
    };

    ctx.out.print_human(
        &(format!(
            "Claude Code configurado com sucesso para o AI Router da Etus!\n\n  Roteador:      {}\n  Chave:         {} ({})\n  Origem:        {}\n  Guardada em:   {}\n  Claude Code:   lê a chave com `{}` (apiKeyHelper)\n  Modelo:        {}\n  Rápido:        {}\n  Etiquetas:     {}\n  Statusline:    {}\n  Arquivo:       {}{}{}{}",
            opts.url,
            masked,
            key_status,
            key.origin_label(),
            vault.describe(),
            key_helper_command("etus"),
            opts.modelo,
            opts.fast_model,
            if !opts.no_work_tags {
                "ativadas (SessionStart, CwdChanged, FileChanged, UserPromptSubmit)"
            } else {
                "desativadas"
            },
            statusline_desc,
            settings_path.display(),
            if let Some(bak) = bak_path.as_ref() {
                format!(
                    " (backup: {})",
                    bak.file_name().and_then(|n| n.to_str()).unwrap_or("")
                )
            } else {
                String::new()
            },
            if !claude_installed {
                "\n\nAviso: Claude Code não encontrado no PATH. Instale-o com:\n  npm install -g @anthropic-ai/claude-code"
            } else {
                ""
            },
            if let Some(resp) = test_result.as_ref() {
                format!("\n\nTeste de acesso executado:\n  Resposta: {resp}\n  Conexão validada com sucesso!")
            } else {
                "\n\nPara testar o acesso:\n  claude -p \"responda apenas OK\"".to_string()
            }
        ) + &match home_egress_enabled.as_ref() {
            Some(enabled) => format!(
                "\n\nSaída pela casa ligada: {} em {}. O admin também precisa ligá-la para você no painel do AI Router; acompanhe com `etus router saida-casa status`.",
                enabled.kind.label(),
                enabled.file.display()
            ),
            None => String::new(),
        }),
        || {
            json!({
                "configured": true,
                "url": opts.url,
                "keyMasked": masked,
                "keyStatus": key_status,
                "keyOrigin": key.origin,
                "deviceName": key.device_name,
                "keyStore": vault.describe(),
                "apiKeyHelper": key_helper_command(&etus_bin),
                "model": opts.modelo,
                "fastModel": opts.fast_model,
                "workTags": !opts.no_work_tags,
                "statusline": statusline_desc,
                "settingsPath": settings_path.display().to_string(),
                "backupPath": bak_path.map(|p| p.display().to_string()),
                "claudeInstalled": claude_installed,
                "testOutput": test_result,
                "homeEgress": home_egress_enabled.as_ref().map(home_egress::Enabled::json),
            })
        },
    );

    Ok(())
}

pub fn status(ctx: &Ctx) -> Result<()> {
    let settings_path = claude_settings_path()?;
    let claude_installed = check_claude_installed();
    let egress = home_egress::report(ctx);
    let now = crate::catalog::now_secs();

    if !settings_path.exists() {
        ctx.out.print_human(
            &format!(
                "Claude Code ainda não está configurado para o AI Router.\nArquivo de configurações não encontrado: {}\n\nPara configurar:\n  etus router setup\n\n{}",
                settings_path.display(),
                egress.text(now)
            ),
            || json!({ "configured": false, "settingsPath": settings_path.display().to_string(), "homeEgress": egress.json(now) }),
        );
        return Ok(());
    }

    let content = fs::read_to_string(&settings_path)?;
    let data: Value = if content.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(&content).map_err(|e| {
            Error::Other(format!(
                "Configuração {} não é um JSON válido: {e}",
                settings_path.display()
            ))
        })?
    };

    let env = data.get("env").and_then(Value::as_object);
    let base_url = env.and_then(|e| e.get("ANTHROPIC_BASE_URL")).and_then(Value::as_str);
    let token = env.and_then(|e| e.get("ANTHROPIC_AUTH_TOKEN")).and_then(Value::as_str);
    let model = env.and_then(|e| e.get("ANTHROPIC_MODEL")).and_then(Value::as_str);
    let fast_model = env
        .and_then(|e| e.get("ANTHROPIC_SMALL_FAST_MODEL"))
        .or_else(|| env.and_then(|e| e.get("ANTHROPIC_DEFAULT_HAIKU_MODEL")))
        .and_then(Value::as_str);

    let has_hooks = data.get("hooks").and_then(Value::as_object).is_some_and(|hooks_obj| {
        hooks_obj.values().any(|groups| {
            groups.as_array().is_some_and(|arr| {
                arr.iter().any(|group| {
                    group.get("hooks").and_then(Value::as_array).is_some_and(|h_list| {
                        h_list.iter().any(|h| {
                            h.get("command")
                                .and_then(Value::as_str)
                                .is_some_and(|cmd| is_etus_command(cmd, "hook"))
                        })
                    })
                })
            })
        })
    });

    let status_cmd = data
        .get("statusLine")
        .and_then(|s| s.get("command"))
        .and_then(Value::as_str);

    let has_etus_statusline = status_cmd.is_some_and(|cmd| is_etus_command(cmd, "statusline"));
    let has_custom_statusline = status_cmd.is_some() && !has_etus_statusline;

    let is_router_url = base_url.is_some_and(|u| {
        let clean = u.trim_end_matches('/');
        clean == DEFAULT_ROUTER_URL.trim_end_matches('/') || clean.contains("router.etus.io")
    });

    let helper = data.get("apiKeyHelper").and_then(Value::as_str);
    let helper_ours = helper.is_some_and(is_key_helper);
    let vault = router_key::vault(ctx);
    let (stored, vault_error) = if helper_ours {
        match vault.load() {
            Ok(stored) => (stored, None),
            Err(error) => (None, Some(error.message().to_string())),
        }
    } else {
        (None, None)
    };
    let (masked_token, key_origin) = match (&stored, token) {
        (Some(key), _) => (mask_token(&key.key), key.origin_label()),
        (None, Some(token)) => (
            mask_token(token),
            "texto puro no settings.json (antigo: rode `etus router setup`)".into(),
        ),
        (None, None) if helper_ours => (
            "-".into(),
            vault_error
                .clone()
                .unwrap_or_else(|| "nenhuma chave guardada (rode `etus router setup`)".into()),
        ),
        (None, None) => ("-".into(), "-".into()),
    };
    let configured = is_router_url && (stored.is_some() || token.is_some());
    let shadowed: Vec<&str> = ["ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_API_KEY"]
        .into_iter()
        .filter(|name| std::env::var_os(name).is_some_and(|v| !v.is_empty()))
        .collect();

    let status_desc = if configured {
        "Configurado"
    } else if base_url.is_some() && !is_router_url {
        "Outro provedor"
    } else {
        "Incompleto"
    };

    let statusline_desc = if has_etus_statusline {
        "Ativada"
    } else if has_custom_statusline {
        "Personalizada (mantida)"
    } else {
        "Desativada"
    };

    let text = format!(
        "Configuração do AI Router no Claude Code:\n\n  Status:        {}\n  Roteador:      {}\n  Chave:         {}\n  Origem:        {}\n  Guardada em:   {}\n  Modelo:        {}\n  Rápido:        {}\n  Hooks:         {}\n  Statusline:    {}\n  Claude Code:   {}\n  Arquivo:       {}",
        status_desc,
        base_url.unwrap_or("-"),
        masked_token,
        key_origin,
        if stored.is_some() || (helper_ours && token.is_none()) {
            vault.describe()
        } else if token.is_some() {
            settings_path.display().to_string()
        } else {
            "-".into()
        },
        model.unwrap_or("-"),
        fast_model.unwrap_or("-"),
        if has_hooks { "Ativados" } else { "Desativados" },
        statusline_desc,
        if claude_installed {
            "Instalado"
        } else {
            "Não encontrado no PATH"
        },
        settings_path.display(),
    ) + &if shadowed.is_empty() {
        String::new()
    } else {
        format!(
            "\n\nAviso: {} no ambiente do shell tem prioridade sobre a chave do apiKeyHelper. Remova do seu perfil do shell.",
            shadowed.join(" e ")
        )
    } + "\n\n"
        + &egress.text(now);

    ctx.out.print_human(&text, || {
        json!({
            "configured": configured,
            "status": status_desc,
            "url": base_url,
            "keyMasked": masked_token,
            "keyOrigin": stored.as_ref().map(|k| json!(k.origin)).unwrap_or_else(|| if token.is_some() { json!("settings") } else { Value::Null }),
            "deviceName": stored.as_ref().and_then(|k| k.device_name.clone()),
            "apiKeyHelper": helper,
            "shadowedBy": shadowed,
            "model": model,
            "fastModel": fast_model,
            "hasHooks": has_hooks,
            "hasStatusline": has_etus_statusline,
            "customStatusline": has_custom_statusline,
            "claudeInstalled": claude_installed,
            "settingsPath": settings_path.display().to_string(),
            "homeEgress": egress.json(now),
        })
    });

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::temp_dir;

    #[test]
    fn mask_token_formats_correctly() {
        assert_eq!(mask_token("sk-7a20143671a23d7a-1c37b1-187a306c"), "sk-7a20...187a306c");
        assert_eq!(mask_token("sk-1234567890A"), "***");
        assert_eq!(mask_token("short"), "***");
        assert_eq!(
            mask_token("🔑sk-7a20143671a23d7a-1c37b1-187a306c"),
            "🔑sk-7a2...187a306c"
        );
    }

    #[test]
    fn normalize_router_url_validates() {
        assert_eq!(
            normalize_router_url("https://router.etus.io/").unwrap(),
            "https://router.etus.io"
        );
        assert_eq!(
            normalize_router_url("https://router.etus.io").unwrap(),
            "https://router.etus.io"
        );
        assert_eq!(
            normalize_router_url("http://localhost:8080").unwrap(),
            "http://localhost:8080"
        );
        assert_eq!(
            normalize_router_url("http://127.0.0.1:3000/").unwrap(),
            "http://127.0.0.1:3000"
        );
        assert!(normalize_router_url("http://router.etus.io").is_err());
        assert!(normalize_router_url("https://router.etus.io?query=1").is_err());
        assert!(normalize_router_url("https://router.etus.io#frag").is_err());
        assert!(normalize_router_url("not a url").is_err());
    }

    #[test]
    fn prune_backups_cleans_older_files() {
        let dir = temp_dir("prune-test");
        let settings = dir.join("settings.json");
        fs::write(&settings, "{}").unwrap();

        for i in 1..=8 {
            let bak = dir.join(format!("settings.json.bak.2026101612000{i}"));
            write_private(&bak, b"{}").unwrap();
        }

        prune_backups(&settings, 5).unwrap();

        let entries: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains(".bak."))
            .collect();
        assert_eq!(entries.len(), 5);
        assert!(!dir.join("settings.json.bak.20261016120001").exists());
        assert!(!dir.join("settings.json.bak.20261016120002").exists());
        assert!(!dir.join("settings.json.bak.20261016120003").exists());
        assert!(dir.join("settings.json.bak.20261016120004").exists());
        assert!(dir.join("settings.json.bak.20261016120008").exists());
    }

    #[test]
    fn is_etus_command_matches_variants() {
        assert!(is_etus_command("etus hook", "hook"));
        assert!(is_etus_command("\"/home/user/.local/bin/etus\" hook", "hook"));
        assert!(is_etus_command("etus.py hook", "hook"));
        assert!(is_etus_command("python3 etus.py hook", "hook"));
        assert!(is_etus_command("C:\\bin\\etus.exe hook", "hook"));
        assert!(is_etus_command("etus statusline", "statusline"));
        assert!(!is_etus_command("git hook", "hook"));
        assert!(!is_etus_command("echo statusline", "statusline"));
        assert!(!is_etus_command("etus hook", "statusline"));
    }

    #[test]
    fn configure_env_sets_all_omniroute_variables() {
        let mut data = json!({
            "env": {
                "ANTHROPIC_API_KEY": "old-secret",
                "CLAUDE_CODE_SUBAGENT_MODEL": "legacy"
            }
        });
        configure_env(&mut data, "https://router.etus.io", "dev-continuo", "rapido");
        let env = data.get("env").unwrap();
        assert_eq!(env.get("ANTHROPIC_BASE_URL").unwrap(), "https://router.etus.io");
        assert!(env.get("ANTHROPIC_AUTH_TOKEN").is_none());
        assert!(env.get("ANTHROPIC_API_KEY").is_none());
        assert_eq!(env.get("ANTHROPIC_MODEL").unwrap(), "dev-continuo");
        assert_eq!(env.get("ANTHROPIC_DEFAULT_OPUS_MODEL").unwrap(), "dev-continuo");
        assert_eq!(env.get("ANTHROPIC_DEFAULT_SONNET_MODEL").unwrap(), "dev-continuo");
        assert_eq!(env.get("ANTHROPIC_DEFAULT_HAIKU_MODEL").unwrap(), "rapido");
        assert_eq!(env.get("ANTHROPIC_SMALL_FAST_MODEL").unwrap(), "rapido");
        assert!(env.get("CLAUDE_CODE_SUBAGENT_MODEL").is_none());
        assert_eq!(
            env.get("CLAUDE_CODE_DISABLE_UNKNOWN_MODEL_WINDOW_ENFORCEMENT").unwrap(),
            "1"
        );
        assert_eq!(env.get("CLAUDE_CODE_MAX_CONTEXT_TOKENS").unwrap(), "1000000");
    }

    #[test]
    fn configure_hooks_cleans_and_injects_cleanly() {
        let mut data = json!({
            "hooks": {
                "SessionStart": [
                    {
                        "hooks": [
                            {
                                "type": "command",
                                "command": "etus.py hook",
                                "timeout": 10
                            }
                        ]
                    }
                ],
                "OtherEvent": [
                    {
                        "hooks": [
                            {
                                "type": "command",
                                "command": "my-custom-script.sh",
                                "timeout": 5
                            }
                        ]
                    }
                ]
            }
        });

        configure_hooks(&mut data, true, "\"/usr/bin/etus\"");
        let hooks = data.get("hooks").unwrap().as_object().unwrap();
        assert!(hooks.contains_key("SessionStart"));
        assert!(hooks.contains_key("CwdChanged"));
        assert!(hooks.contains_key("FileChanged"));
        assert!(hooks.contains_key("UserPromptSubmit"));
        assert!(hooks.contains_key("OtherEvent"));

        let session_hooks = hooks.get("SessionStart").unwrap().as_array().unwrap();
        assert_eq!(session_hooks.len(), 1);
        let cmd = session_hooks[0]["hooks"][0]["command"].as_str().unwrap();
        assert_eq!(cmd, "\"/usr/bin/etus\" hook");

        // When no_work_tags is passed, removes etus hooks
        configure_hooks(&mut data, false, "etus");
        let hooks = data.get("hooks").unwrap().as_object().unwrap();
        assert!(!hooks.contains_key("SessionStart"));
        assert!(hooks.contains_key("OtherEvent"));
    }

    #[test]
    fn configure_statusline_sets_and_removes() {
        let mut data = json!({});
        configure_statusline(&mut data, true, "etus");
        assert_eq!(
            data.get("statusLine").unwrap().get("command").unwrap(),
            "etus statusline"
        );

        configure_statusline(&mut data, false, "etus");
        assert!(data.get("statusLine").is_none());

        // Keeps custom user statusline intact
        let mut custom = json!({
            "statusLine": {
                "type": "command",
                "command": "custom-statusline"
            }
        });
        configure_statusline(&mut custom, true, "etus");
        assert_eq!(
            custom.get("statusLine").unwrap().get("command").unwrap(),
            "custom-statusline"
        );
    }

    #[test]
    fn backup_and_explicit_key_logic() {
        let dir = temp_dir("router-test");
        let settings_path = dir.join("settings.json");
        fs::write(&settings_path, "{}").unwrap();
        let bak = backup_settings(&settings_path).unwrap();
        assert!(bak.is_some());
        assert!(bak.unwrap().exists());

        assert_eq!(
            explicit_key(Some(" sk-explicit ".into()), Some("sk-env".into())).as_deref(),
            Some("sk-explicit")
        );
        assert_eq!(explicit_key(None, Some("sk-env".into())).as_deref(), Some("sk-env"));
        assert_eq!(explicit_key(Some("  ".into()), None), None);
        assert_eq!(explicit_key(None, None), None);
    }

    #[test]
    fn key_helper_points_at_etus_and_replaces_the_plain_token() {
        let mut data = json!({"env": {"ANTHROPIC_AUTH_TOKEN": "sk-old", "ANTHROPIC_API_KEY": "sk-older"}});
        configure_env(&mut data, "https://router.etus.io", "dev-continuo", "rapido");
        configure_key_helper(&mut data, "\"/Users/ana b/.local/bin/etus\"");
        assert!(data["env"].get("ANTHROPIC_AUTH_TOKEN").is_none());
        assert!(data["env"].get("ANTHROPIC_API_KEY").is_none());
        let helper = data["apiKeyHelper"].as_str().unwrap();
        assert_eq!(helper, "\"/Users/ana b/.local/bin/etus\" router chave");
        assert!(is_key_helper(helper));
        assert!(is_key_helper("etus router chave"));
        assert!(is_key_helper("C:\\bin\\etus.exe router chave"));
        assert!(!is_key_helper("vault kv get router chave"));
        assert!(!is_key_helper("etus router status"));
    }

    #[test]
    fn invalid_json_returns_error() {
        let dir = temp_dir("corrupt-json");
        let settings_path = dir.join("settings.json");
        fs::write(&settings_path, "{ broken json").unwrap();

        // Reading corrupt json fails
        let content = fs::read_to_string(&settings_path).unwrap();
        let res: serde_json::Result<Value> = serde_json::from_str(&content);
        assert!(res.is_err());
    }
}
