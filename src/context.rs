use std::cell::OnceCell;
use std::time::Duration;

use reqwest::blocking::Client;
use serde_json::{Value, json};

use crate::auth::oauth::HttpRefresher;
use crate::auth::{Refresher, SecretStore, TokenManager, jwt_claims, store_for};
use crate::catalog::{CatalogCache, Company, clear_caches, load_cache, now_secs, save_cache};
use crate::config::Settings;
use crate::error::{Error, Result};
use crate::mcp::{McpClient, TokenSource};
use crate::output::Output;

pub const WHOAMI_TOOL: &str = "hub__whoami";
pub const SET_COMPANY_TOOL: &str = "hub__set_company";

impl<R: Refresher> TokenSource for TokenManager<R> {
    fn access_token(&self, rejected: Option<&str>) -> Result<String> {
        TokenManager::access_token(self, rejected)
    }
}

pub struct Ctx {
    pub settings: Settings,
    pub out: Output,
    pub yes: bool,
    pub dry_run: bool,
    http: OnceCell<Client>,
}

pub fn http_client() -> Result<Client> {
    Client::builder()
        .user_agent(concat!("etus-cli/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(90))
        .build()
        .map_err(|e| Error::Other(format!("Não foi possível iniciar o cliente HTTP: {e}")))
}

impl Ctx {
    pub fn new(settings: Settings, out: Output, yes: bool, dry_run: bool) -> Ctx {
        Ctx {
            settings,
            out,
            yes,
            dry_run,
            http: OnceCell::new(),
        }
    }

    pub fn http(&self) -> Result<&Client> {
        if let Some(client) = self.http.get() {
            return Ok(client);
        }
        let client = http_client()?;
        Ok(self.http.get_or_init(|| client))
    }

    pub fn store(&self) -> Box<dyn SecretStore> {
        store_for(&self.settings.paths, &self.settings.hub, self.settings.token_store)
    }

    pub fn manager(&self) -> Result<TokenManager<HttpRefresher>> {
        Ok(TokenManager {
            store: self.store(),
            lock_path: self.settings.paths.lock_file(&self.settings.hub),
            refresher: HttpRefresher {
                http: self.http()?.clone(),
            },
        })
    }

    pub fn with_mcp<T>(&self, f: impl FnOnce(&McpClient) -> Result<T>) -> Result<T> {
        let manager = self.manager()?;
        let client = McpClient::new(self.http()?, self.settings.mcp_url(), &manager);
        f(&client)
    }

    pub fn current_sub(&self) -> Result<String> {
        let tokens = self.store().load()?.ok_or_else(Error::not_logged_in)?;
        Ok(jwt_claims(&tokens.access_token)
            .and_then(|c| c.get("sub").and_then(Value::as_str).map(str::to_string))
            .unwrap_or_default())
    }

    pub fn cached_catalog(&self) -> Option<CatalogCache> {
        load_cache(
            &self.settings.paths,
            &self.settings.hub,
            self.settings.perfil.as_deref(),
        )
    }

    pub fn fetch_catalog(&self) -> Result<CatalogCache> {
        let sub = self.current_sub()?;
        let (tools, company) = self.with_mcp(|mcp| {
            let tools = mcp.list_tools()?;
            let company = if tools.iter().any(|t| t.name == WHOAMI_TOOL) {
                mcp.call_tool(WHOAMI_TOOL, &json!({}))
                    .ok()
                    .filter(|o| !o.is_error)
                    .and_then(|o| active_company(&crate::output::payload(&o.result)))
            } else {
                None
            };
            Ok((tools, company))
        })?;
        let cache = CatalogCache {
            fetched_at: now_secs(),
            hub: self.settings.hub.clone(),
            sub,
            perfil: self.settings.perfil.clone(),
            company,
            tools,
        };
        save_cache(&self.settings.paths, &cache)?;
        Ok(cache)
    }

    pub fn catalog(&self) -> Result<(CatalogCache, bool)> {
        let sub = self.current_sub()?;
        let cached = self.cached_catalog().filter(|c| c.sub == sub);
        if let Some(cache) = &cached
            && cache.is_fresh(self.settings.cache_ttl, now_secs())
        {
            return Ok((cache.clone(), false));
        }
        match (self.fetch_catalog(), cached) {
            (Ok(cache), _) => Ok((cache, true)),
            (Err(Error::Network(message)), Some(stale)) => {
                eprintln!("etus: usando o catálogo em cache ({message})");
                Ok((stale, false))
            }
            (Err(other), _) => Err(other),
        }
    }

    pub fn forget_catalog(&self) -> Result<()> {
        clear_caches(&self.settings.paths)
    }

    pub fn whoami(&self) -> Result<Value> {
        let outcome = self.with_mcp(|mcp| mcp.call_tool(WHOAMI_TOOL, &json!({})))?;
        if outcome.is_error {
            return Err(Error::Tool(
                crate::mcp::first_text(&outcome.result)
                    .unwrap_or("hub__whoami falhou")
                    .to_string(),
            ));
        }
        Ok(crate::output::payload(&outcome.result))
    }
}

pub fn active_company(whoami: &Value) -> Option<Company> {
    let company = whoami.get("activeCompany")?;
    Some(Company {
        id: company.get("id")?.as_str()?.to_string(),
        name: company.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_active_company_from_whoami() {
        let value = json!({"activeCompany": {"id": "org_1", "name": "Etus", "slug": "etus"}});
        assert_eq!(
            active_company(&value),
            Some(Company {
                id: "org_1".into(),
                name: "Etus".into()
            })
        );
        assert_eq!(active_company(&json!({"activeCompany": null})), None);
    }
}
