//! The /api/v1 HTTP API. The web app uses exactly this API, nothing private.

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::Router;
use serde_json::json;

use crate::auth;
use crate::model::now_ms;
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

/// Who is making a request, and the API token they used, if any.
#[derive(Clone)]
pub struct CurrentUser {
    pub user: String,
    /// Name of the API token, when the request didn't come from a signed-in session.
    pub token: Option<String>,
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

// Guard: an API token, or a session plus the CSRF header on writes, everywhere except a
// few public endpoints.

async fn guard(State(st): State<Arc<AppState>>, mut req: Request, next: Next) -> Response {
    let path = req.uri().path().to_string();
    // Inbound webhooks carry their own secret in the URL (and maybe a signature).
    if path.starts_with("/hooks/") || path.starts_with("/api/v1/hooks/") {
        return next.run(req).await;
    }
    if path.starts_with("/agent/") || path.starts_with("/api/v1/agent/") {
        // Agents authenticate with their own credential in each handler; no cookies.
        return next.run(req).await;
    }
    // API tokens: no cookies are involved, so no CSRF header is needed.
    if let Some(raw) = auth::bearer_token(req.headers()) {
        if path.ends_with("/system") {
            return next.run(req).await;
        }
        let Some(token) = check_api_token(&st, &raw) else {
            return ApiError::new(StatusCode::UNAUTHORIZED, "This API token is not valid, has expired or was revoked.").into_response();
        };
        if path.contains("/me/tokens") || (path.contains("/auth/") && !path.ends_with("/auth/me")) {
            return ApiError::new(StatusCode::FORBIDDEN, "API tokens can't manage tokens or sign-ins. Sign in to the web app to do that.").into_response();
        }
        req.extensions_mut().insert(CurrentUser { user: token.user, token: Some(token.name) });
        return next.run(req).await;
    }
    let safe = matches!(*req.method(), Method::GET | Method::HEAD);
    if !safe && req.headers().get(auth::CSRF_HEADER).and_then(|v| v.to_str().ok()) != Some("1") {
        return ApiError::new(StatusCode::FORBIDDEN, "Missing the X-OpSkills-Request header.").into_response();
    }
    if path.ends_with("/system") || path.ends_with("/auth/login") {
        return next.run(req).await;
    }
    let user = auth::cookie_token(req.headers()).and_then(|t| st.store.session(&auth::token_hash(&t)).ok().flatten());
    match user {
        Some(session) => {
            req.extensions_mut().insert(CurrentUser { user: session.user, token: None });
            next.run(req).await
        }
        None => ApiError::new(StatusCode::UNAUTHORIZED, "Sign in to continue.").into_response(),
    }
}

