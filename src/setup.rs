use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use dialoguer::{MultiSelect, Select, theme::ColorfulTheme};
use serde_json::{Map, Value, json};

use crate::activate::{Active, STATE_FILE, global_notice, load_state, save_state};
use crate::commands::skill_locations;
use crate::context::Ctx;
use crate::editors::{
    CLAUDE_SKILLS, McpFile, McpMerge, McpMode, McpStatus, SHARED_SKILLS, Scope, claude_scopes, mcp_files, mcp_url,
    merge, mode_of, needs_shared_skills, router_base_url, skills_note,
};
use crate::error::{Error, Result};
use crate::setup_profiles::{MARKETPLACE, Profiles, REPOSITORY, split_ids};
use crate::skills::{self, ACTIVATE_MARKER};
use crate::work;

pub const BLOCK_BEGIN: &str = "<!-- etus-skills:begin -->";
pub const BLOCK_END: &str = "<!-- etus-skills:end -->";
const DEFAULT_WORKSPACE: &str = "etus";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Target {
    ClaudeCode,
    Cursor,
    Codex,
    Antigravity,
    Copilot,
    Windsurf,
}

impl Target {
    pub fn all() -> Vec<Target> {
        vec![
            Target::ClaudeCode,
            Target::Cursor,
            Target::Codex,
            Target::Antigravity,
            Target::Copilot,
            Target::Windsurf,
        ]
    }

    pub fn id(&self) -> &'static str {
        match self {
            Target::ClaudeCode => "claude-code",
            Target::Cursor => "cursor",
            Target::Codex => "codex",
            Target::Antigravity => "antigravity",
            Target::Copilot => "copilot",
            Target::Windsurf => "windsurf",
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Target::ClaudeCode => "Claude Code",
            Target::Cursor => "Cursor",
            Target::Codex => "Codex / OpenAI",
            Target::Antigravity => "Antigravity CLI",
            Target::Copilot => "GitHub Copilot",
            Target::Windsurf => "Windsurf",
        }
    }

    pub fn relative_file(&self) -> &'static str {
        match self {
            Target::ClaudeCode => ".claude/settings.json",
            Target::Cursor => ".cursor/rules/etus.mdc",
            Target::Codex => "AGENTS.md",
            Target::Antigravity => "GEMINI.md",
            Target::Copilot => ".github/copilot-instructions.md",
            Target::Windsurf => ".windsurfrules",
        }
    }

    pub fn from_str_lenient(s: &str) -> Option<Target> {
        match s.trim().to_lowercase().as_str() {
            "claude" | "claude-code" | "claudecode" => Some(Target::ClaudeCode),
            "cursor" => Some(Target::Cursor),
            "codex" | "openai" => Some(Target::Codex),
            "antigravity" | "agy" | "gemini" => Some(Target::Antigravity),
            "copilot" | "github-copilot" => Some(Target::Copilot),
            "windsurf" | "cascade" => Some(Target::Windsurf),
            _ => None,
        }
    }
}

pub fn parse_targets(inputs: Option<&[String]>) -> Result<Vec<Target>> {
    let inputs = match inputs {
        Some(list) if !list.is_empty() => list,
        _ => return Ok(Target::all()),
    };

    let mut out = BTreeSet::new();
    let mut unknown = Vec::new();

    for item in inputs {
        for piece in item.split(',') {
            let clean = piece.trim();
            if clean.is_empty() {
                continue;
            }
            if clean.eq_ignore_ascii_case("all") || clean.eq_ignore_ascii_case("todas") {
                return Ok(Target::all());
            }
            match Target::from_str_lenient(clean) {
                Some(t) => {
                    out.insert(t);
                }
                None => unknown.push(clean.to_string()),
            }
        }
    }

    if !unknown.is_empty() {
        return Err(Error::Usage(format!(
            "Plataforma(s) desconhecida(s): {}. Disponíveis: claude-code, cursor, codex, antigravity, copilot, windsurf ou all",
            unknown.join(", ")
        )));
    }

    if out.is_empty() {
        Ok(vec![Target::ClaudeCode])
    } else {
        Ok(out.into_iter().collect())
    }
}

#[derive(Debug, Clone, Default)]
pub struct SetupOptions {
    pub targets: Option<Vec<Target>>,
    pub replace_targets: bool,
    pub perfis: Vec<String>,
    pub projeto: Option<String>,
    pub workspace: Option<String>,
    pub force: bool,
    pub allow_outside: bool,
    pub interativo: bool,
    pub com_templates: bool,
    pub mcp_modo: Option<McpMode>,
}

struct MarkerRange {
    start: usize,
    end: usize,
}

fn find_line_markers(content: &str) -> (Vec<MarkerRange>, Vec<MarkerRange>) {
    let mut begins = Vec::new();
    let mut ends = Vec::new();
    let mut in_code_block = false;
    let mut fence_char = ' ';
    let mut fence_len = 0;

    let mut char_idx = 0;
    let lines: Vec<&str> = content.split('\n').collect();

    for (i, raw_line) in lines.iter().enumerate() {
        let line_len = raw_line.len() + if i < lines.len() - 1 { 1 } else { 0 };
        let clean = raw_line.strip_suffix('\r').unwrap_or(raw_line);

        let trimmed_leading = clean.trim_start();
        let leading_spaces = clean.len() - trimmed_leading.len();

        let is_fence = (trimmed_leading.starts_with("```") || trimmed_leading.starts_with("~~~"))
            && (trimmed_leading.chars().all(|c| c == '`')
                || trimmed_leading.chars().all(|c| c == '~')
                || trimmed_leading.starts_with("```")
                || trimmed_leading.starts_with("~~~"));

        if !in_code_block && is_fence {
            in_code_block = true;
            fence_char = trimmed_leading.chars().next().unwrap_or('`');
            fence_len = trimmed_leading.chars().take_while(|&c| c == fence_char).count();
        } else if in_code_block {
            let is_close = trimmed_leading.starts_with(fence_char)
                && trimmed_leading.chars().take_while(|&c| c == fence_char).count() >= fence_len;
            if is_close {
                in_code_block = false;
                fence_char = ' ';
                fence_len = 0;
            }
        } else if leading_spaces <= 3 {
            let candidate = clean.trim();
            if candidate == BLOCK_BEGIN {
                begins.push(MarkerRange {
                    start: char_idx,
                    end: char_idx + line_len,
                });
            } else if candidate == BLOCK_END {
                ends.push(MarkerRange {
                    start: char_idx,
                    end: char_idx + line_len,
                });
            }
        }

        char_idx += line_len;
    }

    (begins, ends)
}

pub fn apply_block(existing_content: Option<&str>, block_body: &str, target: Target, force: bool) -> Result<String> {
    let formatted_block = format!("{BLOCK_BEGIN}\n{}\n{BLOCK_END}\n", block_body.trim());

    let existing = match existing_content {
        None => {
            if target == Target::Cursor {
                return Ok(format!(
                    "---\ndescription: Padroes, convencoes e regras de engenharia da Etus\nalwaysApply: true\n---\n\n{formatted_block}"
                ));
            }
            return Ok(formatted_block);
        }
        Some(c) => c,
    };

    let (begins, ends) = find_line_markers(existing);

    if begins.len() == 1 && ends.len() == 1 {
        if begins[0].start >= ends[0].start {
            return Err(Error::Other(format!(
                "marcadores de bloco corrompidos: '{BLOCK_END}' aparece antes de '{BLOCK_BEGIN}'. Corrija o arquivo manualmente."
            )));
        }
        let before = &existing[..begins[0].start];
        let after = &existing[ends[0].end..];
        return Ok(format!("{before}{formatted_block}{after}"));
    }

    if begins.len() == 1 && ends.is_empty() {
        return Err(Error::Other(format!(
            "arquivo corrompido: marcador '{BLOCK_BEGIN}' encontrado sem o correspondente '{BLOCK_END}'. Corrija o arquivo manualmente."
        )));
    }

    if begins.is_empty() && ends.len() == 1 {
        return Err(Error::Other(format!(
            "arquivo corrompido: marcador '{BLOCK_END}' encontrado sem o correspondente '{BLOCK_BEGIN}'. Corrija o arquivo manualmente."
        )));
    }

    if begins.len() > 1 || ends.len() > 1 {
        return Err(Error::Other(format!(
            "arquivo com marcadores duplicados ({} begin, {} end). Mantenha apenas um bloco demarcado no arquivo.",
            begins.len(),
            ends.len()
        )));
    }

    if !force {
        return Err(Error::Usage(format!(
            "arquivo existente sem o marcador '{BLOCK_BEGIN}'. Use --force para anexar as regras preservando o conteudo."
        )));
    }

    let separator = if existing.ends_with("\n\n") {
        ""
    } else if existing.ends_with('\n') {
        "\n"
    } else {
        "\n\n"
    };

    Ok(format!("{existing}{separator}{formatted_block}"))
}

