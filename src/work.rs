use std::fs;
use std::io::{IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use serde_json::{Value, json};

use crate::config::write_private;
use crate::error::{Error, Result};

pub const HEADER: &str = "x-etus-work";
pub const STATE_FILE: &str = "etus-work";
pub const MAX_VALUE: usize = 256;
pub const PHASES: &[&str] = &["plan", "impl", "review", "qa"];
pub const PROJECT_SOURCES: &[&str] = &["hub", "chat", "rotina"];

const NOT_KEYS: &[&str] = &[
    "utf", "iso", "sha", "md", "rfc", "cve", "ipv", "http", "https", "tls", "ssl", "x", "x64", "x86", "amd64", "arm",
    "arm64", "aarch", "win", "win32", "win64", "macos", "ios", "android", "ubuntu", "debian", "alpine", "centos",
    "rhel", "node", "nodejs", "python", "py", "java", "jdk", "go", "golang", "ruby", "php", "net", "dotnet", "es",
    "ecma", "gpt", "claude", "llama", "release", "hotfix", "rc", "beta", "alpha", "pr", "mr", "build", "version",
    "ver", "sprint", "week", "day", "part", "step", "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep",
    "oct", "nov", "dec",
];

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct WorkState {
    pub issue: Option<String>,
    pub phase: Option<String>,
    pub tag: Option<String>,
    pub workspace: Option<String>,
    pub company: Option<String>,
    pub project: Option<String>,
}

impl WorkState {
    pub fn parse(text: &str) -> WorkState {
        let mut state = WorkState::default();
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim();
            if value.is_empty() {
                continue;
            }
            let slot = match key.trim() {
                "issue" => &mut state.issue,
                "phase" => &mut state.phase,
                "tag" => &mut state.tag,
                "workspace" => &mut state.workspace,
                "company" => &mut state.company,
                "project" => &mut state.project,
                _ => continue,
            };
            *slot = Some(value.to_string());
        }
        state
    }

    pub fn render(&self) -> String {
        [
            ("issue", &self.issue),
            ("phase", &self.phase),
            ("tag", &self.tag),
            ("workspace", &self.workspace),
            ("company", &self.company),
            ("project", &self.project),
        ]
        .iter()
        .filter_map(|(key, value)| value.as_ref().map(|v| format!("{key}={v}\n")))
        .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.issue.is_none() && self.tag.is_none()
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct GitContext {
    pub top: PathBuf,
    pub git_dir: PathBuf,
    pub branch: String,
    pub repo: String,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Mapping {
    pub project: Option<String>,
    pub workspace: Option<String>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Fields {
    pub company: String,
    pub project: String,
    pub workspace: String,
    pub repo: String,
    pub plane_project: String,
    pub issue: String,
    pub tag: String,
    pub phase: String,
    pub via: String,
    pub source: String,
}

impl Fields {
    pub fn version(&self) -> &'static str {
        if self.company.is_empty() && self.project.is_empty() {
            "1"
        } else {
            "2"
        }
    }

    pub fn value(&self) -> String {
        let mut fields = self.clone();
        let mut value = fields.render();
        for field in 0..3 {
            let over = value.len().saturating_sub(MAX_VALUE);
            if over == 0 {
                break;
            }
            let slot = match field {
                0 => &mut fields.tag,
                1 => &mut fields.repo,
                _ => &mut fields.workspace,
            };
            let keep = slot.len().saturating_sub(over);
            *slot = slot[..keep].trim_end_matches(['-', '/', '.']).to_string();
            value = fields.render();
        }
        value
    }

    pub fn line(&self) -> String {
        format!("{HEADER}: {}", self.value())
    }

    fn render(&self) -> String {
        let v2 = self.version() == "2";
        let pairs = [
            ("v", self.version()),
            ("c", self.company.as_str()),
            ("pj", self.project.as_str()),
            ("ws", self.workspace.as_str()),
            ("r", self.repo.as_str()),
            ("p", self.plane_project.as_str()),
            ("i", self.issue.as_str()),
            ("t", self.tag.as_str()),
            ("a", self.phase.as_str()),
            ("via", if v2 { self.via.as_str() } else { "" }),
            ("src", self.source.as_str()),
        ];
        pairs
            .iter()
            .filter(|(_, value)| !value.is_empty())
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<_>>()
            .join(";")
    }

    pub fn to_json(&self) -> Value {
        let opt = |s: &str| if s.is_empty() { Value::Null } else { json!(s) };
        json!({
            "version": self.version(),
            "item": opt(&self.issue),
            "planeProject": opt(&self.plane_project),
            "project": opt(&self.project),
            "company": opt(&self.company),
            "tag": opt(&self.tag),
            "repo": opt(&self.repo),
            "workspace": opt(&self.workspace),
            "phase": self.phase,
            "source": self.source,
            "header": self.line(),
        })
    }
}

fn is_identifier(prefix: &str) -> bool {
    let mut chars = prefix.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_uppercase())
        && prefix.len() <= 12
        && chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
}

pub fn normalize_item(value: &str) -> Option<String> {
    let key = value.trim().to_ascii_uppercase();
    let (prefix, number) = key.split_once('-')?;
    let valid_number = !number.is_empty()
        && number.len() <= 7
        && !number.starts_with('0')
        && number.chars().all(|c| c.is_ascii_digit());
    (is_identifier(prefix) && valid_number).then_some(key)
}

pub fn normalize_phase(value: &str) -> Option<String> {
    let phase = value.trim().to_ascii_lowercase();
    PHASES.contains(&phase.as_str()).then_some(phase)
}

pub fn normalize_company(value: &str) -> Option<String> {
    let company = value.trim();
    let valid = !company.is_empty()
        && company.len() <= 32
        && company
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    valid.then(|| company.to_string())
}

pub fn normalize_project(value: &str) -> Option<String> {
    let (source, id) = value.trim().split_once(':')?;
    let source = source.to_ascii_lowercase();
    let valid = PROJECT_SOURCES.contains(&source.as_str())
        && !id.is_empty()
        && id.len() <= 64
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    valid.then(|| format!("{source}:{id}"))
}

pub fn clean_tag(value: &str) -> Option<String> {
    let mut tag = String::new();
    for c in value.to_lowercase().chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            tag.push(c);
        } else if !tag.ends_with('-') {
            tag.push('-');
        }
    }
    let tag: String = tag.trim_matches('-').chars().take(40).collect();
    let tag = tag.trim_end_matches('-').to_string();
    (!tag.is_empty()).then_some(tag)
}

