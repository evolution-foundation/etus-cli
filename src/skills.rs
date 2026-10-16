use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::catalog::{App, AppCommand, Tool, UNPREFIXED, truncate};
use crate::error::Result;
use crate::schema::{self, Param};

pub const MARKER: &str = "<!-- gerado por etus skills sincronizar: não edite, rode o comando de novo -->";
pub const ACTIVATE_MARKER: &str = "<!-- gerado por etus ativar: não edite, rode o comando de novo -->";
pub const RESERVED_SKILL: &str = "etus-cli";
pub const DETAIL_TOOL: &str = "etus_detalhar";
pub const DETAIL_BATCH: usize = 10;
pub const SKILL_FILE: &str = "SKILL.md";
pub const SKILL_PREFIX: &str = "etus-";
const COMPACT_ABOVE: usize = 60;
const DESCRIPTION_MAX: usize = 600;
const PURPOSE_MAX: usize = 400;
const MAX_ENUM_VALUES: usize = 8;
const MAX_TITLE_HINTS: usize = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum SkillAgent {
    Claude,
    Codex,
    Todos,
}

#[derive(Debug, Clone)]
pub struct Locations {
    pub home: PathBuf,
    pub claude_home: PathBuf,
    pub project_root: PathBuf,
}

pub fn target_dirs(agent: SkillAgent, project: bool, folder: Option<&Path>, at: &Locations) -> Vec<PathBuf> {
    if let Some(folder) = folder {
        return vec![folder.to_path_buf()];
    }
    let claude = if project {
        at.project_root.join(".claude").join("skills")
    } else {
        at.claude_home.join("skills")
    };
    let codex = if project {
        at.project_root.join(".agents").join("skills")
    } else {
        at.home.join(".agents").join("skills")
    };
    match agent {
        SkillAgent::Claude => vec![claude],
        SkillAgent::Codex => vec![codex],
        SkillAgent::Todos => vec![claude, codex],
    }
}

pub fn skill_name(word: &str) -> String {
    let name = format!("{SKILL_PREFIX}{word}");
    if name == RESERVED_SKILL {
        format!("{SKILL_PREFIX}app-{word}")
    } else {
        name
    }
}

pub fn hub_host(hub: &str) -> String {
    let host = hub.trim().trim_start_matches("https://").trim_start_matches("http://");
    host.split('/').next().unwrap_or(host).to_string()
}

