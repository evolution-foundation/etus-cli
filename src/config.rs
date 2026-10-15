use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::auth::store::{StoreKind, select_kind};
use crate::error::{Error, Result};

pub const DEFAULT_HUB: &str = "https://apps.etus.io";
pub const DEFAULT_CACHE_TTL_SECS: u64 = 300;

#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConfigFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hub_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub perfil: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_ttl_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_store: Option<StoreKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_actions: Option<bool>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub clients: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct Paths {
    pub config_dir: PathBuf,
    pub cache_dir: PathBuf,
}

impl Paths {
    pub fn resolve() -> Result<Paths> {
        if let Some(dir) = std::env::var_os("ETUS_CONFIG_DIR").filter(|d| !d.is_empty()) {
            let config_dir = PathBuf::from(dir);
            return Ok(Paths {
                cache_dir: config_dir.join("cache"),
                config_dir,
            });
        }
        let base = directories::BaseDirs::new()
            .ok_or_else(|| Error::Other("Não foi possível achar a pasta do usuário.".into()))?;
        Ok(Paths {
            config_dir: base.config_dir().join("etus"),
            cache_dir: base.cache_dir().join("etus"),
        })
    }

    pub fn at(dir: &Path) -> Paths {
        Paths {
            config_dir: dir.to_path_buf(),
            cache_dir: dir.join("cache"),
        }
    }

    pub fn config_file(&self) -> PathBuf {
        self.config_dir.join("config.json")
    }

    pub fn lock_file(&self, hub: &str) -> PathBuf {
        self.config_dir.join(format!("auth-{}.lock", short_hash(&[hub])))
    }

    pub fn keychain_marker(&self, hub: &str) -> PathBuf {
        self.config_dir
            .join(format!("keychain-migrated-{}", short_hash(&[hub])))
    }

    pub fn tokens_file(&self, hub: &str) -> PathBuf {
        self.config_dir.join(format!("tokens-{}.json", short_hash(&[hub])))
    }

    pub fn catalog_file(&self, hub: &str, perfil: Option<&str>) -> PathBuf {
        self.cache_dir
            .join(format!("catalog-{}.json", short_hash(&[hub, perfil.unwrap_or("")])))
    }
}

pub fn short_hash(parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part.as_bytes());
        hasher.update([0u8]);
    }
    hasher.finalize().iter().take(6).map(|b| format!("{b:02x}")).collect()
}

pub fn load(paths: &Paths) -> Result<ConfigFile> {
    match fs::read_to_string(paths.config_file()) {
        Ok(text) => serde_json::from_str(&text).map_err(|e| {
            Error::Usage(format!(
                "Arquivo de configuração inválido ({}): {e}",
                paths.config_file().display()
            ))
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(ConfigFile::default()),
        Err(e) => Err(e.into()),
    }
}

pub fn save(paths: &Paths, config: &ConfigFile) -> Result<()> {
    write_private(&paths.config_file(), serde_json::to_string_pretty(config)?.as_bytes())
}

pub fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path.parent().ok_or_else(|| Error::Other("Caminho inválido.".into()))?;
    fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(
        ".{}.{}.tmp",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("file"),
        std::process::id()
    ));
    {
        let mut options = fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp)?;
        std::io::Write::write_all(&mut file, bytes)?;
        file.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    Ok(())
}

pub fn normalize_hub(raw: &str) -> Result<String> {
    let trimmed = raw.trim().trim_end_matches('/');
    let url = reqwest::Url::parse(trimmed).map_err(|_| Error::Usage(format!("Endereço do hub inválido: {raw}")))?;
    let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    match url.scheme() {
        "https" => {}
        "http" if loopback => {}
        _ => return Err(Error::Usage("O hub precisa ser https (http só em localhost).".into())),
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(Error::Usage(
            "O endereço do hub não pode ter query nem fragmento.".into(),
        ));
    }
    Ok(trimmed.to_string())
}

pub fn parse_toggle(raw: &str) -> Option<bool> {
    match raw.trim().to_lowercase().as_str() {
        "1" | "true" | "sim" | "s" | "yes" | "on" => Some(true),
        "0" | "false" | "nao" | "não" | "n" | "no" | "off" => Some(false),
        _ => None,
    }
}

