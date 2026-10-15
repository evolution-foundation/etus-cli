use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

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
}
