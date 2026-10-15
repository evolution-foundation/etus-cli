use std::collections::{BTreeMap, BTreeSet};
use std::io::{IsTerminal, Read};
use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::ArgMatches;
use serde_json::{Map, Value, json};

use crate::action::{self, Action};
use crate::auth::oauth::{self, AuthorizationRequest};
use crate::auth::store::{StoreKind, keychain_supported, store_for};
use crate::catalog::{App, Tool, find_app};
use crate::cli::{self, Commands, ConfigCmd, EmpresaCmd, PerfilCmd, SkillsCmd, Toggle, TrabalhoCmd};
use crate::config;
use crate::confirm::{self, Decision};
use crate::context::{Ctx, SET_COMPANY_TOOL};
use crate::error::{EXIT_USAGE, Error, Result};
use crate::naming::slug;
use crate::output::{Output, pretty};
use crate::schema;
use crate::skills;
use crate::work;

const SKILLS_SEARCH_TOOL: &str = "extensoes__catalogo_buscar";
const SKILLS_READ_TOOL: &str = "extensoes__catalogo_ler";
const LOGIN_TIMEOUT: Duration = Duration::from_secs(300);

pub fn dispatch(ctx: &Ctx, command: Commands) -> Result<()> {
    match command {
        Commands::Login {
            no_browser,
            somente_leitura,
            novo_cliente,
        } => login(ctx, no_browser, somente_leitura, novo_cliente),
        Commands::Logout => logout(ctx),
        Commands::Whoami => whoami(ctx),
        Commands::Apps => apps(ctx),
        Commands::Empresa { acao } => match acao.unwrap_or(EmpresaCmd::Listar) {
            EmpresaCmd::Listar => companies(ctx),
            EmpresaCmd::Usar { empresa } => use_company(ctx, &empresa),
        },
        Commands::Perfil { acao } => profile(ctx, acao.unwrap_or(PerfilCmd::Mostrar)),
        Commands::Atualizar => refresh(ctx),
        Commands::Skills { acao } => skills(ctx, acao),
        Commands::Setup {
            target,
            perfil,
            projeto,
            workspace,
            force,
            allow_outside,
        } => {
            let targets = match target {
                Some(t) => Some(crate::setup::parse_targets(Some(&t))?),
                None => None,
            };
            let opts = crate::setup::SetupOptions {
                targets,
                perfil,
                projeto,
                workspace,
                force,
                allow_outside,
            };
            crate::setup::run(ctx, opts)
        }
        Commands::Completions { shell } => completions(ctx, shell),
        Commands::AtualizarCli { verificar } => crate::update::run(ctx, verificar),
        Commands::Config { acao } => match acao.unwrap_or(ConfigCmd::Mostrar) {
            ConfigCmd::Mostrar => show_config(ctx),
            ConfigCmd::Armazenamento { onde } => choose_store(ctx, onde),
            ConfigCmd::AbrirLinks { valor } => choose_open_actions(ctx, valor),
        },
        Commands::Token { force } => print_token(ctx, force),
        Commands::Trabalho {
            item,
            fase,
            projeto,
            empresa,
            etiqueta,
            acao,
        } => {
            let change = work::Change {
                item,
                phase: fase,
                project: projeto,
                company: empresa,
                tag: etiqueta,
            };
            trabalho(ctx, change, acao)
        }
        Commands::External(args) => external(ctx, args),
    }
}