pub fn generate_body(target: Target, perfil: &str, project: &str, workspace: &str, has_codex: bool) -> String {
    let header_title = match target {
        Target::Cursor => "Regras de Engenharia da Etus (Cursor)",
        Target::Codex => "Instrucoes de Conduta e Engenharia (Codex / OpenAI)",
        Target::Antigravity => "Regras de Engenharia da Etus (Antigravity CLI / Gemini)",
        Target::Copilot => "GitHub Copilot Instructions",
        Target::Windsurf => "Regras de Engenharia da Etus (Windsurf Cascade)",
        Target::ClaudeCode => "Regras de Engenharia da Etus (Claude Code)",
    };

    let plane_part = if project.is_empty() || project == "ETUS" {
        "Projeto sem vinculo de Plane".to_string()
    } else {
        format!("Projeto Plane: {project}")
    };

    let base_header =
        format!("# {header_title}\n\n# Perfil Etus: {perfil} | {plane_part} | Workspace: {workspace}\n\n");

    let core_rules = format!(
        r#"## 1. Convencoes de Escrita da Etus
1. **Nunca use travessao (U+2014) nem meia-risca (U+2013).** Em nenhum texto, idioma ou contexto. Use exclusivamente o hifen ASCII (-).
2. **Idioma.** Texto para pessoas em pt-BR com acentuacao correta. Codigo, nomes de arquivo, rotas, tabelas, commits e comentarios em ingles.
3. **Comentario em codigo so quando o codigo nao consegue dizer o porque.** Uma boa funcao nao precisa de comentario.
4. **Evidencia marcada.** Toda afirmacao diz se foi medida, inferida ou e palpite. Nunca invente links ou metricas.

## 2. Etiqueta de Trabalho e Atribuicao no Plane
Cada chamada relevante leva o contexto de trabalho com work item, empresa, projeto e fase.

### Comandos de atribuicao (CLI etus)
- Iniciar ou trocar fase: `etus trabalho {project}-123 --fase <fase>`
- Conferir atribuicao ativa: `etus trabalho mostrar`
- Limpar atribuicao ao concluir: `etus trabalho limpar`

### Fases de trabalho
- `plan`: Entender, investigar, desenhar, escrever spec ou plano
- `impl`: Escrever codigo ou o documento principal
- `review`: Revisar diff, interrogar, auditar
- `qa`: Verificar no artefato real, testar e comprovar evidencia

## 3. Principios de Engenharia em Destaque
- **Prove que funciona**: Mostre evidencia factual em artefatos reais antes de dar por pronto.
- **Corrija a causa raiz**: Nao mascare sintomas; entenda o por que e resolva na origem.
- **Operacoes idempotentes**: Scripts e migracoes devem poder rodar N vezes com seguranca.
- **Teste comportamento, nao implementacao**: Testes devem sobreviver a refatoracoes.
- **Subtraia antes de somar**: Menos codigo e menos complexidade sao sempre preferiveis.
- **Proteja a janela de contexto**: Seja cirurgico, leia apenas o necessario e sintetize.

## 4. Catalogo de Skills da Etus
Para pesquisar skills e playbooks especializados da empresa pelo terminal:
`etus skills buscar <termo>`
"#
    );

    match target {
        Target::Antigravity if has_codex => {
            format!(
                "{base_header}As regras de conduta, engenharia e etiqueta de trabalho da Etus estao centralizadas em `AGENTS.md`.\n\n### Diretrizes para o Agente Antigravity\n- Respeite rigorosamente as convencoes de escrita e etiqueta de trabalho declaradas em `AGENTS.md`.\n- Quando planejar tarefas complexas, utilize artefatos de planejamento estruturados e solicite revisao quando necessario.\n- Valide cada etapa executando testes automatizados ou comandos factuais no terminal.\n"
            )
        }
        Target::Cursor => {
            format!("{base_header}{core_rules}")
        }
        Target::Codex => {
            format!(
                "{base_header}{core_rules}\n### Diretrizes de Execucao\n- Respeite rigorosamente as convencoes de escrita e etiqueta de trabalho acima.\n- Antes de declarar uma tarefa concluida, valide no artefato real ou execute os testes automatizados.\n- Foco estrito no escopo da demanda, sem introduzir complexidade ou dependencias nao solicitadas.\n"
            )
        }
        Target::Copilot => {
            format!(
                "{base_header}{core_rules}\n### Diretrizes para Sugestoes de Codigo\n- Siga estritamente as convencoes de nomenclatura e padroes de projeto existentes.\n- Nao gere comentarios redundantes que apenas repitam o que o codigo expressa.\n- Evite sugerir dependencias externas desnecessarias.\n"
            )
        }
        Target::Windsurf => {
            format!(
                "{base_header}{core_rules}\n### Diretrizes de Execucao\n- Siga as fases de trabalho e marque a issue correspondente com etus trabalho.\n- Valide sempre o resultado final com testes e evidencias factuais.\n"
            )
        }
        _ => {
            format!("{base_header}{core_rules}")
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SettingsMerge {
    pub text: String,
    pub added: Vec<String>,
    pub disabled: Vec<String>,
    pub custom_marketplace: bool,
}

fn is_default_marketplace(entry: &Value) -> bool {
    entry.pointer("/source/source").and_then(Value::as_str) == Some("github")
        && entry.pointer("/source/repo").and_then(Value::as_str) == Some(REPOSITORY)
}

pub fn update_claude_settings(existing_content: Option<&str>, wanted: &[String]) -> Result<SettingsMerge> {
    let mut root: Value = match existing_content {
        Some(s) if !s.trim().is_empty() => serde_json::from_str(s)
            .map_err(|e| Error::Other(format!("Configuração .claude/settings.json não é um JSON válido: {e}")))?,
        _ => json!({}),
    };

    if !root.is_object() {
        return Err(Error::Other(
            ".claude/settings.json deveria conter um objeto JSON.".into(),
        ));
    }

    let obj = root.as_object_mut().unwrap();

    let marketplaces = obj
        .entry("extraKnownMarketplaces")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| Error::Other("extraKnownMarketplaces deveria ser um objeto.".into()))?;

    let custom_marketplace = match marketplaces.get(MARKETPLACE) {
        Some(current) => !is_default_marketplace(current),
        None => {
            marketplaces.insert(
                MARKETPLACE.to_string(),
                json!({
                    "source": {
                        "source": "github",
                        "repo": REPOSITORY
                    }
                }),
            );
            false
        }
    };

    let plugins = obj
        .entry("enabledPlugins")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| Error::Other("enabledPlugins deveria ser um objeto.".into()))?;

    let mut added = Vec::new();
    let mut disabled = Vec::new();
    for plugin in wanted {
        match plugins.get(plugin) {
            None => {
                plugins.insert(plugin.clone(), json!(true));
                added.push(plugin.clone());
            }
            Some(Value::Bool(false)) => disabled.push(plugin.clone()),
            Some(_) => {}
        }
    }

    let mut text = serde_json::to_string_pretty(&root).map_err(|e| Error::Other(e.to_string()))?;
    text.push('\n');
    Ok(SettingsMerge {
        text,
        added,
        disabled,
        custom_marketplace,
    })
}

pub fn detect_environment() -> Option<Target> {
    if std::env::var("CURSOR_TRACE_ID").is_ok() || Path::new(".cursor").is_dir() {
        return Some(Target::Cursor);
    }
    if std::env::var("WINDSURF_PORT").is_ok() || Path::new(".windsurf").is_dir() {
        return Some(Target::Windsurf);
    }
    if std::env::var("ANTIGRAVITY_WORKSPACE").is_ok() || Path::new(".gemini").is_dir() {
        return Some(Target::Antigravity);
    }
    if std::env::var("CLAUDE_CODE_ENTRY").is_ok() || Path::new(".claude").is_dir() {
        return Some(Target::ClaudeCode);
    }
    None
}

pub fn detect_default_profile(root: &Path) -> &'static str {
    if root.join("package.json").is_file()
        || root.join("Cargo.toml").is_file()
        || root.join("Gemfile").is_file()
        || root.join("go.mod").is_file()
        || root.join("pyproject.toml").is_file()
    {
        return "codigo";
    }
    if root.join("PRD.md").is_file() || root.join("ROADMAP.md").is_file() {
        return "produto";
    }
    "codigo"
}

