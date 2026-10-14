use std::collections::BTreeMap;
use std::fs;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::{Paths, write_private};
use crate::error::Result;
use crate::naming::{app_names, is_builtin, is_pt_br, kebab, slug};

pub const SEPARATOR: &str = "__";
const UNPREFIXED: &str = "geral";

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Annotations {
    #[serde(default, rename = "readOnlyHint", skip_serializing_if = "Option::is_none")]
    pub read_only_hint: Option<bool>,
    #[serde(default, rename = "destructiveHint", skip_serializing_if = "Option::is_none")]
    pub destructive_hint: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Tool {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, rename = "inputSchema")]
    pub input_schema: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<Annotations>,
}

impl Tool {
    pub fn prefix(&self) -> &str {
        self.name.split_once(SEPARATOR).map(|(p, _)| p).unwrap_or(UNPREFIXED)
    }

    pub fn short_name(&self) -> &str {
        self.name.split_once(SEPARATOR).map(|(_, n)| n).unwrap_or(&self.name)
    }

    pub fn command_name(&self) -> String {
        kebab(self.short_name())
    }

    pub fn display_title(&self) -> Option<&str> {
        self.title
            .as_deref()
            .or_else(|| self.annotations.as_ref().and_then(|a| a.title.as_deref()))
    }

    pub fn pt_name(&self) -> Option<String> {
        self.display_title()
            .filter(|t| is_pt_br(t))
            .map(slug)
            .filter(|s| !s.is_empty())
    }

    fn title_slug(&self) -> Option<String> {
        self.display_title().map(slug).filter(|s| !s.is_empty())
    }

    pub fn read_only(&self) -> bool {
        self.annotations
            .as_ref()
            .and_then(|a| a.read_only_hint)
            .unwrap_or(false)
    }

    pub fn destructive(&self) -> bool {
        let hint = self.annotations.as_ref().and_then(|a| a.destructive_hint);
        match hint {
            Some(value) => value,
            None => !self.read_only(),
        }
    }

    pub fn writes(&self) -> bool {
        !self.read_only() || self.annotations.as_ref().and_then(|a| a.destructive_hint) == Some(true)
    }

    pub fn app_name(&self) -> Option<&str> {
        let description = self.description.as_deref()?;
        let (head, _) = description.split_once(": ")?;
        let head = head.trim();
        let plausible = !head.is_empty() && head.len() <= 40 && !head.contains(['.', '\n', '(', ',']);
        plausible.then_some(head)
    }

    pub fn description_body(&self) -> &str {
        let description = self.description.as_deref().unwrap_or("").trim();
        match self.app_name() {
            Some(name) => description[name.len()..].trim_start_matches(':').trim(),
            None => description,
        }
    }

    pub fn summary(&self) -> String {
        let body = self.description_body();
        let first = body
            .split_terminator(". ")
            .next()
            .unwrap_or(body)
            .lines()
            .next()
            .unwrap_or("");
        let first = first.trim_end_matches('.');
        let text = if first.is_empty() {
            self.display_title().unwrap_or("")
        } else {
            first
        };
        truncate(text, SUMMARY_WIDTH)
    }
}

pub const SUMMARY_WIDTH: usize = 80;
pub const ELLIPSIS: char = '\u{2026}';

pub fn truncate(text: &str, max: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max {
        return text.to_string();
    }
    let head: String = text.chars().take(max.saturating_sub(1)).collect();
    let next_is_space = text.chars().nth(head.chars().count()).is_some_and(char::is_whitespace);
    let cut = if next_is_space {
        head.as_str()
    } else {
        match head.rfind(char::is_whitespace) {
            Some(pos) if pos > 0 => &head[..pos],
            _ => head.as_str(),
        }
    };
    let cut = cut.trim_end_matches(|c: char| c.is_whitespace() || matches!(c, ',' | ';' | ':' | '.' | '(' | '-'));
    format!("{cut}{ELLIPSIS}")
}

#[derive(Debug, Clone, PartialEq)]
pub struct AppCommand {
    pub name: String,
    pub aliases: Vec<String>,
    pub tool: Tool,
}