fn clean_workspace(value: &str) -> String {
    value
        .trim()
        .to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'))
        .take(48)
        .collect()
}

fn clean_plane_project(value: &str) -> String {
    value
        .trim()
        .to_ascii_uppercase()
        .chars()
        .filter(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
        .take(12)
        .collect()
}

fn is_false_key(prefix: &str, number: &str, rest: &str) -> bool {
    let p = prefix.to_lowercase();
    if NOT_KEYS.contains(&p.as_str()) {
        return true;
    }
    let tail = &p[1..];
    if (p.starts_with('v') && !tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit()))
        || (p.len() == 2 && p.starts_with('q') && matches!(tail, "1" | "2" | "3" | "4"))
    {
        return true;
    }
    if number.len() > 6 {
        return true;
    }
    let mut after = rest.chars();
    number.len() == 4
        && (number.starts_with("19") || number.starts_with("20"))
        && matches!(after.next(), Some('-' | '_' | '.' | '/'))
        && matches!(after.next(), Some(c) if c.is_ascii_digit())
}

pub fn key_from_branch(branch: &str) -> Option<String> {
    let bytes = branch.as_bytes();
    let word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut i = 0;
    while i < bytes.len() {
        let boundary = i == 0 || !word(bytes[i - 1]);
        if boundary && bytes[i].is_ascii_alphabetic() {
            let mut j = i + 1;
            while j < bytes.len() && bytes[j].is_ascii_alphanumeric() {
                j += 1;
            }
            let prefix_len = j - i;
            if (2..=12).contains(&prefix_len) && j < bytes.len() && bytes[j] == b'-' {
                let mut k = j + 1;
                while k < bytes.len() && bytes[k].is_ascii_digit() {
                    k += 1;
                }
                let ends_word = k == bytes.len() || !word(bytes[k]);
                if k > j + 1 && ends_word {
                    let prefix = &branch[i..j];
                    let number = &branch[j + 1..k];
                    if !is_false_key(prefix, number, &branch[k..]) {
                        return Some(format!("{}-{number}", prefix.to_ascii_uppercase()));
                    }
                }
            }
        }
        i += 1;
    }
    None
}

pub fn normalize_remote(url: &str) -> String {
    let u = url.trim();
    if u.is_empty() || u.to_lowercase().starts_with("file:") {
        return String::new();
    }
    let (host, path) = if let Some(at) = u.find("://").filter(|&at| {
        let scheme = &u[..at];
        scheme.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))
    }) {
        let rest = u[at + 3..].rsplit('@').next().unwrap_or("");
        let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
        let host = match authority.rfind(':') {
            Some(colon) if authority[colon + 1..].chars().all(|c| c.is_ascii_digit()) => &authority[..colon],
            _ => authority,
        };
        (host.to_string(), path.to_string())
    } else {
        let rest = u.rsplit('@').next().unwrap_or("");
        let Some((host, path)) = rest.split_once(':') else {
            return String::new();
        };
        if host.is_empty() || host.len() == 1 || host.contains(['/', '\\']) || path.is_empty() {
            return String::new();
        }
        (host.to_string(), path.to_string())
    };
    let host = host.to_lowercase();
    let path = path.trim_matches('/').to_lowercase();
    let path = path.strip_suffix(".git").unwrap_or(&path).trim_matches('/').to_string();
    let host_ok = !host.is_empty()
        && host
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '-'));
    if !host_ok || path.is_empty() {
        return String::new();
    }
    let path: String = path
        .chars()
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '/' | '~' | '-'))
        .collect();
    format!("{host}/{path}").chars().take(120).collect()
}

pub fn resolve(state: &WorkState, git: Option<&GitContext>, mapping: &Mapping, via: &str) -> Fields {
    let explicit = state.issue.as_deref().and_then(normalize_item);
    let branch = git.and_then(|g| key_from_branch(&g.branch));
    let issue = explicit.clone().or(branch.clone()).unwrap_or_default();
    let tag = state.tag.as_deref().and_then(clean_tag).unwrap_or_default();
    let repo = git.map(|g| g.repo.clone()).unwrap_or_default();
    let plane_project = match issue.split_once('-') {
        Some((prefix, _)) => prefix.to_string(),
        None => clean_plane_project(mapping.project.as_deref().unwrap_or("")),
    };
    let workspace = clean_workspace(
        state
            .workspace
            .as_deref()
            .or(mapping.workspace.as_deref())
            .unwrap_or(""),
    );
    let source = if explicit.is_some() || (branch.is_none() && !tag.is_empty()) {
        "explicit"
    } else if branch.is_some() {
        "branch"
    } else if !repo.is_empty() || !plane_project.is_empty() || !workspace.is_empty() {
        "repo"
    } else {
        "none"
    };
    Fields {
        company: state.company.as_deref().and_then(normalize_company).unwrap_or_default(),
        project: state.project.as_deref().and_then(normalize_project).unwrap_or_default(),
        workspace,
        repo,
        plane_project,
        issue,
        tag,
        phase: state
            .phase
            .as_deref()
            .and_then(normalize_phase)
            .unwrap_or_else(|| "impl".into()),
        via: via.to_string(),
        source: source.to_string(),
    }
}

