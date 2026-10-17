use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use crate::error::{Error, Result};
use crate::setup::Target;

pub const SERVER: &str = "etus";
const URL_KEYS: [&str; 3] = ["url", "serverUrl", "httpUrl"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum McpMode {
    Completo,
    #[default]
    Quente,
    Compacto,
}

impl McpMode {
    pub fn id(self) -> &'static str {
        match self {
            McpMode::Completo => "completo",
            McpMode::Quente => "quente",
            McpMode::Compacto => "compacto",
        }
    }

    pub fn parse(text: &str) -> Option<McpMode> {
        match text.trim().to_lowercase().as_str() {
            "completo" => Some(McpMode::Completo),
            "quente" => Some(McpMode::Quente),
            "compacto" => Some(McpMode::Compacto),
            _ => None,
        }
    }
}

pub fn mcp_url(hub: &str, mode: McpMode) -> String {
    format!("{}/mcp?modo={}", hub.trim_end_matches('/'), mode.id())
}

pub fn mode_of(url: &str) -> String {
    url.split_once('?')
        .and_then(|(_, query)| {
            query
                .split('&')
                .find_map(|pair| pair.strip_prefix("modo="))
                .map(str::to_string)
        })
        .unwrap_or_else(|| "padrão do hub".into())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Project,
    User,
}

