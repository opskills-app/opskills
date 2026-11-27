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
mod audit;
mod workspaces;
mod shared_secrets;
mod sign_in;
mod inventory;

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
        .merge(audit::routes())
        .merge(workspaces::routes())
        .merge(shared_secrets::routes())
        .merge(sign_in::routes())
        .merge(inventory::routes())
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
    /// Anyone signed in (session or token), with a password of their own.
    User,
    /// Like `User`, from a signed-in session only: API tokens can't manage tokens.
    SessionUser,
    /// At least this role in the workspace the request is about (session or token).
    Role(Role, Scope),
    /// Like `Role`, from a signed-in session only: API tokens can't manage members.
    SessionRole(Role, Scope),
    /// Server admins (session or token).
    ServerAdmin,
    /// Server admins, from a signed-in session only: API tokens can't manage users or tokens.
    SessionServerAdmin,
}

/// How the guard finds the workspace a `Role` route is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// `?workspace=<slug>`, or `default` without one: lists and creates.
    Query,
    /// The `{slug}` in the path.
    Slug,
    /// The workspace of the runbook `{id}`.
    Runbook,
    /// The workspace of the run `{id}`.
    Run,
    /// The workspace of the webhook `{id}`'s runbook.
    Webhook,
    /// Every workspace of the host `{id}`: changing a shared host needs the role in all.
    Host,
    /// Every workspace of the host `{id}`, and the `{slug}` it's being added to.
    HostAndSlug,
    /// The workspace of the inventory source `{id}`.
    Source,
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
    audit::ACCESS,
    workspaces::ACCESS,
    shared_secrets::ACCESS,
    sign_in::ACCESS,
    inventory::ACCESS,
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
    pub admin: bool,
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

/// The workspace a request is about, set by the guard for `Role` routes with one workspace.
#[derive(Clone, Debug)]
pub struct InWorkspace(pub String);

/// The value of `{name}` in `route` (as registered) for the request path `path`.
fn path_param(route: &str, path: &str, name: &str) -> Option<String> {
    let want = format!("{{{name}}}");
    route.split('/').zip(path.split('/')).find(|(r, _)| *r == want).map(|(_, p)| p.to_string())
}

/// `?workspace=<slug>`, or the default workspace.
fn query_workspace(query: Option<&str>) -> String {
    query
        .into_iter()
        .flat_map(|q| q.split('&'))
        .find_map(|kv| kv.strip_prefix("workspace=").map(str::to_string))
        .filter(|w| !w.is_empty())
        .unwrap_or_else(|| crate::model::DEFAULT_WORKSPACE.into())
}

/// The workspaces a request is about, and what to call the thing that isn't found.
fn scope_workspaces(st: &AppState, scope: Scope, route: &str, path: &str, query: Option<&str>) -> anyhow::Result<(Vec<String>, &'static str)> {
    let id = || path_param(route, path, "id").unwrap_or_default();
    let slug = || path_param(route, path, "slug").unwrap_or_default();
    let existing = |slug: String| -> anyhow::Result<Vec<String>> { Ok(st.store.workspace(&slug)?.map(|w| vec![w.slug]).unwrap_or_default()) };
    Ok(match scope {
        Scope::Query => (existing(query_workspace(query))?, "No such workspace."),
        Scope::Slug => (existing(slug())?, "No such workspace."),
        Scope::Runbook => (st.store.runbook(&id())?.map(|r| vec![r.workspace]).unwrap_or_default(), "Runbook not found."),
        Scope::Run => (st.store.run(&id())?.map(|r| vec![r.workspace]).unwrap_or_default(), "Run not found."),
        Scope::Webhook => {
            let rb = match st.store.webhook(&id())? {
                Some(h) => st.store.runbook(&h.hook.runbook_id)?,
                None => None,
            };
            (rb.map(|r| vec![r.workspace]).unwrap_or_default(), "Webhook not found.")
        }
        // The server itself is a host of every workspace; the request's counts.
        Scope::Host if id() == crate::model::LOCAL_HOST_ID => (existing(query_workspace(query))?, "Host not found."),
        Scope::Host => (st.store.stored_host(&id())?.map(|h| h.host.workspaces).unwrap_or_default(), "Host not found."),
        Scope::Source => (st.store.inventory_source(&id())?.map(|s| vec![s.workspace]).unwrap_or_default(), "Inventory source not found."),
        Scope::HostAndSlug => match st.store.stored_host(&id())? {
            Some(h) => {
                let mut all = h.host.workspaces;
                all.extend(existing(slug())?);
                (all, "Host not found.")
            }
            None => (vec![], "Host not found."),
        },
    })
}

