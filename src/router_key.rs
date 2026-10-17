use std::fmt;
use std::fs;
use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::action::{self, Action};
use crate::auth::store::{StoreKind, keychain_allowed, keychain_needs_terminal};
use crate::config::{Paths, short_hash, write_private};
use crate::context::Ctx;
use crate::error::{Error, Result};
use crate::mcp::TokenSource;

pub const PANEL_APP_KEY: &str = "router-panel";
const MAX_NAME_CHARS: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Origin {
    Login,
    Manual,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouterKey {
    pub key: String,
    pub origin: Origin,
    pub router_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_name: Option<String>,
}

impl fmt::Debug for RouterKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RouterKey")
            .field("key", &"<redacted>")
            .field("origin", &self.origin)
            .field("router_url", &self.router_url)
            .field("device_id", &self.device_id)
            .field("device_name", &self.device_name)
            .finish()
    }
}

impl RouterKey {
    pub fn origin_label(&self) -> String {
        match (self.origin, self.device_name.as_deref()) {
            (Origin::Login, Some(name)) => format!("login (computador {name})"),
            (Origin::Login, None) => "login".into(),
            (Origin::Manual, _) => "informada à mão (--chave)".into(),
        }
    }
}

pub trait Slot {
    fn load(&self) -> Result<Option<String>>;
    fn save(&self, text: &str) -> Result<()>;
    fn delete(&self) -> Result<()>;
}

pub struct FileSlot(pub PathBuf);