fn git(cwd: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

pub fn git_context(cwd: &Path) -> Option<GitContext> {
    let out = git(cwd, &["rev-parse", "--show-toplevel", "--git-dir"])?;
    let mut lines = out.lines();
    let top = PathBuf::from(lines.next()?.trim());
    let git_dir = cwd.join(lines.next()?.trim());
    let head = fs::read_to_string(git_dir.join("HEAD")).unwrap_or_default();
    let branch = head.trim().strip_prefix("ref: refs/heads/").unwrap_or("").to_string();
    let mut origin = None;
    let mut first = None;
    for line in git(cwd, &["config", "--get-regexp", r"^remote\..*\.url$"])
        .unwrap_or_default()
        .lines()
    {
        let Some((name, url)) = line.split_once(' ') else {
            continue;
        };
        let remote = name
            .strip_prefix("remote.")
            .and_then(|n| n.strip_suffix(".url"))
            .unwrap_or("");
        if remote == "origin" {
            origin.get_or_insert_with(|| url.to_string());
        }
        first.get_or_insert_with(|| url.to_string());
    }
    let repo = normalize_remote(origin.or(first).as_deref().unwrap_or(""));
    Some(GitContext {
        top,
        git_dir,
        branch,
        repo,
    })
}

pub fn read_mapping(git: Option<&GitContext>) -> Mapping {
    let Some(git) = git else {
        return Mapping::default();
    };
    let text = fs::read_to_string(git.top.join(".etus.json")).unwrap_or_default();
    let value: Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).unwrap_or(Value::Null);
    let field = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_string);
    Mapping {
        project: field("project"),
        workspace: field("workspace"),
    }
}

pub fn claude_home() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("CLAUDE_CONFIG_DIR").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    directories::BaseDirs::new().map(|base| base.home_dir().join(".claude"))
}

pub fn state_path(git: Option<&GitContext>, claude_home: Option<&Path>) -> Result<PathBuf> {
    match (git, claude_home) {
        (Some(git), _) => Ok(git.git_dir.join(STATE_FILE)),
        (None, Some(home)) => Ok(home.join(STATE_FILE)),
        (None, None) => Err(Error::Other("Não foi possível achar a pasta do Claude Code.".into())),
    }
}

pub fn load_state(path: &Path) -> WorkState {
    fs::read_to_string(path)
        .map(|text| WorkState::parse(&text))
        .unwrap_or_default()
}

pub fn save_state(path: &Path, state: &WorkState) -> Result<()> {
    write_private(path, state.render().as_bytes())
}

pub fn clear_state(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

#[derive(Debug, Default, Clone)]
pub struct Change {
    pub item: Option<String>,
    pub phase: Option<String>,
    pub project: Option<String>,
    pub company: Option<String>,
    pub tag: Option<String>,
}

impl Change {
    pub fn is_empty(&self) -> bool {
        self.item.is_none()
            && self.phase.is_none()
            && self.project.is_none()
            && self.company.is_none()
            && self.tag.is_none()
    }
}

pub fn apply(current: &WorkState, change: &Change, active_company: Option<&str>) -> Result<WorkState> {
    let item = change
        .item
        .as_deref()
        .map(|v| normalize_item(v).ok_or_else(|| usage(format!("Item inválido: {v} (exemplo: ENG-123)."))))
        .transpose()?;
    let tag = change
        .tag
        .as_deref()
        .map(|v| clean_tag(v).ok_or_else(|| usage("Etiqueta inválida: use letras, números e -.".into())))
        .transpose()?;
    let phase = change
        .phase
        .as_deref()
        .map(|v| normalize_phase(v).ok_or_else(|| usage(format!("Fase inválida: {v} (plan, impl, review ou qa)."))))
        .transpose()?;
    let project = change
        .project
        .as_deref()
        .map(|v| {
            normalize_project(v)
                .ok_or_else(|| usage(format!("Projeto inválido: {v} (hub:<id>, chat:<id> ou rotina:<id>).")))
        })
        .transpose()?;
    let company = change
        .company
        .as_deref()
        .map(|v| normalize_company(v).ok_or_else(|| usage(format!("Empresa inválida: {v} (o id da empresa no hub)."))))
        .transpose()?;
    let active = active_company.and_then(normalize_company);
    if item.is_some() || tag.is_some() {
        return Ok(WorkState {
            issue: item,
            phase,
            tag,
            workspace: current.workspace.clone(),
            company: company.or(active).or_else(|| current.company.clone()),
            project: project.or_else(|| current.project.clone()),
        });
    }
    if current.is_empty() {
        return Err(usage(
            "Nenhum trabalho marcado aqui: informe o item (etus trabalho ENG-123) ou --etiqueta.".into(),
        ));
    }
    Ok(WorkState {
        phase: phase.or_else(|| current.phase.clone()),
        company: company.or_else(|| current.company.clone()),
        project: project.or_else(|| current.project.clone()),
        ..current.clone()
    })
}

fn usage(message: String) -> Error {
    Error::Usage(message)
}

pub const HEADERS_VAR: &str = "ANTHROPIC_CUSTOM_HEADERS";

pub fn headers_of(settings: &Value) -> Option<String> {
    settings
        .get("env")
        .and_then(|e| e.get(HEADERS_VAR))
        .and_then(Value::as_str)
        .map(str::to_string)
}

pub fn inherited_headers(project_dir: &Path) -> String {
    let candidates = [
        project_dir.join(".claude").join("settings.json"),
        claude_home().map(|h| h.join("settings.json")).unwrap_or_default(),
    ];
    for path in candidates {
        if let Some(h) = fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .and_then(|json| headers_of(&json))
        {
            return h;
        }
    }
    String::new()
}

pub fn ensure_ignored(local_path: &Path, git: &GitContext) -> Result<()> {
    ensure_ignored_impl(local_path, git, None)
}

/// The canonical form of a path that may not exist yet: its nearest existing ancestor resolved, the rest appended.
fn canonical_or_nearest(path: &Path) -> PathBuf {
    let mut missing = Vec::new();
    let mut current = path;
    loop {
        if let Ok(canon) = fs::canonicalize(current) {
            return missing.iter().rev().fold(canon, |acc, part| acc.join(part));
        }
        match (current.parent(), current.file_name()) {
            (Some(parent), Some(name)) => {
                missing.push(name.to_owned());
                current = parent;
            }
            _ => return path.to_path_buf(),
        }
    }
}

fn ensure_ignored_impl(local_path: &Path, git: &GitContext, isolated_home: Option<&Path>) -> Result<()> {
    let top_canon = fs::canonicalize(&git.top).unwrap_or_else(|_| git.top.clone());
    let path_canon = canonical_or_nearest(local_path);

    let rel = match path_canon.strip_prefix(&top_canon) {
        Ok(rel) => rel.to_path_buf(),
        Err(_) => match local_path.strip_prefix(&git.top) {
            Ok(rel) => rel.to_path_buf(),
            Err(_) => return Ok(()),
        },
    };
    let rel_str = rel.to_string_lossy().replace('\\', "/");

    let mut check_cmd = Command::new("git");
    check_cmd
        .args(["check-ignore", "-q", "--", &rel_str])
        .current_dir(&git.top)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(home) = isolated_home {
        check_cmd.env("HOME", home).env("XDG_CONFIG_HOME", home);
    }

    let is_ignored = check_cmd.status().map(|s| s.success()).unwrap_or(false);
    if is_ignored {
        return Ok(());
    }

    let mut common_cmd = Command::new("git");
    common_cmd
        .args(["rev-parse", "--git-common-dir"])
        .current_dir(&git.top)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    if let Some(home) = isolated_home {
        common_cmd.env("HOME", home).env("XDG_CONFIG_HOME", home);
    }

    let common_dir = common_cmd
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| git.top.join(s.trim()))
        .unwrap_or_else(|| git.git_dir.clone());

    let info_dir = common_dir.join("info");
    fs::create_dir_all(&info_dir)?;
    let exclude_file = info_dir.join("exclude");
    let current_exclude = fs::read_to_string(&exclude_file).unwrap_or_default();
    if !current_exclude.contains(".claude/settings.local.json") {
        let mut new_content = current_exclude;
        if !new_content.ends_with('\n') && !new_content.is_empty() {
            new_content.push('\n');
        }
        new_content.push_str("**/.claude/settings.local.json\n");
        write_private(&exclude_file, new_content.as_bytes())?;
    }
    Ok(())
}

