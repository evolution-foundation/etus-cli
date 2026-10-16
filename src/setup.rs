use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::path::Path;

use serde_json::{Value, json};

use crate::context::Ctx;
use crate::error::{Error, Result};
use crate::output::println_safe;
use crate::work;

pub const BLOCK_BEGIN: &str = "<!-- etus-skills:begin -->";
pub const BLOCK_END: &str = "<!-- etus-skills:end -->";

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

#[derive(Debug, Clone)]
pub struct SetupOptions {
    pub targets: Option<Vec<Target>>,
    pub perfil: Option<String>,
    pub projeto: Option<String>,
    pub workspace: Option<String>,
    pub force: bool,
    pub allow_outside: bool,
    pub interativo: bool,
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

pub fn update_claude_settings(existing_content: Option<&str>) -> Result<String> {
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

    marketplaces.insert(
        "etus".to_string(),
        json!({
            "source": {
                "source": "github",
                "repo": "evolution-foundation/etus-skills"
            }
        }),
    );

    let plugins = obj
        .entry("enabledPlugins")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| Error::Other("enabledPlugins deveria ser um objeto.".into()))?;

    plugins.insert("etus-core@etus".to_string(), json!(true));
    plugins.insert("etus-dev@etus".to_string(), json!(true));

    serde_json::to_string_pretty(&root).map_err(|e| Error::Other(e.to_string()))
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

fn prompt_profile(detected: &str) -> Result<String> {
    print!("Perfil de engenharia [{detected}] (Enter para confirmar ou digite outro): ");
    std::io::stdout().flush().map_err(|e| Error::Other(e.to_string()))?;

    let mut input = String::new();
    std::io::stdin()
        .read_line(&mut input)
        .map_err(|e| Error::Other(e.to_string()))?;
    let clean = input.trim();

    if clean.is_empty() {
        Ok(detected.to_string())
    } else {
        Ok(clean.to_string())
    }
}

fn prompt_targets() -> Result<Vec<Target>> {
    println!("Plataformas de IA:");
    println!("  [1] Todas (Cursor, Antigravity, Windsurf, Copilot, Codex, Claude Code)");
    println!("  [2] Cursor  [3] Antigravity  [4] Windsurf  [5] Copilot  [6] Codex  [7] Claude Code");
    print!("Escolha [1-7 ou vírgulas, padrão 1]: ");
    std::io::stdout().flush().map_err(|e| Error::Other(e.to_string()))?;

    let mut input = String::new();
    std::io::stdin()
        .read_line(&mut input)
        .map_err(|e| Error::Other(e.to_string()))?;
    let clean = input.trim();

    if clean.is_empty() || clean == "1" || clean.eq_ignore_ascii_case("all") {
        return Ok(Target::all());
    }

    let mut out = BTreeSet::new();
    for part in clean.split(',') {
        match part.trim() {
            "1" => return Ok(Target::all()),
            "2" => {
                out.insert(Target::Cursor);
            }
            "3" => {
                out.insert(Target::Antigravity);
            }
            "4" => {
                out.insert(Target::Windsurf);
            }
            "5" => {
                out.insert(Target::Copilot);
            }
            "6" => {
                out.insert(Target::Codex);
            }
            "7" => {
                out.insert(Target::ClaudeCode);
            }
            other => {
                if let Some(t) = Target::from_str_lenient(other) {
                    out.insert(t);
                }
            }
        }
    }

    if out.is_empty() {
        Ok(Target::all())
    } else {
        Ok(out.into_iter().collect())
    }
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
    let root = work::git_context(&cwd).map(|git| git.top).unwrap_or(cwd.clone());

    let is_interactive =
        opts.interativo && crate::confirm::interactive() && !ctx.yes && !ctx.dry_run && !ctx.out.json && !ctx.out.quiet;

    let perfil = match opts.perfil {
        Some(p) if !p.trim().is_empty() => p,
        _ => {
            let detected = detect_default_profile(&root);
            if is_interactive {
                prompt_profile(detected)?
            } else {
                detected.to_string()
            }
        }
    };

    let targets = match opts.targets {
        Some(t) => t,
        None => {
            if is_interactive {
                prompt_targets()?
            } else {
                Target::all()
            }
        }
    };

    let git_ctx = work::git_context(&cwd);
    let projeto = opts
        .projeto
        .or_else(|| {
            git_ctx.as_ref().and_then(|g| {
                if g.repo.is_empty() {
                    None
                } else {
                    Some(g.repo.to_uppercase())
                }
            })
        })
        .unwrap_or_else(|| "ETUS".to_string());

    let workspace = opts
        .workspace
        .or(connected_company.clone())
        .unwrap_or_else(|| "etus".to_string());
    let has_codex = targets.contains(&Target::Codex);

    let mut generated_files = Vec::new();

    for target in &targets {
        let rel_file = target.relative_file();
        let target_path = root.join(rel_file);

        if !opts.allow_outside {
            let canonical_target = if target_path.exists() {
                target_path.canonicalize().map_err(|e| Error::Other(e.to_string()))?
            } else {
                let parent = target_path.parent().unwrap_or(&root);
                if parent.exists() {
                    parent
                        .canonicalize()
                        .map_err(|e| Error::Other(e.to_string()))?
                        .join(target_path.file_name().unwrap())
                } else {
                    target_path.clone()
                }
            };
            let canonical_root = root.canonicalize().unwrap_or_else(|_| root.clone());
            if !canonical_target.starts_with(&canonical_root) {
                return Err(Error::Usage(format!(
                    "O arquivo {} ficaria fora da raiz do projeto ({}). Use --allow-outside para permitir.",
                    target_path.display(),
                    root.display()
                )));
            }
        }

        let existing_content = if target_path.is_file() {
            Some(
                fs::read_to_string(&target_path)
                    .map_err(|e| Error::Other(format!("Não foi possível ler {}: {e}", target_path.display())))?,
            )
        } else {
            None
        };

        let final_content = match target {
            Target::ClaudeCode => update_claude_settings(existing_content.as_deref())?,
            _ => {
                let body = generate_body(*target, &perfil, &projeto, &workspace, has_codex);
                apply_block(existing_content.as_deref(), &body, *target, opts.force)?
            }
        };

        if !ctx.dry_run {
            if let Some(parent) = target_path.parent() {
                fs::create_dir_all(parent)
                    .map_err(|e| Error::Other(format!("Não foi possível criar pasta {}: {e}", parent.display())))?;
            }
            fs::write(&target_path, &final_content)
                .map_err(|e| Error::Other(format!("Não foi possível gravar {}: {e}", target_path.display())))?;
        }

        generated_files.push(json!({
            "target": target.id(),
            "file": rel_file,
            "status": if existing_content.is_some() { "atualizado" } else { "criado" }
        }));
    }

    let summary = json!({
        "status": "ok",
        "perfil": perfil,
        "projeto": projeto,
        "workspace": workspace,
        "targets": targets.iter().map(|t| t.id()).collect::<Vec<_>>(),
        "files": generated_files
    });

    if ctx.out.json {
        let mut summary_val = summary;
        if let Some(person) = &connected_person {
            summary_val["connectedUser"] = json!(person);
        }
        if let Some(comp) = &connected_company {
            summary_val["connectedCompany"] = json!(comp);
        }
        println_safe(&summary_val.to_string());
    } else {
        println_safe(&format!("✔ Projeto configurado para {workspace} (perfil: {perfil})"));
        for f in &generated_files {
            let file_str = f["file"].as_str().unwrap_or("");
            println_safe(&format!("  • {file_str}"));
        }
        println_safe("");
        println_safe("💡 Próximo passo: etus skills sincronizar --projeto");
    }

    Ok(())
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

    #[test]
    fn merges_claude_settings_json() {
        let initial = r#"{"theme": "dark"}"#;
        let res = update_claude_settings(Some(initial)).unwrap();
        let parsed: Value = serde_json::from_str(&res).unwrap();
        assert_eq!(parsed["theme"], "dark");
        assert_eq!(
            parsed["extraKnownMarketplaces"]["etus"]["source"]["repo"],
            "evolution-foundation/etus-skills"
        );
        assert_eq!(parsed["enabledPlugins"]["etus-core@etus"], true);
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
            perfil: Some("codigo".into()),
            projeto: Some("TEST".into()),
            workspace: Some("etus".into()),
            force: false,
            allow_outside: false,
            interativo: false,
        };
        let err = run(&ctx, opts).unwrap_err();
        assert!(matches!(err, Error::Auth(_)));
        assert_eq!(err.exit_code(), 3);
        assert!(err.message().contains("etus login"));
    }
}
