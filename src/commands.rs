use std::io::{IsTerminal, Read};
use std::time::Duration;

use clap::ArgMatches;
use serde_json::{Map, Value, json};

use crate::auth::oauth::{self, AuthorizationRequest};
use crate::catalog::{App, Tool, find_app};
use crate::cli::{self, Commands, EmpresaCmd, PerfilCmd, SkillsCmd};
use crate::config;
use crate::confirm::{self, Decision};
use crate::context::{Ctx, SET_COMPANY_TOOL};
use crate::error::{EXIT_USAGE, Error, Result};
use crate::output::{Output, pretty};
use crate::schema;

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
        Commands::Setup => {
            ctx.out.print_human(
                "em breve: o setup vai preparar esta máquina pelo Extensões.",
                || json!({"status": "em breve"}),
            );
            Ok(())
        }
        Commands::Completions { shell } => completions(ctx, shell),
        Commands::AtualizarCli { verificar } => crate::update::run(ctx, verificar),
        Commands::Config => show_config(ctx),
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
        "app": app.prefix,
        "name": app.name,
        "aliases": app.aliases,
        "commands": app.tools.iter().map(Tool::command_name).collect::<Vec<_>>(),
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
    let width = apps.iter().map(|a| a.prefix.len()).max().unwrap_or(0);
    let mut lines = Vec::new();
    if let Some(company) = &cache.company {
        lines.push(format!("Empresa ativa: {} ({})\n", company.name, company.id));
    }
    for app in &apps {
        let writes = app.tools.iter().filter(|t| t.writes()).count();
        let aliases = if app.aliases.is_empty() {
            String::new()
        } else {
            format!(" [{}]", app.aliases.join(", "))
        };
        lines.push(format!(
            "{:width$}  {}{}: {} comandos ({} alteram dados)",
            app.prefix,
            app.name,
            aliases,
            app.tools.len(),
            writes
        ));
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
    let (cache, _) = ctx.catalog()?;
    let (tool_name, preferred, value): (&str, &[&str], String) = match action {
        SkillsCmd::Buscar { termo } => (
            SKILLS_SEARCH_TOOL,
            &["query", "q", "termo", "busca", "search", "texto", "text"],
            termo.join(" "),
        ),
        SkillsCmd::Ler { id } => (SKILLS_READ_TOOL, &["id", "skillId", "slug", "key", "chave"], id),
    };
    let tool = cache.find_tool(tool_name).cloned().ok_or_else(|| {
        Error::Permission(format!("O catálogo do Extensões ({tool_name}) não está liberado para você. Rode `etus atualizar` ou peça acesso no hub."))
    })?;
    let param = pick_param(&tool, preferred)
        .ok_or_else(|| Error::Other(format!("Não sei qual parâmetro de {tool_name} recebe o texto.")))?;
    let mut args = Map::new();
    args.insert(param, Value::String(value));
    execute(ctx, &tool, args, ctx.out, ctx.yes, ctx.dry_run)
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
        "tokenStore": ctx.store().describe(),
        "version": env!("CARGO_PKG_VERSION"),
    });
    ctx.out.print_value(&value);
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
    let mut argv = vec![app.prefix.clone()];
    argv.extend(rest.iter().cloned());
    let matches = match cli::app_command(app).try_get_matches_from(argv) {
        Ok(m) => m,
        Err(e) => {
            let _ = e.print();
            return Err(Error::Exit(e.exit_code()));
        }
    };
    let Some((name, sub)) = matches.subcommand() else {
        return Err(Error::Usage(format!(
            "Falta o comando. Veja `etus {} --help`.",
            app.prefix
        )));
    };
    let tool = app
        .find_command(name)
        .cloned()
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
    execute(
        ctx,
        tool,
        args,
        out,
        ctx.yes || m.get_flag("yes"),
        ctx.dry_run || m.get_flag("dry-run"),
    )
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

fn execute(ctx: &Ctx, tool: &Tool, args: Map<String, Value>, out: Output, yes: bool, dry_run: bool) -> Result<()> {
    match confirm::decide(tool.writes(), yes, dry_run, confirm::interactive()) {
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
    let outcome = ctx.with_mcp(|mcp| mcp.call_tool(&tool.name, &Value::Object(args)))?;
    if tool.name == SET_COMPANY_TOOL && !outcome.is_error {
        ctx.forget_catalog()?;
    }
    out.tool_result(&outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

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