impl Scope {
    pub fn label(self) -> &'static str {
        match self {
            Scope::Project => "projeto",
            Scope::User => "usuário",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Json {
        root: &'static str,
        url_key: &'static str,
        typed: bool,
    },
    CodexToml,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpFile {
    pub editor: &'static str,
    pub path: PathBuf,
    pub scope: Scope,
    pub format: Format,
    pub login: &'static str,
}

const CLAUDE_JSON: Format = Format::Json {
    root: "mcpServers",
    url_key: "url",
    typed: true,
};

pub fn mcp_files(target: Target, root: &Path, home: &Path) -> Vec<McpFile> {
    let file = |editor, path: PathBuf, scope, format, login| McpFile {
        editor,
        path,
        scope,
        format,
        login,
    };
    match target {
        Target::ClaudeCode => vec![file(
            "Claude Code",
            root.join(".mcp.json"),
            Scope::Project,
            CLAUDE_JSON,
            "no Claude Code, /mcp, escolha etus e entre com a conta Etus (na primeira vez ele pergunta se confia no .mcp.json)",
        )],
        Target::Copilot => vec![file(
            "VS Code e Copilot CLI",
            root.join(".mcp.json"),
            Scope::Project,
            CLAUDE_JSON,
            "o VS Code abre o login no navegador na primeira conexão; o Copilot CLI lê o mesmo .mcp.json",
        )],
        Target::Cursor => vec![file(
            "Cursor",
            root.join(".cursor").join("mcp.json"),
            Scope::Project,
            Format::Json {
                root: "mcpServers",
                url_key: "url",
                typed: false,
            },
            "no Cursor, Settings > MCP, etus, entrar",
        )],
        Target::Codex => vec![file(
            "Codex",
            root.join(".codex").join("config.toml"),
            Scope::Project,
            Format::CodexToml,
            "codex mcp login etus (o Codex só lê .codex/config.toml de projeto confiável)",
        )],
        Target::Antigravity => vec![
            file(
                "Gemini CLI",
                root.join(".gemini").join("settings.json"),
                Scope::Project,
                Format::Json {
                    root: "mcpServers",
                    url_key: "httpUrl",
                    typed: false,
                },
                "no Gemini CLI, /mcp auth etus",
            ),
            file(
                "Antigravity",
                root.join(".agents").join("mcp_config.json"),
                Scope::Project,
                Format::Json {
                    root: "mcpServers",
                    url_key: "serverUrl",
                    typed: false,
                },
                "no Antigravity, Agent settings > Customizations > Authenticate",
            ),
        ],
        Target::Windsurf => windsurf_files(home)
            .into_iter()
            .map(|path| {
                file(
                    "Windsurf",
                    path,
                    Scope::User,
                    Format::Json {
                        root: "mcpServers",
                        url_key: "serverUrl",
                        typed: false,
                    },
                    "no painel MCP do Cascade, etus, entrar (o Windsurf não lê MCP do projeto)",
                )
            })
            .collect(),
    }
}

fn windsurf_files(home: &Path) -> Vec<PathBuf> {
    let current = home.join(".config").join("devin");
    let legacy = home.join(".codeium").join("windsurf");
    let found: Vec<PathBuf> = [&current, &legacy]
        .into_iter()
        .filter(|dir| dir.is_dir())
        .map(|dir| dir.join("mcp_config.json"))
        .collect();
    if found.is_empty() {
        vec![current.join("mcp_config.json")]
    } else {
        found
    }
}

pub const CLAUDE_SKILLS: &str = ".claude/skills";
pub const SHARED_SKILLS: &str = ".agents/skills";

pub fn skills_dir(target: Target) -> &'static str {
    match target {
        Target::ClaudeCode => CLAUDE_SKILLS,
        _ => SHARED_SKILLS,
    }
}

pub fn needs_shared_skills(targets: &[Target]) -> bool {
    let has = |t: Target| targets.contains(&t);
    has(Target::Codex)
        || has(Target::Antigravity)
        || has(Target::Windsurf)
        || ((has(Target::Cursor) || has(Target::Copilot)) && !has(Target::ClaudeCode))
}

pub fn skills_note(target: Target, targets: &[Target]) -> String {
    let shared = needs_shared_skills(targets);
    match target {
        Target::ClaudeCode => CLAUDE_SKILLS.to_string(),
        Target::Cursor | Target::Copilot if !shared => {
            format!("{CLAUDE_SKILLS} (o editor também lê essa pasta)")
        }
        _ => SHARED_SKILLS.to_string(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpStatus {
    Created,
    Same,
    Kept { current: String },
    Replaced { previous: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpMerge {
    pub text: Option<String>,
    pub status: McpStatus,
}

pub fn merge(format: Format, existing: Option<&str>, url: &str, explicit: bool, label: &str) -> Result<McpMerge> {
    match format {
        Format::Json { root, url_key, typed } => merge_json(existing, root, url_key, typed, url, explicit, label),
        Format::CodexToml => Ok(merge_toml(existing, url, explicit)),
    }
}

fn entry(url_key: &str, typed: bool, url: &str) -> Value {
    let mut entry = Map::new();
    if typed {
        entry.insert("type".into(), json!("http"));
    }
    entry.insert(url_key.into(), json!(url));
    Value::Object(entry)
}

fn current_url(entry: &Value) -> String {
    URL_KEYS
        .iter()
        .find_map(|k| entry.get(k).and_then(Value::as_str))
        .map(str::to_string)
        .unwrap_or_else(|| "outro tipo de servidor, sem URL".into())
}

fn merge_json(
    existing: Option<&str>,
    root_key: &str,
    url_key: &str,
    typed: bool,
    url: &str,
    explicit: bool,
    label: &str,
) -> Result<McpMerge> {
    let mut root: Value = match existing {
        Some(text) if !text.trim().is_empty() => serde_json::from_str(text.trim_start_matches('\u{feff}'))
            .map_err(|e| Error::Usage(format!("{label} não é um JSON válido ({e}); corrija e rode de novo.")))?,
        _ => json!({}),
    };
    let object = root
        .as_object_mut()
        .ok_or_else(|| Error::Usage(format!("{label} deveria conter um objeto JSON.")))?;
    let servers = object
        .entry(root_key)
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| Error::Usage(format!("{label}: \"{root_key}\" deveria ser um objeto.")))?;
    let status = match servers.get(SERVER) {
        None => McpStatus::Created,
        Some(found) if current_url(found) == url => {
            return Ok(McpMerge {
                text: None,
                status: McpStatus::Same,
            });
        }
        Some(found) if !explicit => {
            return Ok(McpMerge {
                text: None,
                status: McpStatus::Kept {
                    current: current_url(found),
                },
            });
        }
        Some(found) => McpStatus::Replaced {
            previous: current_url(found),
        },
    };
    servers.insert(SERVER.into(), entry(url_key, typed, url));
    let mut text = serde_json::to_string_pretty(&root)?;
    text.push('\n');
    Ok(McpMerge {
        text: Some(text),
        status,
    })
}

fn toml_section(line: &str) -> Option<&str> {
    let line = line.trim();
    line.strip_prefix('[')?.strip_suffix(']').map(str::trim)
}

fn is_etus_table(name: &str) -> bool {
    matches!(name, "mcp_servers.etus" | "mcp_servers.\"etus\"" | "mcp_servers.'etus'")
}

fn toml_value(line: &str, key: &str) -> Option<String> {
    let (k, v) = line.split_once('=')?;
    if k.trim() != key {
        return None;
    }
    let v = v.trim();
    let quoted = v
        .strip_prefix('"')
        .and_then(|r| r.split_once('"'))
        .or_else(|| v.strip_prefix('\'').and_then(|r| r.split_once('\'')));
    quoted.map(|(inner, _)| inner.to_string())
}

fn merge_toml(existing: Option<&str>, url: &str, explicit: bool) -> McpMerge {
    let text = existing.unwrap_or("");
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.iter().position(|l| toml_section(l).is_some_and(is_etus_table));
    let inline = lines.iter().any(|l| {
        let line = l.trim_start();
        !line.starts_with('[')
            && (line.starts_with("mcp_servers.etus") || line.split_once('=').is_some_and(|(k, _)| k.trim() == SERVER))
    });
    let Some(start) = start else {
        if inline {
            return McpMerge {
                text: None,
                status: McpStatus::Kept {
                    current: "definido em outro formato no arquivo".into(),
                },
            };
        }
        let mut out = text.to_string();
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&format!("[mcp_servers.{SERVER}]\nurl = \"{url}\"\n"));
        return McpMerge {
            text: Some(out),
            status: McpStatus::Created,
        };
    };
    let end = lines[start + 1..]
        .iter()
        .position(|l| toml_section(l).is_some())
        .map_or(lines.len(), |i| start + 1 + i);
    let url_line = (start + 1..end).find(|&i| toml_value(lines[i], "url").is_some());
    let current = url_line
        .and_then(|i| toml_value(lines[i], "url"))
        .unwrap_or_else(|| "outro tipo de servidor, sem URL".into());
    if current == url {
        return McpMerge {
            text: None,
            status: McpStatus::Same,
        };
    }
    if !explicit {
        return McpMerge {
            text: None,
            status: McpStatus::Kept { current },
        };
    }
    let new_line = format!("url = \"{url}\"");
    let mut out: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
    match url_line {
        Some(i) => out[i] = new_line,
        None => out.insert(start + 1, new_line),
    }
    let mut joined = out.join("\n");
    joined.push('\n');
    McpMerge {
        text: Some(joined),
        status: McpStatus::Replaced { previous: current },
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClaudeScopes {
    pub user: Option<String>,
    pub local: Option<String>,
}

pub fn claude_scopes(claude_json: &Value, root: &Path) -> ClaudeScopes {
    let url = |servers: Option<&Value>| servers.and_then(|s| s.get(SERVER)).map(current_url);
    let project = claude_json
        .get("projects")
        .and_then(Value::as_object)
        .and_then(|projects| {
            let wanted = root.display().to_string();
            let canonical = root.canonicalize().ok().map(|p| p.display().to_string());
            projects
                .iter()
                .find(|(path, _)| **path == wanted || Some((*path).clone()) == canonical)
                .map(|(_, value)| value)
        });
    ClaudeScopes {
        user: url(claude_json.get("mcpServers")),
        local: url(project.and_then(|p| p.get("mcpServers"))),
    }
}

pub fn router_base_url(claude_settings: &Value) -> Option<String> {
    claude_settings
        .get("env")
        .and_then(|e| e.get("ANTHROPIC_BASE_URL"))
        .and_then(Value::as_str)
        .filter(|url| !url.contains("api.anthropic.com"))
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::temp_dir;

    const URL: &str = "https://apps.etus.io/mcp?modo=quente";

    #[test]
    fn urls_carry_the_mode() {
        assert_eq!(mcp_url("https://apps.etus.io/", McpMode::Quente), URL);
        assert_eq!(mode_of(URL), "quente");
        assert_eq!(mode_of("https://apps.etus.io/mcp"), "padrão do hub");
        assert_eq!(McpMode::parse("Compacto"), Some(McpMode::Compacto));
        assert_eq!(McpMode::default(), McpMode::Quente);
    }

    #[test]
    fn each_editor_gets_its_own_shape() {
        let root = Path::new("/p");
        let home = temp_dir("editors-home");
        let shape = |t: Target| {
            let files = mcp_files(t, root, &home);
            let merged = merge(files[0].format, None, URL, false, "x").unwrap();
            (files, serde_json::from_str::<Value>(&merged.text.unwrap()).ok())
        };
        let (files, claude) = shape(Target::ClaudeCode);
        assert_eq!(files[0].path, root.join(".mcp.json"));
        assert_eq!(
            claude.unwrap()["mcpServers"]["etus"],
            json!({"type": "http", "url": URL})
        );
        let (_, cursor) = shape(Target::Cursor);
        assert_eq!(cursor.unwrap()["mcpServers"]["etus"], json!({"url": URL}));
        let (files, _) = shape(Target::Antigravity);
        assert_eq!(files.len(), 2);
        let gemini = merge(files[0].format, None, URL, false, "x").unwrap().text.unwrap();
        assert!(gemini.contains("\"httpUrl\""));
        let antigravity = merge(files[1].format, None, URL, false, "x").unwrap().text.unwrap();
        assert!(antigravity.contains("\"serverUrl\""));
        let (files, _) = shape(Target::Windsurf);
        assert_eq!(files[0].scope, Scope::User);
        assert_eq!(files[0].path, home.join(".config/devin/mcp_config.json"));
        std::fs::create_dir_all(home.join(".codeium/windsurf")).unwrap();
        let files = mcp_files(Target::Windsurf, root, &home);
        assert_eq!(files[0].path, home.join(".codeium/windsurf/mcp_config.json"));
        let codex = merge(Format::CodexToml, None, URL, false, "x").unwrap().text.unwrap();
        assert_eq!(codex, format!("[mcp_servers.etus]\nurl = \"{URL}\"\n"));
    }

    #[test]
    fn json_merge_keeps_other_servers_and_never_swaps_etus_silently() {
        let existing = r#"{"mcpServers": {"other": {"command": "x"}, "etus": {"type": "http", "url": "https://apps.etus.io/mcp?modo=completo"}}, "theme": 1}"#;
        let kept = merge(CLAUDE_JSON, Some(existing), URL, false, "f").unwrap();
        assert_eq!(kept.text, None);
        assert_eq!(
            kept.status,
            McpStatus::Kept {
                current: "https://apps.etus.io/mcp?modo=completo".into()
            }
        );
        let replaced = merge(CLAUDE_JSON, Some(existing), URL, true, "f").unwrap();
        let value: Value = serde_json::from_str(&replaced.text.unwrap()).unwrap();
        assert_eq!(value["mcpServers"]["etus"]["url"], URL);
        assert_eq!(value["mcpServers"]["other"]["command"], "x");
        assert_eq!(value["theme"], 1);
        assert!(matches!(replaced.status, McpStatus::Replaced { .. }));
        let same = merge(CLAUDE_JSON, Some(&value.to_string()), URL, true, "f").unwrap();
        assert_eq!(same.status, McpStatus::Same);
        let added = merge(CLAUDE_JSON, Some(r#"{"mcpServers": {"other": {}}}"#), URL, false, "f").unwrap();
        assert_eq!(added.status, McpStatus::Created);
        assert!(merge(CLAUDE_JSON, Some("{bad"), URL, false, "f").is_err());
        assert!(merge(CLAUDE_JSON, Some(r#"{"mcpServers": []}"#), URL, false, "f").is_err());
    }

    #[test]
    fn toml_merge_appends_or_respects_the_existing_table() {
        let existing = "model = \"gpt-5\"\n\n[mcp_servers.other]\ncommand = \"x\"\n";
        let added = merge_toml(Some(existing), URL, false);
        let text = added.text.unwrap();
        assert!(text.starts_with(existing));
        assert!(text.ends_with(&format!("\n[mcp_servers.etus]\nurl = \"{URL}\"\n")));
        let old = "[mcp_servers.etus]\nurl = \"https://apps.etus.io/mcp\"\nstartup_timeout_sec = 20\n\n[profiles.x]\nmodel = \"o3\"\n";
        let kept = merge_toml(Some(old), URL, false);
        assert_eq!(kept.text, None);
        assert!(matches!(kept.status, McpStatus::Kept { .. }));
        let replaced = merge_toml(Some(old), URL, true);
        let text = replaced.text.unwrap();
        assert!(text.contains(&format!("url = \"{URL}\"\nstartup_timeout_sec = 20")));
        assert!(text.contains("[profiles.x]\nmodel = \"o3\""));
        assert_eq!(merge_toml(Some(&text), URL, false).status, McpStatus::Same);
        let inline = "[mcp_servers]\netus = { url = \"x\" }\n";
        assert_eq!(merge_toml(Some(inline), URL, true).text, None);
    }

    #[test]
    fn shared_skills_folder_only_when_some_editor_needs_it() {
        assert!(!needs_shared_skills(&[Target::ClaudeCode]));
        assert!(!needs_shared_skills(&[
            Target::ClaudeCode,
            Target::Cursor,
            Target::Copilot
        ]));
        assert!(needs_shared_skills(&[Target::Cursor]));
        assert!(needs_shared_skills(&[Target::ClaudeCode, Target::Codex]));
        assert!(needs_shared_skills(&[Target::ClaudeCode, Target::Windsurf]));
        assert!(needs_shared_skills(&[Target::Antigravity]));
    }

    #[test]
    fn claude_scopes_and_router_are_read_from_claude_files() {
        let root = temp_dir("editors-scopes");
        let file = json!({
            "mcpServers": {"etus": {"type": "http", "url": "https://apps.etus.io/mcp"}},
            "projects": {root.display().to_string(): {"mcpServers": {"etus": {"type": "http", "url": "https://apps.etus.io/mcp?modo=completo"}}}}
        });
        let scopes = claude_scopes(&file, &root);
        assert_eq!(scopes.user.as_deref(), Some("https://apps.etus.io/mcp"));
        assert_eq!(scopes.local.as_deref(), Some("https://apps.etus.io/mcp?modo=completo"));
        assert_eq!(claude_scopes(&json!({}), &root), ClaudeScopes::default());
        assert_eq!(
            router_base_url(&json!({"env": {"ANTHROPIC_BASE_URL": "https://router.etus.io"}})).as_deref(),
            Some("https://router.etus.io")
        );
        assert_eq!(router_base_url(&json!({"env": {}})), None);
    }
}
