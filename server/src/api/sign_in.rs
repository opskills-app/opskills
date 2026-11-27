//! Sign-in with the OIDC provider (Settings > Sign-in), and those settings.
//!
//! An account is found by the provider's issuer and subject once linked. On its first
//! sign-in there, a user a server admin gave the same verified email is linked; if none,
//! and accounts may be created, a new one is made for an allowed email domain. New
//! accounts are in no workspace until a workspace Admin adds them.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use serde_json::json;

use crate::audit::{self, By};
use crate::auth;
use crate::model::{AuditTarget, OidcLink, OidcSettings, SignInSettings, StoredUser, User, normalize_email, now_ms, validate_username};
use crate::oidc::{PENDING_COOKIE, ProviderUser};
use crate::web::AppState;
use super::{Access, ApiError, ApiResult, CurrentUser, bad};

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/auth/oidc/start", get(start))
        .route("/auth/oidc/callback", get(callback))
        .route("/settings/sign-in", get(get_settings).put(put_settings))
        .route("/settings/sign-in/test", post(test_provider))
}

pub(super) const ACCESS: &[(&str, &str, Access)] = &[
    ("GET", "/auth/oidc/start", Access::Public),
    ("GET", "/auth/oidc/callback", Access::Public),
    ("GET", "/settings/sign-in", Access::ServerAdmin),
    ("PUT", "/settings/sign-in", Access::ServerAdmin),
    ("POST", "/settings/sign-in/test", Access::ServerAdmin),
];

const CALLBACK_PATH: &str = "/api/v1/auth/oidc/callback";
/// Most accounts the server keeps; the same limit as for accounts server admins add.
const MAX_USERS: usize = 500;

/// The URL to register at the provider, from the server's public URL.
fn redirect_url(st: &AppState) -> Option<String> {
    st.cfg.server.public_url.as_deref().map(|u| format!("{}{CALLBACK_PATH}", u.trim_end_matches('/')))
}

/// Back to the sign-in page, which shows `message`.
fn to_sign_in_page(message: &str) -> Response {
    let mut url = String::from("/?sign_in_error=");
    url.extend(form_urlencoded(message));
    let mut res = StatusCode::SEE_OTHER.into_response();
    res.headers_mut().insert(header::LOCATION, HeaderValue::from_str(&url).unwrap_or(HeaderValue::from_static("/")));
    res
}

fn form_urlencoded(s: &str) -> impl Iterator<Item = char> + '_ {
    s.bytes().flat_map(|b| {
        let keep = b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~');
        let out: Vec<char> = if keep { vec![b as char] } else { format!("%{b:02X}").chars().collect() };
        out
    })
}

fn pending_cookie(value: &str, max_age: u64, secure: bool) -> String {
    // Lax, not Strict: it must come back on the provider's redirect to the callback.
    format!("{PENDING_COOKIE}={value}; Path={CALLBACK_PATH}; HttpOnly; SameSite=Lax; Max-Age={max_age}{}", if secure { "; Secure" } else { "" })
}

fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .find_map(|kv| kv.trim().strip_prefix(name).and_then(|r| r.strip_prefix('=')).map(str::to_string))
}

/// The provider settings and client secret, if sign-in with the provider is on.
fn provider(st: &AppState) -> Result<(OidcSettings, String, String), String> {
    let settings = st.store.sign_in_settings().map_err(|e| e.to_string())?;
    let p = settings.provider().cloned().ok_or("Sign-in with a provider isn't set up on this server.")?;
    let redirect = redirect_url(st).ok_or("The server's public URL isn't set, so the provider can't send you back.")?;
    let secret = crate::secret_refs::server_value(&st.store, &st.vault, &p.client_secret).map_err(|e| format!("The client secret can't be read: {e}"))?;
    Ok((p, secret, redirect))
}

/// "Sign in with ...": off to the provider, with a cookie to come back with.
async fn start(State(st): State<Arc<AppState>>) -> Response {
    let (p, secret, redirect) = match provider(&st) {
        Ok(x) => x,
        Err(e) => return to_sign_in_page(&e),
    };
    let st2 = st.clone();
    let started = tokio::task::spawn_blocking(move || st2.oidc.start(&p, &secret, &redirect)).await;
    match started {
        Ok(Ok((url, id))) => {
            let mut res = StatusCode::SEE_OTHER.into_response();
            let h = res.headers_mut();
            if let Ok(v) = HeaderValue::from_str(&url) {
                h.insert(header::LOCATION, v);
            }
            h.insert(header::SET_COOKIE, HeaderValue::from_str(&pending_cookie(&id, 600, st.secure_cookies())).unwrap());
            h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            res
        }
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "starting a provider sign-in failed");
            to_sign_in_page(&format!("Couldn't reach the sign-in provider: {e}"))
        }
        Err(e) => to_sign_in_page(&format!("Starting the sign-in failed: {e}")),
    }
}

