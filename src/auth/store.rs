use std::fmt;
use std::fs;
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

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub struct KeyringStore {
    pub account: String,
    pub fallback: FileStore,
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
const KEYRING_SERVICE: &str = "etus-cli";

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl KeyringStore {
    fn entry(&self) -> std::result::Result<keyring::Entry, keyring::Error> {
        keyring::Entry::new(KEYRING_SERVICE, &self.account)
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl SecretStore for KeyringStore {
    fn load(&self) -> Result<Option<Tokens>> {
        match self.entry().and_then(|e| e.get_password()) {
            Ok(text) => Ok(serde_json::from_str(&text).ok()),
            Err(_) => self.fallback.load(),
        }
    }

    fn save(&self, tokens: &Tokens) -> Result<()> {
        let text = serde_json::to_string(tokens)?;
        match self.entry().and_then(|e| e.set_password(&text)) {
            Ok(()) => self.fallback.delete(),
            Err(_) => self.fallback.save(tokens),
        }
    }

    fn delete(&self) -> Result<()> {
        if let Ok(entry) = self.entry() {
            match entry.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => {}
                Err(e) => return Err(Error::Other(format!("Não foi possível apagar do chaveiro: {e}"))),
            }
        }
        self.fallback.delete()
    }

    fn describe(&self) -> String {
        if self.fallback.path.exists() {
            self.fallback.describe()
        } else {
            "chaveiro do sistema".into()
        }
    }
}

pub fn store_for(paths: &Paths, hub: &str) -> Box<dyn SecretStore> {
    let file = FileStore {
        path: paths.tokens_file(hub),
    };
    let forced_file = std::env::var("ETUS_TOKEN_STORE").is_ok_and(|v| v.eq_ignore_ascii_case("file"));
    if forced_file {
        return Box::new(file);
    }
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        Box::new(KeyringStore {
            account: hub.to_string(),
            fallback: file,
        })
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Box::new(file)
    }
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
        let tokens = Tokens {
            access_token: "a".into(),
            refresh_token: None,
            expires_at: 1,
            scope: None,
            client_id: "c".into(),
            token_endpoint: "t".into(),
            revocation_endpoint: None,
            resource: "r".into(),
        };
        store.save(&tokens).unwrap();
        let mode = fs::metadata(&store.path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(store.load().unwrap(), Some(tokens));
        store.delete().unwrap();
        assert_eq!(store.load().unwrap(), None);
    }
}

#[cfg(all(test, target_os = "macos"))]
mod keychain_tests {
    use super::*;

    #[test]
    #[ignore = "touches the real macOS keychain"]
    fn keychain_round_trip() {
        let dir = crate::testutil::temp_dir("keychain");
        let store = KeyringStore {
            account: "https://etus-cli-selftest.invalid".into(),
            fallback: FileStore {
                path: dir.join("tokens.json"),
            },
        };
        let tokens = Tokens {
            access_token: "a".into(),
            refresh_token: Some("r".into()),
            expires_at: 1,
            scope: None,
            client_id: "c".into(),
            token_endpoint: "t".into(),
            revocation_endpoint: None,
            resource: "r".into(),
        };
        store.save(&tokens).unwrap();
        assert!(!store.fallback.path.exists());
        assert_eq!(store.load().unwrap(), Some(tokens));
        store.delete().unwrap();
        assert_eq!(store.load().unwrap(), None);
    }
}