pub fn detail_names(apps: &[App]) -> Vec<String> {
    apps.iter()
        .filter(|a| a.prefix != UNPREFIXED)
        .map(|a| format!("app:{}", a.prefix))
        .collect()
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Recipe {
    pub title: String,
    pub steps: Vec<String>,
    pub tools: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Guide {
    pub purpose: Option<String>,
    pub recipes: Vec<Recipe>,
    pub gotchas: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct AppDetail {
    pub key: Option<String>,
    pub category: Option<String>,
    pub description: Option<String>,
    pub guide: Option<Guide>,
    pub permissions: BTreeMap<String, String>,
}

fn text_of(value: &Value) -> Option<String> {
    value.as_str().map(clean).filter(|text| !text.is_empty())
}

fn texts_of(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|items| items.iter().filter_map(text_of).collect())
        .unwrap_or_default()
}

fn parse_guide(value: &Value) -> Option<Guide> {
    if !value.is_object() {
        return None;
    }
    let recipes = value
        .get("receitas")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    Some(Recipe {
                        title: text_of(item.get("titulo")?)?,
                        steps: texts_of(item.get("passos").unwrap_or(&Value::Null)),
                        tools: item
                            .get("ferramentas")
                            .and_then(Value::as_array)
                            .map(|t| t.iter().filter_map(Value::as_str).map(str::to_string).collect())
                            .unwrap_or_default(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Some(Guide {
        purpose: value.get("proposito").and_then(text_of),
        recipes,
        gotchas: texts_of(value.get("cuidados").unwrap_or(&Value::Null)),
    })
}

pub fn parse_details(payload: &Value) -> BTreeMap<String, AppDetail> {
    let mut details = BTreeMap::new();
    let Some(entries) = payload.get("ferramentas").and_then(Value::as_array) else {
        return details;
    };
    for entry in entries {
        if entry.get("erro").is_some() {
            continue;
        }
        let app = entry.get("app");
        let prefix = app
            .and_then(|a| a.get("prefixo"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                entry
                    .get("nome")
                    .and_then(Value::as_str)
                    .and_then(|n| n.strip_prefix("app:"))
                    .map(str::to_string)
            });
        let Some(prefix) = prefix else { continue };
        if app.is_none() && entry.get("tipo").and_then(Value::as_str) != Some("app") {
            continue;
        }
        let permissions = entry
            .get("ferramentas")
            .and_then(Value::as_array)
            .map(|tools| {
                tools
                    .iter()
                    .filter_map(|t| {
                        let name = t.get("nome")?.as_str()?;
                        let permission = t.get("permissao")?.as_str()?.trim();
                        (!permission.is_empty()).then(|| (name.to_string(), permission.to_string()))
                    })
                    .collect()
            })
            .unwrap_or_default();
        details.insert(
            prefix,
            AppDetail {
                key: app.and_then(|a| a.get("chave")).and_then(text_of),
                category: app.and_then(|a| a.get("categoria")).and_then(text_of),
                description: app.and_then(|a| a.get("descricao")).and_then(text_of),
                guide: entry.get("guia").and_then(parse_guide),
                permissions,
            },
        );
    }
    details
}

pub fn clean(text: &str) -> String {
    let replaced = text
        .replace(" \u{2014} ", ", ")
        .replace(" \u{2013} ", ", ")
        .replace(['\u{2014}', '\u{2013}'], "-");
    replaced.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn sentence(text: &str) -> String {
    let text = text.trim();
    if text.is_empty() || text.ends_with(['.', '!', '?', '\u{2026}', ':']) {
        text.to_string()
    } else {
        format!("{text}.")
    }
}

fn yaml_quoted(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Generator {
    Sync,
    Activate,
}

impl Generator {
    pub fn marker(self) -> &'static str {
        match self {
            Generator::Sync => MARKER,
            Generator::Activate => ACTIVATE_MARKER,
        }
    }

    pub fn command(self) -> &'static str {
        match self {
            Generator::Sync => "etus skills sincronizar",
            Generator::Activate => "etus ativar",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Origin {
    pub company: Option<String>,
    pub hub_host: String,
    pub by: Generator,
}

fn purpose(detail: Option<&AppDetail>) -> Option<String> {
    let detail = detail?;
    detail
        .guide
        .as_ref()
        .and_then(|g| g.purpose.clone())
        .or_else(|| detail.description.clone())
}

fn sorted_commands(app: &App) -> Vec<&AppCommand> {
    let mut commands: Vec<&AppCommand> = app.commands.iter().collect();
    commands.sort_by(|a, b| a.name.cmp(&b.name));
    commands
}

fn description(app: &App, purpose: Option<&str>) -> String {
    let name = clean(&app.name);
    let what = match purpose {
        Some(text) => format!(": {}", sentence(&truncate(text, PURPOSE_MAX))),
        None => {
            let titles: Vec<String> = sorted_commands(app)
                .iter()
                .filter_map(|c| c.tool.display_title().map(clean))
                .filter(|t| !t.is_empty())
                .take(MAX_TITLE_HINTS)
                .collect();
            if titles.is_empty() {
                ".".to_string()
            } else {
                format!(", como {}.", titles.join(", "))
            }
        }
    };
    let text = format!(
        "Use quando a pessoa pedir algo do app {name} da plataforma Etus{what} Feito pelo terminal com `etus {}`.",
        app.word
    );
    truncate(&text, DESCRIPTION_MAX)
}

fn access_label(tool: &Tool) -> &'static str {
    if !tool.writes() {
        "Leitura"
    } else if tool.destructive() {
        "Altera, destrutiva"
    } else {
        "Altera"
    }
}

fn arg_type(param: &Param) -> String {
    let mut text = param.kind.label();
    if !param.enum_values.is_empty() {
        let mut values: Vec<String> = param
            .enum_values
            .iter()
            .take(MAX_ENUM_VALUES)
            .map(|v| match v {
                Value::String(s) => clean(s),
                other => other.to_string(),
            })
            .collect();
        if param.enum_values.len() > MAX_ENUM_VALUES {
            values.push("\u{2026}".into());
        }
        text.push_str(&format!(": {}", values.join(", ")));
    }
    if let Some(format) = &param.format {
        text.push_str(&format!(", formato {format}"));
    }
    text
}

fn command_entry(command: &AppCommand, permission: Option<&str>, compact: bool) -> Vec<String> {
    let tool = &command.tool;
    let mut head = format!("- `{}`", command.name);
    if !command.aliases.is_empty() {
        head.push_str(&format!(" (também `{}`)", command.aliases.join("`, `")));
    }
    let summary = clean(&tool.summary());
    if !summary.is_empty() {
        head.push_str(&format!(": {}", sentence(&summary)));
    }
    head.push_str(&format!(" {}.", access_label(tool)));
    if let Some(permission) = permission {
        head.push_str(&format!(" Permissão `{permission}`."));
    }
    let mut lines = vec![head];
    let params = schema::params(&tool.input_schema);
    let required: Vec<String> = params
        .iter()
        .filter(|p| p.required)
        .map(|p| format!("`--{}` ({})", p.flag, arg_type(p)))
        .collect();
    if !required.is_empty() {
        lines.push(format!("  Obrigatórios: {}.", required.join(", ")));
    }
    let optional: Vec<String> = params
        .iter()
        .filter(|p| !p.required)
        .map(|p| format!("`--{}`", p.flag))
        .collect();
    if !compact && !optional.is_empty() {
        lines.push(format!("  Opcionais: {}.", optional.join(", ")));
    }
    lines
}

fn command_lookup(apps: &[App]) -> BTreeMap<String, String> {
    apps.iter()
        .flat_map(|app| {
            app.commands
                .iter()
                .map(move |c| (c.tool.name.clone(), format!("etus {} {}", app.word, c.name)))
        })
        .collect()
}

fn recipe_lines(recipe: &Recipe, lookup: &BTreeMap<String, String>) -> Option<Vec<String>> {
    let commands: Option<Vec<String>> = recipe
        .tools
        .iter()
        .map(|tool| lookup.get(tool).map(|c| format!("`{c}`")))
        .collect();
    let commands = commands?;
    let mut lines = vec![String::new(), format!("### {}", recipe.title)];
    for (n, step) in recipe.steps.iter().enumerate() {
        lines.push(format!("{}. {}", n + 1, sentence(step)));
    }
    if !commands.is_empty() {
        lines.push(format!("Comandos: {}.", commands.join(", ")));
    }
    Some(lines)
}

pub fn render(app: &App, detail: Option<&AppDetail>, apps: &[App], origin: &Origin) -> String {
    let name = skill_name(&app.word);
    let app_name = clean(&app.name);
    let purpose = purpose(detail);
    let compact = app.commands.len() > COMPACT_ABOVE;
    let company = origin
        .company
        .as_deref()
        .map(clean)
        .filter(|c| !c.is_empty())
        .unwrap_or_else(|| "a empresa ativa".into());
    let mut lines = vec![
        "---".to_string(),
        format!("name: {name}"),
        format!("description: {}", yaml_quoted(&description(app, purpose.as_deref()))),
        "---".to_string(),
        origin.by.marker().to_string(),
        String::new(),
        format!("# {app_name} pelo etus"),
        String::new(),
        format!(
            "Gerada para {company} em {}; rode `{}` depois de trocar de empresa ou ganhar acesso.",
            origin.hub_host,
            origin.by.command()
        ),
    ];
    if let Some(purpose) = &purpose {
        lines.push(String::new());
        lines.push(sentence(purpose));
    }
    lines.push(String::new());
    lines.push("## Como chamar".into());
    lines.push(String::new());
    lines.push(format!(
        "`etus {} <comando> [flags] --json`. Parâmetros completos, exemplo e outras formas aceitas: `etus {} <comando> --help`.",
        app.word, app.word
    ));
    if !app.aliases.is_empty() {
        lines.push(format!(
            "O app também atende por {}.",
            app.aliases
                .iter()
                .map(|a| format!("`etus {a}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    lines
        .push("Login, empresa ativa, saída JSON e códigos de saída valem para todos os apps: skill `etus-cli`.".into());
    lines.push(String::new());
    lines.push("## Comandos".into());
    lines.push(String::new());
    if compact {
        lines.push(format!(
            "São {} comandos; aqui só os parâmetros obrigatórios. Os opcionais estão no `--help` de cada um.",
            app.commands.len()
        ));
        lines.push(String::new());
    }
    for command in sorted_commands(app) {
        let permission = detail
            .and_then(|d| d.permissions.get(&command.tool.name))
            .map(String::as_str);
        lines.extend(command_entry(command, permission, compact));
    }
    let lookup = command_lookup(apps);
    let guide = detail.and_then(|d| d.guide.as_ref());
    let recipes: Vec<Vec<String>> = guide
        .map(|g| g.recipes.iter().filter_map(|r| recipe_lines(r, &lookup)).collect())
        .unwrap_or_default();
    if !recipes.is_empty() {
        lines.push(String::new());
        lines.push("## Receitas".into());
        for recipe in recipes {
            lines.extend(recipe);
        }
    }
    lines.push(String::new());
    lines.push("## Cuidados".into());
    lines.push(String::new());
    for gotcha in guide.map(|g| g.gotchas.as_slice()).unwrap_or_default() {
        lines.push(format!("- {}", sentence(gotcha)));
    }
    lines.extend(
        [
            "- Comando que altera dados pede `--yes`: só passe depois do ok da pessoa para aquela ação. Em comando destrutivo, nunca sem a confirmação explícita dela.",
            "- `--dry-run` mostra a chamada sem executar, bom para conferir os argumentos antes de pedir o ok.",
            "- Código 5 é ação necessária: mostre à pessoa o link do stderr. Use `--open` ou `--wait` só com ela por perto para clicar.",
            "- Código 3: rode `etus login`. Código 4: sem permissão para o comando; a pessoa pede acesso a um admin no hub.",
        ]
        .map(str::to_string),
    );
    lines.push(format!(
        "- Comando que não existe mais ou parâmetro diferente do daqui: rode `etus atualizar` e `{}`.",
        origin.by.command()
    ));
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

pub fn desired(apps: &[App], details: &BTreeMap<String, AppDetail>, origin: &Origin) -> BTreeMap<String, String> {
    apps.iter()
        .map(|app| {
            (
                skill_name(&app.word),
                render(app, details.get(&app.prefix), apps, origin),
            )
        })
        .collect()
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Plan {
    pub create: Vec<String>,
    pub update: Vec<String>,
    pub unchanged: Vec<String>,
    pub remove: Vec<String>,
    pub skipped: Vec<String>,
}

impl Plan {
    pub fn changes(&self) -> usize {
        self.create.len() + self.update.len() + self.remove.len()
    }
}

enum Existing {
    Missing,
    Foreign,
    Owned(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    SkillFolder,
    MarkdownFile,
}

impl Layout {
    pub fn path(self, dir: &Path, name: &str) -> PathBuf {
        match self {
            Layout::SkillFolder => dir.join(name).join(SKILL_FILE),
            Layout::MarkdownFile => dir.join(format!("{name}.md")),
        }
    }

    fn entry_name(self, file_name: &str) -> Option<String> {
        match self {
            Layout::SkillFolder => Some(file_name.to_string()),
            Layout::MarkdownFile => file_name.strip_suffix(".md").map(str::to_string),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Ownership<'a> {
    pub layout: Layout,
    pub update: &'a [&'a str],
    pub remove: &'a [&'a str],
}

impl<'a> Ownership<'a> {
    pub fn folders(markers: &'a [&'a str]) -> Self {
        Ownership {
            layout: Layout::SkillFolder,
            update: markers,
            remove: markers,
        }
    }
}

fn existing(dir: &Path, name: &str, markers: &[&str], layout: Layout) -> Existing {
    if layout == Layout::SkillFolder {
        match fs::symlink_metadata(dir.join(name)) {
            Err(_) => return Existing::Missing,
            Ok(meta) if !meta.is_dir() => return Existing::Foreign,
            Ok(_) => {}
        }
    }
    let file = layout.path(dir, name);
    match fs::symlink_metadata(&file) {
        Err(_) => Existing::Missing,
        Ok(meta) if !meta.is_file() => Existing::Foreign,
        Ok(_) => match fs::read_to_string(&file) {
            Ok(text) if markers.iter().any(|m| text.contains(m)) => Existing::Owned(text),
            _ => Existing::Foreign,
        },
    }
}

pub fn plan(dir: &Path, desired: &BTreeMap<String, String>, markers: &[&str]) -> Plan {
    plan_with(dir, desired, Ownership::folders(markers))
}

pub fn plan_with(dir: &Path, desired: &BTreeMap<String, String>, own: Ownership) -> Plan {
    let mut plan = Plan::default();
    for (name, content) in desired {
        match existing(dir, name, own.update, own.layout) {
            Existing::Missing => plan.create.push(name.clone()),
            Existing::Foreign => plan.skipped.push(name.clone()),
            Existing::Owned(text) if text == *content => plan.unchanged.push(name.clone()),
            Existing::Owned(_) => plan.update.push(name.clone()),
        }
    }
    let mut stale = BTreeSet::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let Some(name) = own.layout.entry_name(&entry.file_name().to_string_lossy()) else {
                continue;
            };
            let candidate = name.starts_with(SKILL_PREFIX) && name != RESERVED_SKILL && !desired.contains_key(&name);
            if candidate && matches!(existing(dir, &name, own.remove, own.layout), Existing::Owned(_)) {
                stale.insert(name);
            }
        }
    }
    plan.remove = stale.into_iter().collect();
    plan
}

pub fn apply(dir: &Path, desired: &BTreeMap<String, String>, plan: &Plan) -> Result<()> {
    apply_with(dir, desired, plan, Layout::SkillFolder)
}

pub fn apply_with(dir: &Path, desired: &BTreeMap<String, String>, plan: &Plan, layout: Layout) -> Result<()> {
    for name in plan.create.iter().chain(&plan.update) {
        let file = layout.path(dir, name);
        if let Some(parent) = file.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(file, &desired[name])?;
    }
    for name in &plan.remove {
        fs::remove_file(layout.path(dir, name))?;
        if layout == Layout::SkillFolder {
            let _ = fs::remove_dir(dir.join(name));
        }
    }
    Ok(())
}

pub fn global_dirs(at: &Locations) -> Vec<PathBuf> {
    target_dirs(SkillAgent::Todos, false, None, at)
}

pub fn synced_skills(dir: &Path) -> Vec<String> {
    plan(dir, &BTreeMap::new(), &[MARKER]).remove
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::group;
    use crate::testutil::temp_dir;
    use serde_json::json;

    fn tools() -> Vec<Tool> {
        serde_json::from_value(json!([
            {"name": "tasks__list_tasks", "title": "Listar tarefas", "description": "Tarefas: Lists tasks of one view. More text.",
             "inputSchema": {"type": "object", "properties": {
                "view": {"type": "string", "enum": ["inbox", "today"]},
                "today": {"type": "string", "format": "date"},
                "projectId": {"type": "string"}}, "required": ["view", "today"]},
             "annotations": {"readOnlyHint": true}},
            {"name": "tasks__create_task", "title": "Criar tarefa", "description": "Tarefas: Creates a task \u{2014} quickly.",
             "inputSchema": {"type": "object", "properties": {"title": {"type": "string"}, "dueDate": {"type": "string"}}, "required": ["title"]},
             "annotations": {"readOnlyHint": false, "destructiveHint": false}},
            {"name": "tasks__delete_task", "title": "Apagar tarefa", "description": "Tarefas: Deletes a task.",
             "inputSchema": {"type": "object", "properties": {"taskId": {"type": "string"}}, "required": ["taskId"]},
             "annotations": {"readOnlyHint": false, "destructiveHint": true}},
            {"name": "records__get_meeting", "title": "Ver reunião", "description": "Reuniões: One meeting.",
             "inputSchema": {"type": "object", "properties": {}}, "annotations": {"readOnlyHint": true}}
        ]))
        .unwrap()
    }

    fn origin() -> Origin {
        Origin {
            company: Some("Etus".into()),
            hub_host: "apps.etus.io".into(),
            by: Generator::Sync,
        }
    }

    fn detail_payload() -> Value {
        json!({"ferramentas": [
            {"nome": "app:tasks", "tipo": "app",
             "app": {"chave": "nexus-tasks", "nome": "Tarefas", "prefixo": "tasks", "descricao": "Tarefas da empresa.", "categoria": null},
             "guia": {"proposito": "Organiza as tarefas da empresa \u{2014} por projeto.",
                      "receitas": [
                        {"titulo": "Criar tarefa para alguém", "passos": ["Ache a pessoa", "Crie a tarefa"], "ferramentas": ["tasks__create_task"]},
                        {"titulo": "Sem acesso", "passos": ["x"], "ferramentas": ["tasks__secret"]}],
                      "cuidados": ["dueDate é só data"]},
             "ferramentas": [
                {"nome": "tasks__create_task", "titulo": "Criar tarefa", "altera": true, "destrutiva": false, "permissao": "tasks.create"},
                {"nome": "tasks__list_tasks", "titulo": "Listar tarefas", "altera": false, "destrutiva": false, "permissao": "tasks.read"}]},
            {"nome": "app:records", "erro": "Ferramenta não encontrada."}
        ]})
    }

    #[test]
    fn parses_app_details_and_skips_errors() {
        let details = parse_details(&detail_payload());
        assert_eq!(details.keys().collect::<Vec<_>>(), vec!["tasks"]);
        let tasks = &details["tasks"];
        assert_eq!(tasks.description.as_deref(), Some("Tarefas da empresa."));
        assert_eq!(tasks.key.as_deref(), Some("nexus-tasks"));
        assert_eq!(tasks.category, None);
        assert_eq!(tasks.permissions["tasks__create_task"], "tasks.create");
        let guide = tasks.guide.as_ref().unwrap();
        assert_eq!(
            guide.purpose.as_deref(),
            Some("Organiza as tarefas da empresa, por projeto.")
        );
        assert_eq!(guide.recipes.len(), 2);
        assert!(parse_details(&json!("texto")).is_empty());
        assert!(parse_details(&json!({"ferramentas": [{"nome": "tasks__x", "titulo": "x"}]})).is_empty());
    }

    #[test]
    fn renders_a_skill_with_the_guide() {
        let apps = group(&tools());
        let tasks = apps.iter().find(|a| a.prefix == "tasks").unwrap();
        let details = parse_details(&detail_payload());
        let text = render(tasks, details.get("tasks"), &apps, &origin());
        assert!(text.starts_with("---\nname: etus-tarefas\ndescription: \"Use quando a pessoa pedir algo do app Tarefas da plataforma Etus: Organiza as tarefas da empresa, por projeto. Feito pelo terminal com `etus tarefas`.\"\n---\n"));
        assert!(text.contains(&format!("---\n{MARKER}\n")));
        assert!(text.contains("Gerada para Etus em apps.etus.io"));
        assert!(text.contains(
            "- `listar-tarefas` (também `list-tasks`): Lists tasks of one view. Leitura. Permissão `tasks.read`."
        ));
        assert!(text.contains("  Obrigatórios: `--view` (texto: inbox, today), `--today` (texto, formato date)."));
        assert!(text.contains("  Opcionais: `--project-id`."));
        assert!(text.contains("Creates a task, quickly. Altera. Permissão `tasks.create`."));
        assert!(text.contains("Deletes a task. Altera, destrutiva.\n"));
        assert!(text.contains(
            "### Criar tarefa para alguém\n1. Ache a pessoa.\n2. Crie a tarefa.\nComandos: `etus tarefas criar-tarefa`."
        ));
        assert!(!text.contains("Sem acesso"));
        assert!(text.contains("- dueDate é só data."));
        assert!(text.contains("skill `etus-cli`"));
        assert!(!text.contains(['\u{2014}', '\u{2013}']));
        assert!(text.ends_with(".\n"));
    }

    #[test]
    fn renders_without_guide_from_tools_alone() {
        let apps = group(&tools());
        let records = apps.iter().find(|a| a.prefix == "records").unwrap();
        let text = render(
            records,
            None,
            &apps,
            &Origin {
                company: None,
                hub_host: "h".into(),
                by: Generator::Activate,
            },
        );
        assert!(text.contains("description: \"Use quando a pessoa pedir algo do app Reuniões da plataforma Etus, como Ver reunião. Feito pelo terminal com `etus reunioes`.\""));
        assert!(text.contains("Gerada para a empresa ativa em h; rode `etus ativar`"));
        assert!(text.contains(&format!("---\n{ACTIVATE_MARKER}\n")));
        assert!(text.contains("rode `etus atualizar` e `etus ativar`."));
        assert!(text.contains("- `ver-reuniao` (também `get-meeting`): One meeting. Leitura.\n"));
        assert!(!text.contains("Permissão"));
        assert!(!text.contains("## Receitas"));
    }

    #[test]
    fn large_apps_list_only_required_arguments() {
        let many: Vec<Tool> = (0..61)
            .map(|n| {
                serde_json::from_value(json!({"name": format!("big__tool_{n:02}"), "description": "Big: Does it.",
                    "inputSchema": {"type": "object", "properties": {"id": {"type": "string"}, "extra": {"type": "integer"}}, "required": ["id"]}}))
                .unwrap()
            })
            .collect();
        let apps = group(&many);
        let text = render(&apps[0], None, &apps, &origin());
        assert!(text.contains("São 61 comandos"));
        assert!(text.contains("Obrigatórios: `--id` (texto)."));
        assert!(!text.contains("Opcionais"));
    }

    #[test]
    fn names_reserve_the_general_skill() {
        assert_eq!(skill_name("tarefas"), "etus-tarefas");
        assert_eq!(skill_name("cli"), "etus-app-cli");
        assert_eq!(hub_host("https://apps.etus.io/"), "apps.etus.io");
        assert_eq!(hub_host("http://localhost:5011"), "localhost:5011");
        let apps = group(&tools());
        assert_eq!(detail_names(&apps), vec!["app:records", "app:tasks"]);
    }

    #[test]
    fn target_dirs_follow_agent_and_scope() {
        let at = Locations {
            home: PathBuf::from("/h"),
            claude_home: PathBuf::from("/h/.claude"),
            project_root: PathBuf::from("/repo"),
        };
        assert_eq!(
            target_dirs(SkillAgent::Claude, false, None, &at),
            vec![PathBuf::from("/h/.claude/skills")]
        );
        assert_eq!(
            target_dirs(SkillAgent::Codex, false, None, &at),
            vec![PathBuf::from("/h/.agents/skills")]
        );
        assert_eq!(
            target_dirs(SkillAgent::Todos, true, None, &at),
            vec![
                PathBuf::from("/repo/.claude/skills"),
                PathBuf::from("/repo/.agents/skills")
            ]
        );
        assert_eq!(
            target_dirs(SkillAgent::Todos, true, Some(Path::new("/x")), &at),
            vec![PathBuf::from("/x")]
        );
    }

    #[test]
    fn plan_is_idempotent_and_removes_only_marked_skills() {
        let dir = temp_dir("skills-plan");
        let apps = group(&tools());
        let details = parse_details(&detail_payload());
        let wanted = desired(&apps, &details, &origin());
        let first = plan(&dir, &wanted, &[MARKER]);
        assert_eq!(first.create, vec!["etus-reunioes", "etus-tarefas"]);
        apply(&dir, &wanted, &first).unwrap();
        let again = plan(&dir, &wanted, &[MARKER]);
        assert_eq!(again.changes(), 0);
        assert_eq!(again.unchanged.len(), 2);

        let marked = format!("---\nname: etus-old\n---\n{MARKER}\n");
        for (name, body) in [
            ("etus-old", marked.as_str()),
            ("etus-cli", marked.as_str()),
            ("etus-mine", "---\nname: etus-mine\n---\nfeita à mão\n"),
            ("other-old", marked.as_str()),
        ] {
            fs::create_dir_all(dir.join(name)).unwrap();
            fs::write(dir.join(name).join(SKILL_FILE), body).unwrap();
        }
        fs::write(
            dir.join("etus-reunioes").join(SKILL_FILE),
            format!("{MARKER}\nantigo\n"),
        )
        .unwrap();
        fs::write(dir.join("etus-tarefas").join(SKILL_FILE), "feita à mão\n").unwrap();

        let next = plan(&dir, &wanted, &[MARKER]);
        assert_eq!(next.update, vec!["etus-reunioes"]);
        assert_eq!(next.skipped, vec!["etus-tarefas"]);
        assert_eq!(next.remove, vec!["etus-old"]);
        apply(&dir, &wanted, &next).unwrap();
        assert!(!dir.join("etus-old").exists());
        for kept in ["etus-cli", "etus-mine", "other-old"] {
            assert!(dir.join(kept).join(SKILL_FILE).exists(), "{kept}");
        }
        assert_eq!(
            fs::read_to_string(dir.join("etus-tarefas").join(SKILL_FILE)).unwrap(),
            "feita à mão\n"
        );
        assert_eq!(
            fs::read_to_string(dir.join("etus-reunioes").join(SKILL_FILE)).unwrap(),
            wanted["etus-reunioes"]
        );

        let empty = plan(&dir, &BTreeMap::new(), &[MARKER]);
        assert_eq!(empty.remove, vec!["etus-reunioes"]);
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_skills_are_never_removed() {
        let dir = temp_dir("skills-link");
        let outside = temp_dir("skills-outside");
        fs::write(outside.join(SKILL_FILE), format!("{MARKER}\n")).unwrap();
        std::os::unix::fs::symlink(&outside, dir.join("etus-linked")).unwrap();
        let result = plan(&dir, &BTreeMap::new(), &[MARKER]);
        assert!(result.remove.is_empty());
        let wanted = BTreeMap::from([("etus-linked".to_string(), "x".to_string())]);
        assert_eq!(plan(&dir, &wanted, &[MARKER]).skipped, vec!["etus-linked"]);
    }
}
