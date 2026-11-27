//! The /api/v1 HTTP API. The web app uses exactly this API, nothing private.
//!
//! Every route has an access rule in its feature's `ACCESS` list, next to its `routes()`.
//! The guard enforces it and refuses any route without one; a test checks that every
//! route has exactly one rule.

use std::sync::Arc;

use axum::extract::{MatchedPath, Request, State};
use axum::http::{Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::Router;
use serde_json::json;

use crate::auth;
use crate::model::{Role, now_ms};
use crate::web::AppState;

mod session;
mod runbooks;
mod tokens;
mod runs;
mod webhooks;
mod notifications;
mod schedules;
mod secrets;
mod hosts;
mod agent;
mod users;

pub use hosts::{Package, packages, release_url};
pub use runs::start_runbook;

pub fn router(state: Arc<AppState>) -> Router<Arc<AppState>> {
    Router::new()
        .merge(session::routes())
        .merge(runbooks::routes())
        .merge(tokens::routes())
        .merge(runs::routes())
        .merge(webhooks::routes())
        .merge(notifications::routes())
        .merge(schedules::routes())
        .merge(secrets::routes())
        .merge(hosts::routes())
        .merge(agent::routes())
        .merge(users::routes())
        .fallback(|| async { ApiError::new(StatusCode::NOT_FOUND, "No such API endpoint.") })
        .layer(middleware::from_fn_with_state(state, guard))
}

// Errors

pub struct ApiError {
    status: StatusCode,
    detail: String,
}

impl ApiError {
    pub fn new(status: StatusCode, detail: impl Into<String>) -> Self {
        ApiError { status, detail: detail.into() }
    }
    fn not_found(what: &str) -> Self {
        Self::new(StatusCode::NOT_FOUND, format!("{what} not found."))
    }
    /// The message for people, also used when a run starts from a schedule or webhook.
    pub fn detail(&self) -> String {
        self.detail.clone()
    }
    pub fn status(&self) -> StatusCode {
        self.status
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(err: anyhow::Error) -> Self {
        tracing::error!(error = %format!("{err:#}"), "request failed");
        ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Something went wrong on the server.")
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = json!({ "title": self.status.canonical_reason().unwrap_or("Error"), "status": self.status.as_u16(), "detail": self.detail });
        (self.status, [(header::CONTENT_TYPE, "application/problem+json")], body.to_string()).into_response()
    }
}

type ApiResult<T> = Result<T, ApiError>;

/// A 400 with the given message.
fn bad(msg: String) -> ApiError {
    ApiError::new(StatusCode::BAD_REQUEST, msg)
}

/// Who may call a route.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    /// Anyone, without signing in.
    Public,
    /// The handler checks its own credential (agents, inbound webhooks).
    OwnCredential,
    /// A signed-in session, also before a generated password is replaced.
    Account,
    /// A session or an API token, also before a generated password is replaced.
    Me,
    /// A session or an API token of a user with at least this role.
    Role(Role),
    /// Like `Role`, from a signed-in session only: API tokens can't manage tokens or users.
    SessionRole(Role),
}

/// `(method, path as registered, rule)`. HEAD uses the GET rule.
type Rules = &'static [(&'static str, &'static str, Access)];

const RULES: &[Rules] = &[
    session::ACCESS,
    runbooks::ACCESS,
    tokens::ACCESS,
    runs::ACCESS,
    webhooks::ACCESS,
    notifications::ACCESS,
    schedules::ACCESS,
    secrets::ACCESS,
    hosts::ACCESS,
    agent::ACCESS,
    users::ACCESS,
];

/// Every access rule, as `(method, route, rule)`.
pub fn access_rules() -> impl Iterator<Item = (&'static str, &'static str, Access)> {
    RULES.iter().flat_map(|r| r.iter().copied())
}

/// The rule for a matched route, if there is one.
fn access_for(method: &Method, route: &str) -> Option<Access> {
    let method = if method == Method::HEAD { "GET" } else { method.as_str() };
    RULES.iter().flat_map(|r| r.iter()).find(|(m, p, _)| *m == method && *p == route).map(|(_, _, a)| *a)
}

/// Who is making a request, and the API token they used, if any.
#[derive(Clone)]
pub struct CurrentUser {
    pub user: String,
    pub role: Role,
    /// Name of the API token, when the request didn't come from a signed-in session.
    pub token: Option<String>,
    /// The SHA-256 of the session token, for a signed-in session.
    pub session: Option<String>,
}

impl CurrentUser {
    /// How runs and other records name the requester: "admin" or "admin (token ci)".
    pub fn label(&self) -> String {
        match &self.token {
            Some(name) => format!("{} (token {name})", self.user),
            None => self.user.clone(),
        }
    }
}

/// How long a token's last use may be stale before it's written again.
const TOKEN_USE_RESOLUTION_MS: u64 = 60_000;

/// The token a Bearer header carries, if it is valid, unexpired and not revoked.
fn check_api_token(st: &AppState, raw: &str) -> Option<crate::model::ApiToken> {
    let (id, secret) = auth::parse_api_token(raw)?;
    let mut stored = st.store.api_token(id).ok()??;
    if !auth::constant_time_eq(auth::token_hash(secret).as_bytes(), stored.secret_hash.as_bytes()) {
        return None;
    }
    let now = now_ms();
    if stored.token.expires_at.is_some_and(|e| e <= now) {
        return None;
    }
    if stored.token.last_used_at.is_none_or(|t| now.saturating_sub(t) > TOKEN_USE_RESOLUTION_MS) {
        stored.token.last_used_at = Some(now);
        let _ = st.store.put_api_token(&stored);
    }
    Some(stored.token)
}

fn refuse(status: StatusCode, detail: &str) -> Response {
    ApiError::new(status, detail).into_response()
}

// Guard: find the route's rule, then the requester (an API token, or a session plus the
// CSRF header on writes), then check the rule.

async fn guard(State(st): State<Arc<AppState>>, matched: Option<MatchedPath>, mut req: Request, next: Next) -> Response {
    let route = matched.map(|m| {
        let p = m.as_str();
        p.strip_prefix("/api/v1").unwrap_or(p).to_string()
    });
    let access = match &route {
        Some(route) => match access_for(req.method(), route) {
            Some(a) => Some(a),
            None => {
                tracing::error!(method = %req.method(), route = %route, "refused a route without an access rule");
                return refuse(StatusCode::FORBIDDEN, "This endpoint has no access rule.");
            }
        },
        // No route matched: the fallback answers 404, but only to signed-in callers.
        None => None,
    };
    if access == Some(Access::OwnCredential) {
        return next.run(req).await;
    }
    let bearer = auth::bearer_token(req.headers());
    if bearer.is_none() {
        // Cookies need the CSRF header on writes; tokens don't use cookies.
        let safe = matches!(*req.method(), Method::GET | Method::HEAD);
        if !safe && req.headers().get(auth::CSRF_HEADER).and_then(|v| v.to_str().ok()) != Some("1") {
            return refuse(StatusCode::FORBIDDEN, "Missing the X-OpSkills-Request header.");
        }
    }
    if access == Some(Access::Public) {
        return next.run(req).await;
    }
    let current = match &bearer {
        Some(raw) => {
            let Some(token) = check_api_token(&st, raw) else {
                return refuse(StatusCode::UNAUTHORIZED, "This API token is not valid, has expired or was revoked.");
            };
            (token.user, Some(token.name), None)
        }
        None => {
            let hash = auth::cookie_token(req.headers()).map(|t| auth::token_hash(&t));
            match hash.as_deref().and_then(|h| st.store.session(h).ok().flatten()) {
                Some(session) => (session.user, None, hash),
                None => return refuse(StatusCode::UNAUTHORIZED, "Sign in to continue."),
            }
        }
    };
    let (name, token, session) = current;
    // The user's role is read on every request, so changes apply at once.
    let user = match st.store.user(&name) {
        Ok(Some(u)) => u.user,
        Ok(None) => return refuse(StatusCode::UNAUTHORIZED, "This user no longer exists."),
        Err(e) => return ApiError::from(e).into_response(),
    };
    let from_token = token.is_some();
    let allowed = match access {
        None | Some(Access::Me) => Ok(()),
        Some(Access::Account) if from_token => Err("API tokens can't manage sign-ins. Sign in to the web app to do that."),
        Some(Access::Account) => Ok(()),
        Some(Access::SessionRole(_)) if from_token => Err("API tokens can't manage tokens or users. Sign in to the web app to do that."),
        Some(Access::Role(_) | Access::SessionRole(_)) if user.must_change_password => Err("Choose a new password first."),
        Some(Access::Role(min) | Access::SessionRole(min)) if user.role < min => Err("Your role doesn't allow this."),
        Some(_) => Ok(()),
    };
    if let Err(detail) = allowed {
        return refuse(StatusCode::FORBIDDEN, detail);
    }
    req.extensions_mut().insert(CurrentUser { user: user.name, role: user.role, token, session });
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    /// Every `.route(...)` in the API sources, as (method, path).
    fn registered() -> BTreeSet<(String, String)> {
        let sources = [
            include_str!("session.rs"),
            include_str!("runbooks.rs"),
            include_str!("tokens.rs"),
            include_str!("runs.rs"),
            include_str!("webhooks.rs"),
            include_str!("notifications.rs"),
            include_str!("schedules.rs"),
            include_str!("secrets.rs"),
            include_str!("hosts.rs"),
            include_str!("agent.rs"),
            include_str!("users.rs"),
        ];
        let route = regex::Regex::new(r#"\.route\("([^"]+)",\s*(.+)\)\s*$"#).unwrap();
        let method = regex::Regex::new(r"\b(get|post|put|delete|patch)\(").unwrap();
        let mut out = BTreeSet::new();
        for src in sources {
            for line in src.lines() {
                if let Some(c) = route.captures(line.trim()) {
                    for m in method.captures_iter(&c[2]) {
                        out.insert((m[1].to_uppercase(), c[1].to_string()));
                    }
                }
            }
        }
        out
    }

    #[test]
    fn every_route_has_exactly_one_access_rule() {
        let routes = registered();
        assert!(routes.len() > 30, "found only {} routes; is the parser broken?", routes.len());
        let mut rules = BTreeSet::new();
        for (m, p, _) in RULES.iter().flat_map(|r| r.iter()) {
            assert!(rules.insert((m.to_string(), p.to_string())), "two rules for {m} {p}");
        }
        let missing: Vec<_> = routes.difference(&rules).collect();
        let extra: Vec<_> = rules.difference(&routes).collect();
        assert!(missing.is_empty(), "routes without an access rule: {missing:?}");
        assert!(extra.is_empty(), "rules for routes that don't exist: {extra:?}");
    }
}