/// The provider sends the browser back here with a code.
async fn callback(State(st): State<Arc<AppState>>, headers: HeaderMap, Query(q): Query<HashMap<String, String>>) -> Response {
    let fail = |st: &AppState, why: &str, tried: Option<&str>| {
        tracing::warn!(reason = %why, "provider sign-in failed");
        audit::record(st, &By::named(tried.unwrap_or("unknown")), None, "auth.sign_in_failed", None, Some(format!("with the sign-in provider: {why}")));
        let mut res = to_sign_in_page(&format!("Sign-in failed: {why}"));
        res.headers_mut().append(header::SET_COOKIE, HeaderValue::from_str(&pending_cookie("", 0, st.secure_cookies())).unwrap());
        res
    };
    if let Some(err) = q.get("error") {
        let detail = q.get("error_description").map(|d| format!(" ({d})")).unwrap_or_default();
        return fail(&st, &format!("the provider said {err}{detail}"), None);
    }
    let (Some(code), Some(state)) = (q.get("code").cloned(), q.get("state").cloned()) else {
        return fail(&st, "the provider sent no sign-in code.", None);
    };
    let (p, secret, redirect) = match provider(&st) {
        Ok(x) => x,
        Err(e) => return fail(&st, &e, None),
    };
    let cookie = cookie_value(&headers, PENDING_COOKIE);
    let st2 = st.clone();
    let p2 = p.clone();
    let finished = tokio::task::spawn_blocking(move || st2.oidc.finish(&p2, &secret, &redirect, cookie.as_deref(), &state, &code)).await;
    let who = match finished {
        Ok(Ok(who)) => who,
        Ok(Err(e)) => return fail(&st, &e, None),
        Err(e) => return fail(&st, &e.to_string(), None),
    };
    let tried = who.email.clone().unwrap_or_else(|| who.subject.clone());
    let mut stored = match account_for(&st, &p, &who) {
        Ok(Ok(u)) => u,
        Ok(Err(why)) => return fail(&st, &why, Some(&tried)),
        Err(e) => return fail(&st, &e.detail(), Some(&tried)),
    };
    let cookie = match super::session::open_session(&st, &mut stored) {
        Ok(c) => c,
        Err(e) => return fail(&st, &e.detail(), Some(&tried)),
    };
    let name = stored.user.name.clone();
    tracing::info!(user = %name, provider = %p.label, "signed in with the provider");
    audit::record(&st, &By::named(&name), None, "auth.sign_in", None, Some(format!("with {}", p.label)));
    let mut res = StatusCode::SEE_OTHER.into_response();
    let h = res.headers_mut();
    h.insert(header::LOCATION, HeaderValue::from_static("/"));
    h.append(header::SET_COOKIE, HeaderValue::from_str(&cookie).unwrap());
    h.append(header::SET_COOKIE, HeaderValue::from_str(&pending_cookie("", 0, st.secure_cookies())).unwrap());
    res
}