pub fn write_header(project_dir: &Path, line: &str, git: Option<&GitContext>, create: bool) -> Result<bool> {
    let local_path = project_dir.join(".claude").join("settings.local.json");
    let exists = local_path.exists();
    if !exists && !create {
        return Ok(false);
    }

    if let Some(g) = git.filter(|_| exists) {
        let is_tracked = local_path
            .strip_prefix(&g.top)
            .ok()
            .map(|rel| {
                let rel_str = rel.to_string_lossy();
                Command::new("git")
                    .args(["ls-files", "--", &rel_str])
                    .current_dir(&g.top)
                    .env("GIT_OPTIONAL_LOCKS", "0")
                    .stdin(Stdio::null())
                    .stderr(Stdio::null())
                    .output()
                    .ok()
                    .map(|o| !o.stdout.is_empty())
                    .unwrap_or(false)
            })
            .unwrap_or(false);
        if is_tracked {
            return Ok(false);
        }
    }

    let mut data: Value = if exists {
        let content = fs::read_to_string(&local_path)?;
        if content.trim().is_empty() {
            json!({})
        } else {
            serde_json::from_str(&content).map_err(|e| {
                Error::Other(format!(
                    "Configuração {} não é um JSON válido: {e}",
                    local_path.display()
                ))
            })?
        }
    } else {
        json!({})
    };

    if !data.is_object() {
        return Err(Error::Other(format!(
            "{} deveria conter um objeto JSON.",
            local_path.display()
        )));
    }

    let env = match data.get_mut("env") {
        Some(v) if v.is_object() => v,
        _ => {
            data["env"] = json!({});
            data.get_mut("env").unwrap()
        }
    };

    let current = env.get(HEADERS_VAR).and_then(Value::as_str);
    let base = current
        .map(str::to_string)
        .unwrap_or_else(|| inherited_headers(project_dir));

    let header_prefix = format!("{}:", HEADER.to_lowercase());
    let keep: Vec<&str> = base
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.to_lowercase().starts_with(&header_prefix))
        .collect();

    let new_value = if keep.is_empty() {
        line.to_string()
    } else {
        format!("{}\n{}", keep.join("\n"), line)
    };

    if current == Some(&new_value) {
        return Ok(false);
    }

    env.as_object_mut()
        .unwrap()
        .insert(HEADERS_VAR.to_string(), json!(new_value));

    let formatted = serde_json::to_string_pretty(&data)? + "\n";
    write_private(&local_path, formatted.as_bytes())?;

    if let Some(g) = git.filter(|_| !exists) {
        let _ = ensure_ignored(&local_path, g);
    }

    Ok(true)
}

pub fn current_header_line(project_dir: &Path) -> String {
    let local_path = project_dir.join(".claude").join("settings.local.json");
    let headers = fs::read_to_string(local_path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|data| headers_of(&data));

    if let Some(headers) = headers {
        let prefix = format!("{}:", HEADER.to_lowercase());
        for line in headers.lines() {
            let trimmed = line.trim();
            if trimmed.to_lowercase().starts_with(&prefix) {
                return trimmed.to_string();
            }
        }
    }
    String::new()
}

