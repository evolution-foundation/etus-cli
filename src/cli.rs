use clap::{Arg, ArgAction, Command, CommandFactory, Parser, Subcommand};
use clap_complete::Shell;

use crate::catalog::{App, Tool};
use crate::schema;

const TEMPLATE: &str = "{about-with-newline}\nUso: {usage}\n\n{all-args}{after-help}";
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
    #[command(about = "Busca e lê skills do catálogo do Extensões")]
    Skills {
        #[command(subcommand)]
        acao: SkillsCmd,
    },
    #[command(about = "Prepara esta máquina para os agentes (em breve)")]
    Setup,
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
    #[command(about = "Mostra a configuração em uso e onde ela fica")]
    Config,
    #[command(external_subcommand)]
    External(Vec<String>),
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
    let mut cmd = Cli::command().arg(
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

pub fn apps_after_help(apps: &[App]) -> String {
    let mut text = String::from("Apps (do catálogo em cache; `etus atualizar` renova):\n");
    let width = apps.iter().map(|a| a.prefix.len()).max().unwrap_or(0);
    for app in apps {
        let aliases = if app.aliases.is_empty() {
            String::new()
        } else {
            format!(" ({})", app.aliases.join(", "))
        };
        text.push_str(&format!("  {:width$}  {}{}\n", app.prefix, app.name, aliases));
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

pub fn tool_command(app: &App, tool: &Tool, taken: &mut Vec<String>, standalone: bool) -> Command {
    let name = tool.command_name();
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
    let example = schema::example(&params);
    let mut usage_line = format!("etus {} {}", app.prefix, name);
    if !example.is_empty() {
        usage_line.push(' ');
        usage_line.push_str(&example);
    }
    if tool.writes() {
        usage_line.push_str(" --yes");
    }
    let after = format!(
        "Exemplo:\n  {usage_line}\n\nFerramenta MCP: {} ({}).\nArgumentos também por --json '{{...}}' ou --input arquivo.json (- lê do stdin).",
        tool.name,
        tool_kind(tool)
    );
    let mut cmd = Command::new(name.clone())
        .about(summary)
        .before_help(long)
        .after_help(after)
        .args(schema::to_args(&params));
    taken.push(name);
    if let Some(alias) = tool.title_alias().filter(|a| !taken.contains(a)) {
        taken.push(alias.clone());
        cmd = cmd.visible_alias(alias);
    }
    let cmd = input_flag(cmd);
    if standalone { standalone_flags(cmd) } else { cmd }
}

pub fn app_command(app: &App) -> Command {
    localize(build_app_command(app, true))
}

fn build_app_command(app: &App, standalone: bool) -> Command {
    let mut taken: Vec<String> = app.tools.iter().map(Tool::command_name).collect();
    let mut cmd = Command::new(app.prefix.clone())
        .bin_name(format!("etus {}", app.prefix))
        .about(format!("{}: {} comandos", app.name, app.tools.len()))
        .after_help(format!("Ajuda de um comando: etus {} <comando> --help", app.prefix))
        .visible_aliases(app.aliases.clone())
        .subcommand_required(true)
        .arg_required_else_help(true);
    for tool in &app.tools {
        cmd = cmd.subcommand(tool_command(app, tool, &mut taken, standalone));
    }
    cmd
}

pub fn full_command(apps: &[App]) -> Command {
    let mut cmd = Cli::command();
    for app in apps {
        if !crate::naming::BUILTIN_COMMANDS.contains(&app.prefix.as_str()) {
            cmd = cmd.subcommand(build_app_command(app, false));
        }
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
            {"name": "tasks__create_task", "title": "Criar tarefa", "description": "Tarefas: Creates a task.", "inputSchema": {"type": "object", "properties": {"title": {"type": "string"}}, "required": ["title"]}, "annotations": {"readOnlyHint": false, "destructiveHint": false}}
        ]))
        .unwrap();
        group(&tools)
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

    #[test]
    fn app_commands_are_generated_from_tools() {
        let apps = apps();
        let cmd = app_command(&apps[0]);
        let matches = cmd
            .clone()
            .try_get_matches_from(["tasks", "listar-tarefas", "--view", "today", "--json"])
            .unwrap();
        let (name, sub) = matches.subcommand().unwrap();
        assert_eq!(name, "list-tasks");
        assert_eq!(sub.get_one::<String>("view").unwrap(), "today");
        assert_eq!(sub.get_one::<String>("json").unwrap(), "");
        let with_object = cmd
            .clone()
            .try_get_matches_from(["tasks", "create-task", "--json", "{\"title\":\"x\"}"])
            .unwrap();
        assert_eq!(
            with_object.subcommand().unwrap().1.get_one::<String>("json").unwrap(),
            "{\"title\":\"x\"}"
        );
        let help = cmd
            .clone()
            .try_get_matches_from(["tasks", "create-task", "--help"])
            .unwrap_err();
        assert_eq!(help.exit_code(), 0);
        let rendered = help.to_string();
        assert!(rendered.contains("--title"));
        assert!(rendered.contains("etus tasks create-task --title <title> --yes"));
        let unknown = cmd.try_get_matches_from(["tasks", "nope"]).unwrap_err();
        assert_eq!(unknown.exit_code(), 2);
    }

    #[test]
    fn full_command_includes_apps_for_completions() {
        let cmd = full_command(&apps());
        assert!(cmd.get_subcommands().any(|s| s.get_name() == "tasks"));
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
