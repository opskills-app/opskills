//! Signing in and out, and what the server reports about itself.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use serde::Deserialize;
use serde_json::json;

use crate::auth;
use crate::model::now_ms;
use crate::store::Session;
use crate::web::AppState;
use super::{ApiError, ApiResult, CurrentUser};

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/system", get(system))
        .route("/auth/login", post(login))
        .route("/auth/logout", post(logout))
        .route("/auth/me", get(me))
}

async fn system(State(st): State<Arc<AppState>>) -> Json<serde_json::Value> {
    Json(json!({
        "name": "OpSkills",
        "version": crate::VERSION,
        "server_id": st.server_id,
        "started_at_unix": st.started_unix,
        "uptime_seconds": st.started.elapsed().as_secs(),
        "public_url": st.cfg.server.public_url,
        "license": "AGPL-3.0-only",
        "source_url": format!("{}/tree/v{}", crate::SOURCE_URL, crate::VERSION),
    }))
}

#[derive(Deserialize)]
struct LoginInput {
    username: String,
    password: String,
}

static FAILED_LOGINS: AtomicU64 = AtomicU64::new(0);

async fn login(State(st): State<Arc<AppState>>, Json(input): Json<LoginInput>) -> ApiResult<Response> {
    let hash_file = st.cfg.data.dir.join("admin.hash");
    let password = input.password.clone();
    let ok = input.username.trim() == auth::ADMIN_USER
        && tokio::task::spawn_blocking(move || auth::verify_admin_password(&hash_file, &password)).await.unwrap_or(false);
    if !ok {
        // Slow down guessing; the delay grows with recent failures, up to 5 seconds.
        let fails = FAILED_LOGINS.fetch_add(1, Ordering::Relaxed).min(9) + 1;
        tokio::time::sleep(Duration::from_millis(500 * fails)).await;
        tracing::warn!(username = %input.username, "failed sign-in");
        return Err(ApiError::new(StatusCode::UNAUTHORIZED, "Wrong username or password."));
    }
    FAILED_LOGINS.store(0, Ordering::Relaxed);
    st.store.purge_expired_sessions()?;
    let token = auth::new_token();
    let max_age = st.cfg.auth.session_hours * 3600;
    let now = now_ms();
    st.store.put_session(&auth::token_hash(&token), &Session { user: auth::ADMIN_USER.into(), created_at: now, expires_at: now + max_age * 1000 })?;
    tracing::info!(user = auth::ADMIN_USER, "signed in");
    let mut res = Json(json!({ "user": auth::ADMIN_USER })).into_response();
    res.headers_mut().insert(header::SET_COOKIE, HeaderValue::from_str(&auth::session_cookie(&token, max_age, st.secure_cookies())).unwrap());
    Ok(res)
}

async fn logout(State(st): State<Arc<AppState>>, req: Request) -> ApiResult<Response> {
    if let Some(token) = auth::cookie_token(req.headers()) {
        st.store.delete_session(&auth::token_hash(&token))?;
    }
    let mut res = StatusCode::NO_CONTENT.into_response();
    res.headers_mut().insert(header::SET_COOKIE, HeaderValue::from_str(&auth::clear_cookie(st.secure_cookies())).unwrap());
    Ok(res)
}

async fn me(Extension(user): Extension<CurrentUser>) -> Json<serde_json::Value> {
    Json(json!({ "user": user.user, "token": user.token }))
}
