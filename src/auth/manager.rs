use std::fs::{self, File, OpenOptions};
use std::path::PathBuf;

use crate::auth::store::{SecretStore, Tokens};
use crate::catalog::now_secs;
use crate::error::{Error, Result};

pub const EXPIRY_SKEW_SECS: u64 = 60;

pub trait Refresher {
    fn refresh(&self, tokens: &Tokens) -> Result<Tokens>;
}

pub struct TokenManager<R: Refresher> {
    pub store: Box<dyn SecretStore>,
    pub lock_path: PathBuf,
    pub refresher: R,
}

fn usable(tokens: &Tokens, rejected: Option<&str>, now: u64) -> bool {
    rejected != Some(tokens.access_token.as_str()) && tokens.expires_at > now + EXPIRY_SKEW_SECS
}

impl<R: Refresher> TokenManager<R> {
    pub fn current(&self) -> Result<Tokens> {
        self.store.load()?.ok_or_else(Error::not_logged_in)
    }

    pub fn access_token(&self, rejected: Option<&str>) -> Result<String> {
        let current = self.current()?;
        if usable(&current, rejected, now_secs()) {
            return Ok(current.access_token);
        }
        let _guard = self.lock()?;
        let latest = self.current()?;
        if usable(&latest, rejected, now_secs()) {
            return Ok(latest.access_token);
        }
        if latest.refresh_token.is_none() {
            return Err(Error::Auth("A sessão expirou. Rode `etus login`.".into()));
        }
        let refreshed = match self.refresher.refresh(&latest) {
            Ok(tokens) => tokens,
            Err(Error::Auth(message)) => {
                self.store.delete()?;
                return Err(Error::Auth(message));
            }
            Err(other) => return Err(other),
        };
        self.store.save(&refreshed)?;
        Ok(refreshed.access_token)
    }

    fn lock(&self) -> Result<File> {
        if let Some(dir) = self.lock_path.parent() {
            fs::create_dir_all(dir)?;
        }
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&self.lock_path)?;
        file.lock()?;
        Ok(file)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::store::FileStore;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    struct Counting {
        calls: Arc<AtomicUsize>,
        fail: bool,
    }

    impl Refresher for Counting {
        fn refresh(&self, tokens: &Tokens) -> Result<Tokens> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
            std::thread::sleep(Duration::from_millis(150));
            if self.fail {
                return Err(Error::Auth("revoked".into()));
            }
            Ok(Tokens {
                access_token: format!("access-{n}"),
                refresh_token: Some(format!("refresh-{n}")),
                expires_at: now_secs() + 3600,
                ..tokens.clone()
            })
        }
    }

    fn expired() -> Tokens {
        Tokens {
            access_token: "access-0".into(),
            refresh_token: Some("refresh-0".into()),
            expires_at: now_secs(),
            scope: Some("mcp".into()),
            client_id: "c".into(),
            token_endpoint: "t".into(),
            revocation_endpoint: None,
            resource: "r".into(),
        }
    }

    fn manager(dir: &std::path::Path, calls: Arc<AtomicUsize>, fail: bool) -> TokenManager<Counting> {
        TokenManager {
            store: Box::new(FileStore {
                path: dir.join("tokens.json"),
            }),
            lock_path: dir.join("auth.lock"),
            refresher: Counting { calls, fail },
        }
    }

    #[test]
    fn concurrent_processes_refresh_once() {
        let dir = crate::testutil::temp_dir("lock");
        FileStore {
            path: dir.join("tokens.json"),
        }
        .save(&expired())
        .unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let dir = dir.clone();
                let calls = calls.clone();
                std::thread::spawn(move || manager(&dir, calls, false).access_token(None).unwrap())
            })
            .collect();
        let tokens: Vec<String> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(tokens.iter().all(|t| t == "access-1"));
        let saved = FileStore {
            path: dir.join("tokens.json"),
        }
        .load()
        .unwrap()
        .unwrap();
        assert_eq!(saved.refresh_token.as_deref(), Some("refresh-1"));
    }

    #[test]
    fn rejected_token_forces_one_refresh() {
        let dir = crate::testutil::temp_dir("reject");
        let mut fresh = expired();
        fresh.expires_at = now_secs() + 3600;
        FileStore {
            path: dir.join("tokens.json"),
        }
        .save(&fresh)
        .unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let m = manager(&dir, calls.clone(), false);
        assert_eq!(m.access_token(None).unwrap(), "access-0");
        assert_eq!(m.access_token(Some("access-0")).unwrap(), "access-1");
        assert_eq!(m.access_token(Some("access-0")).unwrap(), "access-1");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn failed_refresh_logs_out() {
        let dir = crate::testutil::temp_dir("fail");
        FileStore {
            path: dir.join("tokens.json"),
        }
        .save(&expired())
        .unwrap();
        let m = manager(&dir, Arc::new(AtomicUsize::new(0)), true);
        assert_eq!(m.access_token(None).unwrap_err().exit_code(), 3);
        assert_eq!(m.access_token(None).unwrap_err(), Error::not_logged_in());
    }
}