/// The account for the provider's user: linked already, linked now by email, or new.
/// `Ok(Err(why))` when there's none they may use.
fn account_for(st: &AppState, p: &OidcSettings, who: &ProviderUser) -> ApiResult<Result<StoredUser, String>> {
    let link = OidcLink { issuer: who.issuer.clone(), subject: who.subject.clone() };
    let users = st.store.users()?;
    if let Some(u) = users.iter().find(|u| u.oidc.as_ref() == Some(&link)) {
        return Ok(st.store.user(&u.name)?.ok_or_else(|| "the account was just removed.".to_string()));
    }
    let verified = who.email_verified == Some(true) || p.trust_unverified_email;
    let email = who.email.as_deref().and_then(|e| normalize_email(e).ok());
    let Some(email) = email else {
        return Ok(Err("the provider sent no email address, so there's no account to match. Ask a server admin.".into()));
    };
    if !verified {
        return Ok(Err(format!("the provider hasn't verified {email}. Ask a server admin.")));
    }
    // Google: with domains set, the account must belong to a listed Workspace domain, not
    // only use one of its addresses (a personal Google account can verify any address).
    if !google_domain_ok(p, who) {
        return Ok(Err(format!("{email} is not a Google Workspace account of an allowed domain.")));
    }
    if p.link_by_email
        && let Some(u) = users.iter().find(|u| u.email.as_deref() == Some(email.as_str()))
    {
        if u.oidc.is_some() {
            return Ok(Err(format!("the account with {email} is linked to another sign-in. Ask a server admin to unlink it.")));
        }
        let mut stored = st.store.user(&u.name)?.ok_or_else(|| ApiError::not_found("User"))?;
        stored.user.oidc = Some(link);
        // A generated password never chosen by its owner goes away once they use the provider.
        if stored.user.must_change_password {
            stored.user.must_change_password = false;
            stored.password_hash = auth::NO_PASSWORD.into();
        }
        st.store.put_user(&stored)?;
        let name = stored.user.name.clone();
        audit::record(st, &By::named(&name), None, "user.sign_in_link", Some(AuditTarget::new("user", &name, &name)), Some(format!("linked to {} by the verified email {email}", p.label)));
        return Ok(Ok(stored));
    }
    if !p.create_accounts {
        return Ok(Err(format!("there's no OpSkills account for {email}. Ask a server admin to add you.")));
    }
    if !p.domain_allowed(&email) {
        return Ok(Err(format!("{email} is not from an email domain this server accepts. Ask a server admin.")));
    }
    if users.len() >= MAX_USERS {
        return Ok(Err(format!("this server has its limit of {MAX_USERS} accounts.")));
    }
    let name = new_name(&users, who.preferred_username.as_deref(), &email);
    let user = User {
        name: name.clone(),
        admin: false,
        workspaces: Default::default(),
        created_at: now_ms(),
        must_change_password: false,
        last_sign_in_at: None,
        password_changed_at: None,
        email: Some(email.clone()),
        oidc: Some(link),
    };
    let stored = StoredUser { user, password_hash: auth::NO_PASSWORD.into() };
    st.store.put_user(&stored)?;
    tracing::info!(user = %name, provider = %p.label, "account created on first sign-in");
    audit::record(st, &By::named(&name), None, "user.create", Some(AuditTarget::new("user", &name, &name)), Some(format!("first sign-in with {}, as {email}; no workspaces yet", p.label)));
    Ok(Ok(stored))
}

const GOOGLE: &str = "https://accounts.google.com";

/// With Google and email domains set, the account must belong to a listed Workspace domain
/// (the `hd` claim), not only use one of its addresses.
fn google_domain_ok(p: &OidcSettings, who: &ProviderUser) -> bool {
    who.issuer != GOOGLE || p.allowed_domains.is_empty() || who.hosted_domain.as_deref().is_some_and(|d| p.allowed_domains.iter().any(|a| a == d))
}

