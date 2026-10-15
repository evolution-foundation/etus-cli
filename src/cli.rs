use std::path::PathBuf;

use clap::{Arg, ArgAction, Command, CommandFactory, Parser, Subcommand};
use clap_complete::Shell;

use crate::auth::store::StoreKind;
use crate::catalog::{App, AppCommand, Tool};
use crate::schema;
use crate::skills::SkillAgent;

const TEMPLATE: &str = "{about-with-newline}\nUso: {usage}\n\n{all-args}{after-help}";
const OPEN_HELP: &str =
    "Se a ferramenta pedir uma ação sua (autorizar, conectar, entrar, pedir acesso), abre o link no navegador";
const WAIT_HELP: &str = "Se a ferramenta pedir uma ação sua, espera você concluir e repete a chamada (padrão 300 s; --wait=SEGUNDOS, até 1800)";
const DEFAULT_WAIT: &str = "300";
const TOOL_TEMPLATE: &str = "{before-help}Uso: {usage}\n\n{all-args}{after-help}";

#[derive(Debug, Parser)]
#[command(
    name = "etus",
    version,
    about = "CLI da plataforma ETUS: os comandos de cada app vêm das ferramentas MCP liberadas para você no hub.",
    arg_required_else_help = true,
    disable_help_subcommand = true,
    disable_version_flag = true
)]
pub struct Cli {
    #[arg(
        long,
        global = true,
        value_name = "URL",
        help = "Endereço do hub (padrão https://apps.etus.io; também ETUS_HUB_URL)"
    )]
    pub hub: Option<String>,
    #[arg(
        long,
        global = true,
        value_name = "ID",
        help = "Perfil de acesso enviado ao hub (?perfil=ID)"
    )]
    pub perfil: Option<String>,
    #[arg(long, global = true, help = "Saída em JSON compacto, para agentes e scripts")]
    pub json: bool,
    #[arg(short, long, global = true, help = "Não imprime nada em caso de sucesso")]
    pub quiet: bool,
    #[arg(
        short,
        long,
        global = true,
        help = "Confirma ferramentas que alteram dados sem perguntar"
    )]
    pub yes: bool,
    #[arg(long, global = true, help = "Mostra a chamada sem executar")]
    pub dry_run: bool,
    #[arg(long, global = true, visible_alias = "abrir", help = OPEN_HELP)]
    pub open: bool,
    #[arg(
        long,
        global = true,
        visible_alias = "aguardar",
        value_name = "SEGUNDOS",
        num_args = 0..=1,
        require_equals = true,
        default_missing_value = DEFAULT_WAIT,
        help = WAIT_HELP
    )]
    pub wait: Option<u64>,
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    #[command(about = "Conecta a CLI à sua conta ETUS pelo navegador")]
    Login {
        #[arg(
            long,
            visible_alias = "sem-navegador",
            help = "Não abre o navegador: só mostra o endereço (aceita colar o endereço de retorno)"
        )]
        no_browser: bool,
        #[arg(long, help = "Pede só leitura (escopo mcp:read)")]
        somente_leitura: bool,
        #[arg(long, help = "Registra um cliente novo no hub antes de entrar")]
        novo_cliente: bool,
    },
    #[command(about = "Desconecta e revoga a sessão desta máquina")]
    Logout,
    #[command(about = "Mostra quem está conectado e a empresa ativa")]
    Whoami,
    #[command(about = "Lista os apps que você pode usar pela CLI")]
    Apps,
    #[command(about = "Lista ou troca a empresa ativa")]
    Empresa {
        #[command(subcommand)]
        acao: Option<EmpresaCmd>,
    },
    #[command(about = "Mostra, escolhe ou limpa o perfil enviado ao hub")]
    Perfil {
        #[command(subcommand)]
        acao: Option<PerfilCmd>,
    },
    #[command(about = "Renova o catálogo de comandos (ferramentas liberadas para você)")]
    Atualizar,
    #[command(about = "Busca e lê skills do Extensões e gera as skills dos seus apps para os agentes")]
    Skills {
        #[command(subcommand)]
        acao: SkillsCmd,
    },
    #[command(
        visible_alias = "init",
        about = "Configura regras de governança e perfis nos agentes do projeto",
        long_about = "Configura regras de conduta, engenharia e convenções da Etus para múltiplos agentes (Cursor, Antigravity, Windsurf, Copilot, Codex, Claude Code).\n\nInjeta blocos demarcados que preservam o restante do arquivo intacto."
    )]
    Setup {
        #[arg(
            long,
            value_name = "TARGETS",
            value_delimiter = ',',
            help = "cursor, antigravity, windsurf, copilot, codex, claude-code ou all"
        )]
        target: Option<Vec<String>>,
        #[arg(long, value_name = "PERFIL", help = "codigo, produto, design, ops, admin, app-nexus")]
        perfil: Option<String>,
        #[arg(long, value_name = "PROJETO", help = "Chave do projeto no Plane (ex: ENG, EVO)")]
        projeto: Option<String>,
        #[arg(long, value_name = "WORKSPACE", help = "Workspace no Plane (padrão etus)")]
        workspace: Option<String>,
        #[arg(long, help = "Força sobrescrita/anexação em arquivos existentes sem marcadores")]
        force: bool,
        #[arg(long, help = "Permite gravar fora da raiz do projeto")]
        allow_outside: bool,
    },
    #[command(about = "Gera o autocompletar para o shell")]
    Completions {
        #[arg(value_enum, help = "bash, zsh, fish, powershell ou elvish")]
        shell: Shell,
    },
    #[command(name = "atualizar-cli", about = "Atualiza o etus para a versão mais nova publicada")]
    AtualizarCli {
        #[arg(long, help = "Só verifica se há versão nova")]
        verificar: bool,
    },
    #[command(about = "Mostra a configuração em uso ou escolhe onde guardar o token")]
    Config {
        #[command(subcommand)]
        acao: Option<ConfigCmd>,
    },
    #[command(
        about = "Para scripts: imprime um access token válido (é um segredo)",
        long_about = "Para scripts e suítes de avaliação: imprime no stdout um access token válido, renovado se estiver perto de expirar.\n\nATENÇÃO: o token é um segredo e dá acesso à sua conta. Não cole em chat, log ou arquivo versionado.\n\nExemplo: MCP_TOKEN=$(etus token)\n\nRecusa quando a saída é um terminal, para o token não ficar visível na tela; use --force para imprimir mesmo assim."
    )]
    Token {
        #[arg(long, help = "Imprime mesmo quando a saída é um terminal")]
        force: bool,
    },
    #[command(
        about = "Marca o item do Plane em que você está trabalhando (etiqueta x-etus-work)",
        long_about = "Marca o item do Plane em que você está trabalhando, para o uso de IA ser medido por empresa, projeto e item.\n\nO estado fica no repositório atual (ou na pasta do Claude Code fora de um repositório) até você mudar. Os hooks do instalador do roteador leem esse estado e mandam o header x-etus-work em toda chamada do Claude Code.\n\nSem argumentos, mostra o estado. Só com --fase, --projeto ou --empresa, altera o trabalho já marcado.",
        args_conflicts_with_subcommands = true
    )]
    Trabalho {
        #[arg(value_name = "ITEM", help = "Chave do item no Plane, por exemplo ENG-123")]
        item: Option<String>,
        #[arg(long, value_name = "FASE", help = "plan, impl, review ou qa")]
        fase: Option<String>,
        #[arg(
            long,
            value_name = "PROJETO",
            help = "Projeto da Etus: hub:<id>, chat:<id> ou rotina:<id>"
        )]
        projeto: Option<String>,
        #[arg(
            long,
            value_name = "EMPRESA",
            help = "Id da empresa no hub (padrão: a empresa ativa do etus)"
        )]
        empresa: Option<String>,
        #[arg(long, value_name = "ETIQUETA", help = "Etiqueta livre, para trabalho fora do Plane")]
        etiqueta: Option<String>,
        #[command(subcommand)]
        acao: Option<TrabalhoCmd>,
    },
    #[command(external_subcommand)]
    External(Vec<String>),
}

