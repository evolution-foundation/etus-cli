use std::collections::BTreeMap;
use std::fs;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::{Paths, write_private};
use crate::error::Result;
use crate::naming::{app_aliases, kebab, slug};

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

    pub fn title_alias(&self) -> Option<String> {
        self.display_title()
            .map(slug)
            .filter(|s| !s.is_empty() && *s != self.command_name())
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
        truncate(text, 90)
    }
}

pub fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let cut: String = text.chars().take(max - 3).collect();
    format!("{}...", cut.trim_end())
}

#[derive(Debug, Clone, PartialEq)]
pub struct App {
    pub prefix: String,
    pub name: String,
    pub aliases: Vec<String>,
    pub tools: Vec<Tool>,
}

impl App {
    pub fn matches(&self, word: &str) -> bool {
        self.prefix == word || self.aliases.iter().any(|a| a == word)
    }

    pub fn find_command(&self, command: &str) -> Option<&Tool> {
        self.tools.iter().find(|t| t.command_name() == command)
    }
}

pub fn group(tools: &[Tool]) -> Vec<App> {
    let mut by_prefix: BTreeMap<String, Vec<Tool>> = BTreeMap::new();
    for tool in tools {
        by_prefix
            .entry(tool.prefix().to_string())
            .or_default()
            .push(tool.clone());
    }
    let mut apps: Vec<App> = by_prefix
        .into_iter()
        .map(|(prefix, tools)| {
            let name = common_app_name(&tools).unwrap_or_else(|| prefix.clone());
            let aliases = app_aliases(&prefix, Some(&name));
            App {
                prefix,
                name,
                aliases,
                tools,
            }
        })
        .collect();
    let taken: Vec<String> = apps.iter().map(|a| a.prefix.clone()).collect();
    let mut seen: Vec<String> = Vec::new();
    for app in &mut apps {
        app.aliases.retain(|alias| {
            let free = !taken.contains(alias) && !seen.contains(alias);
            if free {
                seen.push(alias.clone());
            }
            free
        });
    }
    apps
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
    apps.iter()
        .find(|a| a.prefix == word)
        .or_else(|| apps.iter().find(|a| a.matches(word)))
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
    fn groups_tools_by_prefix_with_names_and_aliases() {
        let tools = vec![
            tool(
                json!({"name": "tasks__list_tasks", "title": "Listar tarefas", "description": "Tarefas: Lists tasks. More.", "inputSchema": {}, "annotations": {"readOnlyHint": true}}),
            ),
            tool(json!({"name": "records__get_meeting", "description": "Reuniões: One meeting.", "inputSchema": {}})),
            tool(json!({"name": "tasks__create_task", "description": "Tarefas: Creates a task.", "inputSchema": {}})),
        ];
        let apps = group(&tools);
        assert_eq!(apps.len(), 2);
        assert_eq!(apps[0].prefix, "records");
        assert_eq!(apps[0].name, "Reuniões");
        assert_eq!(apps[0].aliases, vec!["reunioes", "gravador"]);
        assert_eq!(apps[1].prefix, "tasks");
        assert_eq!(apps[1].tools.len(), 2);
        assert!(find_app(&apps, "tarefas").is_some());
        assert_eq!(apps[1].find_command("create-task").unwrap().name, "tasks__create_task");
        assert_eq!(apps[1].tools[0].title_alias().as_deref(), Some("listar-tarefas"));
        assert_eq!(apps[1].tools[0].summary(), "Lists tasks");
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
