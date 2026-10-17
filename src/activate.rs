use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use crate::action;
use crate::catalog::{App, SEPARATOR, find_app, truncate};
use crate::cli;
use crate::commands::{SKILLS_READ_TOOL, fetch_details, skill_locations};
use crate::context::Ctx;
use crate::editors::needs_shared_skills;
use crate::error::{Error, Result};
use crate::mcp::{first_text, refusal};
use crate::naming::slug;
use crate::output::payload;
use crate::setup::{ProjectConfig, Target};
use crate::skills::{
    self, ACTIVATE_MARKER, AppDetail, Generator, Layout, Locations, MARKER, Origin, Ownership, Plan, SkillAgent,
};
use crate::work;

pub const STATE_FILE: &str = ".etus.json";
const STATE_KEY: &str = "active";
const AGENT_PREFIXES: [&str; 2] = ["agente:", "agent:"];
const AGENT_CATALOG_PREFIX: &str = "agente:agentes:";
const AGENT_DESCRIPTION_MAX: usize = 600;
pub const BUILTIN_PACKS: [(&str, [&str; 3]); 1] = [("stalk", ["radar", "personas", "clone"])];

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Active {
    pub apps: BTreeSet<String>,
    pub packs: BTreeMap<String, BTreeSet<String>>,
    pub agents: BTreeMap<String, BTreeSet<String>>,
}

fn string_set(value: Option<&Value>) -> BTreeSet<String> {
    value
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default()
}

fn string_sets(value: Option<&Value>) -> BTreeMap<String, BTreeSet<String>> {
    value
        .and_then(Value::as_object)
        .map(|map| map.iter().map(|(k, v)| (k.clone(), string_set(Some(v)))).collect())
        .unwrap_or_default()
}

impl Active {
    pub fn from_value(value: Option<&Value>) -> Active {
        let Some(value) = value else {
            return Active::default();
        };
        Active {
            apps: string_set(value.get("apps")),
            packs: string_sets(value.get("packs")),
            agents: string_sets(value.get("agents")),
        }
    }

    pub fn to_value(&self) -> Value {
        json!({"apps": self.apps, "packs": self.packs, "agents": self.agents})
    }

    pub fn is_empty(&self) -> bool {
        self.apps.is_empty() && self.packs.is_empty() && self.agents.is_empty()
    }

    pub fn effective(&self) -> BTreeSet<String> {
        self.apps
            .iter()
            .chain(self.packs.values().flatten())
            .chain(self.agents.values().flatten())
            .cloned()
            .collect()
    }

    pub fn sources(&self, prefix: &str) -> Vec<String> {
        let mut sources = Vec::new();
        if self.apps.contains(prefix) {
            sources.push("app".to_string());
        }
        for (name, members) in &self.packs {
            if members.contains(prefix) {
                sources.push(format!("pacote:{name}"));
            }
        }
        for (name, members) in &self.agents {
            if members.contains(prefix) {
                sources.push(format!("agente:{name}"));
            }
        }
        sources
    }
}

pub fn project_root() -> Result<PathBuf> {
    let cwd = std::env::current_dir()?;
    Ok(work::git_context(&cwd).map(|git| git.top).unwrap_or(cwd))
}

pub fn load_state(root: &Path) -> Result<(Map<String, Value>, Active)> {
    let path = root.join(STATE_FILE);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((Map::new(), Active::default())),
        Err(e) => return Err(e.into()),
    };
    let value: Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .map_err(|e| Error::Usage(format!("{} não é um JSON válido: {e}", path.display())))?;
    let Value::Object(file) = value else {
        return Err(Error::Usage(format!(
            "{} deveria conter um objeto JSON.",
            path.display()
        )));
    };
    let active = Active::from_value(file.get(STATE_KEY));
    Ok((file, active))
}

pub fn save_state(root: &Path, mut file: Map<String, Value>, active: &Active) -> Result<()> {
    if active.is_empty() {
        file.remove(STATE_KEY);
    } else {
        file.insert(STATE_KEY.into(), active.to_value());
    }
    let path = root.join(STATE_FILE);
    if file.is_empty() {
        match fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
            _ => return Ok(()),
        }
    }
    let mut text = serde_json::to_string_pretty(&Value::Object(file))?;
    text.push('\n');
    fs::write(path, text)?;
    Ok(())
}

pub fn agent_slug(target: &str) -> Option<String> {
    let lower = target.trim().to_lowercase();
    let rest = AGENT_PREFIXES.iter().find_map(|p| lower.strip_prefix(p))?;
    let rest = rest.strip_prefix("agentes:").unwrap_or(rest);
    Some(slug(rest)).filter(|s| !s.is_empty())
}

pub fn tool_prefixes(tools: &[String]) -> BTreeSet<String> {
    tools
        .iter()
        .filter_map(|name| name.split_once(SEPARATOR).map(|(prefix, _)| prefix.to_string()))
        .filter(|prefix| !prefix.is_empty())
        .collect()
}

pub fn categories(apps: &[App], details: &BTreeMap<String, AppDetail>) -> BTreeMap<String, String> {
    apps.iter()
        .filter_map(|app| details.get(&app.prefix)?.category.clone())
        .map(|name| (slug(&name), name))
        .collect()
}

