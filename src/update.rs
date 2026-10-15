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
pub const DEFAULT_DOWNLOAD_BASE: &str = "https://github.com/evolution-foundation/etus-cli/releases/latest/download/";
pub const DOWNLOAD_BASE_ENV: &str = "ETUS_CLI_DOWNLOAD_BASE";
pub const SUMS_FILE: &str = "SHA256SUMS";
pub const VERSION_FILE: &str = "VERSION";

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

enum Source {
    Public { base: String },
    GitHub { token: String, assets: Vec<Asset> },
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

pub fn download_url(base: &str, file: &str) -> String {
    format!("{}/{file}", base.trim_end_matches('/'))
}

pub fn expected_sum(sums: &str, file: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        let hash = fields.next()?;
        let name = fields.next()?.trim_start_matches('*');
        (name == file && hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit())).then(|| hash.to_lowercase())
    })
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

fn download_base() -> (String, bool) {
    match std::env::var(DOWNLOAD_BASE_ENV) {
        Ok(value) if !value.trim().is_empty() => (value.trim().to_string(), true),
        _ => (DEFAULT_DOWNLOAD_BASE.to_string(), false),
    }
}

fn manual_instructions(base: &str) -> String {
    format!(
        "Não consegui baixar de {base} e não achei um token do GitHub (GH_TOKEN, GITHUB_TOKEN ou `gh auth token`) para o repositório privado {REPO}.\nPara atualizar à mão: abra https://github.com/{REPO}/releases, baixe {} da versão mais nova, confira com o {SUMS_FILE}, dê permissão de execução (chmod +x) e troque o binário atual ({}).",
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

fn public_get(http: &Client, base: &str, file: &str) -> Result<Option<Vec<u8>>> {
    let url = download_url(base, file);
    let response = http.get(&url).send()?;
    match response.status().as_u16() {
        200 => Ok(Some(response.bytes()?.to_vec())),
        401 | 403 | 404 => Ok(None),
        status => Err(Error::Network(format!("{url} respondeu HTTP {status}."))),
    }
}

fn public_version(http: &Client, base: &str) -> Result<Option<String>> {
    let Some(bytes) = public_get(http, base, VERSION_FILE)? else {
        return Ok(None);
    };
    let text = String::from_utf8_lossy(&bytes).trim().to_string();
    if parse_version(&text).is_none() {
        return Err(Error::Other(format!(
            "O arquivo {VERSION_FILE} em {base} não traz uma versão válida."
        )));
    }
    Ok(Some(text))
}

impl Source {
    fn fetch(&self, http: &Client, file: &str) -> Result<Option<Vec<u8>>> {
        match self {
            Source::Public { base } => public_get(http, base, file),
            Source::GitHub { token, assets } => match assets.iter().find(|a| a.name == file) {
                Some(asset) => Ok(Some(
                    api_get(http, &asset.url, token, "application/octet-stream")?
                        .bytes()?
                        .to_vec(),
                )),
                None => Ok(None),
            },
        }
    }

    fn expected_sum(&self, http: &Client, file: &str) -> Result<Option<String>> {
        if let Some(sums) = self.fetch(http, SUMS_FILE)? {
            return Ok(expected_sum(&String::from_utf8_lossy(&sums), file));
        }
        Ok(self
            .fetch(http, &format!("{file}.sha256"))?
            .and_then(|text| expected_sum(&String::from_utf8_lossy(&text), file)))
    }
}

fn resolve(ctx: &Ctx, http: &Client) -> Result<(Source, String)> {
    let (base, explicit) = download_base();
    match public_version(http, &base) {
        Ok(Some(version)) => return Ok((Source::Public { base }, version)),
        Ok(None) if explicit => {
            return Err(Error::Other(format!(
                "Não achei {VERSION_FILE} em {base}. Confira o {DOWNLOAD_BASE_ENV}."
            )));
        }
        Err(error) if explicit => return Err(error),
        _ => {}
    }
    let Some((token, source)) = github_token() else {
        return Err(Error::Other(manual_instructions(&base)));
    };
    if source == "gh auth token" && !ctx.out.json {
        eprintln!("Usando o token do GitHub CLI (gh auth token) para ler o repositório privado.");
    }
    let release: Release = api_get(
        http,
        &format!("https://api.github.com/repos/{REPO}/releases/latest"),
        &token,
        "application/vnd.github+json",
    )?
    .json()?;
    Ok((
        Source::GitHub {
            token,
            assets: release.assets,
        },
        release.tag_name,
    ))
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
    let http = ctx.http()?;
    let (source, tag) = resolve(ctx, http)?;
    let latest = tag.trim().trim_start_matches('v').to_string();
    let newer = is_newer(&latest, current);
    if check_only || !newer {
        let text = if newer {
            format!("Versão nova disponível: {latest} (você tem {current}). Rode `etus atualizar-cli`.")
        } else {
            format!("Você já está na versão mais nova ({current}).")
        };
        ctx.out.print_human(
            &text,
            || json!({"current": current, "latest": latest, "updateAvailable": newer}),
        );
        return Ok(());
    }
    let name = asset_name();
    let bytes = source
        .fetch(http, &name)?
        .ok_or_else(|| Error::Other(format!("A versão {latest} não tem binário para esta máquina ({name}).")))?;
    let expected = source.expected_sum(http, &name)?.ok_or_else(|| {
        Error::Other(format!(
            "A versão {latest} não publica o sha256 de {name}. Nada foi trocado."
        ))
    })?;
    if expected != sha256_hex(&bytes) {
        return Err(Error::Other(
            "O binário baixado não confere com o sha256 publicado. Nada foi trocado.".into(),
        ));
    }
    let target = std::env::current_exe()?;
    replace_binary(&target, &bytes)?;
    ctx.out.print_human(
        &format!("etus atualizado de {current} para {latest}."),
        || json!({"previous": current, "current": latest}),
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

    #[test]
    fn download_url_joins_with_one_slash() {
        assert_eq!(download_url("https://x.io/dl/", "VERSION"), "https://x.io/dl/VERSION");
        assert_eq!(download_url("https://x.io/dl", "VERSION"), "https://x.io/dl/VERSION");
    }

    #[test]
    fn sums_are_found_by_exact_file_name() {
        let a = "a".repeat(64);
        let b = "B".repeat(64);
        let sums = format!("{a}  etus-x86_64-linux\n{b} *etus-aarch64-linux\nshort  etus-x\n");
        assert_eq!(expected_sum(&sums, "etus-x86_64-linux"), Some(a));
        assert_eq!(expected_sum(&sums, "etus-aarch64-linux"), Some("b".repeat(64)));
        assert_eq!(expected_sum(&sums, "etus-x"), None);
        assert_eq!(expected_sum(&sums, "etus-x86_64-macos"), None);
    }

    #[test]
    fn sha256_is_lowercase_hex() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
