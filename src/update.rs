use std::fs;
use std::path::Path;
use std::process::Command;

use reqwest::blocking::Client;
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::context::Ctx;
use crate::error::{Error, Result};

pub const REPO: &str = "evolution-foundation/etus-cli";

#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Debug, Deserialize)]
struct Asset {
    name: String,
    url: String,
}

pub fn asset_name() -> String {
    format!("etus-{}-{}", std::env::consts::ARCH, std::env::consts::OS)
}

pub fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let core = text.trim().trim_start_matches('v').split(['-', '+']).next()?;
    let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
    Some((
        parts.next()??,
        parts.next().unwrap_or(Some(0))?,
        parts.next().unwrap_or(Some(0))?,
    ))
}

pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

fn manual_instructions() -> String {
    format!(
        "Não achei um token do GitHub (GH_TOKEN, GITHUB_TOKEN ou `gh auth token`). O repositório {REPO} é privado.\nPara atualizar à mão: abra https://github.com/{REPO}/releases, baixe {} da versão mais nova, dê permissão de execução (chmod +x) e troque o binário atual ({}).",
        asset_name(),
        std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "etus".into())
    )
}

fn github_token() -> Option<(String, &'static str)> {
    for var in ["GH_TOKEN", "GITHUB_TOKEN"] {
        if let Ok(value) = std::env::var(var)
            && !value.trim().is_empty()
        {
            return Some((value.trim().to_string(), "variável de ambiente"));
        }
    }
    let output = Command::new("gh").args(["auth", "token"]).output().ok()?;
    let token = String::from_utf8(output.stdout).ok()?.trim().to_string();
    (output.status.success() && !token.is_empty()).then_some((token, "gh auth token"))
}

fn api_get(http: &Client, url: &str, token: &str, accept: &str) -> Result<reqwest::blocking::Response> {
    let response = http
        .get(url)
        .bearer_auth(token)
        .header("accept", accept)
        .header("x-github-api-version", "2022-11-28")
        .send()?;
    match response.status().as_u16() {
        200 => Ok(response),
        404 => Err(Error::Other(format!(
            "Nenhuma versão publicada em {REPO}, ou o token não tem acesso ao repositório."
        ))),
        401 | 403 => Err(Error::Permission(
            "O GitHub recusou o token. Rode `gh auth login` ou confira o GH_TOKEN.".into(),
        )),
        status => Err(Error::Network(format!("O GitHub respondeu HTTP {status}."))),
    }
}

fn replace_binary(target: &Path, bytes: &[u8]) -> Result<()> {
    let dir = target
        .parent()
        .ok_or_else(|| Error::Other("Caminho do binário inválido.".into()))?;
    let tmp = dir.join(format!(".etus-update-{}", std::process::id()));
    fs::write(&tmp, bytes).map_err(|e| {
        Error::Other(format!(
            "Sem permissão para escrever em {} ({e}). Rode com sudo ou reinstale.",
            dir.display()
        ))
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o755))?;
    }
    fs::rename(&tmp, target).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })?;
    Ok(())
}

pub fn run(ctx: &Ctx, check_only: bool) -> Result<()> {
    let current = env!("CARGO_PKG_VERSION");
    let Some((token, source)) = github_token() else {
        return Err(Error::Other(manual_instructions()));
    };
    if source == "gh auth token" && !ctx.out.json {
        eprintln!("Usando o token do GitHub CLI (gh auth token) para ler o repositório privado.");
    }
    let http = ctx.http()?;
    let release: Release = api_get(
        http,
        &format!("https://api.github.com/repos/{REPO}/releases/latest"),
        &token,
        "application/vnd.github+json",
    )?
    .json()?;
    let newer = is_newer(&release.tag_name, current);
    if check_only || !newer {
        let text = if newer {
            format!(
                "Versão nova disponível: {} (você tem {current}). Rode `etus atualizar-cli`.",
                release.tag_name
            )
        } else {
            format!("Você já está na versão mais nova ({current}).")
        };
        ctx.out.print_human(
            &text,
            || json!({"current": current, "latest": release.tag_name, "updateAvailable": newer}),
        );
        return Ok(());
    }
    let name = asset_name();
    let asset = release.assets.iter().find(|a| a.name == name).ok_or_else(|| {
        Error::Other(format!(
            "A versão {} não tem binário para esta máquina ({name}).",
            release.tag_name
        ))
    })?;
    let bytes = api_get(http, &asset.url, &token, "application/octet-stream")?.bytes()?;
    if let Some(sum) = release.assets.iter().find(|a| a.name == format!("{name}.sha256")) {
        let expected = api_get(http, &sum.url, &token, "application/octet-stream")?.text()?;
        let expected = expected.split_whitespace().next().unwrap_or("").to_lowercase();
        let actual: String = Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect();
        if expected != actual {
            return Err(Error::Other(
                "O binário baixado não confere com o sha256 publicado. Nada foi trocado.".into(),
            ));
        }
    }
    let target = std::env::current_exe()?;
    replace_binary(&target, &bytes)?;
    ctx.out.print_human(
        &format!("etus atualizado de {current} para {}.", release.tag_name),
        || json!({"previous": current, "current": release.tag_name}),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_numerically() {
        assert_eq!(parse_version("v0.10.2"), Some((0, 10, 2)));
        assert_eq!(parse_version("1.2"), Some((1, 2, 0)));
        assert_eq!(parse_version("v1.0.0-beta.1"), Some((1, 0, 0)));
        assert!(is_newer("v0.10.0", "0.9.9"));
        assert!(!is_newer("v0.1.0", "0.1.0"));
        assert!(!is_newer("nightly", "0.1.0"));
    }

    #[test]
    fn asset_name_has_arch_and_os() {
        let name = asset_name();
        assert!(name.starts_with("etus-"));
        assert!(name.contains(std::env::consts::OS));
    }
}