pub fn pack_members(apps: &[App], details: &BTreeMap<String, AppDetail>, wanted: &str) -> BTreeSet<String> {
    let wanted = slug(wanted);
    let by_category: BTreeSet<String> = apps
        .iter()
        .filter(|app| {
            details
                .get(&app.prefix)
                .and_then(|d| d.category.as_deref())
                .is_some_and(|c| slug(c) == wanted)
        })
        .map(|app| app.prefix.clone())
        .collect();
    if !by_category.is_empty() {
        return by_category;
    }
    BUILTIN_PACKS
        .iter()
        .find(|(name, _)| *name == wanted)
        .map(|(_, members)| {
            members
                .iter()
                .filter_map(|word| find_app(apps, word))
                .map(|app| app.prefix.clone())
                .collect()
        })
        .unwrap_or_default()
}

pub fn pack_names(apps: &[App], details: &BTreeMap<String, AppDetail>) -> Vec<String> {
    let mut names: BTreeSet<String> = categories(apps, details).into_keys().collect();
    names.extend(BUILTIN_PACKS.iter().map(|(name, _)| name.to_string()));
    names.into_iter().collect()
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgentDefinition {
    pub slug: String,
    pub title: String,
    pub description: String,
    pub body: String,
    pub tools: Vec<String>,
}

pub fn parse_agent(slug: &str, value: &Value) -> Result<AgentDefinition> {
    if value.get("locked").and_then(Value::as_bool) == Some(true) {
        return Err(Error::Permission(format!(
            "Você não tem acesso ao agente {slug} no Etus Agentes. Peça a quem administra o agente."
        )));
    }
    let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::trim).unwrap_or("");
    let body = text("body");
    if body.is_empty() {
        return Err(Error::Tool(format!(
            "O Extensões não mandou as instruções do agente {slug}."
        )));
    }
    let title = [text("title"), text("name")]
        .into_iter()
        .find(|t| !t.is_empty())
        .unwrap_or(slug);
    Ok(AgentDefinition {
        slug: slug.to_string(),
        title: title.to_string(),
        description: text("description").to_string(),
        body: body.to_string(),
        tools: value
            .get("tools")
            .and_then(Value::as_array)
            .map(|t| t.iter().filter_map(Value::as_str).map(str::to_string).collect())
            .unwrap_or_default(),
    })
}

pub fn agent_file_name(slug: &str) -> String {
    skills::skill_name(slug)
}

fn without_dashes(text: &str) -> String {
    text.replace(" \u{2014} ", ", ")
        .replace(" \u{2013} ", ", ")
        .replace(['\u{2014}', '\u{2013}'], "-")
}