#[derive(Debug, Subcommand)]
pub enum TrabalhoCmd {
    #[command(about = "Mostra o trabalho marcado e o header que vai sair")]
    Mostrar,
    #[command(about = "Imprime só a linha x-etus-work, para hooks e scripts")]
    Cabecalho,
    #[command(about = "Apaga o trabalho marcado (volta ao que o branch disser)")]
    Limpar,
}

#[derive(Debug, Subcommand)]
pub enum ConfigCmd {
    #[command(about = "Mostra a configuração em uso e onde ela fica")]
    Mostrar,
    #[command(
        alias = "token-store",
        about = "Escolhe onde guardar o token: arquivo (padrão) ou chaveiro do sistema"
    )]
    Armazenamento {
        #[arg(value_enum, value_name = "ONDE", help = "arquivo ou chaveiro")]
        onde: StoreKind,
    },
    #[command(
        name = "abrir-links",
        about = "Abre sozinho no navegador os links de ação pedidos pelas ferramentas, quando há terminal"
    )]
    AbrirLinks {
        #[arg(value_enum, value_name = "VALOR", help = "sim ou nao")]
        valor: Toggle,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Toggle {
    Sim,
    Nao,
}

#[derive(Debug, Subcommand)]
pub enum EmpresaCmd {
    #[command(about = "Lista as suas empresas")]
    Listar,
    #[command(about = "Troca a empresa ativa (id, slug ou nome)")]
    Usar {
        #[arg(value_name = "EMPRESA")]
        empresa: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum PerfilCmd {
    #[command(about = "Mostra o perfil em uso")]
    Mostrar,
    #[command(about = "Usa este perfil nas próximas chamadas")]
    Usar {
        #[arg(value_name = "ID")]
        id: String,
    },
    #[command(about = "Volta a usar o acesso normal, sem perfil")]
    Limpar,
}

#[derive(Debug, Subcommand)]
pub enum SkillsCmd {
    #[command(about = "Busca skills pelo termo")]
    Buscar {
        #[arg(required = true, num_args = 1.., value_name = "TERMO")]
        termo: Vec<String>,
    },
    #[command(about = "Lê uma skill pelo id")]
    Ler {
        #[arg(value_name = "ID")]
        id: String,
    },
    #[command(
        visible_alias = "sync",
        about = "Gera uma skill por app que você pode usar, na pasta de skills do agente",
        long_about = "Gera uma skill (SKILL.md) por app que você pode usar na empresa ativa, com os comandos, parâmetros, permissões, receitas e cuidados, a partir do catálogo atual do hub.\n\nGrava em ~/.claude/skills (Claude Code) ou ~/.agents/skills (Codex); --projeto usa a pasta do repositório. Só mexe nas pastas etus-<app> que ele mesmo gerou: atualiza as que mudaram e remove as dos apps que você não usa mais. Rode de novo depois de trocar de empresa ou ganhar acesso a um app."
    )]
    Sincronizar {
        #[arg(
            long,
            value_enum,
            default_value = "claude",
            value_name = "AGENTE",
            help = "claude, codex ou todos"
        )]
        agente: SkillAgent,
        #[arg(
            long,
            help = "Grava nas pastas do projeto (raiz do repositório ou pasta atual) em vez das suas"
        )]
        projeto: bool,
        #[arg(
            long,
            value_name = "PASTA",
            help = "Pasta de skills explícita; vale mais que --agente e --projeto"
        )]
        pasta: Option<PathBuf>,
    },
}