fn prompt_profile(profiles: &[&str], detected: &str) -> Result<String> {
    let default_idx = profiles.iter().position(|&p| p == detected).unwrap_or(0);

    let selection = Select::with_theme(&ColorfulTheme::default())
        .with_prompt("Escolha o perfil de trabalho")
        .default(default_idx)
        .items(profiles)
        .interact()
        .map_err(|e| Error::Other(e.to_string()))?;

    Ok(profiles[selection].to_string())
}

fn prompt_targets() -> Result<Vec<Target>> {
    let options = [
        "Todas as plataformas (Universal)",
        "Cursor (.cursor/rules/etus.mdc)",
        "Antigravity CLI (GEMINI.md)",
        "Windsurf Cascade (.windsurfrules)",
        "GitHub Copilot (.github/copilot-instructions.md)",
        "Codex / OpenAI (AGENTS.md)",
        "Claude Code (.claude/settings.json)",
        "Personalizado (marcar com espaço)...",
    ];

    let selection = Select::with_theme(&ColorfulTheme::default())
        .with_prompt("Escolha as plataformas de IA")
        .default(0)
        .items(options)
        .interact()
        .map_err(|e| Error::Other(e.to_string()))?;

    match selection {
        0 => Ok(Target::all()),
        1 => Ok(vec![Target::Cursor]),
        2 => Ok(vec![Target::Antigravity]),
        3 => Ok(vec![Target::Windsurf]),
        4 => Ok(vec![Target::Copilot]),
        5 => Ok(vec![Target::Codex]),
        6 => Ok(vec![Target::ClaudeCode]),
        7 => {
            let targets_list = [
                ("Cursor (.cursor/rules/etus.mdc)", Target::Cursor),
                ("Antigravity CLI (GEMINI.md)", Target::Antigravity),
                ("Windsurf Cascade (.windsurfrules)", Target::Windsurf),
                ("GitHub Copilot (.github/copilot-instructions.md)", Target::Copilot),
                ("Codex / OpenAI (AGENTS.md)", Target::Codex),
                ("Claude Code (.claude/settings.json)", Target::ClaudeCode),
            ];
            let labels: Vec<&str> = targets_list.iter().map(|(l, _)| *l).collect();
            let chosen = MultiSelect::with_theme(&ColorfulTheme::default())
                .with_prompt("Marque com Espaço e confirme com Enter")
                .items(&labels)
                .interact()
                .map_err(|e| Error::Other(e.to_string()))?;

            if chosen.is_empty() {
                Ok(Target::all())
            } else {
                Ok(chosen.into_iter().map(|i| targets_list[i].1).collect())
            }
        }
        _ => Ok(Target::all()),
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProjectConfig {
    pub project: Option<String>,
    pub workspace: Option<String>,
    pub perfil: Vec<String>,
    pub targets: Option<Vec<Target>>,
    pub mcp_mode: Option<McpMode>,
}

fn config_text(file: &Map<String, Value>, key: &str) -> Result<Option<String>> {
    match file.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.clone()).filter(|t| !t.trim().is_empty())),
        Some(_) => Err(Error::Usage(format!("{STATE_FILE}: \"{key}\" deveria ser um texto."))),
    }
}

fn config_list(file: &Map<String, Value>, key: &str, example: &str) -> Result<Option<Vec<String>>> {
    match file.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| item.as_str().map(str::to_string))
            .collect::<Option<Vec<String>>>()
            .map(Some)
            .ok_or_else(|| {
                Error::Usage(format!(
                    "{STATE_FILE}: \"{key}\" deveria ser uma lista, por exemplo {example}."
                ))
            }),
        Some(_) => Err(Error::Usage(format!(
            "{STATE_FILE}: \"{key}\" deveria ser uma lista, por exemplo {example}."
        ))),
    }
}

impl ProjectConfig {
    pub fn read(file: &Map<String, Value>) -> Result<ProjectConfig> {
        let targets = match config_list(file, "targets", "[\"claude-code\", \"cursor\"]")? {
            None => None,
            Some(list) if list.is_empty() => {
                return Err(Error::Usage(format!(
                    "{STATE_FILE}: \"targets\" não pode ser uma lista vazia."
                )));
            }
            Some(list) => {
                Some(parse_targets(Some(&list)).map_err(|e| Error::Usage(format!("{STATE_FILE}: {}", e.message())))?)
            }
        };
        let mcp_mode = match file.get("mcp").and_then(|m| m.get("modo")) {
            None | Some(Value::Null) => None,
            Some(value) => Some(value.as_str().and_then(McpMode::parse).ok_or_else(|| {
                Error::Usage(format!(
                    "{STATE_FILE}: \"mcp.modo\" deveria ser completo, quente ou compacto."
                ))
            })?),
        };
        Ok(ProjectConfig {
            project: config_text(file, "project")?,
            workspace: config_text(file, "workspace")?,
            perfil: config_list(file, "perfil", "[\"codigo\"]")?.unwrap_or_default(),
            targets,
            mcp_mode,
        })
    }

    pub fn write(&self, file: &mut Map<String, Value>) {
        if let Some(project) = &self.project {
            file.insert("project".into(), json!(project));
        }
        if let Some(workspace) = &self.workspace {
            file.insert("workspace".into(), json!(workspace));
        }
        file.insert("perfil".into(), json!(self.perfil));
        if let Some(targets) = &self.targets {
            file.insert(
                "targets".into(),
                json!(targets.iter().map(|t| t.id()).collect::<Vec<_>>()),
            );
        }
        if let Some(mode) = self.mcp_mode {
            let mcp = file.entry("mcp").or_insert_with(|| json!({}));
            if !mcp.is_object() {
                *mcp = json!({});
            }
            mcp["modo"] = json!(mode.id());
        }
    }
}

fn union<T: PartialEq + Clone>(first: &[T], second: &[T]) -> Vec<T> {
    let mut out = first.to_vec();
    for item in second {
        if !out.contains(item) {
            out.push(item.clone());
        }
    }
    out
}

fn check_inside(root: &Path, target_path: &Path, allow_outside: bool) -> Result<()> {
    if allow_outside {
        return Ok(());
    }
    let mut existing = target_path.to_path_buf();
    let mut rest = Vec::new();
    while !existing.exists() {
        let (Some(parent), Some(name)) = (existing.parent(), existing.file_name()) else {
            break;
        };
        rest.push(name.to_os_string());
        existing = parent.to_path_buf();
    }
    let mut canonical_target = existing.canonicalize().unwrap_or(existing);
    canonical_target.extend(rest.iter().rev());
    let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    if !canonical_target.starts_with(&canonical_root) {
        return Err(Error::Usage(format!(
            "O arquivo {} ficaria fora da raiz do projeto ({}). Use --allow-outside para permitir.",
            target_path.display(),
            root.display()
        )));
    }
    Ok(())
}