fn login(ctx: &Ctx, no_browser: bool, read_only: bool, new_client: bool) -> Result<()> {
    let http = ctx.http()?;
    let hub = ctx.settings.hub.clone();
    let discovery = oauth::discover(http, &hub)?;
    let mut file = config::load(&ctx.settings.paths)?;
    let client_id = match file.clients.get(&hub).filter(|_| !new_client) {
        Some(id) => id.clone(),
        None => {
            let id = oauth::register_client(http, &discovery)?;
            file.clients.insert(hub.clone(), id.clone());
            config::save(&ctx.settings.paths, &file)?;
            id
        }
    };
    let loopback = oauth::bind_loopback()?;
    let verifier = oauth::random_token(32)?;
    let state = oauth::random_token(16)?;
    let challenge = oauth::pkce_challenge(&verifier);
    let redirect_uri = loopback.redirect_uri.clone();
    let url = oauth::authorization_url(
        &discovery,
        &AuthorizationRequest {
            client_id: &client_id,
            redirect_uri: &redirect_uri,
            challenge: &challenge,
            state: &state,
            scope: if read_only { "mcp:read" } else { "mcp" },
        },
    )?;
    eprintln!("Abra este endereço no navegador para entrar no hub:\n\n  {url}\n");
    let paste = no_browser && std::io::stdin().is_terminal();
    if no_browser {
        eprintln!(
            "Se o navegador estiver em outra máquina, depois de permitir copie o endereço da página de retorno (começa com {redirect_uri}) e cole aqui."
        );
    } else if oauth::open_browser(&url) {
        eprintln!("Abrimos o navegador. Aguardando a autorização...");
    } else {
        eprintln!("Não deu para abrir o navegador; abra o endereço acima.");
    }
    let callback = loopback.wait(LOGIN_TIMEOUT, paste)?;
    let code = oauth::accept_callback(callback, &state, &discovery.issuer)?;
    let tokens = match oauth::exchange_code(http, &discovery, &client_id, &code, &redirect_uri, &verifier) {
        Ok(tokens) => tokens,
        Err(error) => {
            if error.message().contains("novo-cliente") {
                file.clients.remove(&hub);
                config::save(&ctx.settings.paths, &file)?;
            }
            return Err(error);
        }
    };
    ctx.store().save(&tokens)?;
    ctx.forget_catalog()?;
    match ctx.whoami() {
        Ok(me) => {
            let text = format!("Conectado. {}", whoami_text(&me));
            ctx.out.print_human(&text, || me.clone());
            Ok(())
        }
        Err(error @ (Error::Permission(_) | Error::Auth(_))) => Err(error),
        Err(error) => {
            eprintln!("Login feito, mas não consegui ler seus dados agora: {error}");
            Ok(())
        }
    }
}

fn logout(ctx: &Ctx) -> Result<()> {
    let store = ctx.store();
    if let Some(tokens) = store.load()?
        && let Err(error) = oauth::revoke(ctx.http()?, &tokens)
    {
        eprintln!("etus: aviso: {error}");
    }
    store.delete()?;
    ctx.forget_catalog()?;
    ctx.out
        .print_human("Desconectado.", || json!({"status": "desconectado"}));
    Ok(())
}

fn whoami_text(me: &Value) -> String {
    let person = &me["person"];
    let mut lines = vec![format!(
        "Pessoa: {} <{}>",
        person["name"].as_str().unwrap_or("?"),
        person["email"].as_str().unwrap_or("?")
    )];
    match me.get("activeCompany").filter(|c| !c.is_null()) {
        Some(company) => lines.push(format!(
            "Empresa ativa: {} ({})",
            company["name"].as_str().unwrap_or("?"),
            company["id"].as_str().unwrap_or("?")
        )),
        None => lines.push("Empresa ativa: nenhuma (use `etus empresa usar <id>`)".into()),
    }
    let access = match me["access"].as_str() {
        Some("read_only") => "só leitura",
        Some("read_write") => "leitura e alterações",
        _ => "desconhecido",
    };
    lines.push(format!("Acesso: {access}"));
    if person["platformAdmin"].as_bool() == Some(true) {
        lines.push("Admin da plataforma: sim".into());
    }
    lines.join("\n")
}

fn whoami(ctx: &Ctx) -> Result<()> {
    match ctx.whoami() {
        Ok(me) => {
            let text = format!("{}\nHub: {}", whoami_text(&me), ctx.settings.hub);
            ctx.out.print_human(&text, || me.clone());
            Ok(())
        }
        Err(Error::Network(message)) => {
            let tokens = ctx.store().load()?.ok_or_else(Error::not_logged_in)?;
            let claims = crate::auth::jwt_claims(&tokens.access_token).unwrap_or(Value::Null);
            eprintln!("etus: o hub não respondeu ({message}); mostrando o que está no token.");
            let value = json!({
                "sub": claims.get("sub"),
                "scope": tokens.scope,
                "clientId": tokens.client_id,
                "expiresAt": tokens.expires_at,
                "hub": ctx.settings.hub,
            });
            ctx.out.print_value(&value);
            Ok(())
        }
        Err(other) => Err(other),
    }
}

fn app_summary(app: &App) -> Value {
    json!({
        "app": app.word,
        "name": app.name,
        "aliases": app.aliases,
        "prefix": app.prefix,
        "commands": app.commands.iter().map(|c| json!({
            "name": c.name,
            "aliases": c.aliases,
            "tool": c.tool.name,
            "writes": c.tool.writes(),
        })).collect::<Vec<_>>(),
    })
}