pub fn localize(cmd: Command) -> Command {
    let names: Vec<String> = cmd.get_subcommands().map(|s| s.get_name().to_string()).collect();
    let template = if cmd.get_before_help().is_some() {
        TOOL_TEMPLATE
    } else {
        TEMPLATE
    };
    let mut cmd = cmd
        .help_template(template)
        .disable_help_subcommand(true)
        .subcommand_help_heading("Comandos")
        .subcommand_value_name("COMANDO")
        .disable_help_flag(true)
        .arg(
            Arg::new("help")
                .short('h')
                .long("help")
                .action(ArgAction::Help)
                .help("Mostra a ajuda"),
        )
        .mut_args(|arg| {
            if arg.get_help_heading().is_some() {
                arg
            } else if arg.is_positional() {
                arg.help_heading("Argumentos")
            } else {
                arg.help_heading("Opções")
            }
        });
    for name in names {
        cmd = cmd.mut_subcommand(name, localize);
    }
    cmd
}

pub fn root_command(after_help: Option<String>) -> Command {
    let mut cmd = Cli::command().override_usage("etus [OPÇÕES] [COMANDO]").arg(
        Arg::new("version")
            .short('V')
            .long("version")
            .action(ArgAction::Version)
            .help("Mostra a versão"),
    );
    if let Some(text) = after_help {
        cmd = cmd.after_help(text);
    }
    localize(cmd)
}