/// A free user name from the provider's user name or the email's local part.
fn new_name(users: &[User], preferred: Option<&str>, email: &str) -> String {
    let source = preferred.map(|p| p.split('@').next().unwrap_or(p)).filter(|p| !p.is_empty()).unwrap_or_else(|| email.split('@').next().unwrap_or("user"));
    let cleaned: String = source.to_lowercase().chars().map(|c| if c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-') { c } else { '-' }).collect();
    let cleaned = cleaned.trim_start_matches(|c: char| !c.is_ascii_alphanumeric()).chars().take(56).collect::<String>();
    let base = validate_username(&cleaned).unwrap_or_else(|_| "user".into());
    let taken = |n: &str| users.iter().any(|u| u.name == n);
    if !taken(&base) {
        return base;
    }
    (2..).map(|i| format!("{base}-{i}")).find(|n| !taken(n)).unwrap_or(base)
}

async fn get_settings(State(st): State<Arc<AppState>>) -> ApiResult<Json<serde_json::Value>> {
    Ok(Json(json!({ "settings": st.store.sign_in_settings()?, "redirect_url": redirect_url(&st), "public_url": st.cfg.server.public_url })))
}

async fn put_settings(State(st): State<Arc<AppState>>, Extension(user): Extension<CurrentUser>, body: String) -> ApiResult<Json<serde_json::Value>> {
    let mut s: SignInSettings = serde_json::from_str(&body).map_err(|e| bad(format!("Invalid sign-in settings: {e}")))?;
    s.validate().map_err(bad)?;
    if let Some(o) = &s.oidc {
        if let Err(why) = crate::secret_refs::server_check(&st.store, &o.client_secret)? {
            return Err(bad(why));
        }
        if o.enabled && redirect_url(&st).is_none() {
            return Err(ApiError::new(StatusCode::CONFLICT, "Set the server's public URL (server.public_url) first: the provider sends people back to it."));
        }
    }
    st.store.put_sign_in_settings(&s)?;
    st.oidc.forget();
    let detail = match s.oidc.as_ref() {
        None => "no provider".to_string(),
        Some(o) => format!(
            "{} {} at {}{}{}",
            o.label,
            if o.enabled { "on" } else { "off" },
            o.issuer,
            if o.create_accounts { format!(", creates accounts for {}", o.allowed_domains.join(", ")) } else { String::new() },
            if s.passwords_for_server_admins_only { ", passwords only for server admins" } else { "" },
        ),
    };
    tracing::info!(detail = %detail, "sign-in settings saved");
    audit::record(&st, &By::user(&user), None, "settings.sign_in_update", Some(AuditTarget::new("settings", "sign_in", "Sign-in")), Some(detail));
    get_settings(State(st)).await
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct TestInput {
    issuer: String,
}

/// Read the provider's settings, to check the issuer before saving.
async fn test_provider(State(st): State<Arc<AppState>>, body: String) -> ApiResult<Json<serde_json::Value>> {
    let input: TestInput = serde_json::from_str(&body).map_err(|e| bad(format!("Invalid request: {e}")))?;
    let issuer = input.issuer.trim().trim_end_matches('/').to_string();
    let st2 = st.clone();
    let found = tokio::task::spawn_blocking(move || st2.oidc.discover_now(&issuer)).await.map_err(|e| anyhow::anyhow!("{e}"))?;
    match found {
        Ok(m) => Ok(Json(json!({
            "issuer": m.issuer().to_string(),
            "authorization_endpoint": m.authorization_endpoint().to_string(),
            "token_endpoint": m.token_endpoint().map(|u| u.to_string()),
            "keys": m.jwks().keys().len(),
            "redirect_url": redirect_url(&st),
        }))),
        Err(e) => Err(ApiError::new(StatusCode::BAD_GATEWAY, e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn google_accounts_must_belong_to_an_allowed_workspace_domain() {
        let mut p: OidcSettings = serde_json::from_value(json!({"label": "Google", "issuer": GOOGLE, "client_id": "c", "client_secret": "shared/S", "allowed_domains": ["example.com"]})).unwrap();
        let who = |issuer: &str, hd: Option<&str>| ProviderUser {
            issuer: issuer.into(),
            subject: "1".into(),
            email: Some("a@example.com".into()),
            email_verified: Some(true),
            preferred_username: None,
            hosted_domain: hd.map(str::to_string),
        };
        assert!(google_domain_ok(&p, &who(GOOGLE, Some("example.com"))));
        assert!(!google_domain_ok(&p, &who(GOOGLE, None)), "a personal Google account using a company address");
        assert!(!google_domain_ok(&p, &who(GOOGLE, Some("other.example"))));
        assert!(google_domain_ok(&p, &who("https://example.okta.com", None)), "only Google sends hd");
        p.allowed_domains.clear();
        assert!(google_domain_ok(&p, &who(GOOGLE, None)), "no domains set: linking pre-created accounts by email only");
    }

    #[test]
    fn new_names_come_from_the_provider_and_stay_unique() {
        let taken = |names: &[&str]| names.iter().map(|n| User { name: n.to_string(), admin: false, workspaces: Default::default(), created_at: 0, must_change_password: false, last_sign_in_at: None, password_changed_at: None, email: None, oidc: None }).collect::<Vec<_>>();
        assert_eq!(new_name(&[], Some("Jane.Doe@corp.example"), "x@corp.example"), "jane.doe");
        assert_eq!(new_name(&[], None, "o'brien+ops@corp.example"), "o-brien-ops");
        assert_eq!(new_name(&taken(&["jane", "jane-2"]), Some("jane"), "j@x.example"), "jane-3");
        assert_eq!(new_name(&[], Some("___"), "___@x.example"), "user");
    }
}