fn apps(ctx: &Ctx) -> Result<()> {
    let (cache, _) = ctx.catalog()?;
    let apps = cache.apps();
    if ctx.out.json {
        ctx.out
            .print_value(&Value::Array(apps.iter().map(app_summary).collect()));
        return Ok(());
    }
    if apps.is_empty() {
        ctx.out
            .print_human("Nenhum app liberado para você na empresa ativa.", || json!([]));
        return Ok(());
    }
    let width = apps.iter().map(|a| a.word.chars().count()).max().unwrap_or(0);
    let mut lines = Vec::new();
    if let Some(company) = &cache.company {
        lines.push(format!("Empresa ativa: {} ({})\n", company.name, company.id));
    }
    for app in &apps {
        let writes = app.tools().filter(|t| t.writes()).count();
        let mut line = format!(
            "{:width$}  {}: {} ({} {})",
            app.word,
            app.name,
            cli::count(app.commands.len(), "comando", "comandos"),
            writes,
            if writes == 1 { "altera dados" } else { "alteram dados" }
        );
        let aliases = cli::app_line_aliases(app);
        if !aliases.is_empty() {
            line.push_str("; ");
            line.push_str(&aliases);
        }
        lines.push(line);
    }
    lines.push("\nUse `etus <app> --help` para ver os comandos de um app.".into());
    ctx.out.print_human(&lines.join("\n"), || Value::Null);
    Ok(())
}

fn companies(ctx: &Ctx) -> Result<()> {
    let me = ctx.whoami()?;
    let active = me["activeCompany"]["id"].as_str().unwrap_or("").to_string();
    let list = me["companies"].as_array().cloned().unwrap_or_default();
    let lines: Vec<String> = list
        .iter()
        .map(|c| {
            let id = c["id"].as_str().unwrap_or("?");
            let mark = if id == active { "*" } else { " " };
            format!(
                "{mark} {id}  {} ({})",
                c["name"].as_str().unwrap_or("?"),
                c["slug"].as_str().unwrap_or("")
            )
        })
        .collect();
    let text = if lines.is_empty() {
        "Você não é membro de nenhuma empresa.".into()
    } else {
        lines.join("\n")
    };
    ctx.out.print_human(&text, || Value::Array(list.clone()));
    Ok(())
}

pub fn resolve_company(companies: &[Value], wanted: &str) -> Option<String> {
    let wanted_lower = wanted.to_lowercase();
    let field = |c: &Value, key: &str| c[key].as_str().unwrap_or("").to_string();
    companies
        .iter()
        .find(|c| field(c, "id") == wanted)
        .or_else(|| {
            companies
                .iter()
                .find(|c| field(c, "slug").to_lowercase() == wanted_lower)
        })
        .or_else(|| {
            companies
                .iter()
                .find(|c| field(c, "name").to_lowercase() == wanted_lower)
        })
        .map(|c| field(c, "id"))
}

fn use_company(ctx: &Ctx, wanted: &str) -> Result<()> {
    let me = ctx.whoami()?;
    let list = me["companies"].as_array().cloned().unwrap_or_default();
    let id = resolve_company(&list, wanted)
        .ok_or_else(|| Error::Usage(format!("Empresa não encontrada: {wanted}. Veja `etus empresa listar`.")))?;
    let outcome = ctx.with_mcp(|mcp| mcp.call_tool(SET_COMPANY_TOOL, &json!({"companyId": id})))?;
    ctx.forget_catalog()?;
    if outcome.is_error {
        return Output { quiet: true, ..ctx.out }.tool_result(&outcome);
    }
    let me = crate::output::payload(&outcome.result);
    let text = format!("Empresa ativa trocada.\n{}", whoami_text(&me));
    ctx.out.print_human(&text, || me.clone());
    Ok(())
}

fn profile(ctx: &Ctx, action: PerfilCmd) -> Result<()> {
    let mut file = config::load(&ctx.settings.paths)?;
    match action {
        PerfilCmd::Mostrar => {
            let text = match &ctx.settings.perfil {
                Some(p) => format!("Perfil em uso: {p}"),
                None => "Sem perfil: o hub usa o seu acesso normal.".into(),
            };
            ctx.out.print_human(&text, || json!({"perfil": ctx.settings.perfil}));
            return Ok(());
        }
        PerfilCmd::Usar { id } => file.perfil = Some(id.trim().to_string()).filter(|p| !p.is_empty()),
        PerfilCmd::Limpar => file.perfil = None,
    }
    config::save(&ctx.settings.paths, &file)?;
    let text = match &file.perfil {
        Some(p) => format!("Perfil {p} salvo. As próximas chamadas levam ?perfil={p} ao hub."),
        None => "Perfil removido.".into(),
    };
    ctx.out.print_human(&text, || json!({"perfil": file.perfil}));
    Ok(())
}