impl Slot for FileSlot {
    fn load(&self) -> Result<Option<String>> {
        match fs::read_to_string(&self.0) {
            Ok(text) => Ok(Some(text)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn save(&self, text: &str) -> Result<()> {
        write_private(&self.0, text.as_bytes())
    }

    fn delete(&self) -> Result<()> {
        match fs::remove_file(&self.0) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
struct KeychainSlot(String);

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl KeychainSlot {
    fn entry(&self) -> Result<keyring::Entry> {
        keyring::Entry::new("etus-cli", &self.0).map_err(keychain_error)
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn keychain_error(error: keyring::Error) -> Error {
    Error::Other(format!("Falha no chaveiro do sistema: {error}"))
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl Slot for KeychainSlot {
    fn load(&self) -> Result<Option<String>> {
        match self.entry()?.get_password() {
            Ok(text) => Ok(Some(text)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(keychain_error(error)),
        }
    }

    fn save(&self, text: &str) -> Result<()> {
        self.entry()?.set_password(text).map_err(keychain_error)
    }

    fn delete(&self) -> Result<()> {
        match self.entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(keychain_error(error)),
        }
    }
}

/// The router key lives where the login tokens live: a 0600 file by default, or the system keychain when chosen.
pub struct KeyVault {
    pub file: FileSlot,
    pub keychain: Option<Box<dyn Slot>>,
    pub keychain_allowed: bool,
}

impl KeyVault {
    pub fn for_hub(paths: &Paths, hub: &str, kind: StoreKind) -> KeyVault {
        KeyVault {
            file: FileSlot(paths.config_dir.join(format!("router-key-{}.json", short_hash(&[hub])))),
            keychain: if kind == StoreKind::Keychain {
                system_keychain(hub)
            } else {
                None
            },
            keychain_allowed: keychain_allowed(),
        }
    }

    fn keychain(&self) -> Result<Option<&dyn Slot>> {
        match self.keychain.as_deref() {
            Some(_) if !self.keychain_allowed => Err(keychain_needs_terminal()),
            other => Ok(other),
        }
    }

    pub fn load(&self) -> Result<Option<RouterKey>> {
        let text = match self.keychain()? {
            Some(keychain) => match keychain.load() {
                Ok(Some(text)) => Some(text),
                _ => self.file.load()?,
            },
            None => self.file.load()?,
        };
        Ok(text.and_then(|t| serde_json::from_str(&t).ok()))
    }

    pub fn save(&self, key: &RouterKey) -> Result<()> {
        let text = serde_json::to_string(key)?;
        if let Some(keychain) = self.keychain()?
            && keychain.save(&text).is_ok()
        {
            return self.file.delete();
        }
        self.file.save(&text)
    }

    pub fn delete(&self) -> Result<()> {
        if let Some(keychain) = self.keychain()? {
            keychain.delete()?;
        }
        self.file.delete()
    }

    pub fn describe(&self) -> String {
        if self.keychain.is_some() && !self.file.0.exists() {
            "chaveiro do sistema".into()
        } else {
            format!("arquivo {}", self.file.0.display())
        }
    }
}

fn system_keychain(hub: &str) -> Option<Box<dyn Slot>> {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        Some(Box::new(KeychainSlot(format!("router-key:{hub}"))))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = hub;
        None
    }
}

pub fn vault(ctx: &Ctx) -> KeyVault {
    KeyVault::for_hub(&ctx.settings.paths, &ctx.settings.hub, ctx.settings.token_store)
}

/// The panel API behind nginx (`/admin/api` on the router host); ETUS_ROUTER_API_URL points elsewhere for tests.
pub fn panel_api(router_url: &str) -> String {
    match std::env::var("ETUS_ROUTER_API_URL").ok().map(|v| v.trim().to_string()) {
        Some(url) if !url.is_empty() => url.trim_end_matches('/').to_string(),
        _ => format!("{}/admin/api", router_url.trim_end_matches('/')),
    }
}

pub fn machine_name() -> String {
    let from_env = std::env::var("COMPUTERNAME").ok();
    let from_command = || {
        Command::new("hostname")
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
    };
    clean_name(from_env.or_else(from_command).as_deref().unwrap_or(""))
}

pub fn clean_name(raw: &str) -> String {
    let name: String = raw
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_NAME_CHARS)
        .collect();
    if name.is_empty() { "computador".into() } else { name }
}

/// Why the key could not come from the login session. `Unavailable` lets setup fall back to a key the person already has.
#[derive(Debug)]
pub enum Fetch {
    NeedsLogin(String),
    Unavailable(String),
    Failed(Error),
}

impl From<Error> for Fetch {
    fn from(error: Error) -> Self {
        Fetch::Failed(error)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Server {
    Hub,
    Panel,
}

fn error_code(body: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v.get("error").and_then(Value::as_str).map(str::to_string))
        .unwrap_or_default()
}

fn action_of(body: &str) -> Option<Action> {
    let value: Value = serde_json::from_str(body).ok()?;
    if value.get("status").and_then(Value::as_str) != Some(action::STATUS) {
        return None;
    }
    let action: Action = serde_json::from_value(value.get("action")?.clone()).ok()?;
    action::safe_url(&action.url).then_some(action)
}

const NOT_DEPLOYED: &str = "O servidor ainda não entrega a chave do AI Router pelo login. Use `etus router setup --chave <sk-...>` por enquanto.";

pub fn refusal(server: Server, status: u16, body: &str) -> Fetch {
    if let Some(action) = action_of(body) {
        return Fetch::Failed(Error::ActionRequired(Box::new(action)));
    }
    let code = error_code(body);
    let who = match server {
        Server::Hub => "O hub",
        Server::Panel => "O AI Router",
    };
    let failed = |error: Error| Fetch::Failed(error);
    match (server, status, code.as_str()) {
        (Server::Hub, 401, _) => Fetch::NeedsLogin("O hub não aceitou a sessão.".into()),
        (Server::Hub, 403, "agents_not_allowed") => failed(Error::Permission(
            "Sua conta não tem a permissão \"Conectar agentes externos\" na empresa ativa. Peça a um admin no hub.".into(),
        )),
        (Server::Hub, 403, "chat_only") | (_, 404, "not_found" | "") | (_, 405, _) => {
            Fetch::Unavailable(NOT_DEPLOYED.into())
        }
        (Server::Hub, 403, "no_active_company") => failed(Error::Permission(
            "Você não tem empresa ativa no hub. Escolha uma com `etus empresa usar <id>`.".into(),
        )),
        (Server::Panel, 401, _) => failed(Error::Auth(
            "O AI Router não aceitou o token do hub. Rode `etus login` e tente de novo.".into(),
        )),
        (Server::Panel, 409, "key_not_ready") => failed(Error::Permission(
            "Sua chave do AI Router ainda não foi criada. Ela aparece alguns minutos depois de você receber a permissão \"Usar a IA\". Tente de novo mais tarde ou fale com um admin."
                .into(),
        )),
        (Server::Panel, 403, "key_inactive") => failed(Error::Permission(
            "Sua chave do AI Router está desativada. Fale com um admin.".into(),
        )),
        (Server::Panel, 409, "too_many_devices") => failed(Error::Permission(
            "Você já tem o máximo de computadores conectados ao AI Router. Revogue os que não usa em Minha chave, no painel do roteador, e tente de novo."
                .into(),
        )),
        (_, 403, _) => failed(Error::Permission(format!(
            "{who} recusou o acesso ({}).",
            if code.is_empty() { "403" } else { &code }
        ))),
        (_, 429, _) => failed(Error::Network(format!(
            "{who} recebeu tentativas demais. Espere um minuto e tente de novo."
        ))),
        _ => failed(Error::Network(format!("{who} respondeu HTTP {status}."))),
    }
}

fn post(http: &Client, url: &str, bearer: &str, body: &Value) -> Result<(u16, String)> {
    let response = http
        .post(url)
        .bearer_auth(bearer)
        .header("accept", "application/json")
        .json(body)
        .send()?;
    Ok((response.status().as_u16(), response.text()?))
}

pub fn forward_token(http: &Client, hub: &str, tokens: &dyn TokenSource) -> std::result::Result<String, Fetch> {
    let url = format!("{}/api/v1/mcp/forward-tokens", hub.trim_end_matches('/'));
    let body = json!({ "appKey": PANEL_APP_KEY });
    let mut access = tokens.access_token(None).map_err(login_unavailable)?;
    let mut reply = post(http, &url, &access, &body)?;
    if reply.0 == 401 {
        access = tokens.access_token(Some(&access)).map_err(login_unavailable)?;
        reply = post(http, &url, &access, &body)?;
    }
    let (status, text) = reply;
    if status != 200 {
        return Err(refusal(Server::Hub, status, &text));
    }
    serde_json::from_str::<Value>(&text)
        .ok()
        .and_then(|v| v.get("token").and_then(Value::as_str).map(str::to_string))
        .filter(|t| !t.is_empty())
        .ok_or_else(|| Fetch::Failed(Error::Network("O hub respondeu sem o token de repasse.".into())))
}

fn login_unavailable(error: Error) -> Fetch {
    match error {
        Error::Auth(message) => Fetch::NeedsLogin(message),
        other => Fetch::Failed(other),
    }
}

pub struct Issued {
    pub key: String,
    pub device_id: String,
    pub device_name: String,
}

pub fn create_device_key(
    http: &Client,
    api: &str,
    forward: &str,
    name: &str,
    replaces: Option<&str>,
) -> std::result::Result<Issued, Fetch> {
    let (status, text) = post(
        http,
        &format!("{api}/device-keys"),
        forward,
        &json!({ "name": name, "replaces": replaces }),
    )?;
    if status != 201 && status != 200 {
        return Err(refusal(Server::Panel, status, &text));
    }
    let value: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    let key = value.get("key").and_then(Value::as_str).unwrap_or("");
    let device = value.get("device").cloned().unwrap_or(Value::Null);
    let device_id = device.get("id").and_then(Value::as_str).unwrap_or("");
    if !key.starts_with("sk-") || device_id.is_empty() {
        return Err(Fetch::Failed(Error::Network(
            "O AI Router respondeu sem a chave deste computador.".into(),
        )));
    }
    Ok(Issued {
        key: key.to_string(),
        device_id: device_id.to_string(),
        device_name: device.get("name").and_then(Value::as_str).unwrap_or(name).to_string(),
    })
}

/// Revokes this computer's key with the key itself, so logout works even after the hub session ended.
pub fn revoke_device_key(http: &Client, api: &str, key: &str) -> Result<()> {
    let status = http
        .delete(format!("{api}/device-keys/current"))
        .bearer_auth(key)
        .send()?
        .status()
        .as_u16();
    if (200..300).contains(&status) {
        Ok(())
    } else {
        Err(Error::Network(format!(
            "o AI Router respondeu HTTP {status} ao revogar a chave deste computador"
        )))
    }
}

pub fn obtain(ctx: &Ctx, router_url: &str, previous: Option<&RouterKey>) -> std::result::Result<RouterKey, Fetch> {
    let http = ctx.http()?;
    let manager = ctx.manager()?;
    let forward = forward_token(http, &ctx.settings.hub, &manager)?;
    let replaces = previous
        .filter(|p| p.origin == Origin::Login)
        .and_then(|p| p.device_id.as_deref());
    let issued = create_device_key(http, &panel_api(router_url), &forward, &machine_name(), replaces)?;
    Ok(RouterKey {
        key: issued.key,
        origin: Origin::Login,
        router_url: router_url.to_string(),
        device_id: Some(issued.device_id),
        device_name: Some(issued.device_name),
    })
}

pub fn print_key(ctx: &Ctx, force: bool) -> Result<()> {
    if std::io::stdout().is_terminal() && !force {
        return Err(Error::Usage(
            "A chave do AI Router é um segredo e a saída é um terminal. Ela é lida pelo Claude Code (apiKeyHelper); para imprimir mesmo assim, passe --force."
                .into(),
        ));
    }
    let stored = vault(ctx)
        .load()?
        .ok_or_else(|| Error::Auth("Nenhuma chave do AI Router neste computador. Rode `etus router setup`.".into()))?;
    println!("{}", stored.key);
    Ok(())
}

/// Logout: revokes this computer's key in the router (best effort) and erases it from the vault.
pub fn forget(ctx: &Ctx) -> Result<Option<RouterKey>> {
    let vault = vault(ctx);
    let Some(stored) = vault.load()? else {
        return Ok(None);
    };
    if stored.origin == Origin::Login
        && let Err(error) = revoke_device_key(ctx.http()?, &panel_api(&stored.router_url), &stored.key)
    {
        eprintln!("etus: aviso: {error}; revogue o computador em Minha chave, no painel do roteador.");
    }
    vault.delete()?;
    Ok(Some(stored))
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::testutil::temp_dir;

    fn sample(origin: Origin) -> RouterKey {
        RouterKey {
            key: "sk-etd-secretvalue".into(),
            origin,
            router_url: "https://router.etus.io".into(),
            device_id: Some("dev_1".into()),
            device_name: Some("mac-ana".into()),
        }
    }

    #[test]
    fn debug_never_shows_the_key() {
        assert!(!format!("{:?}", sample(Origin::Login)).contains("secret"));
    }

    #[cfg(unix)]
    #[test]
    fn file_vault_is_private_and_round_trips() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_dir("router-key");
        let vault = KeyVault::for_hub(&Paths::at(&dir), "https://hub.test", StoreKind::File);
        assert_eq!(vault.load().unwrap(), None);
        vault.save(&sample(Origin::Login)).unwrap();
        let mode = fs::metadata(&vault.file.0).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(vault.load().unwrap(), Some(sample(Origin::Login)));
        vault.delete().unwrap();
        assert_eq!(vault.load().unwrap(), None);
    }

    struct Fake(Arc<Mutex<Option<String>>>);

    impl Slot for Fake {
        fn load(&self) -> Result<Option<String>> {
            Ok(self.0.lock().unwrap().clone())
        }
        fn save(&self, text: &str) -> Result<()> {
            *self.0.lock().unwrap() = Some(text.to_string());
            Ok(())
        }
        fn delete(&self) -> Result<()> {
            *self.0.lock().unwrap() = None;
            Ok(())
        }
    }

    #[test]
    fn keychain_vault_keeps_no_file_and_needs_a_terminal_on_macos() {
        let dir = temp_dir("router-keychain");
        let state = Arc::new(Mutex::new(None));
        let vault = KeyVault {
            file: FileSlot(dir.join("router-key.json")),
            keychain: Some(Box::new(Fake(state.clone()))),
            keychain_allowed: true,
        };
        vault.save(&sample(Origin::Manual)).unwrap();
        assert!(!vault.file.0.exists());
        assert!(state.lock().unwrap().is_some());
        assert_eq!(vault.load().unwrap(), Some(sample(Origin::Manual)));
        assert_eq!(vault.describe(), "chaveiro do sistema");
        let locked = KeyVault {
            keychain_allowed: false,
            ..vault
        };
        assert_eq!(locked.load().unwrap_err().exit_code(), 3);
    }

    #[test]
    fn names_are_trimmed_and_bounded() {
        assert_eq!(clean_name("  mac-ana\n"), "mac-ana");
        assert_eq!(clean_name(""), "computador");
        assert_eq!(clean_name(&"x".repeat(300)).chars().count(), 100);
    }

    #[test]
    fn panel_api_sits_under_admin() {
        if std::env::var_os("ETUS_ROUTER_API_URL").is_none() {
            assert_eq!(panel_api("https://router.etus.io/"), "https://router.etus.io/admin/api");
        }
    }

    #[test]
    fn refusals_map_to_actions_and_fallbacks() {
        let body = json!({"error": "permission_required", "status": "action_required", "action": {"kind": "permission", "title": "Pedir acesso", "message": "Peça o acesso Usar a IA.", "url": "https://apps.etus.io/access-requests/new?app=router-panel&permission=ai.use"}}).to_string();
        match refusal(Server::Panel, 403, &body) {
            Fetch::Failed(Error::ActionRequired(action)) => assert_eq!(action.kind, "permission"),
            other => panic!("unexpected {other:?}"),
        }
        let unsafe_link = body.replace("https://apps.etus.io", "javascript:alert(1)//");
        assert!(matches!(
            refusal(Server::Panel, 403, &unsafe_link),
            Fetch::Failed(Error::Permission(_))
        ));
        assert!(matches!(
            refusal(Server::Hub, 403, r#"{"error":"chat_only"}"#),
            Fetch::Unavailable(_)
        ));
        assert!(matches!(
            refusal(Server::Panel, 404, r#"{"error":"not_found"}"#),
            Fetch::Unavailable(_)
        ));
        assert!(matches!(
            refusal(Server::Panel, 409, r#"{"error":"key_not_ready"}"#),
            Fetch::Failed(Error::Permission(_))
        ));
        assert!(matches!(refusal(Server::Hub, 401, "{}"), Fetch::NeedsLogin(_)));
        assert!(matches!(refusal(Server::Panel, 404, ""), Fetch::Unavailable(_)));
        assert!(matches!(
            refusal(Server::Panel, 502, ""),
            Fetch::Failed(Error::Network(_))
        ));
    }
}