pub fn parse_header_line(line: &str) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    let body = match line.split_once(':') {
        Some((_, b)) => b.trim(),
        None => line.trim(),
    };
    for part in body.split(';') {
        if let Some((k, v)) = part.split_once('=') {
            map.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    map
}

fn do_run_hook(raw: Option<&str>, deferred: bool) -> Result<()> {
    let input = match raw {
        Some(s) if !s.trim().is_empty() => s.to_string(),
        _ => {
            if !std::io::stdin().is_terminal() {
                let mut buffer = String::new();
                let _ = std::io::stdin().read_to_string(&mut buffer);
                buffer
            } else {
                String::new()
            }
        }
    };

    let data: Value = if input.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(&input).unwrap_or_else(|_| json!({}))
    };

    let event = data.get("hook_event_name").and_then(Value::as_str).unwrap_or("");
    let cwd_val = if event == "CwdChanged" {
        data.get("new_cwd").and_then(Value::as_str)
    } else {
        None
    };
    let cwd_str = cwd_val
        .or_else(|| data.get("cwd").and_then(Value::as_str))
        .unwrap_or("");

    let cwd = if !cwd_str.is_empty() && Path::new(cwd_str).is_dir() {
        PathBuf::from(cwd_str)
    } else {
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    };

    let perm_mode = data.get("permission_mode").and_then(Value::as_str);

    let git = git_context(&cwd);
    let mapping = read_mapping(git.as_ref());
    let path = state_path(git.as_ref(), claude_home().as_deref())?;
    let state = load_state(&path);
    let mut fields = resolve(&state, git.as_ref(), &mapping, "cli");
    if state.phase.is_none() && perm_mode == Some("plan") {
        fields.phase = "plan".into();
    }

    let project_dir = if let Some(d) = std::env::var_os("CLAUDE_PROJECT_DIR").filter(|d| !d.is_empty()) {
        let p = PathBuf::from(d);
        if p.is_dir() {
            p
        } else if let Some(g) = git.as_ref() {
            g.top.clone()
        } else {
            cwd.clone()
        }
    } else if let Some(g) = git.as_ref() {
        g.top.clone()
    } else {
        cwd.clone()
    };

    let create = fields.source != "none";
    let local = project_dir.join(".claude").join("settings.local.json");

    if deferred && local.exists() {
        return Ok(());
    }

    if event == "SessionStart" && create && !deferred && !local.exists() {
        spawn_deferred(&input, &project_dir);
    } else {
        let _ = write_header(&project_dir, &fields.line(), git.as_ref(), create);
    }

    if let Some(g) = git
        .as_ref()
        .filter(|_| !deferred && matches!(event, "SessionStart" | "CwdChanged" | "FileChanged"))
    {
        let head = g.git_dir.join("HEAD");
        let out = json!({
        "suppressOutput": true,
        "hookSpecificOutput": {
                "hookEventName": event,
                "watchPaths": [head.display().to_string()]
            }
        });
        let text = serde_json::to_string(&out)?;
        let mut stdout = std::io::stdout().lock();
        let _ = stdout.write_all(text.as_bytes());
        let _ = stdout.flush();
    }

    Ok(())
}

fn spawn_deferred(raw: &str, project_dir: &Path) {
    if let Ok(exe) = std::env::current_exe() {
        let _ = Command::new(exe)
            .arg("hook-deferred")
            .arg(raw)
            .env("CLAUDE_PROJECT_DIR", project_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }
}

pub fn run_hook(raw: Option<&str>, deferred: bool) -> Result<()> {
    let _ = do_run_hook(raw, deferred);
    Ok(())
}

pub fn run_hook_deferred(raw: Option<&str>) -> Result<()> {
    std::thread::sleep(Duration::from_millis(3000));
    run_hook(raw, true)
}

fn do_run_statusline(raw: Option<&str>) -> Result<()> {
    let input = match raw {
        Some(s) if !s.trim().is_empty() => s.to_string(),
        _ => {
            if !std::io::stdin().is_terminal() {
                let mut buffer = String::new();
                let _ = std::io::stdin().read_to_string(&mut buffer);
                buffer
            } else {
                String::new()
            }
        }
    };

    let data: Value = if input.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(&input).unwrap_or_else(|_| json!({}))
    };

    let ws = data.get("workspace").and_then(Value::as_object);
    let project_dir_str = ws
        .and_then(|w| w.get("project_dir"))
        .and_then(Value::as_str)
        .or_else(|| data.get("cwd").and_then(Value::as_str));

    let project_dir = match project_dir_str {
        Some(s) if !s.is_empty() && Path::new(s).is_dir() => PathBuf::from(s),
        _ => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
    };

    let current = current_header_line(&project_dir);
    let fields_map = parse_header_line(&current);

    let (issue, plane_project, repo, tag, phase) = if !fields_map.is_empty() {
        (
            fields_map.get("i").cloned().unwrap_or_default(),
            fields_map.get("p").cloned().unwrap_or_default(),
            fields_map.get("r").cloned().unwrap_or_default(),
            fields_map.get("t").cloned().unwrap_or_default(),
            fields_map.get("a").cloned().unwrap_or_default(),
        )
    } else {
        let git = git_context(&project_dir);
        let path = state_path(git.as_ref(), claude_home().as_deref()).ok();
        let state = path.map(|p| load_state(&p)).unwrap_or_default();
        let mapping = read_mapping(git.as_ref());
        let f = resolve(&state, git.as_ref(), &mapping, "cli");
        (f.issue, f.plane_project, f.repo, f.tag, f.phase)
    };

    let model = data
        .get("model")
        .and_then(|m| m.get("display_name"))
        .and_then(Value::as_str)
        .unwrap_or("");

    let mut label = if !issue.is_empty() {
        issue
    } else if !plane_project.is_empty() {
        plane_project
    } else if let Some((_, r)) = repo.rsplit_once('/') {
        r.to_string()
    } else {
        repo
    };

    if !tag.is_empty() {
        if label.is_empty() {
            label = format!("#{tag}");
        } else {
            label = format!("{label} #{tag}");
        }
    }

    let work = if !label.is_empty() {
        if !phase.is_empty() {
            format!("{label} {phase}")
        } else {
            label
        }
    } else {
        "no work tag".to_string()
    };

    let output = if !model.is_empty() {
        format!("{model} | {work}")
    } else {
        work
    };

    let mut stdout = std::io::stdout().lock();
    let _ = stdout.write_all(output.as_bytes());
    let _ = stdout.flush();

    Ok(())
}