fn refresh(ctx: &Ctx) -> Result<()> {
    let cache = ctx.fetch_catalog()?;
    let apps = cache.apps();
    let text = format!(
        "Catálogo atualizado: {} comandos em {} apps.",
        cache.tools.len(),
        apps.len()
    );
    ctx.out
        .print_human(&text, || json!({"tools": cache.tools.len(), "apps": apps.len()}));
    Ok(())
}

pub fn pick_param(tool: &Tool, preferred: &[&str]) -> Option<String> {
    let params = schema::params(&tool.input_schema);
    preferred
        .iter()
        .find_map(|name| params.iter().find(|p| p.name == *name))
        .or_else(|| params.iter().find(|p| p.required && p.kind == schema::Kind::String))
        .or_else(|| params.iter().find(|p| p.kind == schema::Kind::String))
        .map(|p| p.name.clone())
}

fn skills(ctx: &Ctx, action: SkillsCmd) -> Result<()> {
    let (tool_name, preferred, value): (&str, &[&str], String) = match action {
        SkillsCmd::Buscar { termo } => (
            SKILLS_SEARCH_TOOL,
            &["query", "q", "termo", "busca", "search", "texto", "text"],
            termo.join(" "),
        ),
        SkillsCmd::Ler { id } => (SKILLS_READ_TOOL, &["id", "skillId", "slug", "key", "chave"], id),
        SkillsCmd::Sincronizar { agente, projeto, pasta } => {
            return sync_skills(ctx, agente, projeto, pasta.as_deref());
        }
    };
    let (cache, _) = ctx.catalog()?;
    let tool = cache.find_tool(tool_name).cloned().ok_or_else(|| {
        Error::Permission(format!("O catálogo do Extensões ({tool_name}) não está liberado para você. Rode `etus atualizar` ou peça acesso no hub."))
    })?;
    let param = pick_param(&tool, preferred)
        .ok_or_else(|| Error::Other(format!("Não sei qual parâmetro de {tool_name} recebe o texto.")))?;
    let mut args = Map::new();
    args.insert(param, Value::String(value));
    execute(ctx, &tool, args, ctx.out, RunFlags::from_ctx(ctx))
}

fn fetch_details(ctx: &Ctx, apps: &[App]) -> BTreeMap<String, skills::AppDetail> {
    let names = skills::detail_names(apps);
    let mut details = BTreeMap::new();
    let _ = ctx.with_mcp(|mcp| {
        for chunk in names.chunks(skills::DETAIL_BATCH) {
            let outcome = mcp.call_tool(skills::DETAIL_TOOL, &json!({ "nomes": chunk }))?;
            if !outcome.is_error {
                details.extend(skills::parse_details(&crate::output::payload(&outcome.result)));
            }
        }
        Ok(())
    });
    details
}

fn skill_locations() -> Result<skills::Locations> {
    let base = directories::BaseDirs::new()
        .ok_or_else(|| Error::Other("Não foi possível achar a pasta do usuário.".into()))?;
    let home = base.home_dir().to_path_buf();
    let cwd = std::env::current_dir()?;
    Ok(skills::Locations {
        claude_home: work::claude_home().unwrap_or_else(|| home.join(".claude")),
        project_root: work::git_context(&cwd).map(|git| git.top).unwrap_or(cwd),
        home,
    })
}

fn sync_skills(ctx: &Ctx, agent: skills::SkillAgent, project: bool, folder: Option<&Path>) -> Result<()> {
    let cache = ctx.fetch_catalog()?;
    let apps = cache.apps();
    let details = fetch_details(ctx, &apps);
    let origin = skills::Origin {
        company: cache.company.as_ref().map(|c| c.name.clone()),
        hub_host: skills::hub_host(&ctx.settings.hub),
    };
    let wanted = skills::desired(&apps, &details, &origin);
    let dirs = skills::target_dirs(agent, project, folder, &skill_locations()?);
    let mut reports = Vec::new();
    for dir in dirs {
        let plan = skills::plan(&dir, &wanted);
        if !ctx.dry_run {
            skills::apply(&dir, &wanted, &plan)?;
        }
        reports.push((dir, plan));
    }
    let without_guide = apps
        .iter()
        .filter(|a| a.prefix != crate::catalog::UNPREFIXED && !details.contains_key(&a.prefix))
        .count();
    if without_guide > 0 && !ctx.out.json && !ctx.out.quiet {
        eprintln!(
            "etus: o hub não mandou o detalhe de {}; essas skills usam só as ferramentas (sem guia nem permissões).",
            cli::count(without_guide, "app", "apps")
        );
    }
    let text = sync_report_text(&reports, ctx.dry_run);
    ctx.out.print_human(&text, || sync_report_json(&reports));
    Ok(())
}

