use std::fmt;
use std::fs;
use std::io::IsTerminal;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::config::{Paths, write_private};
use crate::error::{Error, Result};

#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct Tokens {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    pub expires_at: u64,
    #[serde(default)]
    pub scope: Option<String>,
    pub client_id: String,
    pub token_endpoint: String,
    #[serde(default)]
    pub revocation_endpoint: Option<String>,
    pub resource: String,
}

impl fmt::Debug for Tokens {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Tokens")
            .field("access_token", &"<redacted>")
            .field("refresh_token", &self.refresh_token.as_ref().map(|_| "<redacted>"))
            .field("expires_at", &self.expires_at)
            .field("scope", &self.scope)
            .field("client_id", &self.client_id)
            .finish()
    }
}

pub trait SecretStore: Send + Sync {
    fn load(&self) -> Result<Option<Tokens>>;
    fn save(&self, tokens: &Tokens) -> Result<()>;
    fn delete(&self) -> Result<()>;
    fn describe(&self) -> String;
}

pub struct FileStore {
    pub path: PathBuf,
}

impl SecretStore for FileStore {
    fn load(&self) -> Result<Option<Tokens>> {
        match fs::read_to_string(&self.path) {
            Ok(text) => Ok(serde_json::from_str(&text).ok()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn save(&self, tokens: &Tokens) -> Result<()> {
        write_private(&self.path, serde_json::to_string(tokens)?.as_bytes())
    }

    fn delete(&self) -> Result<()> {
        match fs::remove_file(&self.path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        }
    }

    fn describe(&self) -> String {
        format!("arquivo {}", self.path.display())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum StoreKind {
    #[serde(alias = "arquivo")]
    #[value(name = "arquivo", alias = "file")]
    File,
    #[serde(alias = "chaveiro")]
    #[value(name = "chaveiro", alias = "keychain")]
    Keychain,
}

impl StoreKind {
    pub fn parse(raw: &str) -> Option<StoreKind> {
        match raw.trim().to_lowercase().as_str() {
            "file" | "arquivo" => Some(StoreKind::File),
            "keychain" | "chaveiro" => Some(StoreKind::Keychain),
            _ => None,
        }
    }

    pub fn platform_default() -> StoreKind {
        if cfg!(target_os = "windows") {
            StoreKind::Keychain
        } else {
            StoreKind::File
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            StoreKind::File => "arquivo",
            StoreKind::Keychain => "chaveiro",
        }
    }
}

pub fn select_kind(env: Option<&str>, configured: Option<StoreKind>) -> Result<StoreKind> {
    match env.map(str::trim).filter(|v| !v.is_empty()) {
        Some(raw) => StoreKind::parse(raw).ok_or_else(|| {
            Error::Usage(format!(
                "ETUS_TOKEN_STORE inválido: {raw}. Use file (arquivo) ou keychain (chaveiro)."
            ))
        }),
        None => Ok(configured.unwrap_or_else(StoreKind::platform_default)),
    }
}

pub fn keychain_supported() -> bool {
    cfg!(any(target_os = "macos", target_os = "windows"))
}

pub fn keychain_allowed() -> bool {
    !cfg!(target_os = "macos") || (std::io::stdin().is_terminal() && std::io::stdout().is_terminal())
}

pub fn keychain_needs_terminal() -> Error {
    Error::Auth(
        "O token está no chaveiro do macOS, que pode abrir uma janela pedindo permissão, e este comando não roda num terminal. Para scripts e agentes, guarde o token em arquivo: rode `etus config armazenamento arquivo` num terminal (ou use ETUS_TOKEN_STORE=file).".into(),
    )
}

const LEGACY_UNATTENDED: &str = "Você não está conectado nesta máquina. Rode `etus login` num terminal. Se você entrou com uma versão anterior, o token ficou no chaveiro do macOS: rodar qualquer comando etus num terminal traz ele para o arquivo.";

const LEGACY_UNREADABLE: &str =
    "Não consegui ler o token que a versão anterior guardou no chaveiro. Rode `etus login` de novo.";

#[cfg(any(target_os = "macos", target_os = "windows"))]
const KEYRING_SERVICE: &str = "etus-cli";

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub struct KeychainStore {
    pub account: String,
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn keychain_error(error: keyring::Error) -> Error {
    Error::Other(format!("Falha no chaveiro do sistema: {error}"))
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl KeychainStore {
    fn entry(&self) -> Result<keyring::Entry> {
        keyring::Entry::new(KEYRING_SERVICE, &self.account).map_err(keychain_error)
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl SecretStore for KeychainStore {
    fn load(&self) -> Result<Option<Tokens>> {
        match self.entry()?.get_password() {
            Ok(text) => Ok(serde_json::from_str(&text).ok()),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(keychain_error(error)),
        }
    }

    fn save(&self, tokens: &Tokens) -> Result<()> {
        self.entry()?
            .set_password(&serde_json::to_string(tokens)?)
            .map_err(keychain_error)
    }

    fn delete(&self) -> Result<()> {
        match self.entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(keychain_error(error)),
        }
    }

    fn describe(&self) -> String {
        "chaveiro do sistema".into()
    }
}

pub struct KeychainFirst {
    pub keychain: Box<dyn SecretStore>,
    pub file: FileStore,
    pub keychain_allowed: bool,
}

impl KeychainFirst {
    fn guard(&self) -> Result<()> {
        if self.keychain_allowed {
            Ok(())
        } else {
            Err(keychain_needs_terminal())
        }
    }
}

impl SecretStore for KeychainFirst {
    fn load(&self) -> Result<Option<Tokens>> {
        self.guard()?;
        match self.keychain.load() {
            Ok(Some(tokens)) => Ok(Some(tokens)),
            _ => self.file.load(),
        }
    }

    fn save(&self, tokens: &Tokens) -> Result<()> {
        self.guard()?;
        match self.keychain.save(tokens) {
            Ok(()) => self.file.delete(),
            Err(_) => self.file.save(tokens),
        }
    }

    fn delete(&self) -> Result<()> {
        self.guard()?;
        self.keychain.delete()?;
        self.file.delete()
    }

    fn describe(&self) -> String {
        if self.file.path.exists() {
            self.file.describe()
        } else {
            self.keychain.describe()
        }
    }
}

pub struct MigratingFileStore {
    pub file: FileStore,
    pub legacy: Option<Box<dyn SecretStore>>,
    pub marker: PathBuf,
    pub keychain_allowed: bool,
}

impl MigratingFileStore {
    fn pending(&self) -> Option<&dyn SecretStore> {
        self.legacy.as_deref().filter(|_| !self.marker.exists())
    }

    fn migrate(&self, legacy: &dyn SecretStore) -> Result<Option<Tokens>> {
        if !self.keychain_allowed {
            return Err(Error::Auth(LEGACY_UNATTENDED.into()));
        }
        let read = legacy.load();
        write_private(&self.marker, b"")?;
        match read {
            Ok(Some(tokens)) => {
                self.file.save(&tokens)?;
                if let Err(error) = legacy.delete() {
                    eprintln!(
                        "etus: aviso: o token foi para o arquivo, mas não consegui apagá-lo do chaveiro: {error}"
                    );
                }
                Ok(Some(tokens))
            }
            Ok(None) => Ok(None),
            Err(_) => Err(Error::Auth(LEGACY_UNREADABLE.into())),
        }
    }

    fn retire_legacy(&self) -> Result<()> {
        if let Some(legacy) = self.pending()
            && self.keychain_allowed
        {
            let _ = legacy.delete();
            write_private(&self.marker, b"")?;
        }
        Ok(())
    }
}

impl SecretStore for MigratingFileStore {
    fn load(&self) -> Result<Option<Tokens>> {
        if let Some(tokens) = self.file.load()? {
            return Ok(Some(tokens));
        }
        match self.pending() {
            Some(legacy) => self.migrate(legacy),
            None => Ok(None),
        }
    }

    fn save(&self, tokens: &Tokens) -> Result<()> {
        self.file.save(tokens)?;
        self.retire_legacy()
    }

    fn delete(&self) -> Result<()> {
        self.file.delete()?;
        self.retire_legacy()
    }

    fn describe(&self) -> String {
        self.file.describe()
    }
}

fn system_keychain(hub: &str) -> Option<Box<dyn SecretStore>> {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        Some(Box::new(KeychainStore {
            account: hub.to_string(),
        }))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = hub;
        None
    }
}

pub fn build_store(
    paths: &Paths,
    hub: &str,
    kind: StoreKind,
    keychain: Option<Box<dyn SecretStore>>,
    keychain_allowed: bool,
) -> Box<dyn SecretStore> {
    let file = FileStore {
        path: paths.tokens_file(hub),
    };
    match (kind, keychain) {
        (StoreKind::Keychain, Some(keychain)) => Box::new(KeychainFirst {
            keychain,
            file,
            keychain_allowed,
        }),
        (_, legacy) => Box::new(MigratingFileStore {
            file,
            legacy,
            marker: paths.keychain_marker(hub),
            keychain_allowed,
        }),
    }
}

pub fn store_for(paths: &Paths, hub: &str, kind: StoreKind) -> Box<dyn SecretStore> {
    build_store(paths, hub, kind, system_keychain(hub), keychain_allowed())
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_never_shows_tokens() {
        let tokens = Tokens {
            access_token: "secret-access".into(),
            refresh_token: Some("secret-refresh".into()),
            expires_at: 1,
            scope: None,
            client_id: "c".into(),
            token_endpoint: "t".into(),
            revocation_endpoint: None,
            resource: "r".into(),
        };
        let text = format!("{tokens:?}");
        assert!(!text.contains("secret"));
    }

    #[cfg(unix)]
    #[test]
    fn file_store_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = crate::testutil::temp_dir("store");
        let store = FileStore {
            path: dir.join("tokens.json"),
        };
        let tokens = sample("a");
        store.save(&tokens).unwrap();
        let mode = fs::metadata(&store.path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(store.load().unwrap(), Some(tokens));
        store.delete().unwrap();
        assert_eq!(store.load().unwrap(), None);
    }

    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Default)]
    struct FakeState {
        tokens: Mutex<Option<Tokens>>,
        unreadable: bool,
        reads: AtomicUsize,
        writes: AtomicUsize,
        deletes: AtomicUsize,
    }

    impl FakeState {
        fn touched(&self) -> usize {
            self.reads.load(Ordering::SeqCst) + self.writes.load(Ordering::SeqCst) + self.deletes.load(Ordering::SeqCst)
        }
    }

    struct FakeKeychain(Arc<FakeState>);

    impl SecretStore for FakeKeychain {
        fn load(&self) -> Result<Option<Tokens>> {
            self.0.reads.fetch_add(1, Ordering::SeqCst);
            if self.0.unreadable {
                return Err(Error::Other("denied".into()));
            }
            Ok(self.0.tokens.lock().unwrap().clone())
        }

        fn save(&self, tokens: &Tokens) -> Result<()> {
            self.0.writes.fetch_add(1, Ordering::SeqCst);
            *self.0.tokens.lock().unwrap() = Some(tokens.clone());
            Ok(())
        }

        fn delete(&self) -> Result<()> {
            self.0.deletes.fetch_add(1, Ordering::SeqCst);
            *self.0.tokens.lock().unwrap() = None;
            Ok(())
        }

        fn describe(&self) -> String {
            "fake keychain".into()
        }
    }

    fn sample(access: &str) -> Tokens {
        Tokens {
            access_token: access.into(),
            refresh_token: Some("r".into()),
            expires_at: 1,
            scope: None,
            client_id: "c".into(),
            token_endpoint: "t".into(),
            revocation_endpoint: None,
            resource: "r".into(),
        }
    }

    const HUB: &str = "https://hub.test";

    fn fake(tokens: Option<Tokens>, unreadable: bool) -> Arc<FakeState> {
        Arc::new(FakeState {
            tokens: Mutex::new(tokens),
            unreadable,
            ..FakeState::default()
        })
    }

    fn store(paths: &Paths, kind: StoreKind, keychain: &Arc<FakeState>, allowed: bool) -> Box<dyn SecretStore> {
        build_store(
            paths,
            HUB,
            kind,
            Some(Box::new(FakeKeychain(keychain.clone()))),
            allowed,
        )
    }

    #[test]
    fn env_wins_over_config_and_defaults_follow_the_platform() {
        assert_eq!(select_kind(None, None).unwrap(), StoreKind::platform_default());
        assert_eq!(
            select_kind(None, Some(StoreKind::Keychain)).unwrap(),
            StoreKind::Keychain
        );
        assert_eq!(
            select_kind(Some("file"), Some(StoreKind::Keychain)).unwrap(),
            StoreKind::File
        );
        assert_eq!(select_kind(Some("Chaveiro"), None).unwrap(), StoreKind::Keychain);
        assert_eq!(select_kind(Some("  "), Some(StoreKind::File)).unwrap(), StoreKind::File);
        assert_eq!(select_kind(Some("vault"), None).unwrap_err().exit_code(), 2);
        if cfg!(any(target_os = "macos", target_os = "linux")) {
            assert_eq!(StoreKind::platform_default(), StoreKind::File);
        }
    }

    #[test]
    fn config_accepts_both_languages() {
        let kind: StoreKind = serde_json::from_str("\"chaveiro\"").unwrap();
        assert_eq!(kind, StoreKind::Keychain);
        assert_eq!(serde_json::to_string(&StoreKind::File).unwrap(), "\"file\"");
    }

    #[test]
    fn migrates_once_from_the_keychain_to_the_file() {
        let dir = crate::testutil::temp_dir("migrate");
        let paths = Paths::at(&dir);
        let keychain = fake(Some(sample("old")), false);
        let first = store(&paths, StoreKind::File, &keychain, true);
        assert_eq!(first.load().unwrap(), Some(sample("old")));
        assert_eq!(keychain.reads.load(Ordering::SeqCst), 1);
        assert_eq!(keychain.deletes.load(Ordering::SeqCst), 1);
        assert!(paths.tokens_file(HUB).exists());
        assert!(paths.keychain_marker(HUB).exists());
        let again = store(&paths, StoreKind::File, &keychain, false);
        assert_eq!(again.load().unwrap(), Some(sample("old")));
        again.delete().unwrap();
        assert_eq!(again.load().unwrap(), None);
        assert_eq!(keychain.touched(), 2);
    }

    #[test]
    fn unattended_migration_asks_for_login_without_touching_the_keychain() {
        let dir = crate::testutil::temp_dir("unattended");
        let paths = Paths::at(&dir);
        let keychain = fake(Some(sample("old")), false);
        let error = store(&paths, StoreKind::File, &keychain, false).load().unwrap_err();
        assert_eq!(error.exit_code(), 3);
        assert!(error.message().contains("etus login"));
        assert_eq!(keychain.touched(), 0);
        assert!(!paths.keychain_marker(HUB).exists());
    }

    #[test]
    fn unreadable_keychain_asks_for_login_once() {
        let dir = crate::testutil::temp_dir("unreadable");
        let paths = Paths::at(&dir);
        let keychain = fake(Some(sample("old")), true);
        let s = store(&paths, StoreKind::File, &keychain, true);
        assert_eq!(s.load().unwrap_err().exit_code(), 3);
        assert_eq!(s.load().unwrap(), None);
        assert_eq!(keychain.reads.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn empty_keychain_is_marked_and_login_retires_leftovers() {
        let dir = crate::testutil::temp_dir("empty");
        let paths = Paths::at(&dir);
        let keychain = fake(None, false);
        assert_eq!(store(&paths, StoreKind::File, &keychain, true).load().unwrap(), None);
        assert!(paths.keychain_marker(HUB).exists());

        let other = crate::testutil::temp_dir("leftover");
        let paths = Paths::at(&other);
        let keychain = fake(Some(sample("stale")), false);
        store(&paths, StoreKind::File, &keychain, true)
            .save(&sample("new"))
            .unwrap();
        assert_eq!(*keychain.tokens.lock().unwrap(), None);
        assert_eq!(keychain.reads.load(Ordering::SeqCst), 0);
        assert_eq!(
            store(&paths, StoreKind::File, &keychain, false).load().unwrap(),
            Some(sample("new"))
        );
    }

    #[test]
    fn file_store_without_keychain_never_migrates() {
        let dir = crate::testutil::temp_dir("plain");
        let paths = Paths::at(&dir);
        let s = build_store(&paths, HUB, StoreKind::Keychain, None, false);
        assert_eq!(s.load().unwrap(), None);
        s.save(&sample("a")).unwrap();
        assert_eq!(s.load().unwrap(), Some(sample("a")));
        assert!(s.describe().starts_with("arquivo "));
    }

    #[test]
    fn keychain_store_fails_fast_when_unattended() {
        let dir = crate::testutil::temp_dir("failfast");
        let paths = Paths::at(&dir);
        let keychain = fake(Some(sample("k")), false);
        let s = store(&paths, StoreKind::Keychain, &keychain, false);
        assert_eq!(s.load().unwrap_err().exit_code(), 3);
        assert_eq!(s.save(&sample("x")).unwrap_err().exit_code(), 3);
        assert_eq!(s.delete().unwrap_err().exit_code(), 3);
        assert_eq!(keychain.touched(), 0);
    }

    #[test]
    fn keychain_store_reads_the_keychain_when_attended() {
        let dir = crate::testutil::temp_dir("attended");
        let paths = Paths::at(&dir);
        let keychain = fake(None, false);
        FileStore {
            path: paths.tokens_file(HUB),
        }
        .save(&sample("file"))
        .unwrap();
        let s = store(&paths, StoreKind::Keychain, &keychain, true);
        assert_eq!(s.load().unwrap(), Some(sample("file")));
        s.save(&sample("k")).unwrap();
        assert!(!paths.tokens_file(HUB).exists());
        assert_eq!(s.load().unwrap(), Some(sample("k")));
        assert_eq!(s.describe(), "fake keychain");
    }
}

#[cfg(all(test, any(target_os = "macos", target_os = "windows")))]
mod keychain_tests {
    use super::*;

    #[test]
    #[ignore = "touches the real system keychain"]
    fn keychain_round_trip() {
        let store = KeychainStore {
            account: "https://etus-cli-selftest.invalid".into(),
        };
        let tokens = sample("a");
        store.save(&tokens).unwrap();
        assert_eq!(store.load().unwrap(), Some(tokens));
        store.delete().unwrap();
        assert_eq!(store.load().unwrap(), None);
    }

    fn sample(access: &str) -> Tokens {
        Tokens {
            access_token: access.into(),
            refresh_token: Some("r".into()),
            expires_at: 1,
            scope: None,
            client_id: "c".into(),
            token_endpoint: "t".into(),
            revocation_endpoint: None,
            resource: "r".into(),
        }
    }
}