fn read_text(path: &Path) -> Result<Option<String>> {
    if !path.is_file() {
        return Ok(None);
    }
    fs::read_to_string(path)
        .map(Some)
        .map_err(|e| Error::Other(format!("Não foi possível ler {}: {e}", path.display())))
}

#[derive(Debug, Clone)]
pub struct FileWrite {
    pub path: PathBuf,
    pub scope: Scope,
    pub before: Option<String>,
    pub after: Option<String>,
}

impl FileWrite {
    pub fn status(&self) -> &'static str {
        match (&self.before, &self.after) {
            (_, None) => "mantido",
            (None, Some(_)) => "criado",
            (Some(before), Some(after)) if before == after => "igual",
            _ => "atualizado",
        }
    }

    fn changed(&self) -> bool {
        matches!(self.status(), "criado" | "atualizado")
    }
}

#[derive(Debug, Clone)]
pub struct McpOutcome {
    pub target: Target,
    pub file: McpFile,
    pub status: McpStatus,
    pub shared_with: Option<&'static str>,
}

#[derive(Debug, Clone)]
pub struct Inputs {
    pub targets: Vec<Target>,
    pub perfil: Vec<String>,
    pub project_text: String,
    pub workspace: String,
    pub with_templates: bool,
    pub mcp_mode: McpMode,
    pub explicit_mode: bool,
    pub force: bool,
    pub allow_outside: bool,
    pub hub: String,
    pub home: PathBuf,
}

#[derive(Debug, Default)]
pub struct Planned {
    pub writes: Vec<FileWrite>,
    pub rules: Vec<(Target, usize)>,
    pub mcp: Vec<McpOutcome>,
    pub settings: Option<SettingsMerge>,
    pub warnings: Vec<String>,
}

impl Planned {
    fn current(&self, path: &Path) -> Result<Option<String>> {
        match self.writes.iter().find(|w| w.path == path) {
            Some(write) => Ok(write.after.clone().or_else(|| write.before.clone())),
            None => read_text(path),
        }
    }

    fn put(&mut self, path: &Path, scope: Scope, after: Option<String>) -> Result<usize> {
        if let Some(index) = self.writes.iter().position(|w| w.path == path) {
            if after.is_some() {
                self.writes[index].after = after;
            }
            return Ok(index);
        }
        self.writes.push(FileWrite {
            path: path.to_path_buf(),
            scope,
            before: read_text(path)?,
            after,
        });
        Ok(self.writes.len() - 1)
    }
}

pub fn plan(root: &Path, inputs: &Inputs, profiles: &Profiles) -> Result<Planned> {
    let mut planned = Planned::default();
    let has_codex = inputs.targets.contains(&Target::Codex);
    let url = mcp_url(&inputs.hub, inputs.mcp_mode);
    for target in &inputs.targets {
        let target_path = root.join(target.relative_file());
        check_inside(root, &target_path, inputs.allow_outside)?;
        let existing = read_text(&target_path)?;
        let content = match target {
            Target::ClaudeCode => {
                let merged = update_claude_settings(
                    existing.as_deref(),
                    &profiles.plugins(&inputs.perfil, inputs.with_templates),
                )?;
                let text = merged.text.clone();
                planned.settings = Some(merged);
                text
            }
            _ => {
                let body = generate_body(
                    *target,
                    &inputs.perfil.join(", "),
                    &inputs.project_text,
                    &inputs.workspace,
                    has_codex,
                );
                apply_block(existing.as_deref(), &body, *target, inputs.force)?
            }
        };
        let index = planned.put(&target_path, Scope::Project, Some(content))?;
        planned.rules.push((*target, index));

        for file in mcp_files(*target, root, &inputs.home) {
            if file.scope == Scope::Project {
                check_inside(root, &file.path, inputs.allow_outside)?;
            }
            let shared_with = planned
                .mcp
                .iter()
                .find(|m| m.file.path == file.path)
                .map(|m| m.file.editor);
            let current = planned.current(&file.path)?;
            let label = file.path.display().to_string();
            let merged = merge(file.format, current.as_deref(), &url, inputs.explicit_mode, &label)?;
            match &merged.status {
                McpStatus::Kept { current } => planned.warnings.push(format!(
                    "{}: o etus em {label} já aponta para {current} (modo {}); ficou como estava. Para trocar: etus setup --mcp-modo {}",
                    file.editor,
                    mode_of(current),
                    inputs.mcp_mode.id()
                )),
                McpStatus::Replaced { previous } => planned.warnings.push(format!(
                    "{}: o etus em {label} foi trocado de {previous} para {url}.",
                    file.editor
                )),
                McpStatus::Created | McpStatus::Same => {}
            }
            planned.put(&file.path, file.scope, merged.text)?;
            planned.mcp.push(McpOutcome {
                target: *target,
                file,
                status: merged.status,
                shared_with,
            });
        }
    }
    Ok(planned)
}

fn apply_writes(planned: &Planned) -> Result<()> {
    for write in planned.writes.iter().filter(|w| w.changed()) {
        let Some(text) = &write.after else { continue };
        if let Some(parent) = write.path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| Error::Other(format!("Não foi possível criar pasta {}: {e}", parent.display())))?;
        }
        fs::write(&write.path, text)
            .map_err(|e| Error::Other(format!("Não foi possível gravar {}: {e}", write.path.display())))?;
    }
    Ok(())
}

fn home_dir() -> Result<PathBuf> {
    directories::BaseDirs::new()
        .map(|base| base.home_dir().to_path_buf())
        .ok_or_else(|| Error::Other("Não foi possível achar a pasta do usuário.".into()))
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
}

fn claude_notes(root: &Path, home: &Path, mode: McpMode) -> Vec<String> {
    let mut notes = Vec::new();
    let claude_json = match std::env::var_os("CLAUDE_CONFIG_DIR").filter(|d| !d.is_empty()) {
        Some(dir) => PathBuf::from(dir).join(".claude.json"),
        None => home.join(".claude.json"),
    };
    if let Some(value) = read_json(&claude_json) {
        let scopes = claude_scopes(&value, root);
        if let Some(local) = scopes.local {
            notes.push(format!(
                "Claude Code: há um etus local para esta pasta (modo {}), que vale mais que o .mcp.json. Para usar o do projeto: claude mcp remove etus -s local",
                mode_of(&local)
            ));
        } else if let Some(user) = scopes.user.filter(|u| mode_of(u) != mode.id()) {
            notes.push(format!(
                "Claude Code: o etus do usuário (modo {}) continua nos outros projetos; aqui vale o .mcp.json.",
                mode_of(&user)
            ));
        }
    }
    let routed = work::claude_home()
        .and_then(|dir| read_json(&dir.join("settings.json")))
        .and_then(|settings| router_base_url(&settings))
        .is_some();
    if routed && mode == McpMode::Completo {
        notes.push(
            "Claude Code passa pelo AI Router, que desliga o carregamento sob demanda de ferramentas: em modo completo, todas as ferramentas do etus entram em toda sessão. Prefira --mcp-modo quente.".into(),
        );
    }
    notes
}

fn relative(root: &Path, path: &Path) -> String {
    match path.strip_prefix(root) {
        Ok(rel) => rel.display().to_string(),
        Err(_) => path.display().to_string(),
    }
}

fn mcp_status_text(status: &McpStatus) -> String {
    match status {
        McpStatus::Created => "criado".into(),
        McpStatus::Same => "igual".into(),
        McpStatus::Kept { current } => format!("mantido, modo {}", mode_of(current)),
        McpStatus::Replaced { previous } => format!("trocado, era modo {}", mode_of(previous)),
    }
}