fn sync_report_text(reports: &[(PathBuf, skills::Plan)], dry_run: bool) -> String {
    let mut lines = Vec::new();
    for (dir, plan) in reports {
        lines.push(format!("Pasta {}", dir.display()));
        let groups: [(&Vec<String>, &str, &str); 4] = [
            (&plan.create, "+", "criada"),
            (&plan.update, "~", "atualizada"),
            (&plan.remove, "-", "removida"),
            (&plan.skipped, "!", "ignorada: SKILL.md que não foi gerado pelo etus"),
        ];
        for (names, mark, label) in groups {
            lines.extend(names.iter().map(|name| format!("{mark} {name} ({label})")));
        }
        lines.push(format!(
            "{} criadas, {} atualizadas, {} removidas, {} sem mudança.",
            plan.create.len(),
            plan.update.len(),
            plan.remove.len(),
            plan.unchanged.len()
        ));
    }
    if dry_run {
        lines.push("Simulação (--dry-run): nada foi gravado.".into());
    } else if reports.iter().any(|(_, plan)| plan.changes() > 0) {
        lines.push("Reinicie o agente (ou abra uma conversa nova) para ele carregar as skills.".into());
    }
    lines.join("\n")
}

fn sync_report_json(reports: &[(PathBuf, skills::Plan)]) -> Value {
    let union = |pick: fn(&skills::Plan) -> &Vec<String>| -> Vec<String> {
        reports
            .iter()
            .flat_map(|(_, plan)| pick(plan).iter().cloned())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    };
    json!({
        "pastas": reports.iter().map(|(dir, _)| dir.display().to_string()).collect::<Vec<_>>(),
        "criadas": union(|p| &p.create),
        "atualizadas": union(|p| &p.update),
        "removidas": union(|p| &p.remove),
        "iguais": union(|p| &p.unchanged),
        "ignoradas": union(|p| &p.skipped),
    })
}

fn completions(ctx: &Ctx, shell: clap_complete::Shell) -> Result<()> {
    let apps = ctx.cached_catalog().map(|c| c.apps()).unwrap_or_default();
    let mut cmd = cli::full_command(&apps);
    clap_complete::generate(shell, &mut cmd, "etus", &mut std::io::stdout());
    Ok(())
}

fn show_config(ctx: &Ctx) -> Result<()> {
    let s = &ctx.settings;
    let value = json!({
        "hub": s.hub,
        "mcpUrl": s.mcp_url(),
        "perfil": s.perfil,
        "cacheTtlSecs": s.cache_ttl.as_secs(),
        "configFile": s.paths.config_file(),
        "cacheDir": s.paths.cache_dir,
        "tokenStore": s.token_store.label(),
        "tokenLocation": ctx.store().describe(),
        "openActions": s.open_actions,
        "version": env!("CARGO_PKG_VERSION"),
    });
    ctx.out.print_value(&value);
    Ok(())
}

fn choose_open_actions(ctx: &Ctx, value: Toggle) -> Result<()> {
    let enabled = value == Toggle::Sim;
    let mut file = config::load(&ctx.settings.paths)?;
    file.open_actions = Some(enabled);
    config::save(&ctx.settings.paths, &file)?;
    let text = if enabled {
        "Os links de ação vão abrir sozinhos no navegador quando houver terminal."
    } else {
        "Os links de ação só abrem com --open."
    };
    ctx.out.print_human(text, || json!({"openActions": enabled}));
    Ok(())
}

