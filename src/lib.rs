pub mod action;
pub mod auth;
pub mod catalog;
pub mod cli;
pub mod commands;
pub mod config;
pub mod confirm;
pub mod context;
pub mod error;
pub mod mcp;
pub mod naming;
pub mod output;
pub mod schema;
pub mod setup;
pub mod skills;
pub mod update;
pub mod work;

#[cfg(test)]
pub mod testutil;

use clap::FromArgMatches;

use crate::cli::{Cli, NO_CATALOG_HELP, apps_after_help, extract_flag, root_command};
use crate::config::{Paths, Settings};
use crate::context::Ctx;
use crate::error::Error;
use crate::output::Output;

pub fn run_args(argv: Vec<String>) -> i32 {
    let raw: Vec<String> = argv.iter().skip(1).cloned().collect();
    let paths = match Paths::resolve() {
        Ok(paths) => paths,
        Err(error) => {
            eprintln!("etus: {error}");
            return error.exit_code();
        }
    };
    let early = Settings::resolve(
        paths.clone(),
        extract_flag(&raw, "--hub").as_deref(),
        extract_flag(&raw, "--perfil").as_deref(),
    );
    let after_help = early
        .as_ref()
        .ok()
        .and_then(|s| catalog::load_cache(&s.paths, &s.hub, s.perfil.as_deref()))
        .map(|cache| apps_after_help(&cache.apps()))
        .unwrap_or_else(|| NO_CATALOG_HELP.to_string());
    let matches = match root_command(Some(after_help)).try_get_matches_from(&argv) {
        Ok(matches) => matches,
        Err(error) => {
            let _ = error.print();
            return error.exit_code();
        }
    };
    let cli = match Cli::from_arg_matches(&matches) {
        Ok(cli) => cli,
        Err(error) => {
            let _ = error.print();
            return error.exit_code();
        }
    };
    let out = Output {
        json: cli.json,
        quiet: cli.quiet,
    };
    let settings = match early {
        Ok(settings) => settings,
        Err(error) => {
            out.error(&error);
            return error.exit_code();
        }
    };
    let Some(command) = cli.command else {
        let _ = root_command(Some(NO_CATALOG_HELP.to_string())).print_help();
        return error::EXIT_USAGE;
    };
    let ctx = Ctx::new(settings, out, cli.yes, cli.dry_run).with_actions(cli.open, cli.wait);
    match commands::dispatch(&ctx, command) {
        Ok(()) => 0,
        Err(error) => {
            report(&ctx.out, &raw, &error);
            error.exit_code()
        }
    }
}

fn report(out: &Output, raw: &[String], error: &Error) {
    let json = out.json
        || raw
            .iter()
            .take_while(|a| *a != "--")
            .any(|a| a == "--json" || a.starts_with("--json="));
    Output { json, ..*out }.error(error);
}

pub fn run() -> i32 {
    run_args(std::env::args().collect())
}
