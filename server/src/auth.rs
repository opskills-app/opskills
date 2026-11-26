//! Sign-in for the built-in admin user, and cookie sessions.

use std::path::Path;

use axum::http::HeaderMap;
use sha2::{Digest, Sha256};

use crate::firststart::random_bytes;

pub const COOKIE: &str = "opskills_session";
pub const ADMIN_USER: &str = "admin";
/// Header every state-changing request must carry. Browsers can't add it to a
/// cross-site request without a CORS preflight, which this server never grants.
pub const CSRF_HEADER: &str = "x-opskills-request";

/// Check a password against the admin hash file written on first start.
pub fn verify_admin_password(hash_file: &Path, password: &str) -> bool {
    let Ok(record) = std::fs::read_to_string(hash_file) else { return false };
    // argon2id$v=19$m=19456,t=2,p=1$<salt hex>$<hash hex>
    let parts: Vec<&str> = record.trim().split('$').collect();
    if parts.len() != 5 || parts[0] != "argon2id" || parts[1] != "v=19" || parts[2] != "m=19456,t=2,p=1" {
        return false;
    }
    let (Ok(salt), Ok(expected)) = (hex::decode(parts[3]), hex::decode(parts[4])) else { return false };
    let mut out = vec![0u8; expected.len()];
    if argon2::Argon2::default().hash_password_into(password.as_bytes(), &salt, &mut out).is_err() {
        return false;
    }
    constant_time_eq(&out, &expected)
}

pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// API tokens look like `osk_<12 hex id>_<64 hex secret>`, so they're easy to spot in
/// logs and secret scanners. Returns (id, secret, the whole token).
pub fn new_api_token() -> (String, String, String) {
    let id = hex::encode(random_bytes::<6>());
    let secret = hex::encode(random_bytes::<32>());
    let whole = format!("{API_TOKEN_PREFIX}{id}_{secret}");
    (id, secret, whole)
}

pub const API_TOKEN_PREFIX: &str = "osk_";

/// Split a token into its id and secret, if it has the right shape.
pub fn parse_api_token(token: &str) -> Option<(&str, &str)> {
    let (id, secret) = token.strip_prefix(API_TOKEN_PREFIX)?.split_once('_')?;
    let hex = |s: &str, n: usize| s.len() == n && s.chars().all(|c| c.is_ascii_hexdigit());
    (hex(id, 12) && hex(secret, 64)).then_some((id, secret))
}

/// The token in an `Authorization: Bearer ...` header.
pub fn bearer_token(headers: &HeaderMap) -> Option<String> {
    let value = headers.get(axum::http::header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    scheme.eq_ignore_ascii_case("bearer").then(|| token.trim().to_string())
}

pub fn new_token() -> String {
    hex::encode(random_bytes::<32>())
}

/// Sessions are stored by the SHA-256 of their token, never the token itself.
pub fn token_hash(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

pub fn cookie_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(axum::http::header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(k, _)| *k == COOKIE)
        .map(|(_, v)| v.to_string())
        .filter(|v| v.len() == 64 && v.chars().all(|c| c.is_ascii_hexdigit()))
}

pub fn session_cookie(token: &str, max_age_secs: u64, secure: bool) -> String {
    format!(
        "{COOKIE}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age={max_age_secs}{}",
        if secure { "; Secure" } else { "" }
    )
}

pub fn clear_cookie(secure: bool) -> String {
    session_cookie("", 0, secure)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cookie_parsing() {
        let token = new_token();
        let mut h = HeaderMap::new();
        h.insert("cookie", format!("a=b; {COOKIE}={token}; c=d").parse().unwrap());
        assert_eq!(cookie_token(&h), Some(token));
        let mut bad = HeaderMap::new();
        bad.insert("cookie", format!("{COOKIE}=not-hex").parse().unwrap());
        assert_eq!(cookie_token(&bad), None);
    }

    #[test]
    fn api_tokens_parse() {
        let (id, secret, whole) = new_api_token();
        assert_eq!(parse_api_token(&whole), Some((id.as_str(), secret.as_str())));
        for bad in ["", "osk_", "osk_abc_def", "xyz_0123456789ab_0000", &whole.replace("osk_", "osx_"), &format!("{whole}0")] {
            assert_eq!(parse_api_token(bad), None, "{bad}");
        }
        let mut h = HeaderMap::new();
        h.insert("authorization", format!("bearer {whole}").parse().unwrap());
        assert_eq!(bearer_token(&h), Some(whole));
    }

    #[test]
    fn token_hash_is_stable_and_hex() {
        assert_eq!(token_hash("abc"), token_hash("abc"));
        assert_eq!(token_hash("abc").len(), 64);
        assert_ne!(token_hash("abc"), token_hash("abd"));
    }
}