fn choose_store(ctx: &Ctx, kind: StoreKind) -> Result<()> {
    let s = &ctx.settings;
    if s.token_store_from_env {
        return Err(Error::Usage(
            "ETUS_TOKEN_STORE está definida e vale mais que a configuração. Remova a variável e rode de novo.".into(),
        ));
    }
    if kind == StoreKind::Keychain && !keychain_supported() {
        return Err(Error::Usage(
            "Esta plataforma não tem chaveiro suportado; o token fica em arquivo.".into(),
        ));
    }
    let tokens = if kind == s.token_store {
        None
    } else {
        ctx.store().load()?
    };
    let next = store_for(&s.paths, &s.hub, kind);
    if let Some(tokens) = tokens {
        if kind == StoreKind::File {
            match std::fs::remove_file(s.paths.keychain_marker(&s.hub)) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
                _ => {}
            }
        }
        next.save(&tokens)?;
    }
    let mut file = config::load(&s.paths)?;
    file.token_store = Some(kind);
    config::save(&s.paths, &file)?;
    let location = next.describe();
    ctx.out.print_human(
        &format!("O token fica em: {location}"),
        || json!({"tokenStore": kind.label(), "tokenLocation": location}),
    );
    Ok(())
}

pub fn token_output_allowed(stdout_is_terminal: bool, force: bool) -> Result<()> {
    if stdout_is_terminal && !force {
        return Err(Error::Usage(
            "O token é um segredo e a saída é um terminal. Use num script (MCP_TOKEN=$(etus token)) ou passe --force."
                .into(),
        ));
    }
    Ok(())
}

fn print_token(ctx: &Ctx, force: bool) -> Result<()> {
    token_output_allowed(std::io::stdout().is_terminal(), force)?;
    let manager = ctx.manager()?;
    let token = manager.access_token(None)?;
    if ctx.out.json {
        let expires_at = manager
            .current()
            .ok()
            .filter(|t| t.access_token == token)
            .map(|t| t.expires_at);
        println!("{}", json!({"accessToken": token, "expiresAt": expires_at}));
    } else {
        println!("{token}");
    }
    Ok(())
}

fn read_input(path: &str) -> Result<Value> {
    let text = if path == "-" {
        let mut text = String::new();
        std::io::stdin().read_to_string(&mut text)?;
        text
    } else {
        std::fs::read_to_string(path).map_err(|e| Error::Usage(format!("Não consegui ler {path}: {e}")))?
    };
    serde_json::from_str(&text).map_err(|e| Error::Usage(format!("--input não é JSON válido: {e}")))
}

fn external(ctx: &Ctx, args: Vec<String>) -> Result<()> {
    let Some((word, rest)) = args.split_first() else {
        return Err(Error::Usage("Falta o nome do app.".into()));
    };
    if word == "help" {
        let mut argv = vec!["etus".to_string()];
        argv.extend(rest.iter().cloned());
        argv.push("--help".into());
        return Err(Error::Exit(crate::run_args(argv)));
    }
    let (mut cache, mut fresh) = ctx.catalog()?;
    if find_app(&cache.apps(), word).is_none() && !fresh {
        cache = ctx.fetch_catalog()?;
        fresh = true;
    }
    let apps = cache.apps();
    let Some(app) = find_app(&apps, word) else {
        let hint = if fresh { "" } else { " (catálogo em cache)" };
        return Err(Error::Usage(format!(
            "App ou comando desconhecido: {word}{hint}. Veja `etus apps` e `etus --help`."
        )));
    };
    let mut rest = rest.to_vec();
    if let Some(first) = rest.first_mut()
        && !first.starts_with('-')
        && app.find_command(first).is_none()
        && let Some(found) = app.find_command(&slug(first))
    {
        *first = found.name.clone();
    }
    let invocation = cli::Invocation {
        app: word,
        command: rest.first().map(String::as_str),
    };
    let mut argv = vec![word.clone()];
    argv.extend(rest.iter().cloned());
    let matches = match cli::app_command(app, &invocation).try_get_matches_from(argv) {
        Ok(m) => m,
        Err(e) => {
            let _ = e.print();
            return Err(Error::Exit(e.exit_code()));
        }
    };
    let Some((name, sub)) = matches.subcommand() else {
        return Err(Error::Usage(format!("Falta o comando. Veja `etus {word} --help`.")));
    };
    let tool = app
        .find_command(name)
        .map(|c| c.tool.clone())
        .ok_or_else(|| Error::Usage(format!("Comando desconhecido: {name}.")))?;
    run_tool(ctx, &tool, sub)
}