pub fn count(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

pub fn app_line_aliases(app: &App) -> String {
    if app.aliases.is_empty() {
        String::new()
    } else {
        format!("também: {}", app.aliases.join(", "))
    }
}

pub fn apps_after_help(apps: &[App]) -> String {
    let mut text = String::from("Apps (do catálogo em cache; `etus atualizar` renova):\n");
    let width = apps.iter().map(|a| a.word.chars().count()).max().unwrap_or(0);
    for app in apps {
        let aliases = app_line_aliases(app);
        let line = format!("  {:width$}  {}", app.word, app.name);
        if aliases.is_empty() {
            text.push_str(&format!("{line}\n"));
        } else {
            text.push_str(&format!("{line} ({aliases})\n"));
        }
    }
    text.push_str("\nAgentes: `etus <app> --help` lista os comandos; use --json para saída compacta.");
    text
}

pub const NO_CATALOG_HELP: &str = "Apps: rode `etus login` e depois `etus apps`.\nAgentes: `etus <app> --help` lista os comandos; use --json para saída compacta.";

fn input_flag(cmd: Command) -> Command {
    cmd.arg(
        Arg::new("input")
            .long("input")
            .value_name("ARQUIVO")
            .help("Lê os argumentos de um arquivo JSON (- para stdin)"),
    )
}

fn standalone_flags(cmd: Command) -> Command {
    cmd.arg(
        Arg::new("json")
            .long("json")
            .num_args(0..=1)
            .default_missing_value("")
            .value_name("OBJETO")
            .help("Saída em JSON compacto; com um objeto JSON, usa-o como argumentos"),
    )
    .arg(
        Arg::new("yes")
            .short('y')
            .long("yes")
            .action(ArgAction::SetTrue)
            .help("Confirma sem perguntar"),
    )
    .arg(
        Arg::new("dry-run")
            .long("dry-run")
            .action(ArgAction::SetTrue)
            .help("Mostra a chamada sem executar"),
    )
    .arg(
        Arg::new("quiet")
            .short('q')
            .long("quiet")
            .action(ArgAction::SetTrue)
            .help("Não imprime nada em caso de sucesso"),
    )
    .arg(
        Arg::new("open")
            .long("open")
            .visible_alias("abrir")
            .action(ArgAction::SetTrue)
            .help(OPEN_HELP),
    )
    .arg(
        Arg::new("wait")
            .long("wait")
            .visible_alias("aguardar")
            .value_name("SEGUNDOS")
            .num_args(0..=1)
            .require_equals(true)
            .default_missing_value(DEFAULT_WAIT)
            .value_parser(clap::value_parser!(u64))
            .help(WAIT_HELP),
    )
    .arg(Arg::new("hub").long("hub").value_name("URL").hide(true))
    .arg(Arg::new("perfil").long("perfil").value_name("ID").hide(true))
}

fn tool_kind(tool: &Tool) -> &'static str {
    if !tool.writes() {
        "só leitura"
    } else if tool.destructive() {
        "altera dados, destrutiva; pede --yes"
    } else {
        "altera dados; pede --yes"
    }
}

pub struct Invocation<'a> {
    pub app: &'a str,
    pub command: Option<&'a str>,
}

impl<'a> Invocation<'a> {
    pub fn canonical(app: &'a App) -> Self {
        Invocation {
            app: &app.word,
            command: None,
        }
    }
}

pub fn tool_command(command: &AppCommand, invocation: &Invocation, standalone: bool) -> Command {
    let tool = &command.tool;
    let params = schema::params(&tool.input_schema);
    let summary = if tool.writes() {
        format!("{} [altera]", tool.summary())
    } else {
        tool.summary()
    };
    let mut long = String::new();
    if let Some(title) = tool.display_title() {
        long.push_str(title);
        long.push_str("\n\n");
    }
    long.push_str(tool.description_body());
    let long = long.trim().to_string();
    let typed = invocation
        .command
        .filter(|word| command.matches(word))
        .unwrap_or(&command.name);
    let example = schema::example(&params);
    let mut usage_line = format!("etus {} {}", invocation.app, typed);
    if !example.is_empty() {
        usage_line.push(' ');
        usage_line.push_str(&example);
    }
    if tool.writes() {
        usage_line.push_str(" --yes");
    }
    let mut after = format!("Exemplo:\n  {usage_line}\n\n");
    let others: Vec<&str> = std::iter::once(&command.name)
        .chain(&command.aliases)
        .map(String::as_str)
        .filter(|form| *form != typed)
        .collect();
    if !others.is_empty() {
        after.push_str(&format!("Também aceita: {}.\n", others.join(", ")));
    }
    after.push_str(&format!(
        "Ferramenta MCP: {} ({}).\nArgumentos também por --json '{{...}}' ou --input arquivo.json (- lê do stdin).",
        tool.name,
        tool_kind(tool)
    ));
    let cmd = Command::new(command.name.clone())
        .override_usage(format!("etus {} {} [OPÇÕES]", invocation.app, typed))
        .about(summary)
        .before_help(long)
        .after_help(after)
        .aliases(command.aliases.clone())
        .args(schema::to_args(&params));
    let cmd = input_flag(cmd);
    if standalone { standalone_flags(cmd) } else { cmd }
}