#[derive(Debug, Clone)]
pub struct Settings {
    pub hub: String,
    pub perfil: Option<String>,
    pub cache_ttl: Duration,
    pub token_store: StoreKind,
    pub token_store_from_env: bool,
    pub open_actions: bool,
    pub paths: Paths,
    pub file: ConfigFile,
}

impl Settings {
    pub fn resolve(paths: Paths, hub_flag: Option<&str>, perfil_flag: Option<&str>) -> Result<Settings> {
        let file = load(&paths)?;
        let env_hub = std::env::var("ETUS_HUB_URL").ok().filter(|v| !v.trim().is_empty());
        let hub = hub_flag
            .map(str::to_string)
            .or(env_hub)
            .or_else(|| file.hub_url.clone())
            .unwrap_or_else(|| DEFAULT_HUB.to_string());
        let perfil = perfil_flag
            .map(str::to_string)
            .or_else(|| file.perfil.clone())
            .filter(|p| !p.trim().is_empty());
        let open_actions = std::env::var("ETUS_OPEN_ACTIONS")
            .ok()
            .and_then(|v| parse_toggle(&v))
            .or(file.open_actions)
            .unwrap_or(false);
        let env_store = std::env::var("ETUS_TOKEN_STORE").ok().filter(|v| !v.trim().is_empty());
        Ok(Settings {
            token_store: select_kind(env_store.as_deref(), file.token_store)?,
            token_store_from_env: env_store.is_some(),
            open_actions,
            hub: normalize_hub(&hub)?,
            perfil,
            cache_ttl: Duration::from_secs(file.cache_ttl_secs.unwrap_or(DEFAULT_CACHE_TTL_SECS)),
            paths,
            file,
        })
    }

    pub fn mcp_url(&self) -> String {
        let mut url = reqwest::Url::parse(&format!("{}/mcp", self.hub)).expect("hub already validated");
        if let Some(perfil) = &self.perfil {
            url.query_pairs_mut().append_pair("perfil", perfil);
        }
        url.to_string()
    }

    pub fn save_file(&self) -> Result<()> {
        save(&self.paths, &self.file)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hub_must_be_https_or_loopback() {
        assert_eq!(normalize_hub("https://apps.etus.io/").unwrap(), "https://apps.etus.io");
        assert_eq!(normalize_hub("http://localhost:5011").unwrap(), "http://localhost:5011");
        assert!(normalize_hub("http://apps.etus.io").is_err());
        assert!(normalize_hub("ftp://x").is_err());
        assert!(normalize_hub("https://apps.etus.io/?a=1").is_err());
    }

    #[test]
    fn perfil_goes_to_the_mcp_url_as_query() {
        let settings = Settings {
            hub: "https://apps.etus.io".into(),
            perfil: Some("prf 1".into()),
            cache_ttl: Duration::from_secs(1),
            token_store: StoreKind::File,
            token_store_from_env: false,
            open_actions: false,
            paths: Paths::at(Path::new("/tmp/x")),
            file: ConfigFile::default(),
        };
        assert_eq!(settings.mcp_url(), "https://apps.etus.io/mcp?perfil=prf+1");
        let plain = Settings {
            perfil: None,
            ..settings
        };
        assert_eq!(plain.mcp_url(), "https://apps.etus.io/mcp");
    }

    #[test]
    fn toggles_accept_pt_and_en() {
        assert_eq!(parse_toggle("sim"), Some(true));
        assert_eq!(parse_toggle(" 1 "), Some(true));
        assert_eq!(parse_toggle("não"), Some(false));
        assert_eq!(parse_toggle("off"), Some(false));
        assert_eq!(parse_toggle("talvez"), None);
    }

    #[test]
    fn cache_files_differ_by_profile() {
        let paths = Paths::at(Path::new("/tmp/x"));
        assert_ne!(paths.catalog_file("h", None), paths.catalog_file("h", Some("p")));
        assert_eq!(paths.catalog_file("h", None), paths.catalog_file("h", None));
    }
}