fn run_tool(ctx: &Ctx, tool: &Tool, m: &ArgMatches) -> Result<()> {
    let params = schema::params(&tool.input_schema);
    let flags = schema::collect(&params, m)?;
    let json_arg = m.get_one::<String>("json").cloned();
    let input = m.get_one::<String>("input").cloned();
    let base = match (json_arg.as_deref().filter(|s| !s.is_empty()), input) {
        (Some(_), Some(_)) => return Err(Error::Usage("Use --json OBJETO ou --input, não os dois.".into())),
        (Some(object), None) => Some(
            serde_json::from_str(object)
                .map_err(|e| Error::Usage(format!("--json não é um objeto JSON válido: {e}")))?,
        ),
        (None, Some(path)) => Some(read_input(&path)?),
        (None, None) => None,
    };
    let args = schema::merge(base, flags)?;
    schema::validate(&params, &args)?;
    let out = Output {
        json: ctx.out.json || json_arg.is_some(),
        quiet: ctx.out.quiet || m.get_flag("quiet"),
    };
    let flags = RunFlags {
        yes: ctx.yes || m.get_flag("yes"),
        dry_run: ctx.dry_run || m.get_flag("dry-run"),
        open: ctx.open || m.get_flag("open"),
        wait: m.get_one::<u64>("wait").copied().or(ctx.wait),
    };
    execute(ctx, tool, args, out, flags)
}

pub struct RunFlags {
    pub yes: bool,
    pub dry_run: bool,
    pub open: bool,
    pub wait: Option<u64>,
}

impl RunFlags {
    fn from_ctx(ctx: &Ctx) -> RunFlags {
        RunFlags {
            yes: ctx.yes,
            dry_run: ctx.dry_run,
            open: ctx.open,
            wait: ctx.wait,
        }
    }
}

pub fn dry_run_plan(tool: &Tool, args: &Map<String, Value>, endpoint: &str) -> Value {
    json!({
        "tool": tool.name,
        "arguments": args,
        "endpoint": endpoint,
        "readOnly": tool.read_only(),
        "destructive": tool.writes() && tool.destructive(),
        "wouldAskConfirmation": tool.writes(),
    })
}

fn execute(ctx: &Ctx, tool: &Tool, args: Map<String, Value>, out: Output, flags: RunFlags) -> Result<()> {
    match confirm::decide(tool.writes(), flags.yes, flags.dry_run, confirm::interactive()) {
        Decision::DryRun => {
            let plan = dry_run_plan(tool, &args, &ctx.settings.mcp_url());
            if out.json {
                println!("{plan}")
            } else {
                println!("{}", pretty(&plan))
            }
            return Ok(());
        }
        Decision::Refuse => {
            return Err(Error::Usage(format!(
                "{} altera dados. Repita com --yes para confirmar (ou --dry-run para só ver a chamada).",
                tool.name
            )));
        }
        Decision::Ask => {
            let kind = if tool.destructive() {
                "altera dados e pode apagar algo"
            } else {
                "altera dados"
            };
            eprintln!("{} {kind}. Argumentos: {}", tool.name, Value::Object(args.clone()));
            if !confirm::ask("Confirma?")? {
                eprintln!("Cancelado.");
                return Err(Error::Exit(EXIT_USAGE));
            }
        }
        Decision::Run => {}
    }
    let arguments = Value::Object(args);
    let call = || ctx.with_mcp(|mcp| mcp.call_tool(&tool.name, &arguments));
    let mut outcome = call()?;
    if let Some(required) = action::detect(&outcome.result) {
        offer_link(ctx, &required, flags.open);
        if let Some(seconds) = flags.wait {
            let limit = action::wait_limit(seconds);
            eprintln!("{}", action::notice(&required));
            eprintln!(
                "Aguardando você concluir no navegador (até {} s; Ctrl+C para parar)...",
                limit.as_secs()
            );
            outcome = action::poll_for_real(limit, call)?;
            if action::detect(&outcome.result).is_some() {
                eprintln!("O tempo de espera acabou e a ação ainda não foi concluída.");
            } else {
                eprintln!("Ação concluída. Seguindo com a chamada.");
            }
        }
    }
    if tool.name == SET_COMPANY_TOOL && !outcome.is_error {
        ctx.forget_catalog()?;
    }
    out.tool_result(&outcome)
}

fn offer_link(ctx: &Ctx, required: &Action, explicit: bool) {
    let interactive = std::io::stderr().is_terminal() && std::io::stdin().is_terminal();
    let auto = ctx.settings.open_actions && interactive;
    if !(explicit || auto) {
        return;
    }
    if oauth::open_browser(&required.url) {
        eprintln!("Abrimos o link no navegador.");
    } else {
        eprintln!("Não deu para abrir o navegador; abra o link abaixo.");
    }
}

const WORK_VIA: &str = "cli";