pub fn app_command(app: &App, invocation: &Invocation) -> Command {
    localize(build_app_command(app, invocation, true))
}

fn build_app_command(app: &App, invocation: &Invocation, standalone: bool) -> Command {
    let mut cmd = Command::new(invocation.app.to_string())
        .bin_name(format!("etus {}", invocation.app))
        .about(format!(
            "{}: {}",
            app.name,
            count(app.commands.len(), "comando", "comandos")
        ))
        .after_help(format!("Ajuda de um comando: etus {} <comando> --help", invocation.app))
        .subcommand_required(true)
        .arg_required_else_help(true);
    if invocation.app == app.word {
        cmd = cmd.visible_aliases(app.aliases.clone());
    }
    for command in &app.commands {
        cmd = cmd.subcommand(tool_command(command, invocation, standalone));
    }
    cmd
}

pub fn full_command(apps: &[App]) -> Command {
    let mut cmd = Cli::command();
    for app in apps {
        cmd = cmd.subcommand(build_app_command(app, &Invocation::canonical(app), false));
    }
    localize(
        cmd.arg(
            Arg::new("version")
                .short('V')
                .long("version")
                .action(ArgAction::Version)
                .help("Mostra a versão"),
        ),
    )
}

pub fn extract_flag(args: &[String], flag: &str) -> Option<String> {
    let with_eq = format!("{flag}=");
    let mut iter = args.iter();
    let mut found = None;
    while let Some(arg) = iter.next() {
        if arg == "--" {
            break;
        }
        if arg == flag {
            found = iter.next().cloned();
        } else if let Some(value) = arg.strip_prefix(&with_eq) {
            found = Some(value.to_string());
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::group;
    use serde_json::json;

    fn apps() -> Vec<App> {
        let tools: Vec<Tool> = serde_json::from_value(json!([
            {"name": "tasks__list_tasks", "title": "Listar tarefas", "description": "Tarefas: Lists tasks.", "inputSchema": {"type": "object", "properties": {"view": {"type": "string", "enum": ["inbox", "today"]}}, "required": ["view"]}, "annotations": {"readOnlyHint": true}},
            {"name": "tasks__create_task", "title": "Criar tarefa", "description": "Tarefas: Creates a task as the person, in the inbox or in a project board, with a due date and a priority.", "inputSchema": {"type": "object", "properties": {"title": {"type": "string"}}, "required": ["title"]}, "annotations": {"readOnlyHint": false, "destructiveHint": false}}
        ]))
        .unwrap();
        group(&tools)
    }

    #[test]
    fn trabalho_parses_item_flags_and_subcommands() {
        let cli = |args: &[&str]| {
            let m = root_command(None).try_get_matches_from(args).unwrap();
            <Cli as clap::FromArgMatches>::from_arg_matches(&m).unwrap().command
        };
        match cli(&["etus", "trabalho", "ENG-123", "--fase", "qa", "--projeto", "hub:p1"]) {
            Some(Commands::Trabalho {
                item,
                fase,
                projeto,
                acao: None,
                ..
            }) => assert_eq!(
                (item.as_deref(), fase.as_deref(), projeto.as_deref()),
                (Some("ENG-123"), Some("qa"), Some("hub:p1"))
            ),
            other => panic!("unexpected {other:?}"),
        }
        assert!(matches!(
            cli(&["etus", "trabalho", "mostrar"]),
            Some(Commands::Trabalho {
                item: None,
                acao: Some(TrabalhoCmd::Mostrar),
                ..
            })
        ));
        assert!(matches!(
            cli(&["etus", "trabalho", "limpar"]),
            Some(Commands::Trabalho {
                acao: Some(TrabalhoCmd::Limpar),
                ..
            })
        ));
        assert!(matches!(
            cli(&["etus", "trabalho", "--fase", "qa", "limpar"]),
            Some(Commands::Trabalho {
                item: Some(_),
                acao: None,
                ..
            })
        ));
    }

    #[test]
    fn root_parses_builtins_and_external_apps() {
        let cli = |args: &[&str]| {
            let m = root_command(None).try_get_matches_from(args).unwrap();
            <Cli as clap::FromArgMatches>::from_arg_matches(&m).unwrap()
        };
        assert!(matches!(
            cli(&["etus", "login", "--no-browser"]).command,
            Some(Commands::Login { no_browser: true, .. })
        ));
        let parsed = cli(&["etus", "--json", "tarefas", "list-tasks", "--view", "inbox"]);
        assert!(parsed.json);
        match parsed.command {
            Some(Commands::External(args)) => assert_eq!(args, vec!["tarefas", "list-tasks", "--view", "inbox"]),
            other => panic!("unexpected {other:?}"),
        }
    }

    fn render_help(app: &App, argv: &[&str]) -> String {
        let invocation = Invocation {
            app: argv[0],
            command: argv.get(1).copied(),
        };
        let error = app_command(app, &invocation).try_get_matches_from(argv).unwrap_err();
        assert_eq!(error.exit_code(), 0);
        error.to_string()
    }

    #[test]
    fn app_commands_are_generated_from_tools() {
        let apps = apps();
        let app = &apps[0];
        assert_eq!(app.word, "tarefas");
        let cmd = app_command(app, &Invocation::canonical(app));
        let matches = cmd
            .clone()
            .try_get_matches_from(["tarefas", "list-tasks", "--view", "today", "--json"])
            .unwrap();
        let (name, sub) = matches.subcommand().unwrap();
        assert_eq!(name, "listar-tarefas");
        assert_eq!(sub.get_one::<String>("view").unwrap(), "today");
        assert_eq!(sub.get_one::<String>("json").unwrap(), "");
        let with_object = cmd
            .clone()
            .try_get_matches_from(["tarefas", "criar-tarefa", "--json", "{\"title\":\"x\"}"])
            .unwrap();
        let (name, sub) = with_object.subcommand().unwrap();
        assert_eq!(name, "criar-tarefa");
        assert_eq!(sub.get_one::<String>("json").unwrap(), "{\"title\":\"x\"}");
        let unknown = cmd.try_get_matches_from(["tarefas", "nope"]).unwrap_err();
        assert_eq!(unknown.exit_code(), 2);
    }

    #[test]
    fn help_uses_the_words_the_person_typed() {
        let apps = apps();
        let app = &apps[0];
        let pt = render_help(app, &["tarefas", "--help"]);
        assert!(pt.contains("Uso: etus tarefas <COMANDO>"), "{pt}");
        assert!(pt.contains("listar-tarefas") && pt.contains("criar-tarefa"));
        let en = render_help(app, &["tasks", "--help"]);
        assert!(en.contains("Uso: etus tasks <COMANDO>"), "{en}");
        let tool = render_help(app, &["tasks", "create-task", "--help"]);
        assert!(tool.contains("Uso: etus tasks create-task"), "{tool}");
        assert!(tool.contains("etus tasks create-task --title <title> --yes"));
        assert!(tool.contains("--title"));
        assert!(tool.contains("Creates a task as the person, in the inbox or in a project board"));
        let canonical = render_help(app, &["tarefas", "criar-tarefa", "--help"]);
        assert!(canonical.contains("Uso: etus tarefas criar-tarefa"), "{canonical}");
        assert!(canonical.contains("Também aceita: create-task."));
        assert!(tool.contains("Também aceita: criar-tarefa."));
    }

    #[test]
    fn long_descriptions_are_cut_in_the_list_only() {
        let apps = apps();
        let help = render_help(&apps[0], &["tarefas", "--help"]);
        assert!(help.contains("\u{2026}"), "{help}");
        assert!(!help.contains("due date and a priority"));
        assert!(!help.contains("alias"));
    }

    #[test]
    fn full_command_includes_apps_for_completions() {
        let cmd = full_command(&apps());
        let tasks = cmd.get_subcommands().find(|s| s.get_name() == "tarefas").unwrap();
        assert!(tasks.get_all_aliases().any(|a| a == "tasks"));
        cmd.debug_assert();
    }

    #[test]
    fn extracts_global_flags_from_raw_args() {
        let args: Vec<String> = ["tasks", "x", "--perfil", "p1", "--hub=https://h"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(extract_flag(&args, "--perfil").as_deref(), Some("p1"));
        assert_eq!(extract_flag(&args, "--hub").as_deref(), Some("https://h"));
        assert_eq!(extract_flag(&args, "--json"), None);
    }
}