pub const NOT_SUPPORTED: [(&str, &str); 3] = [
    (
        "copilot",
        "Copilot coding agent na nuvem: não aceita MCP com OAuth (só token em header), então o etus fica de fora lá",
    ),
    (
        "windsurf",
        "Windsurf: não lê MCP do projeto; o etus vai no arquivo do usuário",
    ),
    (
        "codex",
        "Codex: .codex/config.toml só vale em projeto confiável; fora disso, use ~/.codex/config.toml",
    ),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Setup,
    Sync,
}

struct Context {
    root: PathBuf,
    file: Map<String, Value>,
    active: Active,
    config: ProjectConfig,
}

fn load_context(root: PathBuf) -> Result<Context> {
    let (file, active) = load_state(&root)?;
    let config = ProjectConfig::read(&file)?;
    Ok(Context {
        root,
        file,
        active,
        config,
    })
}

fn project_root(cwd: &Path) -> PathBuf {
    work::git_context(cwd).map(|git| git.top).unwrap_or(cwd.to_path_buf())
}

pub fn run(ctx: &Ctx, opts: SetupOptions) -> Result<()> {
    if ctx.store().load()?.is_none() {
        return Err(Error::Auth(
            "Você não está conectado ao Hub da Etus. Rode `etus login` antes de configurar o projeto.".to_string(),
        ));
    }

    let whoami_data = match ctx.whoami() {
        Ok(data) => data,
        Err(Error::Auth(msg)) => {
            return Err(Error::Auth(msg));
        }
        Err(Error::Network(msg)) => {
            eprintln!("etus: o hub não respondeu ({msg}); continuando a configuração do projeto.");
            Value::Null
        }
        Err(e) => return Err(e),
    };

    let connected_company = whoami_data
        .get("activeCompany")
        .and_then(|c| c.get("name"))
        .and_then(Value::as_str)
        .map(|s| s.to_string());
    let connected_person = whoami_data.get("person").and_then(|p| {
        let name = p.get("name").and_then(Value::as_str).unwrap_or("");
        let email = p.get("email").and_then(Value::as_str).unwrap_or("");
        if name.is_empty() {
            None
        } else {
            Some(format!("{name} <{email}>"))
        }
    });

    let cwd = std::env::current_dir().map_err(|e| Error::Other(e.to_string()))?;
    let context = load_context(project_root(&cwd))?;
    let profiles = Profiles::load()?;
    profiles.check(&context.config.perfil, &format!(" no {STATE_FILE}"))?;

    let is_interactive =
        opts.interativo && crate::confirm::interactive() && !ctx.yes && !ctx.dry_run && !ctx.out.json && !ctx.out.quiet;

    let asked = split_ids(&opts.perfis);
    profiles.check(&asked, "")?;
    let chosen = if !asked.is_empty() || !context.config.perfil.is_empty() {
        asked
    } else {
        let mut detected = profiles.detect(&context.root);
        if detected.is_empty() {
            detected.push(detect_default_profile(&context.root).to_string());
        }
        if is_interactive {
            vec![prompt_profile(&profiles.ids(), &detected[0])?]
        } else {
            detected
        }
    };
    let perfil = union(&context.config.perfil, &chosen);

    let stored_targets = context
        .config
        .targets
        .clone()
        .or_else(|| (!context.config.perfil.is_empty()).then(|| vec![Target::ClaudeCode]));
    let targets = match opts.targets.clone() {
        Some(t) if opts.replace_targets => t,
        Some(t) => union(&stored_targets.unwrap_or_default(), &t),
        None => match stored_targets {
            Some(t) => t,
            None if is_interactive => prompt_targets()?,
            None => Target::all(),
        },
    };

    let project = opts
        .projeto
        .clone()
        .map(|p| p.trim().to_uppercase())
        .filter(|p| !p.is_empty())
        .or(context.config.project.clone());
    let workspace = opts
        .workspace
        .clone()
        .filter(|w| !w.trim().is_empty())
        .or(context.config.workspace.clone())
        .unwrap_or_else(|| DEFAULT_WORKSPACE.to_string());
    let config = ProjectConfig {
        project,
        workspace: Some(workspace),
        perfil,
        targets: Some(targets),
        mcp_mode: opts.mcp_modo.or(context.config.mcp_mode),
    };
    let extra = Extra {
        company: connected_company,
        person: connected_person,
    };
    execute(ctx, context, config, &opts, &profiles, Mode::Setup, extra)
}

pub fn sync(ctx: &Ctx, opts: SetupOptions) -> Result<()> {
    let cwd = std::env::current_dir().map_err(|e| Error::Other(e.to_string()))?;
    let context = load_context(project_root(&cwd))?;
    if context.file.is_empty() {
        return Err(Error::Usage(format!(
            "{STATE_FILE} não encontrado em {}. Rode antes: etus setup",
            context.root.display()
        )));
    }
    if context.config.perfil.is_empty() {
        return Err(Error::Usage(format!(
            "Nenhum perfil declarado no {STATE_FILE}. Rode: etus setup <perfil>"
        )));
    }
    let profiles = Profiles::load()?;
    profiles.check(&context.config.perfil, &format!(" no {STATE_FILE}"))?;
    let mut config = context.config.clone();
    if config.targets.is_none() {
        config.targets = Some(vec![Target::ClaudeCode]);
    }
    config.mcp_mode = opts.mcp_modo.or(config.mcp_mode);
    execute(ctx, context, config, &opts, &profiles, Mode::Sync, Extra::default())
}

#[derive(Default)]
struct Extra {
    company: Option<String>,
    person: Option<String>,
}