fn trabalho(ctx: &Ctx, change: work::Change, action: Option<TrabalhoCmd>) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let git = work::git_context(&cwd);
    let path = work::state_path(git.as_ref(), work::claude_home().as_deref())?;
    let current = work::load_state(&path);
    match action {
        Some(TrabalhoCmd::Limpar) => work::clear_state(&path)?,
        Some(TrabalhoCmd::Mostrar | TrabalhoCmd::Cabecalho) | None if change.is_empty() => {}
        _ => {
            let active =
                crate::catalog::load_cache(&ctx.settings.paths, &ctx.settings.hub, ctx.settings.perfil.as_deref())
                    .and_then(|cache| cache.company)
                    .map(|company| company.id);
            work::save_state(&path, &work::apply(&current, &change, active.as_deref())?)?;
        }
    }
    let state = work::load_state(&path);
    let fields = work::resolve(&state, git.as_ref(), &work::read_mapping(git.as_ref()), WORK_VIA);
    if matches!(action, Some(TrabalhoCmd::Cabecalho)) {
        ctx.out.print_human(&fields.line(), || json!({"header": fields.line()}));
        return Ok(());
    }
    let dash = |s: &str| if s.is_empty() { "-".to_string() } else { s.to_string() };
    let text = [
        format!("item:              {}", dash(&fields.issue)),
        format!("projeto do Plane:  {}", dash(&fields.plane_project)),
        format!("projeto:           {}", dash(&fields.project)),
        format!("empresa:           {}", dash(&fields.company)),
        format!("etiqueta:          {}", dash(&fields.tag)),
        format!("repositório:       {}", dash(&fields.repo)),
        format!("fase:              {}", fields.phase),
        format!("origem:            {}", fields.source),
        format!("header:            {}", fields.line()),
    ]
    .join("\n");
    let text = if fields.source == "none" {
        format!("{text}\n\nNenhum trabalho marcado: o uso desta pasta não está sendo atribuído a nenhum item.")
    } else {
        text
    };
    ctx.out.print_human(&text, || {
        let mut value = fields.to_json();
        value["statePath"] = json!(path.display().to_string());
        value
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_is_refused_on_a_terminal_unless_forced() {
        let refused = token_output_allowed(true, false).unwrap_err();
        assert_eq!(refused.exit_code(), 2);
        assert!(refused.message().contains("--force"));
        assert!(token_output_allowed(true, true).is_ok());
        assert!(token_output_allowed(false, false).is_ok());
    }

    #[test]
    fn companies_resolve_by_id_slug_or_name() {
        let list = vec![
            json!({"id": "org_a", "name": "Etus Digital", "slug": "etus"}),
            json!({"id": "org_b", "name": "Evolution", "slug": "evo"}),
        ];
        assert_eq!(resolve_company(&list, "org_b").as_deref(), Some("org_b"));
        assert_eq!(resolve_company(&list, "ETUS").as_deref(), Some("org_a"));
        assert_eq!(resolve_company(&list, "evolution").as_deref(), Some("org_b"));
        assert_eq!(resolve_company(&list, "x"), None);
    }

    #[test]
    fn skills_param_is_picked_from_the_schema() {
        let tool: Tool = serde_json::from_value(json!({
            "name": "extensoes__catalogo_buscar",
            "inputSchema": {"type": "object", "properties": {"limit": {"type": "integer"}, "termo": {"type": "string"}}, "required": ["termo"]}
        }))
        .unwrap();
        assert_eq!(pick_param(&tool, &["query", "termo"]).as_deref(), Some("termo"));
        let other: Tool = serde_json::from_value(json!({
            "name": "x__y",
            "inputSchema": {"type": "object", "properties": {"n": {"type": "integer"}, "palavra": {"type": "string"}}, "required": ["palavra"]}
        }))
        .unwrap();
        assert_eq!(pick_param(&other, &["query"]).as_deref(), Some("palavra"));
    }

    #[test]
    fn dry_run_plan_describes_the_call() {
        let tool: Tool = serde_json::from_value(
            json!({"name": "tasks__create_task", "annotations": {"readOnlyHint": false, "destructiveHint": false}}),
        )
        .unwrap();
        let mut args = Map::new();
        args.insert("title".into(), json!("x"));
        let plan = dry_run_plan(&tool, &args, "https://apps.etus.io/mcp");
        assert_eq!(plan["tool"], "tasks__create_task");
        assert_eq!(plan["arguments"]["title"], "x");
        assert_eq!(plan["wouldAskConfirmation"], true);
        assert_eq!(plan["destructive"], false);
    }
}
