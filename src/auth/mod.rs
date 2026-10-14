pub mod manager;
pub mod oauth;
pub mod store;

pub use manager::{Refresher, TokenManager};
pub use store::{SecretStore, Tokens, store_for};

use base64::Engine;
use serde_json::Value;

pub fn jwt_claims(token: &str) -> Option<Value> {
    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_claims_without_verifying() {
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(br#"{"sub":"u1","scope":"mcp"}"#);
        let token = format!("h.{payload}.s");
        let claims = jwt_claims(&token).unwrap();
        assert_eq!(claims["sub"], "u1");
        assert!(jwt_claims("not-a-jwt").is_none());
    }
}