fn execute(
    ctx: &Ctx,
    context: Context,
    config: ProjectConfig,
    opts: &SetupOptions,
    profiles: &Profiles,
    mode: Mode,
    extra: Extra,
) -> Result<()> {
    let root = context.root.clone();
    let targets = config.targets.clone().unwrap_or_default();
    let mcp_mode = config.mcp_mode.unwrap_or_default();
    let home = home_dir()?;
    let project_text = config.project.clone().unwrap_or_else(|| {
        work::git_context(&root)
            .map(|g| g.repo.to_uppercase())
            .filter(|r| !r.is_empty())
            .unwrap_or_else(|| "ETUS".to_string())
    });
    let workspace = config.workspace.clone().unwrap_or_else(|| DEFAULT_WORKSPACE.into());
    let inputs = Inputs {
        targets: targets.clone(),
        perfil: config.perfil.clone(),
        project_text,
        workspace: workspace.clone(),
        with_templates: opts.com_templates,
        mcp_mode,
        explicit_mode: opts.mcp_modo.is_some(),
        force: opts.force,
        allow_outside: opts.allow_outside,
        hub: ctx.settings.hub.clone(),
        home: home.clone(),
    };
    let state_path = root.join(STATE_FILE);
    check_inside(&root, &state_path, opts.allow_outside)?;
    let mut planned = plan(&root, &inputs, profiles)?;
    if targets.contains(&Target::ClaudeCode) {
        planned.warnings.extend(claude_notes(&root, &home, mcp_mode));
    }
    let mut file = context.file.clone();
    config.write(&mut file);
    let state_changed = file != context.file;
    if !ctx.dry_run {
        apply_writes(&planned)?;
        if state_changed {
            save_state(&root, file, &context.active)?;
        }
    }
    let globals = skill_locations().map(|at| global_notice(&at)).ok();
    let active_count = context.active.effective().len();

    let mut lines = Vec::new();
    let verb = match mode {
        Mode::Setup => "Projeto configurado",
        Mode::Sync => "Projeto sincronizado",
    };
    lines.push(format!(
        "✔ {verb} em {} (perfil: {}; workspace: {workspace}; MCP etus: modo {})",
        root.display(),
        config.perfil.join(", "),
        mcp_mode.id()
    ));
    if let Some(settings) = &planned.settings {
        if settings.custom_marketplace {
            lines.push(format!(
                "  Marketplace '{MARKETPLACE}' já tem uma origem customizada; mantida."
            ));
        }
        for plugin in &settings.disabled {
            lines.push(format!(
                "  {plugin} está desligado no .claude/settings.json; mantido. Para ligar: /plugin"
            ));
        }
    }
    let mut files_json = Vec::new();
    for target in &targets {
        lines.push(format!("{}:", target.name()));
        if let Some((_, index)) = planned.rules.iter().find(|(t, _)| t == target) {
            let write = &planned.writes[*index];
            let label = if *target == Target::ClaudeCode {
                "plugins"
            } else {
                "regras"
            };
            lines.push(format!(
                "  {label}: {} ({})",
                relative(&root, &write.path),
                write.status()
            ));
            files_json.push(json!({"target": target.id(), "tipo": label, "file": relative(&root, &write.path), "status": write.status()}));
        }
        for outcome in planned.mcp.iter().filter(|m| m.target == *target) {
            let scope = outcome.file.scope.label();
            let status = match outcome.shared_with {
                Some(editor) => format!("o mesmo arquivo do {editor}"),
                None => mcp_status_text(&outcome.status),
            };
            lines.push(format!(
                "  MCP etus ({}): {} [{scope}] ({status}); login: {}",
                outcome.file.editor,
                relative(&root, &outcome.file.path),
                outcome.file.login
            ));
            files_json.push(json!({
                "target": target.id(),
                "tipo": "mcp",
                "editor": outcome.file.editor,
                "file": relative(&root, &outcome.file.path),
                "escopo": scope,
                "status": mcp_status_text(&outcome.status),
            }));
        }
        lines.push(format!(
            "  skills dos apps (etus ativar): {}",
            skills_note(*target, &targets)
        ));
        if let Some((_, text)) = NOT_SUPPORTED.iter().find(|(id, _)| *id == target.id()) {
            lines.push(format!("  não suporta: {text}"));
        }
    }
    lines.push(format!(
        "{STATE_FILE}: {}",
        match (state_changed, context.file.is_empty()) {
            (false, _) => "igual",
            (true, true) => "criado",
            (true, false) => "atualizado",
        }
    ));
    if config.project.is_none() {
        lines.push(
            "Aviso: projeto do Plane não definido no .etus.json; as regras usam o nome do repositório. Use --projeto <CHAVE>.".into(),
        );
    }
    for warning in &planned.warnings {
        lines.push(format!("Aviso: {warning}"));
    }
    if ctx.dry_run {
        lines.push("Simulação (--dry-run): nada foi gravado.".into());
    } else {
        if let Some(settings) = &planned.settings {
            lines.push(format!(
                "Plugins novos: {}",
                if settings.added.is_empty() {
                    "nenhum".to_string()
                } else {
                    settings.added.join(", ")
                }
            ));
        }
        lines.push(String::new());
        lines.push(if active_count > 0 {
            format!(
                "💡 Próximo passo: etus ativar (gera de novo as skills dos {active_count} apps ativos nas pastas acima)"
            )
        } else {
            "💡 Próximo passo: etus ativar <app|pacote|agente:slug> (skills só dos apps deste projeto)".to_string()
        });
    }
    if let Some((_, Some(text))) = &globals {
        lines.push(text.clone());
    }

    ctx.out.print_human(&lines.join("\n"), || {
        let mut summary = json!({
            "status": "ok",
            "raiz": root.display().to_string(),
            "perfil": config.perfil,
            "projeto": config.project,
            "workspace": workspace,
            "targets": targets.iter().map(|t| t.id()).collect::<Vec<_>>(),
            "mcpModo": mcp_mode.id(),
            "files": files_json,
            "skills": targets.iter().map(|t| (t.id().to_string(), json!(skills_note(*t, &targets)))).collect::<Map<String, Value>>(),
            "naoSuporta": NOT_SUPPORTED.iter().filter(|(id, _)| targets.iter().any(|t| t.id() == *id)).map(|(_, text)| *text).collect::<Vec<_>>(),
            "avisos": planned.warnings,
            "skillsGlobais": globals.as_ref().map_or(0, |(n, _)| *n),
            "plugins": planned.settings.as_ref().map(|s| json!({"novos": s.added, "desligados": s.disabled})),
        });
        if let Some(person) = &extra.person {
            summary["connectedUser"] = json!(person);
        }
        if let Some(company) = &extra.company {
            summary["connectedCompany"] = json!(company);
        }
        summary
    });
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    Ok,
    Warn,
    Fail,
}

impl Check {
    fn label(self) -> &'static str {
        match self {
            Check::Ok => "OK",
            Check::Warn => "AVISO",
            Check::Fail => "FALHA",
        }
    }
}

pub fn diagnose_at(root: &Path, home: &Path, hub: &str) -> Vec<(Check, String)> {
    let mut checks = Vec::new();
    let mut add = |check: Check, text: String| checks.push((check, text));
    let git = root.join(".git").exists();
    add(
        if git { Check::Ok } else { Check::Warn },
        format!("Repositório Git: {}", if git { "presente" } else { "não detectado" }),
    );
    let profiles = match Profiles::load() {
        Ok(p) => p,
        Err(e) => {
            add(Check::Fail, e.message().to_string());
            return checks;
        }
    };
    let (config, active) = match load_state(root).and_then(|(file, active)| {
        if file.is_empty() {
            Ok((None, active))
        } else {
            ProjectConfig::read(&file).map(|c| (Some(c), active))
        }
    }) {
        Ok(pair) => pair,
        Err(e) => {
            add(Check::Fail, format!("{STATE_FILE}: {}", e.message()));
            (None, Active::default())
        }
    };
    let Some(config) = config else {
        add(Check::Warn, format!("{STATE_FILE}: ausente (rode etus setup)"));
        return checks;
    };
    add(
        if config.project.is_some() {
            Check::Ok
        } else {
            Check::Warn
        },
        format!(
            "{STATE_FILE}: projeto {}, perfil {}",
            config.project.as_deref().unwrap_or("não definido"),
            if config.perfil.is_empty() {
                "nenhum".to_string()
            } else {
                config.perfil.join(", ")
            }
        ),
    );
    if let Err(e) = profiles.check(&config.perfil, &format!(" no {STATE_FILE}")) {
        add(Check::Fail, e.message().to_string());
    }
    let targets = config.targets.clone().unwrap_or_else(|| vec![Target::ClaudeCode]);
    let wanted_url = mcp_url(hub, config.mcp_mode.unwrap_or_default());
    for target in &targets {
        let path = root.join(target.relative_file());
        let text = fs::read_to_string(&path).ok();
        match (target, text) {
            (_, None) => add(
                Check::Fail,
                format!(
                    "{}: falta {} (rode etus setup sincronizar)",
                    target.name(),
                    target.relative_file()
                ),
            ),
            (Target::ClaudeCode, Some(text)) => match serde_json::from_str::<Value>(&text) {
                Err(e) => add(Check::Fail, format!(".claude/settings.json: JSON inválido ({e})")),
                Ok(settings) => {
                    match settings.pointer(&format!("/extraKnownMarketplaces/{MARKETPLACE}")) {
                        None => add(Check::Fail, format!("Marketplace '{MARKETPLACE}': ausente")),
                        Some(entry) => add(
                            Check::Ok,
                            format!(
                                "Marketplace '{MARKETPLACE}': {}",
                                if is_default_marketplace(entry) {
                                    "configurado"
                                } else {
                                    "configurado com origem customizada"
                                }
                            ),
                        ),
                    }
                    let enabled = settings.get("enabledPlugins").and_then(Value::as_object);
                    let expected = profiles.plugins(&config.perfil, false);
                    let missing: Vec<&str> = expected
                        .iter()
                        .filter(|p| enabled.is_none_or(|e| !e.contains_key(*p)))
                        .map(String::as_str)
                        .collect();
                    let disabled: Vec<&str> = expected
                        .iter()
                        .filter(|p| enabled.and_then(|e| e.get(*p)) == Some(&json!(false)))
                        .map(String::as_str)
                        .collect();
                    if missing.is_empty() {
                        add(Check::Ok, format!("Plugins do perfil: {}", expected.join(", ")));
                    } else {
                        add(
                            Check::Fail,
                            format!(
                                "Faltam plugins do perfil: {} (rode etus setup sincronizar)",
                                missing.join(", ")
                            ),
                        );
                    }
                    if !disabled.is_empty() {
                        add(Check::Warn, format!("Desligados de propósito: {}", disabled.join(", ")));
                    }
                }
            },
            (_, Some(text)) => add(
                if text.contains(BLOCK_BEGIN) {
                    Check::Ok
                } else {
                    Check::Warn
                },
                format!(
                    "{}: {}{}",
                    target.name(),
                    target.relative_file(),
                    if text.contains(BLOCK_BEGIN) {
                        ""
                    } else {
                        " sem o bloco da Etus"
                    }
                ),
            ),
        }
        for file in mcp_files(*target, root, home) {
            let shown = relative(root, &file.path);
            let found = fs::read_to_string(&file.path)
                .ok()
                .map(|text| merge(file.format, Some(&text), &wanted_url, false, &shown));
            match found {
                None
                | Some(Ok(McpMerge {
                    status: McpStatus::Created,
                    ..
                })) => add(
                    Check::Warn,
                    format!(
                        "MCP etus ({}): ausente em {shown} (rode etus setup sincronizar)",
                        file.editor
                    ),
                ),
                Some(Ok(McpMerge {
                    status: McpStatus::Kept { current },
                    ..
                })) => add(
                    Check::Warn,
                    format!(
                        "MCP etus ({}): {shown} em modo {}, diferente do esperado ({})",
                        file.editor,
                        mode_of(&current),
                        mode_of(&wanted_url)
                    ),
                ),
                Some(Ok(_)) => add(
                    Check::Ok,
                    format!("MCP etus ({}): {shown}, modo {}", file.editor, mode_of(&wanted_url)),
                ),
                Some(Err(e)) => add(Check::Fail, format!("MCP etus ({}): {}", file.editor, e.message())),
            }
        }
    }
    if !active.is_empty() {
        let mut dirs = Vec::new();
        if targets.contains(&Target::ClaudeCode) {
            dirs.push(CLAUDE_SKILLS);
        }
        if needs_shared_skills(&targets) {
            dirs.push(SHARED_SKILLS);
        }
        for dir in dirs {
            let generated = skills::synced_with(&root.join(dir), &[ACTIVATE_MARKER]).len();
            let wanted = active.effective().len();
            add(
                if generated >= wanted { Check::Ok } else { Check::Warn },
                format!("Skills dos apps ativos em {dir}: {generated} de {wanted} (rode etus ativar)"),
            );
        }
    }
    let legacy = root.join(".cursorrules");
    if let Ok(text) = fs::read_to_string(&legacy)
        && (text.contains("Regras de Engenharia da Etus (Cursor)") || text.contains(BLOCK_BEGIN))
    {
        add(
            Check::Warn,
            ".cursorrules legado com regras da Etus: o Cursor usa .cursor/rules/etus.mdc. Remova o .cursorrules para não duplicar contexto.".into(),
        );
    }
    checks
}