pub fn run_statusline(raw: Option<&str>) -> Result<()> {
    let _ = do_run_statusline(raw);
    Ok(())
}

pub fn refresh(cwd: &Path) -> Result<()> {
    let git = git_context(cwd);
    let mut targets = Vec::new();
    if let Some(g) = git.as_ref() {
        targets.push(g.top.clone());
    }
    let local_settings = cwd.join(".claude").join("settings.local.json");
    if local_settings.exists() && !targets.contains(&cwd.to_path_buf()) {
        targets.push(cwd.to_path_buf());
    }

    for target in targets {
        let current = current_header_line(&target);
        let parsed = parse_header_line(&current);
        let mode = if parsed.get("a").map(String::as_str) == Some("plan") {
            Some("plan")
        } else {
            None
        };
        let mapping = read_mapping(git.as_ref());
        let path = match state_path(git.as_ref(), claude_home().as_deref()) {
            Ok(p) => p,
            Err(_) => continue,
        };
        let state = load_state(&path);
        let mut fields = resolve(&state, git.as_ref(), &mapping, "cli");
        if state.phase.is_none() && mode == Some("plan") {
            fields.phase = "plan".into();
        }
        let create = fields.source != "none";
        let _ = write_header(&target, &fields.line(), git.as_ref(), create);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::temp_dir;

    const AT: char = '\u{40}';

    fn fields() -> Fields {
        Fields {
            repo: "github.com/etusdigital/crm".into(),
            plane_project: "ENG".into(),
            issue: "ENG-123".into(),
            phase: "impl".into(),
            via: "cli".into(),
            source: "explicit".into(),
            ..Fields::default()
        }
    }

    #[test]
    fn items_are_validated_and_uppercased() {
        assert_eq!(normalize_item(" eng-123 ").as_deref(), Some("ENG-123"));
        assert_eq!(normalize_item("A1-9999999").as_deref(), Some("A1-9999999"));
        assert_eq!(normalize_item("ENG-0"), None);
        assert_eq!(normalize_item("ENG-012"), None);
        assert_eq!(normalize_item("1ENG-1"), None);
        assert_eq!(normalize_item("ENG-12345678"), None);
        assert_eq!(normalize_item("ABCDEFGHIJKLM-1"), None);
        assert_eq!(normalize_item("ENG"), None);
    }

    #[test]
    fn projects_companies_and_phases_are_validated() {
        assert_eq!(normalize_project("HUB:prj_7Hs-2").as_deref(), Some("hub:prj_7Hs-2"));
        assert_eq!(normalize_project("rotina:rt1").as_deref(), Some("rotina:rt1"));
        assert_eq!(normalize_project("plane:ENG"), None);
        assert_eq!(normalize_project("hub:"), None);
        assert_eq!(normalize_project("hub:a b"), None);
        assert_eq!(
            normalize_company("Xk3p9Qa2bLmN0rTu").as_deref(),
            Some("Xk3p9Qa2bLmN0rTu")
        );
        assert_eq!(normalize_company(&"a".repeat(33)), None);
        assert_eq!(normalize_company("etus;c=x"), None);
        assert_eq!(normalize_phase("Review").as_deref(), Some("review"));
        assert_eq!(normalize_phase("deploy"), None);
        assert_eq!(clean_tag("Estudo de Cache!").as_deref(), Some("estudo-de-cache"));
        assert_eq!(clean_tag("!!!"), None);
    }

    #[test]
    fn header_is_v1_without_company_or_project() {
        assert_eq!(
            fields().line(),
            "x-etus-work: v=1;r=github.com/etusdigital/crm;p=ENG;i=ENG-123;a=impl;src=explicit"
        );
    }

    #[test]
    fn header_is_v2_in_canonical_order_with_company_or_project() {
        let f = Fields {
            company: "Xk3p9Qa2bLmN0rTu".into(),
            project: "hub:prj7".into(),
            workspace: "etus".into(),
            tag: "cliente-x".into(),
            ..fields()
        };
        assert_eq!(
            f.value(),
            "v=2;c=Xk3p9Qa2bLmN0rTu;pj=hub:prj7;ws=etus;r=github.com/etusdigital/crm;p=ENG;i=ENG-123;t=cliente-x;a=impl;via=cli;src=explicit"
        );
        let only_project = Fields {
            project: "chat:abc".into(),
            ..fields()
        };
        assert!(only_project.value().starts_with("v=2;pj=chat:abc;"));
    }

    #[test]
    fn long_header_trims_tag_repo_and_workspace_only() {
        let f = Fields {
            company: "Xk3p9Qa2bLmN0rTu".into(),
            project: format!("hub:{}", "p".repeat(64)),
            workspace: "w".repeat(48),
            repo: format!("github.com/{}", "r".repeat(100)),
            tag: "t".repeat(40),
            ..fields()
        };
        let value = f.value();
        assert!(value.len() <= MAX_VALUE, "{}", value.len());
        assert!(value.contains(&format!("pj=hub:{}", "p".repeat(64))));
        assert!(value.contains("c=Xk3p9Qa2bLmN0rTu"));
        assert!(value.contains("i=ENG-123"));
        assert!(!value.contains(";t="));
    }

    #[test]
    fn branch_keys_skip_false_positives() {
        assert_eq!(key_from_branch("feat/eng-123-upload").as_deref(), Some("ENG-123"));
        assert_eq!(key_from_branch("utf-8-fix-crm-742").as_deref(), Some("CRM-742"));
        assert_eq!(key_from_branch("release-2026-10-14"), None);
        assert_eq!(key_from_branch("v2-10"), None);
        assert_eq!(key_from_branch("q3-2"), None);
        assert_eq!(key_from_branch("main"), None);
        assert_eq!(key_from_branch("ab-1234567"), None);
    }

    #[test]
    fn remotes_are_normalized() {
        assert_eq!(
            normalize_remote(&format!("git{AT}github.com:etusdigital/CRM.git")),
            "github.com/etusdigital/crm"
        );
        assert_eq!(
            normalize_remote(&format!("https://user:pw{AT}gitlab.com:443/grupo/sub/app.git")),
            "gitlab.com/grupo/sub/app"
        );
        assert_eq!(
            normalize_remote(&format!("ssh://git{AT}github.com/etusdigital/bfp")),
            "github.com/etusdigital/bfp"
        );
        assert_eq!(normalize_remote("file:///tmp/repo"), "");
        assert_eq!(normalize_remote("C:\\repo"), "");
        assert_eq!(normalize_remote(""), "");
    }

    #[test]
    fn resolve_follows_etus_py_sources() {
        let git = GitContext {
            branch: "feat/crm-742-x".into(),
            repo: "github.com/etusdigital/bfp".into(),
            ..GitContext::default()
        };
        let from_branch = resolve(&WorkState::default(), Some(&git), &Mapping::default(), "cli");
        assert_eq!(
            (from_branch.issue.as_str(), from_branch.source.as_str()),
            ("CRM-742", "branch")
        );
        let explicit = WorkState {
            issue: Some("bfp-12".into()),
            phase: Some("qa".into()),
            ..WorkState::default()
        };
        let f = resolve(&explicit, Some(&git), &Mapping::default(), "cli");
        assert_eq!(
            (
                f.issue.as_str(),
                f.plane_project.as_str(),
                f.phase.as_str(),
                f.source.as_str()
            ),
            ("BFP-12", "BFP", "qa", "explicit")
        );
        let repo_only = GitContext {
            branch: "main".into(),
            ..git.clone()
        };
        let mapping = Mapping {
            project: Some("crm".into()),
            workspace: Some("Etus".into()),
        };
        let f = resolve(&WorkState::default(), Some(&repo_only), &mapping, "cli");
        assert_eq!(
            (f.plane_project.as_str(), f.workspace.as_str(), f.source.as_str()),
            ("CRM", "etus", "repo")
        );
        let tagged = WorkState {
            tag: Some("estudo".into()),
            ..WorkState::default()
        };
        assert_eq!(
            resolve(&tagged, None, &Mapping::default(), "cli").line(),
            "x-etus-work: v=1;t=estudo;a=impl;src=explicit"
        );
        assert_eq!(
            resolve(&WorkState::default(), None, &Mapping::default(), "cli").line(),
            "x-etus-work: v=1;a=impl;src=none"
        );
    }

    #[test]
    fn apply_sets_updates_and_refuses() {
        let current = WorkState {
            issue: Some("OLD-1".into()),
            phase: Some("plan".into()),
            workspace: Some("etus".into()),
            project: Some("hub:prj1".into()),
            ..WorkState::default()
        };
        let set = apply(
            &current,
            &Change {
                item: Some("eng-123".into()),
                ..Change::default()
            },
            Some("org1"),
        )
        .unwrap();
        assert_eq!(set.issue.as_deref(), Some("ENG-123"));
        assert_eq!(set.phase, None);
        assert_eq!(set.workspace.as_deref(), Some("etus"));
        assert_eq!(set.company.as_deref(), Some("org1"));
        assert_eq!(set.project.as_deref(), Some("hub:prj1"));
        let explicit_company = apply(
            &current,
            &Change {
                item: Some("ENG-1".into()),
                company: Some("org2".into()),
                ..Change::default()
            },
            Some("org1"),
        )
        .unwrap();
        assert_eq!(explicit_company.company.as_deref(), Some("org2"));
        let phase = apply(
            &current,
            &Change {
                phase: Some("review".into()),
                ..Change::default()
            },
            Some("org1"),
        )
        .unwrap();
        assert_eq!(
            (phase.issue.as_deref(), phase.phase.as_deref(), phase.company),
            (Some("OLD-1"), Some("review"), None)
        );
        assert!(
            apply(
                &WorkState::default(),
                &Change {
                    phase: Some("qa".into()),
                    ..Change::default()
                },
                None
            )
            .is_err()
        );
        assert!(
            apply(
                &current,
                &Change {
                    item: Some("nope".into()),
                    ..Change::default()
                },
                None
            )
            .is_err()
        );
        assert!(
            apply(
                &current,
                &Change {
                    item: Some("ENG-1".into()),
                    project: Some("x:y".into()),
                    ..Change::default()
                },
                None
            )
            .is_err()
        );
    }

    #[test]
    fn state_round_trips_and_keeps_python_keys() {
        let dir = temp_dir("work-state");
        let path = dir.join(STATE_FILE);
        assert_eq!(load_state(&path), WorkState::default());
        let state = WorkState {
            issue: Some("ENG-123".into()),
            phase: Some("impl".into()),
            tag: None,
            workspace: Some("etus".into()),
            company: Some("org1".into()),
            project: Some("hub:prj1".into()),
        };
        save_state(&path, &state).unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "issue=ENG-123\nphase=impl\nworkspace=etus\ncompany=org1\nproject=hub:prj1\n"
        );
        assert_eq!(load_state(&path), state);
        assert_eq!(
            WorkState::parse("issue=CRM-1\nunknown=x\n tag = a \nphase=\n")
                .tag
                .as_deref(),
            Some("a")
        );
        clear_state(&path).unwrap();
        clear_state(&path).unwrap();
        assert!(!path.exists());
        assert_eq!(state_path(None, Some(&dir)).unwrap(), dir.join(STATE_FILE));
    }

    #[test]
    fn git_context_reads_worktree_branch_and_origin() {
        if Command::new("git").arg("--version").output().is_err() {
            return;
        }
        let dir = temp_dir("work-git");
        let run = |args: &[&str]| {
            let ok = Command::new("git")
                .args(args)
                .current_dir(&dir)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .unwrap();
            assert!(ok.success(), "git {args:?}");
        };
        run(&["init", "-q", "-b", "feat/eng-42-upload"]);
        let origin = format!("git{AT}github.com:etusdigital/Bfp.git");
        run(&["remote", "add", "origin", &origin]);
        fs::write(dir.join(".etus.json"), r#"{"workspace":"etus","project":"bfp"}"#).unwrap();
        let sub = dir.join("src");
        fs::create_dir_all(&sub).unwrap();
        let git = git_context(&sub).unwrap();
        assert_eq!(git.branch, "feat/eng-42-upload");
        assert_eq!(git.repo, "github.com/etusdigital/bfp");
        let path = state_path(Some(&git), None).unwrap();
        assert_eq!(
            fs::canonicalize(path.parent().unwrap()).unwrap(),
            fs::canonicalize(dir.join(".git")).unwrap()
        );
        let mapping = read_mapping(Some(&git));
        let f = resolve(
            &WorkState {
                company: Some("org1".into()),
                ..WorkState::default()
            },
            Some(&git),
            &mapping,
            "cli",
        );
        assert_eq!(
            f.line(),
            "x-etus-work: v=2;c=org1;ws=etus;r=github.com/etusdigital/bfp;p=ENG;i=ENG-42;a=impl;via=cli;src=branch"
        );
    }

    #[test]
    fn write_header_creates_and_updates_settings_local() {
        let dir = temp_dir("work-header");
        let line1 = "x-etus-work: v=1;i=ENG-100;a=impl;src=explicit";
        let created = write_header(&dir, line1, None, true).unwrap();
        assert!(created);

        let local_path = dir.join(".claude").join("settings.local.json");
        assert!(local_path.exists());
        assert_eq!(current_header_line(&dir), line1);

        let map = parse_header_line(&current_header_line(&dir));
        assert_eq!(map.get("i").map(String::as_str), Some("ENG-100"));
        assert_eq!(map.get("a").map(String::as_str), Some("impl"));

        // Add a custom non-etus header to simulate user custom headers
        let mut data: Value = serde_json::from_str(&fs::read_to_string(&local_path).unwrap()).unwrap();
        data["env"]["ANTHROPIC_CUSTOM_HEADERS"] = json!(format!("x-custom-foo: bar\n{line1}"));
        fs::write(&local_path, serde_json::to_string(&data).unwrap()).unwrap();

        // Update with line 2
        let line2 = "x-etus-work: v=1;i=ENG-200;a=review;src=explicit";
        let updated = write_header(&dir, line2, None, true).unwrap();
        assert!(updated);

        let final_text = fs::read_to_string(&local_path).unwrap();
        let final_data: Value = serde_json::from_str(&final_text).unwrap();
        let headers = headers_of(&final_data).unwrap();
        assert!(headers.contains("x-custom-foo: bar"));
        assert!(headers.contains(line2));
        assert!(!headers.contains(line1));
    }

    #[test]
    fn ensure_ignored_appends_to_git_exclude() {
        if Command::new("git").arg("--version").output().is_err() {
            return;
        }
        let dir = temp_dir("work-exclude");
        let _ = Command::new("git").args(["init", "-q"]).current_dir(&dir).status();

        let git = git_context(&dir).unwrap();
        let fake_home = temp_dir("fake-home");
        let local_path = dir.join(".claude").join("settings.local.json");
        ensure_ignored_impl(&local_path, &git, Some(&fake_home)).unwrap();

        let exclude = git.git_dir.join("info").join("exclude");
        assert!(exclude.exists());
        let content = fs::read_to_string(&exclude).unwrap();
        assert!(content.contains("**/.claude/settings.local.json"));
    }

    #[test]
    fn write_header_fails_on_corrupt_json() {
        let dir = temp_dir("corrupt-work");
        let claude_dir = dir.join(".claude");
        fs::create_dir_all(&claude_dir).unwrap();
        let local_path = claude_dir.join("settings.local.json");
        fs::write(&local_path, "{ corrupt json").unwrap();

        let res = write_header(&dir, "x-etus-work: v=1;i=ENG-1", None, false);
        assert!(res.is_err());
        // Verify original file was not modified or overwritten
        assert_eq!(fs::read_to_string(&local_path).unwrap(), "{ corrupt json");
    }

    #[test]
    fn refresh_updates_settings_local_json() {
        if Command::new("git").arg("--version").output().is_err() {
            return;
        }
        let dir = temp_dir("refresh-test");
        let _ = Command::new("git").args(["init", "-q"]).current_dir(&dir).status();
        let git = git_context(&dir).unwrap();
        let state_file = git.git_dir.join(STATE_FILE);
        let state = WorkState {
            issue: Some("ENG-999".into()),
            phase: Some("impl".into()),
            company: Some("org_x".into()),
            ..Default::default()
        };
        save_state(&state_file, &state).unwrap();

        // Refresh updates settings.local.json
        refresh(&dir).unwrap();

        let local_path = dir.join(".claude").join("settings.local.json");
        assert!(local_path.exists());
        let text = fs::read_to_string(&local_path).unwrap();
        assert!(text.contains("ENG-999"));
        assert!(text.contains("org_x"));
    }

    #[test]
    fn do_run_hook_handles_event_cleanly() {
        let dir = temp_dir("hook-test");
        let raw_payload = json!({
            "hook_event_name": "UserPromptSubmit",
            "cwd": dir.to_str().unwrap()
        })
        .to_string();

        let res = do_run_hook(Some(&raw_payload), false);
        assert!(res.is_ok());
    }

    #[test]
    fn do_run_statusline_handles_json() {
        let raw_payload = json!({
            "cwd": "."
        })
        .to_string();

        let res = do_run_statusline(Some(&raw_payload));
        assert!(res.is_ok());
    }
}