impl AppCommand {
    pub fn matches(&self, word: &str) -> bool {
        self.name == word || self.aliases.iter().any(|a| a == word)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct App {
    pub word: String,
    pub prefix: String,
    pub name: String,
    pub aliases: Vec<String>,
    pub commands: Vec<AppCommand>,
}

impl App {
    pub fn matches(&self, word: &str) -> bool {
        self.word == word || self.aliases.iter().any(|a| a == word)
    }

    pub fn find_command(&self, word: &str) -> Option<&AppCommand> {
        self.commands
            .iter()
            .find(|c| c.name == word)
            .or_else(|| self.commands.iter().find(|c| c.matches(word)))
    }

    pub fn tools(&self) -> impl Iterator<Item = &Tool> {
        self.commands.iter().map(|c| &c.tool)
    }
}

fn claim(claimed: &mut Vec<String>, name: String) -> String {
    claimed.push(name.clone());
    name
}

pub fn name_commands(tools: Vec<Tool>) -> Vec<AppCommand> {
    let english: Vec<String> = tools.iter().map(Tool::command_name).collect();
    let mut order: Vec<usize> = (0..tools.len()).collect();
    order.sort_by(|a, b| tools[*a].name.cmp(&tools[*b].name));
    let mut names: Vec<Option<String>> = vec![None; tools.len()];
    let mut claimed: Vec<String> = Vec::new();
    for &i in &order {
        let Some(pt) = tools[i].pt_name() else { continue };
        let owned_elsewhere = english.iter().enumerate().any(|(j, e)| j != i && *e == pt);
        if !owned_elsewhere && !claimed.contains(&pt) {
            names[i] = Some(claim(&mut claimed, pt));
        }
    }
    for &i in &order {
        if names[i].is_some() {
            continue;
        }
        let base = if english[i].is_empty() {
            "comando"
        } else {
            english[i].as_str()
        };
        let mut name = base.to_string();
        let mut n = 2;
        while claimed.contains(&name) {
            name = format!("{base}-{n}");
            n += 1;
        }
        names[i] = Some(claim(&mut claimed, name));
    }
    let mut aliases: Vec<Vec<String>> = vec![Vec::new(); tools.len()];
    for &i in &order {
        for candidate in [Some(english[i].clone()), tools[i].title_slug()].into_iter().flatten() {
            if !candidate.is_empty() && !claimed.contains(&candidate) {
                aliases[i].push(claim(&mut claimed, candidate));
            }
        }
    }
    tools
        .into_iter()
        .zip(names)
        .zip(aliases)
        .map(|((tool, name), aliases)| AppCommand {
            name: name.unwrap_or_default(),
            aliases,
            tool,
        })
        .collect()
}

pub fn group(tools: &[Tool]) -> Vec<App> {
    let mut by_prefix: BTreeMap<String, Vec<Tool>> = BTreeMap::new();
    for tool in tools {
        by_prefix
            .entry(tool.prefix().to_string())
            .or_default()
            .push(tool.clone());
    }
    let prefixes: Vec<String> = by_prefix.keys().cloned().collect();
    let foreign_prefix = |word: &str, own: &str| word != own && prefixes.iter().any(|p| p == word);
    let drafts: Vec<(String, String, Vec<String>, Vec<Tool>)> = by_prefix
        .into_iter()
        .map(|(prefix, tools)| {
            let name = common_app_name(&tools);
            let names = app_names(&prefix, name.as_deref());
            (prefix.clone(), name.unwrap_or(prefix), names, tools)
        })
        .collect();
    let mut claimed: Vec<String> = Vec::new();
    let mut words: Vec<String> = Vec::new();
    for (prefix, _, names, _) in &drafts {
        let own = names.iter().position(|n| n == prefix).unwrap_or(names.len());
        let word = names
            .iter()
            .take(own + 1)
            .find(|n| !is_builtin(n) && !claimed.contains(n) && !foreign_prefix(n, prefix))
            .cloned()
            .unwrap_or_else(|| format!("{prefix}-app"));
        words.push(claim(&mut claimed, word));
    }
    drafts
        .into_iter()
        .zip(words)
        .map(|((prefix, name, names, tools), word)| {
            let aliases = names
                .into_iter()
                .filter(|n| !is_builtin(n) && !claimed.contains(n) && !foreign_prefix(n, &prefix))
                .collect::<Vec<_>>();
            claimed.extend(aliases.iter().cloned());
            App {
                word,
                prefix,
                name,
                aliases,
                commands: name_commands(tools),
            }
        })
        .collect()
}

fn common_app_name(tools: &[Tool]) -> Option<String> {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for name in tools.iter().filter_map(Tool::app_name) {
        *counts.entry(name).or_default() += 1;
    }
    let (name, count) = counts.into_iter().max_by_key(|(_, count)| *count)?;
    (count * 2 >= tools.len()).then(|| name.to_string())
}

pub fn find_app<'a>(apps: &'a [App], word: &str) -> Option<&'a App> {
    let exact = |w: &str| {
        apps.iter()
            .find(|a| a.word == w)
            .or_else(|| apps.iter().find(|a| a.matches(w)))
    };
    exact(word).or_else(|| exact(&slug(word)))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Company {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CatalogCache {
    pub fetched_at: u64,
    pub hub: String,
    pub sub: String,
    #[serde(default)]
    pub perfil: Option<String>,
    #[serde(default)]
    pub company: Option<Company>,
    pub tools: Vec<Tool>,
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl CatalogCache {
    pub fn is_fresh(&self, ttl: Duration, now: u64) -> bool {
        now.saturating_sub(self.fetched_at) < ttl.as_secs()
    }

    pub fn apps(&self) -> Vec<App> {
        group(&self.tools)
    }

    pub fn find_tool(&self, name: &str) -> Option<&Tool> {
        self.tools.iter().find(|t| t.name == name)
    }
}

pub fn load_cache(paths: &Paths, hub: &str, perfil: Option<&str>) -> Option<CatalogCache> {
    let text = fs::read_to_string(paths.catalog_file(hub, perfil)).ok()?;
    let cache: CatalogCache = serde_json::from_str(&text).ok()?;
    (cache.hub == hub && cache.perfil.as_deref() == perfil).then_some(cache)
}

pub fn save_cache(paths: &Paths, cache: &CatalogCache) -> Result<()> {
    let path = paths.catalog_file(&cache.hub, cache.perfil.as_deref());
    write_private(&path, serde_json::to_string(cache)?.as_bytes())
}

pub fn clear_caches(paths: &Paths) -> Result<()> {
    let Ok(entries) = fs::read_dir(&paths.cache_dir) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if name.to_string_lossy().starts_with("catalog-") {
            fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tool(value: Value) -> Tool {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn groups_tools_under_pt_br_app_words() {
        let tools = vec![
            tool(
                json!({"name": "tasks__list_tasks", "title": "Listar tarefas", "description": "Tarefas: Lists tasks. More.", "inputSchema": {}, "annotations": {"readOnlyHint": true}}),
            ),
            tool(json!({"name": "records__get_meeting", "description": "Reuniões: One meeting.", "inputSchema": {}})),
            tool(
                json!({"name": "tasks__create_task", "title": "Create task", "description": "Tarefas: Creates a task.", "inputSchema": {}}),
            ),
            tool(
                json!({"name": "muraldereconheci__give", "description": "Mural de Reconhecimento: Gives kudos.", "inputSchema": {}}),
            ),
        ];
        let apps = group(&tools);
        let words: Vec<&str> = apps.iter().map(|a| a.word.as_str()).collect();
        assert_eq!(words, vec!["mural", "reunioes", "tarefas"]);
        assert_eq!(apps[1].name, "Reuniões");
        assert_eq!(apps[1].aliases, vec!["records", "meetings", "gravador"]);
        assert_eq!(apps[2].aliases, vec!["tasks"]);
        for word in ["tarefas", "tasks", "Tarefas", "reuniões", "mural-de-reconhecimento"] {
            assert!(find_app(&apps, word).is_some(), "{word}");
        }
        let tasks = &apps[2];
        let list = tasks.find_command("listar-tarefas").unwrap();
        assert_eq!(list.name, "listar-tarefas");
        assert_eq!(list.aliases, vec!["list-tasks"]);
        assert_eq!(tasks.find_command("list-tasks").unwrap().tool.name, "tasks__list_tasks");
        let create = tasks.find_command("create-task").unwrap();
        assert_eq!(create.name, "create-task");
        assert!(create.aliases.is_empty());
        assert_eq!(list.tool.summary(), "Lists tasks");
    }

    #[test]
    fn command_names_are_unique_and_stable() {
        let tools = vec![
            tool(json!({"name": "a__z_second", "title": "Ver tarefa", "inputSchema": {}})),
            tool(json!({"name": "a__a_first", "title": "Ver tarefa", "inputSchema": {}})),
            tool(json!({"name": "a__ver_projeto", "inputSchema": {}})),
            tool(json!({"name": "a__other", "title": "Ver projeto", "inputSchema": {}})),
        ];
        let commands = name_commands(tools.clone());
        let named: Vec<(&str, &str)> = commands
            .iter()
            .map(|c| (c.tool.short_name(), c.name.as_str()))
            .collect();
        assert_eq!(
            named,
            vec![
                ("z_second", "z-second"),
                ("a_first", "ver-tarefa"),
                ("ver_projeto", "ver-projeto"),
                ("other", "other")
            ]
        );
        let mut reversed = tools;
        reversed.reverse();
        let again = name_commands(reversed);
        for command in &commands {
            let same = again.iter().find(|c| c.tool.name == command.tool.name).unwrap();
            assert_eq!(same.name, command.name);
            assert_eq!(same.aliases, command.aliases);
        }
        let mut all: Vec<&String> = commands
            .iter()
            .flat_map(|c| std::iter::once(&c.name).chain(&c.aliases))
            .collect();
        let total = all.len();
        all.sort();
        all.dedup();
        assert_eq!(all.len(), total);
    }

    #[test]
    fn app_words_never_shadow_builtins_or_other_prefixes() {
        let tools = vec![
            tool(json!({"name": "config__x", "inputSchema": {}})),
            tool(json!({"name": "tarefas__y", "inputSchema": {}})),
            tool(json!({"name": "tasks__z", "description": "Tarefas: z.", "inputSchema": {}})),
        ];
        let apps = group(&tools);
        let words: Vec<&str> = apps.iter().map(|a| a.word.as_str()).collect();
        assert_eq!(words, vec!["config-app", "tarefas", "tasks"]);
        assert!(apps[0].aliases.is_empty());
    }

    #[test]
    fn truncates_at_word_boundaries() {
        assert_eq!(truncate("short text", 20), "short text");
        assert_eq!(
            truncate("Lists the tasks of one view in the board", 20),
            "Lists the tasks of\u{2026}"
        );
        assert_eq!(truncate("Lists, tasks, everywhere", 14), "Lists, tasks\u{2026}");
        assert_eq!(truncate("Supercalifragilistic", 10), "Supercali\u{2026}");
        assert_eq!(truncate("one two", 4), "one\u{2026}");
    }

    #[test]
    fn write_detection_follows_hints() {
        let read = tool(json!({"name": "a__b", "annotations": {"readOnlyHint": true, "destructiveHint": false}}));
        let write = tool(json!({"name": "a__c", "annotations": {"readOnlyHint": false, "destructiveHint": false}}));
        let destructive =
            tool(json!({"name": "a__d", "annotations": {"readOnlyHint": false, "destructiveHint": true}}));
        let odd = tool(json!({"name": "a__e", "annotations": {"readOnlyHint": true, "destructiveHint": true}}));
        let bare = tool(json!({"name": "a__f"}));
        assert!(!read.writes());
        assert!(write.writes() && !write.destructive());
        assert!(destructive.writes() && destructive.destructive());
        assert!(odd.writes());
        assert!(bare.writes() && bare.destructive());
    }

    #[test]
    fn app_name_is_only_taken_from_a_short_prefix() {
        let named = tool(json!({"name": "a__b", "description": "Etus Design: Creates a project."}));
        let plain = tool(json!({"name": "a__b", "description": "Lists things: all of them."}));
        let sentence = tool(json!({"name": "a__b", "description": "Lists things, really: yes."}));
        assert_eq!(named.app_name(), Some("Etus Design"));
        assert_eq!(named.description_body(), "Creates a project.");
        assert_eq!(plain.app_name(), Some("Lists things"));
        assert_eq!(sentence.app_name(), None);
    }

    #[test]
    fn cache_freshness() {
        let cache = CatalogCache {
            fetched_at: 100,
            hub: "h".into(),
            sub: "s".into(),
            perfil: None,
            company: None,
            tools: vec![],
        };
        assert!(cache.is_fresh(Duration::from_secs(300), 399));
        assert!(!cache.is_fresh(Duration::from_secs(300), 400));
    }
}