pub fn diagnose(ctx: &Ctx) -> Result<()> {
    let cwd = std::env::current_dir().map_err(|e| Error::Other(e.to_string()))?;
    let root = project_root(&cwd);
    let home = home_dir()?;
    let mut checks = diagnose_at(&root, &home, &ctx.settings.hub);
    if let Ok((file, _)) = load_state(&root)
        && let Ok(config) = ProjectConfig::read(&file)
        && config
            .targets
            .as_deref()
            .unwrap_or(&[Target::ClaudeCode])
            .contains(&Target::ClaudeCode)
    {
        checks.extend(
            claude_notes(&root, &home, config.mcp_mode.unwrap_or_default())
                .into_iter()
                .map(|text| (Check::Warn, text)),
        );
    }
    if let Ok(at) = skill_locations()
        && let (_, Some(text)) = global_notice(&at)
    {
        checks.push((Check::Warn, text));
    }
    let failed = checks.iter().any(|(c, _)| *c == Check::Fail);
    let mut lines = vec![format!("Diagnóstico do projeto em {}:", root.display())];
    lines.extend(checks.iter().map(|(c, text)| format!("  [{}] {text}", c.label())));
    ctx.out.print_human(&lines.join("\n"), || {
        json!({
            "raiz": root.display().to_string(),
            "ok": !failed,
            "checks": checks.iter().map(|(c, text)| json!({"status": c.label(), "texto": text})).collect::<Vec<_>>(),
        })
    });
    if failed { Err(Error::Exit(1)) } else { Ok(()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_target_aliases_and_all() {
        let targets = parse_targets(Some(&["cursor".into(), "gemini".into()])).unwrap();
        assert_eq!(targets, vec![Target::Cursor, Target::Antigravity]);

        let all = parse_targets(Some(&["all".into()])).unwrap();
        assert_eq!(all.len(), 6);

        let default_targets = parse_targets(None).unwrap();
        assert_eq!(default_targets, Target::all());
    }

    #[test]
    fn rejects_unknown_targets() {
        let err = parse_targets(Some(&["vscode_fake".into()])).unwrap_err();
        assert!(matches!(err, Error::Usage(_)));
    }

    #[test]
    fn applies_block_to_new_file() {
        let res = apply_block(None, "minhas regras", Target::Codex, false).unwrap();
        assert!(res.contains(BLOCK_BEGIN));
        assert!(res.contains("minhas regras"));
        assert!(res.contains(BLOCK_END));
    }

    #[test]
    fn adds_cursor_frontmatter_on_new_file() {
        let res = apply_block(None, "regras cursor", Target::Cursor, false).unwrap();
        assert!(res.starts_with("---\ndescription:"));
        assert!(res.contains("alwaysApply: true"));
        assert!(res.contains(BLOCK_BEGIN));
    }

    #[test]
    fn updates_existing_block_preserving_outside_content() {
        let original = "linha antes\n<!-- etus-skills:begin -->\nvelho\n<!-- etus-skills:end -->\nlinha depois\n";
        let res = apply_block(Some(original), "novo conteudo", Target::Codex, false).unwrap();
        assert!(res.starts_with("linha antes\n"));
        assert!(res.contains("novo conteudo"));
        assert!(!res.contains("velho"));
        assert!(res.ends_with("linha depois\n"));
    }

    #[test]
    fn refuses_existing_file_without_markers_unless_forced() {
        let original = "arquivo customizado do dev sem marcadores\n";
        let err = apply_block(Some(original), "regras", Target::Codex, false).unwrap_err();
        assert!(matches!(err, Error::Usage(_)));

        let res = apply_block(Some(original), "regras", Target::Codex, true).unwrap();
        assert!(res.starts_with("arquivo customizado do dev"));
        assert!(res.contains(BLOCK_BEGIN));
    }

    #[test]
    fn ignores_markers_inside_code_fence() {
        let inside_fence = "```markdown\n<!-- etus-skills:begin -->\n```\n";
        let err = apply_block(Some(inside_fence), "regras", Target::Codex, false).unwrap_err();
        assert!(matches!(err, Error::Usage(_)));
    }

    #[test]
    fn ignores_indented_markers_with_four_spaces() {
        let indented = "    <!-- etus-skills:begin -->\n";
        let err = apply_block(Some(indented), "regras", Target::Codex, false).unwrap_err();
        assert!(matches!(err, Error::Usage(_)));
    }

    #[test]
    fn detects_corrupt_markers() {
        let inverted = "<!-- etus-skills:end -->\n<!-- etus-skills:begin -->\n";
        let err = apply_block(Some(inverted), "regras", Target::Codex, false).unwrap_err();
        assert!(matches!(err, Error::Other(_)));

        let orphan = "<!-- etus-skills:begin -->\n";
        let err_orphan = apply_block(Some(orphan), "regras", Target::Codex, false).unwrap_err();
        assert!(matches!(err_orphan, Error::Other(_)));
    }

    fn plugins(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|p| p.to_string()).collect()
    }

    #[test]
    fn merges_claude_settings_json() {
        let initial = r#"{"theme": "dark"}"#;
        let res = update_claude_settings(Some(initial), &plugins(&["etus-core@etus", "etus-dev@etus"])).unwrap();
        let parsed: Value = serde_json::from_str(&res.text).unwrap();
        assert_eq!(parsed["theme"], "dark");
        assert_eq!(
            parsed["extraKnownMarketplaces"]["etus"]["source"]["repo"],
            "evolution-foundation/etus-skills"
        );
        assert_eq!(parsed["enabledPlugins"]["etus-core@etus"], true);
        assert_eq!(res.added, vec!["etus-core@etus", "etus-dev@etus"]);
    }

    #[test]
    fn settings_keep_custom_marketplace_and_plugins_turned_off() {
        let initial = r#"{"extraKnownMarketplaces": {"etus": {"source": {"source": "directory", "path": "/dev/etus-skills"}}},
            "enabledPlugins": {"etus-dev@etus": false, "foo@other": true}}"#;
        let res = update_claude_settings(Some(initial), &plugins(&["etus-core@etus", "etus-dev@etus"])).unwrap();
        let parsed: Value = serde_json::from_str(&res.text).unwrap();
        assert_eq!(
            parsed["extraKnownMarketplaces"]["etus"]["source"]["source"],
            "directory"
        );
        assert_eq!(parsed["enabledPlugins"]["etus-dev@etus"], false);
        assert_eq!(parsed["enabledPlugins"]["foo@other"], true);
        assert!(res.custom_marketplace);
        assert_eq!(res.added, vec!["etus-core@etus"]);
        assert_eq!(res.disabled, vec!["etus-dev@etus"]);
    }

    #[test]
    fn project_config_round_trips_and_keeps_other_keys() {
        let mut file: Map<String, Value> = serde_json::from_str(
            r#"{"project": "ENG", "perfil": ["codigo"], "targets": ["claude-code", "gemini"], "mcp": {"modo": "compacto"}, "active": {"apps": ["tasks"]}}"#,
        )
        .unwrap();
        let mut config = ProjectConfig::read(&file).unwrap();
        assert_eq!(config.targets, Some(vec![Target::ClaudeCode, Target::Antigravity]));
        assert_eq!(config.mcp_mode, Some(McpMode::Compacto));
        assert_eq!(config.workspace, None);
        config.workspace = Some("etus".into());
        config.mcp_mode = Some(McpMode::Quente);
        config.write(&mut file);
        assert_eq!(file["targets"], json!(["claude-code", "antigravity"]));
        assert_eq!(file["mcp"], json!({"modo": "quente"}));
        assert_eq!(file["active"], json!({"apps": ["tasks"]}));
        for bad in [
            r#"{"perfil": "codigo"}"#,
            r#"{"targets": []}"#,
            r#"{"targets": ["vim"]}"#,
            r#"{"mcp": {"modo": "tudo"}}"#,
            r#"{"project": 1}"#,
        ] {
            let file: Map<String, Value> = serde_json::from_str(bad).unwrap();
            assert_eq!(ProjectConfig::read(&file).unwrap_err().exit_code(), 2, "{bad}");
        }
    }

    fn inputs(home: &Path, targets: Vec<Target>) -> Inputs {
        Inputs {
            targets,
            perfil: vec!["codigo".into()],
            project_text: "ENG".into(),
            workspace: "etus".into(),
            with_templates: false,
            mcp_mode: McpMode::Quente,
            explicit_mode: false,
            force: false,
            allow_outside: false,
            hub: "https://apps.etus.io".into(),
            home: home.to_path_buf(),
        }
    }

    #[test]
    fn plan_writes_rules_plugins_and_mcp_for_every_target() {
        let root = crate::testutil::temp_dir("setup-plan");
        let home = crate::testutil::temp_dir("setup-plan-home");
        let profiles = Profiles::load().unwrap();
        let planned = plan(&root, &inputs(&home, Target::all()), &profiles).unwrap();
        apply_writes(&planned).unwrap();
        let url = "https://apps.etus.io/mcp?modo=quente";
        let read =
            |path: &str| -> Value { serde_json::from_str(&fs::read_to_string(root.join(path)).unwrap()).unwrap() };
        assert_eq!(
            read(".mcp.json")["mcpServers"]["etus"],
            json!({"type": "http", "url": url})
        );
        assert_eq!(read(".cursor/mcp.json")["mcpServers"]["etus"], json!({"url": url}));
        assert_eq!(
            read(".gemini/settings.json")["mcpServers"]["etus"],
            json!({"httpUrl": url})
        );
        assert_eq!(
            read(".agents/mcp_config.json")["mcpServers"]["etus"],
            json!({"serverUrl": url})
        );
        assert!(
            fs::read_to_string(root.join(".codex/config.toml"))
                .unwrap()
                .contains(url)
        );
        assert!(home.join(".config/devin/mcp_config.json").is_file());
        assert_eq!(
            planned
                .writes
                .iter()
                .filter(|w| w.path == root.join(".mcp.json"))
                .count(),
            1
        );
        for target in Target::all() {
            assert!(root.join(target.relative_file()).is_file(), "{}", target.id());
        }
        assert_eq!(planned.mcp.len(), 7);

        let again = plan(&root, &inputs(&home, Target::all()), &profiles).unwrap();
        assert!(again.writes.iter().all(|w| !w.changed()), "idempotent");

        let mut completo = inputs(&home, vec![Target::Cursor]);
        completo.mcp_mode = McpMode::Completo;
        let kept = plan(&root, &completo, &profiles).unwrap();
        assert!(matches!(kept.mcp[0].status, McpStatus::Kept { .. }));
        assert_eq!(kept.warnings.len(), 1);
        completo.explicit_mode = true;
        let swapped = plan(&root, &completo, &profiles).unwrap();
        assert!(matches!(swapped.mcp[0].status, McpStatus::Replaced { .. }));
    }

    #[test]
    fn diagnose_reports_missing_pieces_and_passes_after_setup() {
        let root = crate::testutil::temp_dir("setup-doctor");
        let home = crate::testutil::temp_dir("setup-doctor-home");
        let hub = "https://apps.etus.io";
        let checks = diagnose_at(&root, &home, hub);
        assert!(checks.iter().any(|(c, t)| *c == Check::Warn && t.contains("ausente")));
        fs::write(
            root.join(STATE_FILE),
            r#"{"project": "ENG", "perfil": ["codigo"], "targets": ["claude-code", "cursor"]}"#,
        )
        .unwrap();
        let checks = diagnose_at(&root, &home, hub);
        assert!(checks.iter().any(|(c, _)| *c == Check::Fail));
        let profiles = Profiles::load().unwrap();
        let planned = plan(
            &root,
            &inputs(&home, vec![Target::ClaudeCode, Target::Cursor]),
            &profiles,
        )
        .unwrap();
        apply_writes(&planned).unwrap();
        let checks = diagnose_at(&root, &home, hub);
        assert!(checks.iter().all(|(c, _)| *c != Check::Fail), "{checks:?}");
        assert!(
            checks
                .iter()
                .any(|(_, t)| t.contains("MCP etus (Cursor)") && t.contains("modo quente"))
        );
    }

    #[test]
    fn run_fails_if_not_logged_in() {
        let dir = crate::testutil::temp_dir("setup-auth-test");
        let paths = crate::config::Paths::at(&dir);
        let settings = crate::config::Settings::resolve(paths, Some("https://apps.etus.io"), None).unwrap();
        let ctx = Ctx::new(
            settings,
            crate::output::Output {
                json: true,
                quiet: true,
            },
            true,
            false,
        );
        let opts = SetupOptions {
            targets: Some(vec![Target::Cursor]),
            perfis: vec!["codigo".into()],
            projeto: Some("TEST".into()),
            workspace: Some("etus".into()),
            ..SetupOptions::default()
        };
        let err = run(&ctx, opts).unwrap_err();
        assert!(matches!(err, Error::Auth(_)));
        assert_eq!(err.exit_code(), 3);
        assert!(err.message().contains("etus login"));
    }
}