// Guard: find the route's rule, then the requester (an API token, or a session plus the
// CSRF header on writes), then check the rule: for workspace routes, the requester's role
// in the workspace the request is about. Someone outside that workspace gets 404, as if
// it didn't exist, so workspace contents and names don't leak.

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
    // The user and their roles are read on every request, so changes apply at once.
    let user = match st.store.user(&name) {
        Ok(Some(u)) => u.user,
        Ok(None) => return refuse(StatusCode::UNAUTHORIZED, "This user no longer exists."),
        Err(e) => return ApiError::from(e).into_response(),
    };
    let from_token = token.is_some();
    let session_only = matches!(access, Some(Access::Account | Access::SessionUser | Access::SessionRole(..) | Access::SessionServerAdmin));
    if session_only && from_token {
        let detail = match access {
            Some(Access::Account) => "API tokens can't manage sign-ins. Sign in to the web app to do that.",
            Some(Access::SessionRole(..)) => "API tokens can't manage workspace members. Sign in to the web app to do that.",
            _ => "API tokens can't manage tokens or users. Sign in to the web app to do that.",
        };
        return refuse(StatusCode::FORBIDDEN, detail);
    }
    let needs_own_password = !matches!(access, None | Some(Access::Account | Access::Me));
    if needs_own_password && user.must_change_password {
        return refuse(StatusCode::FORBIDDEN, "Choose a new password first.");
    }
    let mut in_workspace = None;
    match access {
        Some(Access::ServerAdmin | Access::SessionServerAdmin) if !user.admin => {
            return refuse(StatusCode::FORBIDDEN, "Only server admins can do this.");
        }
        Some(Access::Role(min, scope) | Access::SessionRole(min, scope)) => {
            let (route, path) = (route.as_deref().unwrap_or(""), req.uri().path().strip_prefix("/api/v1").unwrap_or(req.uri().path()).to_string());
            let (workspaces, not_found) = match scope_workspaces(&st, scope, route, &path, req.uri().query()) {
                Ok(found) => found,
                Err(e) => return ApiError::from(e).into_response(),
            };
            let roles: Vec<Option<Role>> = workspaces.iter().map(|w| user.role_in(w)).collect();
            if roles.iter().all(Option::is_none) {
                return refuse(StatusCode::NOT_FOUND, not_found);
            }
            if roles.iter().any(Option::is_none) {
                return refuse(StatusCode::FORBIDDEN, "This host is also in a workspace you don't belong to. Ask a server admin.");
            }
            if roles.iter().any(|r| r.is_some_and(|r| r < min)) {
                return refuse(StatusCode::FORBIDDEN, "Your role doesn't allow this.");
            }
            if let [one] = workspaces.as_slice() {
                in_workspace = Some(InWorkspace(one.clone()));
            } else if matches!(scope, Scope::HostAndSlug) {
                in_workspace = path_param(route, &path, "slug").map(InWorkspace);
            }
        }
        _ => {}
    }
    if let Some(ws) = in_workspace {
        req.extensions_mut().insert(ws);
    }
    req.extensions_mut().insert(CurrentUser { user: user.name, admin: user.admin, token, session });
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
            include_str!("audit.rs"),
            include_str!("workspaces.rs"),
            include_str!("shared_secrets.rs"),
            include_str!("sign_in.rs"),
            include_str!("inventory.rs"),
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

    /// A rule's scope must fit its route, or the guard would check the wrong workspace.
    #[test]
    fn every_scope_matches_its_route() {
        for (m, route, access) in access_rules() {
            let (Access::Role(_, scope) | Access::SessionRole(_, scope)) = access else { continue };
            let ok = match scope {
                Scope::Query => !route.contains("{id}") && !route.contains("{slug}"),
                Scope::Slug => route.contains("{slug}"),
                Scope::Runbook => route.starts_with("/runbooks/{id}"),
                Scope::Run => route.starts_with("/runs/{id}"),
                Scope::Webhook => route.starts_with("/webhooks/{id}"),
                Scope::Host => route == "/hosts/{id}",
                Scope::HostAndSlug => route == "/hosts/{id}/workspaces/{slug}",
                Scope::Source => route.starts_with("/inventory/sources/{id}"),
            };
            assert!(ok, "{m} {route} has scope {scope:?}");
        }
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
