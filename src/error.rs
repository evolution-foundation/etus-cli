use std::fmt;

use crate::action::Action;

pub const EXIT_OK: i32 = 0;
pub const EXIT_TOOL_ERROR: i32 = 1;
pub const EXIT_USAGE: i32 = 2;
pub const EXIT_AUTH_REQUIRED: i32 = 3;
pub const EXIT_PERMISSION_DENIED: i32 = 4;
pub const EXIT_ACTION_REQUIRED: i32 = 5;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Usage(String),
    Auth(String),
    Permission(String),
    Tool(String),
    Network(String),
    Other(String),
    ActionRequired(Box<Action>),
    Exit(i32),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn exit_code(&self) -> i32 {
        match self {
            Error::Usage(_) => EXIT_USAGE,
            Error::Auth(_) => EXIT_AUTH_REQUIRED,
            Error::Permission(_) => EXIT_PERMISSION_DENIED,
            Error::Tool(_) | Error::Network(_) | Error::Other(_) => EXIT_TOOL_ERROR,
            Error::ActionRequired(_) => EXIT_ACTION_REQUIRED,
            Error::Exit(code) => *code,
        }
    }

    pub fn message(&self) -> &str {
        match self {
            Error::Usage(m)
            | Error::Auth(m)
            | Error::Permission(m)
            | Error::Tool(m)
            | Error::Network(m)
            | Error::Other(m) => m,
            Error::ActionRequired(action) => &action.message,
            Error::Exit(_) => "",
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Error::Usage(_) => "usage",
            Error::Auth(_) => "auth_required",
            Error::Permission(_) => "permission_denied",
            Error::Tool(_) => "tool_error",
            Error::Network(_) => "network",
            Error::ActionRequired(_) => "action_required",
            Error::Other(_) | Error::Exit(_) => "error",
        }
    }

    pub fn not_logged_in() -> Error {
        Error::Auth("Você não está conectado. Rode `etus login`.".into())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Error::Other(format!("Erro de arquivo ou sistema: {error}"))
    }
}

impl From<serde_json::Error> for Error {
    fn from(error: serde_json::Error) -> Self {
        Error::Other(format!("JSON inválido: {error}"))
    }
}

impl From<reqwest::Error> for Error {
    fn from(error: reqwest::Error) -> Self {
        let error = error.without_url();
        if error.is_timeout() {
            Error::Network("O hub não respondeu a tempo.".into())
        } else if error.is_connect() {
            Error::Network("Não foi possível conectar ao hub. Confira a rede e o endereço (--hub).".into())
        } else {
            Error::Network(format!("Falha de rede: {error}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes_follow_the_contract() {
        assert_eq!(Error::Usage("x".into()).exit_code(), 2);
        assert_eq!(Error::Auth("x".into()).exit_code(), 3);
        assert_eq!(Error::Permission("x".into()).exit_code(), 4);
        assert_eq!(Error::Tool("x".into()).exit_code(), 1);
        assert_eq!(Error::Network("x".into()).exit_code(), 1);
        assert_eq!(Error::Other("x".into()).exit_code(), 1);
        assert_eq!(Error::not_logged_in().exit_code(), EXIT_AUTH_REQUIRED);
        assert_eq!(Error::Exit(0).exit_code(), EXIT_OK);
        let action = Action {
            kind: "consent".into(),
            title: "t".into(),
            message: "m".into(),
            url: "https://apps.etus.io/x".into(),
            missing: Vec::new(),
            expires_at: None,
        };
        let required = Error::ActionRequired(Box::new(action));
        assert_eq!(required.exit_code(), 5);
        assert_eq!(required.kind(), "action_required");
    }
}