fn yaml_quoted(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

pub fn render_agent(agent: &AgentDefinition, apps: &[&App], origin: &Origin) -> String {
    let name = agent_file_name(&agent.slug);
    let title = skills::clean(&agent.title);
    let words: Vec<String> = apps.iter().map(|a| format!("`etus {}`", a.word)).collect();
    let what = if agent.description.is_empty() {
        String::new()
    } else {
        format!(" {}", skills::clean(&agent.description))
    };
    let using = if words.is_empty() {
        String::new()
    } else {
        format!(" Usa {} pelo terminal.", words.join(", "))
    };
    let description = truncate(
        &format!("{title}, agente da Etus.{what} Delegue a ele os pedidos desse assunto.{using}"),
        AGENT_DESCRIPTION_MAX,
    );
    let company = origin.company.as_deref().unwrap_or("a empresa ativa");
    let mut lines = vec![
        "---".to_string(),
        format!("name: {name}"),
        format!("description: {}", yaml_quoted(&description)),
        "---".to_string(),
        origin.by.marker().to_string(),
        String::new(),
        "# No Claude Code".to_string(),
        String::new(),
        format!(
            "Agente `{}` do Etus Agentes, gerado para {company} em {}; rode `{}` para trazer a versão nova.",
            agent.slug,
            origin.hub_host,
            origin.by.command()
        ),
        String::new(),
    ];
    if !apps.is_empty() {
        lines.push(
            "As ferramentas citadas abaixo são comandos do `etus`, com o nome da ferramenta em kebab-case (`list_changes` vira `list-changes`):"
                .into(),
        );
        for app in apps {
            lines.push(format!(
                "- `{}__<ferramenta>`: `etus {} <comando> --json`; comandos, parâmetros e permissões na skill `{}`.",
                app.prefix,
                app.word,
                skills::skill_name(&app.word)
            ));
        }
        lines.push(String::new());
    }
    lines.extend(
        [
            "- Comando que altera dados pede `--yes`: só depois do ok da pessoa para aquela ação.",
            "- Código 5 é ação necessária: mostre o link do stderr e diga que só a pessoa pode clicar.",
            "- Código 4: sem permissão para o comando; a pessoa pede acesso a um admin no hub.",
        ]
        .map(str::to_string),
    );
    lines.push(String::new());
    lines.push(without_dashes(&agent.body));
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Targets {
    pub claude: bool,
    pub codex: bool,
}

pub fn targets_for(agent: Option<SkillAgent>, root: &Path, configured: Option<&[Target]>) -> Targets {
    match agent {
        Some(SkillAgent::Claude) => Targets {
            claude: true,
            codex: false,
        },
        Some(SkillAgent::Codex) => Targets {
            claude: false,
            codex: true,
        },
        Some(SkillAgent::Todos) => Targets {
            claude: true,
            codex: true,
        },
        None => match configured {
            Some(targets) => Targets {
                claude: targets.contains(&Target::ClaudeCode),
                codex: needs_shared_skills(targets),
            },
            None => Targets {
                claude: true,
                codex: root.join("AGENTS.md").is_file() || root.join(".agents").is_dir(),
            },
        },
    }
}

fn readers(targets: Targets, configured: Option<&[Target]>) -> Vec<String> {
    let names = |want: &dyn Fn(Target) -> bool| -> String {
        configured
            .map(|list| {
                list.iter()
                    .filter(|t| want(**t))
                    .map(|t| t.name())
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default()
    };
    let mut lines = Vec::new();
    if targets.claude {
        let who =
            names(&|t| t == Target::ClaudeCode || (!targets.codex && matches!(t, Target::Cursor | Target::Copilot)));
        lines.push(format!(
            "Skills em .claude/skills: {}",
            if who.is_empty() { "Claude Code".to_string() } else { who }
        ));
    }
    if targets.codex {
        let who = names(&|t| t != Target::ClaudeCode);
        lines.push(format!(
            "Skills em .agents/skills: {}",
            if who.is_empty() {
                "Codex e os editores que leem .agents/skills".to_string()
            } else {
                who
            }
        ));
    }
    lines
}

#[derive(Debug, Clone)]
pub struct Dirs {
    pub claude_skills: PathBuf,
    pub codex_skills: PathBuf,
    pub claude_agents: PathBuf,
}

impl Dirs {
    pub fn at(root: &Path) -> Dirs {
        Dirs {
            claude_skills: root.join(".claude").join("skills"),
            codex_skills: root.join(".agents").join("skills"),
            claude_agents: root.join(".claude").join("agents"),
        }
    }
}

const OWNED_FOR_UPDATE: [&str; 2] = [ACTIVATE_MARKER, MARKER];
const OWNED_FOR_REMOVAL: [&str; 1] = [ACTIVATE_MARKER];

pub struct Step {
    pub dir: PathBuf,
    pub layout: Layout,
    pub desired: BTreeMap<String, String>,
    pub plan: Plan,
}

pub fn plan_files(
    dirs: &Dirs,
    targets: Targets,
    skills: &BTreeMap<String, String>,
    agents: &BTreeMap<String, String>,
) -> Vec<Step> {
    let empty = BTreeMap::new();
    let step = |dir: &Path, layout: Layout, desired: &BTreeMap<String, String>| {
        let own = Ownership {
            layout,
            update: &OWNED_FOR_UPDATE,
            remove: &OWNED_FOR_REMOVAL,
        };
        Step {
            dir: dir.to_path_buf(),
            layout,
            plan: skills::plan_with(dir, desired, own),
            desired: desired.clone(),
        }
    };
    vec![
        step(
            &dirs.claude_skills,
            Layout::SkillFolder,
            if targets.claude { skills } else { &empty },
        ),
        step(
            &dirs.codex_skills,
            Layout::SkillFolder,
            if targets.codex { skills } else { &empty },
        ),
        step(
            &dirs.claude_agents,
            Layout::MarkdownFile,
            if targets.claude { agents } else { &empty },
        ),
    ]
}

fn apply_steps(steps: &[Step], git: Option<&work::GitContext>) -> Result<()> {
    for step in steps {
        skills::apply_with(&step.dir, &step.desired, &step.plan, step.layout)?;
        let Some(git) = git else { continue };
        for name in step
            .plan
            .create
            .iter()
            .chain(&step.plan.update)
            .chain(&step.plan.unchanged)
        {
            let path = match step.layout {
                Layout::SkillFolder => step.dir.join(name),
                Layout::MarkdownFile => step.layout.path(&step.dir, name),
            };
            work::ensure_excluded(&path, git)?;
        }
    }
    Ok(())
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root).unwrap_or(path).display().to_string()
}

fn files_report(root: &Path, steps: &[Step]) -> (Vec<String>, Value) {
    let mut lines = Vec::new();
    let mut groups: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for step in steps {
        let entries: [(&Vec<String>, &str, &str, &str); 4] = [
            (&step.plan.create, "+", "criado", "criados"),
            (&step.plan.update, "~", "atualizado", "atualizados"),
            (&step.plan.remove, "-", "removido", "removidos"),
            (
                &step.plan.skipped,
                "!",
                "ignorado: feito à mão, não foi gerado pelo etus",
                "ignorados",
            ),
        ];
        for (names, mark, label, key) in entries {
            for name in names {
                let path = relative(root, &step.layout.path(&step.dir, name));
                lines.push(format!("{mark} {path} ({label})"));
                groups.entry(key).or_default().push(path);
            }
        }
        for name in &step.plan.unchanged {
            groups
                .entry("iguais")
                .or_default()
                .push(relative(root, &step.layout.path(&step.dir, name)));
        }
    }
    let value = json!({
        "criados": groups.remove("criados").unwrap_or_default(),
        "atualizados": groups.remove("atualizados").unwrap_or_default(),
        "removidos": groups.remove("removidos").unwrap_or_default(),
        "ignorados": groups.remove("ignorados").unwrap_or_default(),
        "iguais": groups.remove("iguais").unwrap_or_default(),
    });
    (lines, value)
}

fn read_agent(ctx: &Ctx, slug: &str) -> Result<AgentDefinition> {
    let id = format!("{AGENT_CATALOG_PREFIX}{slug}");
    let outcome = ctx
        .with_mcp(|mcp| mcp.call_tool(SKILLS_READ_TOOL, &json!({ "id": id })))
        .map_err(|error| match error {
            Error::Tool(message) => Error::Permission(format!(
                "Não consegui ler o agente {slug} pelo Extensões ({message}). Confira o nome em `etus agentes` ou peça acesso ao Extensões no hub."
            )),
            other => other,
        })?;
    if let Some(required) = action::detect(&outcome.result) {
        return Err(Error::ActionRequired(Box::new(required)));
    }
    if outcome.is_error {
        if let Some(error) = refusal(&outcome.result) {
            return Err(error);
        }
        let message = first_text(&outcome.result).unwrap_or("");
        if message.contains("item_not_found") {
            return Err(Error::Usage(format!(
                "O Etus Agentes não tem o agente {slug} publicado para você. Veja os agentes em `etus agentes --help`."
            )));
        }
        return Err(Error::Usage(format!(
            "Não deu para ler o agente {slug} pelo Extensões: {message}"
        )));
    }
    parse_agent(slug, &payload(&outcome.result))
}

fn origin(ctx: &Ctx, company: Option<String>) -> Origin {
    Origin {
        company,
        hub_host: skills::hub_host(&ctx.settings.hub),
        by: Generator::Activate,
    }
}

pub(crate) fn global_notice(at: &Locations) -> (usize, Option<String>) {
    let mut total = 0;
    let mut places = Vec::new();
    for dir in skills::global_dirs(at) {
        let found = skills::synced_skills(&dir).len();
        if found > 0 {
            total += found;
            places.push(dir.display().to_string());
        }
    }
    let text = (total > 0).then(|| {
        format!(
            "Há {} de `etus skills sincronizar` em {}: elas entram em toda sessão, de qualquer projeto. Para ficar só com as deste projeto: `etus skills limpar-globais`.",
            cli::count(total, "skill global", "skills globais"),
            places.join(" e ")
        )
    });
    (total, text)
}

fn app_label(apps: &[App], prefix: &str) -> String {
    match apps.iter().find(|a| a.prefix == prefix) {
        Some(app) => format!("{} ({})", app.word, app.name),
        None => format!("{prefix} (sem acesso ou fora do catálogo)"),
    }
}

fn words(apps: &[App], prefixes: &BTreeSet<String>) -> String {
    prefixes
        .iter()
        .map(|p| {
            apps.iter()
                .find(|a| &a.prefix == p)
                .map_or(p.as_str(), |a| a.word.as_str())
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn state_lines(active: &Active, apps: &[App]) -> Vec<String> {
    let mut lines = Vec::new();
    for prefix in active.effective() {
        let sources = active.sources(&prefix);
        if sources == ["app"] {
            lines.push(format!("  {}", app_label(apps, &prefix)));
            continue;
        }
        let why: Vec<&str> = sources
            .iter()
            .map(|s| if s == "app" { "direto" } else { s.as_str() })
            .collect();
        lines.push(format!("  {}; por {}", app_label(apps, &prefix), why.join(", ")));
    }
    lines
}

fn state_json(active: &Active, apps: &[App], details: Option<&BTreeMap<String, AppDetail>>) -> Value {
    let list: Vec<Value> = active
        .effective()
        .iter()
        .map(|prefix| {
            let app = apps.iter().find(|a| &a.prefix == prefix);
            let mut entry = json!({
                "prefixo": prefix,
                "app": app.map(|a| a.word.clone()),
                "nome": app.map(|a| a.name.clone()),
                "origem": active.sources(prefix),
            });
            if let Some(details) = details {
                entry["guia"] = json!(details.get(prefix).is_some_and(|d| d.guide.is_some()));
            }
            entry
        })
        .collect();
    json!({"apps": list, "pacotes": active.packs, "agentes": active.agents})
}

pub fn activate(ctx: &Ctx, targets: &[String], agent: Option<SkillAgent>) -> Result<()> {
    let at = skill_locations()?;
    let root = at.project_root.clone();
    let (file, mut active) = load_state(&root)?;
    if targets.is_empty() && active.is_empty() {
        return Err(Error::Usage(
            "Diga o que ativar: um app (`etus ativar radar`), um pacote (`etus ativar arbitragem` ou `etus ativar stalk`) ou um agente do Etus Agentes (`etus ativar agente:stalk`). Veja `etus apps`.".into(),
        ));
    }
    let (mut cache, fresh) = ctx.catalog()?;
    let unknown = targets
        .iter()
        .any(|t| agent_slug(t).is_none() && find_app(&cache.apps(), t).is_none());
    if unknown && !fresh {
        cache = ctx.fetch_catalog()?;
    }
    let apps = cache.apps();
    let mut warnings = Vec::new();
    let mut all_details: Option<BTreeMap<String, AppDetail>> = None;
    let mut definitions: BTreeMap<String, AgentDefinition> = BTreeMap::new();
    let known = |prefixes: BTreeSet<String>, warnings: &mut Vec<String>, owner: &str| {
        let (present, missing): (BTreeSet<String>, BTreeSet<String>) =
            prefixes.into_iter().partition(|p| apps.iter().any(|a| &a.prefix == p));
        if !missing.is_empty() {
            warnings.push(format!(
                "{owner} usa {}, que você não pode usar na empresa ativa: peça acesso no hub.",
                missing.into_iter().collect::<Vec<_>>().join(", ")
            ));
        }
        present
    };
    for target in targets {
        if let Some(slug) = agent_slug(target) {
            let definition = read_agent(ctx, &slug)?;
            let members = known(
                tool_prefixes(&definition.tools),
                &mut warnings,
                &format!("O agente {slug}"),
            );
            active.agents.insert(slug.clone(), members);
            definitions.insert(slug, definition);
        } else if let Some(app) = find_app(&apps, target) {
            active.apps.insert(app.prefix.clone());
        } else {
            let details = all_details.get_or_insert_with(|| fetch_details(ctx, &apps));
            let members = pack_members(&apps, details, target);
            if members.is_empty() {
                let hint = format!(" Pacotes: {}.", pack_names(&apps, details).join(", "));
                return Err(Error::Usage(format!(
                    "Não achei app, pacote nem agente chamado {target}. Veja `etus apps`.{hint} Agente: `etus ativar agente:<slug>`."
                )));
            }
            active.packs.insert(slug(target), members);
        }
    }
    let pending: Vec<String> = active
        .agents
        .keys()
        .filter(|slug| !definitions.contains_key(*slug))
        .cloned()
        .collect();
    for slug in pending {
        match read_agent(ctx, &slug) {
            Ok(definition) => {
                let members = known(
                    tool_prefixes(&definition.tools),
                    &mut warnings,
                    &format!("O agente {slug}"),
                );
                active.agents.insert(slug.clone(), members);
                definitions.insert(slug, definition);
            }
            Err(error) => warnings.push(format!(
                "Não deu para atualizar o agente {slug} ({}); o arquivo dele fica como está.",
                error.message()
            )),
        }
    }
    let effective = active.effective();
    let chosen: Vec<App> = apps.iter().filter(|a| effective.contains(&a.prefix)).cloned().collect();
    let lost: Vec<&String> = effective
        .iter()
        .filter(|p| !apps.iter().any(|a| &a.prefix == *p))
        .collect();
    if !lost.is_empty() {
        warnings.push(format!(
            "Sem acesso a {} na empresa ativa: a skill sai do projeto até você ter acesso de novo.",
            lost.iter().map(|p| p.as_str()).collect::<Vec<_>>().join(", ")
        ));
    }
    let details = match all_details {
        Some(details) => details,
        None => fetch_details(ctx, &chosen),
    };
    let origin = origin(ctx, cache.company.as_ref().map(|c| c.name.clone()));
    let wanted_skills: BTreeMap<String, String> = chosen
        .iter()
        .map(|app| {
            (
                skills::skill_name(&app.word),
                skills::render(app, details.get(&app.prefix), &apps, &origin),
            )
        })
        .collect();
    let mut wanted_agents: BTreeMap<String, String> = BTreeMap::new();
    for (slug, members) in &active.agents {
        let Some(definition) = definitions.get(slug) else {
            if let Ok(text) =
                fs::read_to_string(Layout::MarkdownFile.path(&Dirs::at(&root).claude_agents, &agent_file_name(slug)))
            {
                wanted_agents.insert(agent_file_name(slug), text);
            }
            continue;
        };
        let used: Vec<&App> = chosen.iter().filter(|a| members.contains(&a.prefix)).collect();
        wanted_agents.insert(agent_file_name(slug), render_agent(definition, &used, &origin));
    }
    let configured = ProjectConfig::read(&file).ok().and_then(|c| c.targets);
    let targets = targets_for(agent, &root, configured.as_deref());
    let steps = plan_files(&Dirs::at(&root), targets, &wanted_skills, &wanted_agents);
    if !ctx.dry_run {
        let cwd = std::env::current_dir()?;
        apply_steps(&steps, work::git_context(&cwd).as_ref())?;
        save_state(&root, file, &active)?;
    }
    let without_guide: Vec<&str> = chosen
        .iter()
        .filter(|a| details.get(&a.prefix).is_none_or(|d| d.guide.is_none()))
        .map(|a| a.word.as_str())
        .collect();
    let (global_count, global_text) = global_notice(&at);
    let (file_lines, files_json) = files_report(&root, &steps);
    for warning in &warnings {
        eprintln!("etus: {warning}");
    }
    let mut lines = vec![format!("Ativos em {}:", root.display())];
    lines.extend(state_lines(&active, &apps));
    if !without_guide.is_empty() {
        lines.push(format!(
            "Sem guia no hub (a skill traz só os comandos): {}.",
            without_guide.join(", ")
        ));
    }
    lines.extend(file_lines);
    lines.extend(readers(targets, configured.as_deref()));
    if ctx.dry_run {
        lines.push("Simulação (--dry-run): nada foi gravado.".into());
    } else if steps.iter().any(|s| s.plan.changes() > 0) {
        lines.push("Reinicie o agente (ou abra uma conversa nova) para ele carregar as skills.".into());
    }
    if let Some(text) = &global_text {
        lines.push(text.clone());
    }
    ctx.out.print_human(&lines.join("\n"), || {
        let mut value = state_json(&active, &apps, Some(&details));
        value["raiz"] = json!(root.display().to_string());
        value["arquivos"] = files_json;
        value["avisos"] = json!(warnings);
        value["skillsGlobais"] = json!(global_count);
        value
    });
    Ok(())
}

pub fn deactivate(ctx: &Ctx, targets: &[String]) -> Result<()> {
    let root = project_root()?;
    let (file, mut active) = load_state(&root)?;
    if active.is_empty() {
        ctx.out.print_human("Nada ativo neste projeto.", || json!({"apps": []}));
        return Ok(());
    }
    let (cache, _) = ctx.catalog()?;
    let apps = cache.apps();
    let mut warnings = Vec::new();
    for target in targets {
        if let Some(slug) = agent_slug(target) {
            if active.agents.remove(&slug).is_none() {
                warnings.push(format!("O agente {slug} não estava ativo."));
            }
            continue;
        }
        if active.packs.remove(&slug(target)).is_some() {
            continue;
        }
        let prefix = find_app(&apps, target)
            .map(|a| a.prefix.clone())
            .unwrap_or_else(|| target.clone());
        if !active.apps.remove(&prefix) && !active.effective().contains(&prefix) {
            warnings.push(format!("{target} não estava ativo."));
        }
        let still = active.sources(&prefix);
        if !still.is_empty() {
            warnings.push(format!(
                "{target} continua ativo por causa de {}; desative esses também se quiser tirá-lo.",
                still.join(", ")
            ));
        }
    }
    let effective = active.effective();
    let keep_skills: BTreeMap<String, String> = apps
        .iter()
        .filter(|a| effective.contains(&a.prefix))
        .map(|a| (skills::skill_name(&a.word), String::new()))
        .collect();
    let keep_agents: BTreeMap<String, String> = active
        .agents
        .keys()
        .map(|slug| (agent_file_name(slug), String::new()))
        .collect();
    let dirs = Dirs::at(&root);
    let all = Targets {
        claude: true,
        codex: true,
    };
    let steps: Vec<Step> = plan_files(&dirs, all, &keep_skills, &keep_agents)
        .into_iter()
        .map(|step| Step {
            plan: Plan {
                remove: step.plan.remove,
                ..Plan::default()
            },
            ..step
        })
        .collect();
    if !ctx.dry_run {
        for step in &steps {
            skills::apply_with(&step.dir, &step.desired, &step.plan, step.layout)?;
        }
        save_state(&root, file, &active)?;
    }
    for warning in &warnings {
        eprintln!("etus: {warning}");
    }
    let (file_lines, files_json) = files_report(&root, &steps);
    let mut lines = if active.is_empty() {
        vec!["Nada mais ativo neste projeto.".to_string()]
    } else {
        let mut lines = vec![format!("Ativos em {}:", root.display())];
        lines.extend(state_lines(&active, &apps));
        lines
    };
    lines.extend(file_lines);
    if ctx.dry_run {
        lines.push("Simulação (--dry-run): nada foi gravado.".into());
    }
    ctx.out.print_human(&lines.join("\n"), || {
        let mut value = state_json(&active, &apps, None);
        value["raiz"] = json!(root.display().to_string());
        value["arquivos"] = files_json;
        value["avisos"] = json!(warnings);
        value
    });
    Ok(())
}

pub fn show(ctx: &Ctx) -> Result<()> {
    let root = project_root()?;
    let (_, active) = load_state(&root)?;
    let apps = ctx.cached_catalog().map(|c| c.apps()).unwrap_or_default();
    let text = if active.is_empty() {
        "Nada ativo neste projeto. Use `etus ativar <app|pacote|agente:slug>`.".to_string()
    } else {
        let mut lines = vec![format!("Ativos em {}:", root.display())];
        lines.extend(state_lines(&active, &apps));
        for (name, members) in &active.packs {
            lines.push(format!("Pacote {name}: {}", words(&apps, members)));
        }
        for (name, members) in &active.agents {
            lines.push(format!(
                "Agente {name}: .claude/agents/{}.md; apps {}",
                agent_file_name(name),
                words(&apps, members)
            ));
        }
        lines.join("\n")
    };
    ctx.out.print_human(&text, || {
        let mut value = state_json(&active, &apps, None);
        value["raiz"] = json!(root.display().to_string());
        value
    });
    Ok(())
}

pub fn clean_globals(ctx: &Ctx) -> Result<Vec<(PathBuf, Plan)>> {
    let at = skill_locations()?;
    let mut reports = Vec::new();
    for dir in skills::global_dirs(&at) {
        let plan = Plan {
            remove: skills::synced_skills(&dir),
            ..Plan::default()
        };
        if !ctx.dry_run {
            skills::apply(&dir, &BTreeMap::new(), &plan)?;
        }
        reports.push((dir, plan));
    }
    Ok(reports)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{Tool, group};
    use crate::testutil::temp_dir;

    fn apps() -> Vec<App> {
        let tools: Vec<Tool> = serde_json::from_value(json!([
            {"name": "etusradar__list_changes", "title": "Listar mudanças", "description": "Etus Radar: What changed.", "inputSchema": {"type": "object", "properties": {}}, "annotations": {"readOnlyHint": true}},
            {"name": "etuspersonas__measure_page", "title": "Medir página", "description": "Etus Personas: Measures a page.", "inputSchema": {"type": "object", "properties": {}}, "annotations": {"readOnlyHint": false}},
            {"name": "tasks__list_tasks", "title": "Listar tarefas", "description": "Tarefas: Lists tasks.", "inputSchema": {"type": "object", "properties": {}}, "annotations": {"readOnlyHint": true}}
        ]))
        .unwrap();
        group(&tools)
    }

    fn details() -> BTreeMap<String, AppDetail> {
        let mut details = BTreeMap::new();
        for (prefix, category) in [
            ("etusradar", Some("Arbitragem")),
            ("etuspersonas", Some("Arbitragem")),
            ("tasks", Some("Produtividade")),
        ] {
            details.insert(
                prefix.to_string(),
                AppDetail {
                    category: category.map(str::to_string),
                    ..AppDetail::default()
                },
            );
        }
        details
    }

    fn origin() -> Origin {
        Origin {
            company: Some("Etus".into()),
            hub_host: "apps.etus.io".into(),
            by: Generator::Activate,
        }
    }

    #[test]
    fn agent_targets_accept_both_prefixes_and_the_catalog_id() {
        assert_eq!(agent_slug("agente:stalk").as_deref(), Some("stalk"));
        assert_eq!(agent_slug("agent:Stalk").as_deref(), Some("stalk"));
        assert_eq!(agent_slug("agente:agentes:stalk").as_deref(), Some("stalk"));
        assert_eq!(agent_slug("agente:"), None);
        assert_eq!(agent_slug("radar"), None);
    }

    #[test]
    fn tool_names_and_app_prefixes_become_apps() {
        let tools = ["etusradar__list_ads", "etusradar__get_ad", "records__", "etus_buscar"].map(String::from);
        let prefixes: Vec<String> = tool_prefixes(&tools).into_iter().collect();
        assert_eq!(prefixes, vec!["etusradar", "records"]);
    }

    #[test]
    fn categories_are_packs() {
        let apps = apps();
        let details = details();
        let members: Vec<String> = pack_members(&apps, &details, "Arbitragem").into_iter().collect();
        assert_eq!(members, vec!["etuspersonas", "etusradar"]);
        assert!(pack_members(&apps, &details, "arbitragem").contains("etusradar"));
        assert!(pack_members(&apps, &details, "nada").is_empty());
        assert_eq!(
            categories(&apps, &details).into_keys().collect::<Vec<_>>(),
            vec!["arbitragem", "produtividade"]
        );
        assert_eq!(
            pack_names(&apps, &details),
            vec!["arbitragem", "produtividade", "stalk"]
        );
    }

    #[test]
    fn stalk_is_a_builtin_pack_when_no_category_matches() {
        let apps = apps();
        let members: Vec<String> = pack_members(&apps, &BTreeMap::new(), "Stalk").into_iter().collect();
        assert_eq!(members, vec!["etuspersonas", "etusradar"]);
        let mut details = details();
        details.get_mut("tasks").unwrap().category = Some("Stalk".into());
        let members: Vec<String> = pack_members(&apps, &details, "stalk").into_iter().collect();
        assert_eq!(members, vec!["tasks"], "a hub category wins over the builtin pack");
    }

    #[test]
    fn state_keeps_other_keys_and_merges_sources() {
        let root = temp_dir("activate-state");
        fs::write(root.join(STATE_FILE), r#"{"project":"ENG","workspace":"etus"}"#).unwrap();
        let (file, mut active) = load_state(&root).unwrap();
        assert!(active.is_empty());
        active.apps.insert("etusradar".into());
        active.packs.insert(
            "arbitragem".into(),
            ["etusradar", "etuspersonas"].map(String::from).into(),
        );
        active
            .agents
            .insert("stalk".into(), ["etusclone"].map(String::from).into());
        save_state(&root, file, &active).unwrap();
        let (file, again) = load_state(&root).unwrap();
        assert_eq!(again, active);
        assert_eq!(file["project"], "ENG");
        assert_eq!(
            again.effective().into_iter().collect::<Vec<_>>(),
            vec!["etusclone", "etuspersonas", "etusradar"]
        );
        assert_eq!(again.sources("etusradar"), vec!["app", "pacote:arbitragem"]);
        assert_eq!(again.sources("etusclone"), vec!["agente:stalk"]);
        save_state(&root, file, &Active::default()).unwrap();
        let text = fs::read_to_string(root.join(STATE_FILE)).unwrap();
        assert!(!text.contains("active"));
        let (file, _) = load_state(&root).unwrap();
        let mut bare = file.clone();
        bare.clear();
        save_state(&root, bare, &Active::default()).unwrap();
        assert!(!root.join(STATE_FILE).exists());
        fs::write(root.join(STATE_FILE), "[]").unwrap();
        assert_eq!(load_state(&root).unwrap_err().exit_code(), 2);
    }

    #[test]
    fn agent_definition_is_read_from_the_catalog_item() {
        let item = json!({"id": "agente:agentes:stalk", "title": "Stalk da arbitragem", "description": "O que os concorrentes fazem.",
            "tools": ["etusradar__list_changes", "etuspersonas__measure_page"], "body": "# Quem você é\n\nAgente \u{2014} do stalk.", "locked": false});
        let agent = parse_agent("stalk", &item).unwrap();
        assert_eq!(agent.title, "Stalk da arbitragem");
        assert_eq!(agent.tools.len(), 2);
        let locked = parse_agent("stalk", &json!({"locked": true, "body": ""})).unwrap_err();
        assert_eq!(locked.exit_code(), 4);
        assert!(parse_agent("stalk", &json!({"title": "x"})).is_err());

        let apps = apps();
        let used: Vec<&App> = apps.iter().filter(|a| a.prefix != "tasks").collect();
        let text = render_agent(&agent, &used, &origin());
        assert!(text.starts_with("---\nname: etus-stalk\ndescription: \"Stalk da arbitragem, agente da Etus. O que os concorrentes fazem. Delegue a ele os pedidos desse assunto. Usa `etus personas`, `etus radar` pelo terminal.\"\n---\n"));
        assert!(text.contains(&format!("---\n{ACTIVATE_MARKER}\n")));
        assert!(text.contains("- `etusradar__<ferramenta>`: `etus radar <comando> --json`; comandos, parâmetros e permissões na skill `etus-radar`."));
        assert!(text.contains("# Quem você é\n\nAgente, do stalk.\n"));
        assert!(!text.contains(['\u{2014}', '\u{2013}']));
    }

    #[test]
    fn codex_follows_the_flag_or_the_project() {
        let root = temp_dir("activate-targets");
        assert_eq!(
            targets_for(None, &root, None),
            Targets {
                claude: true,
                codex: false
            }
        );
        fs::write(root.join("AGENTS.md"), "x").unwrap();
        assert!(targets_for(None, &root, None).codex);
        assert!(!targets_for(Some(SkillAgent::Claude), &root, None).codex);
        assert!(!targets_for(Some(SkillAgent::Codex), &root, None).claude);

        let editors = [Target::ClaudeCode, Target::Cursor, Target::Copilot];
        assert_eq!(
            targets_for(None, &root, Some(&editors)),
            Targets {
                claude: true,
                codex: false
            },
            "Cursor and Copilot read .claude/skills, so no copy in .agents"
        );
        let gemini_only = [Target::Antigravity];
        assert_eq!(
            targets_for(None, &root, Some(&gemini_only)),
            Targets {
                claude: false,
                codex: true
            }
        );
        assert!(targets_for(Some(SkillAgent::Todos), &root, Some(&gemini_only)).claude);
    }

    #[test]
    fn files_are_planned_per_target_and_only_activate_files_are_removed() {
        let root = temp_dir("activate-files");
        let dirs = Dirs::at(&root);
        let skills = BTreeMap::from([("etus-radar".to_string(), format!("---\n{ACTIVATE_MARKER}\nradar\n"))]);
        let agents = BTreeMap::from([("etus-stalk".to_string(), format!("---\n{ACTIVATE_MARKER}\nstalk\n"))]);
        let both = Targets {
            claude: true,
            codex: true,
        };
        let steps = plan_files(&dirs, both, &skills, &agents);
        apply_steps(&steps, None).unwrap();
        assert!(root.join(".claude/skills/etus-radar/SKILL.md").is_file());
        assert!(root.join(".agents/skills/etus-radar/SKILL.md").is_file());
        assert!(root.join(".claude/agents/etus-stalk.md").is_file());

        let synced = format!("---\n{MARKER}\nold\n");
        fs::create_dir_all(dirs.claude_skills.join("etus-tarefas")).unwrap();
        fs::write(dirs.claude_skills.join("etus-tarefas/SKILL.md"), &synced).unwrap();
        fs::write(dirs.claude_agents.join("etus-mine.md"), "feito à mão").unwrap();
        fs::write(dirs.claude_agents.join("notes.md"), ACTIVATE_MARKER).unwrap();

        let claude_only = Targets {
            claude: true,
            codex: false,
        };
        let steps = plan_files(&dirs, claude_only, &skills, &BTreeMap::new());
        assert_eq!(steps[0].plan.unchanged, vec!["etus-radar"]);
        assert!(steps[0].plan.remove.is_empty(), "skills from sincronizar stay");
        assert_eq!(steps[1].plan.remove, vec!["etus-radar"]);
        assert_eq!(steps[2].plan.remove, vec!["etus-stalk"]);
        apply_steps(&steps, None).unwrap();
        assert!(!root.join(".agents/skills/etus-radar").exists());
        assert!(!root.join(".claude/agents/etus-stalk.md").exists());
        assert!(root.join(".claude/agents/etus-mine.md").exists());
        assert!(root.join(".claude/agents/notes.md").exists());
        assert!(root.join(".claude/skills/etus-tarefas/SKILL.md").exists());

        let replaced = BTreeMap::from([("etus-tarefas".to_string(), format!("---\n{ACTIVATE_MARKER}\nnew\n"))]);
        let steps = plan_files(&dirs, claude_only, &replaced, &BTreeMap::new());
        assert_eq!(steps[0].plan.update, vec!["etus-tarefas"]);
        assert_eq!(steps[0].plan.remove, vec!["etus-radar"]);
    }
}
