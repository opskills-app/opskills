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
use crate::model::{Role, now_ms, validate_password};
use crate::store::Session;
use crate::audit::{self, By};
use crate::model::AuditTarget;
use crate::web::AppState;
use super::{Access, ApiError, ApiResult, CurrentUser, bad};

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/system", get(system))
        .route("/auth/login", post(login))
        .route("/auth/logout", post(logout))
        .route("/auth/me", get(me))
        .route("/auth/password", post(change_password))
}

pub(super) const ACCESS: &[(&str, &str, Access)] = &[
    ("GET", "/system", Access::Public),
    ("POST", "/auth/login", Access::Public),
    ("POST", "/auth/logout", Access::Account),
    ("GET", "/auth/me", Access::Me),
    ("POST", "/auth/password", Access::Account),
];

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
        // False until an admin has replaced the generated password; the sign-in page only
        // explains where to find that password while this is false.
        "setup_done": setup_done(&st),
    }))
}

fn setup_done(st: &AppState) -> bool {
    st.store.users().map(|users| users.iter().any(|u| u.role == Role::Admin && !u.must_change_password)).unwrap_or(false)
}

#[derive(Deserialize)]
struct LoginInput {
    username: String,
    password: String,
}

static FAILED_LOGINS: AtomicU64 = AtomicU64::new(0);

async fn login(State(st): State<Arc<AppState>>, Json(input): Json<LoginInput>) -> ApiResult<Response> {
    let name = input.username.trim().to_lowercase();
    let stored = st.store.user(&name)?;
    let stored_exists = stored.is_some();
    // Unknown names are checked against a dummy hash, so both take the same time.
    let hash = stored.as_ref().map_or(auth::DUMMY_HASH.to_string(), |u| u.password_hash.clone());
    let password = input.password.clone();
    let ok = tokio::task::spawn_blocking(move || auth::verify_password(&hash, &password)).await.unwrap_or(false);
    let Some(mut stored) = stored.filter(|_| ok) else {
        // Slow down guessing; the delay grows with recent failures, up to 5 seconds.
        let fails = FAILED_LOGINS.fetch_add(1, Ordering::Relaxed).min(9) + 1;
        tokio::time::sleep(Duration::from_millis(500 * fails)).await;
        tracing::warn!(username = %input.username, "failed sign-in");
        let tried: String = name.chars().filter(|c| !c.is_control()).take(64).collect();
        audit::record(&st, &By::named(&tried), "auth.sign_in_failed", None, Some(if stored_exists { "wrong password" } else { "no such user" }.into()));
        return Err(ApiError::new(StatusCode::UNAUTHORIZED, "Wrong username or password."));
    };
    FAILED_LOGINS.store(0, Ordering::Relaxed);
    st.store.purge_expired_sessions()?;
    let token = auth::new_token();
    let max_age = st.cfg.auth.session_hours * 3600;
    let now = now_ms();
    st.store.put_session(&auth::token_hash(&token), &Session { user: name.clone(), created_at: now, expires_at: now + max_age * 1000 })?;
    stored.user.last_sign_in_at = Some(now);
    st.store.put_user(&stored)?;
    tracing::info!(user = %name, "signed in");
    audit::record(&st, &By::named(&name), "auth.sign_in", None, None);
    let mut res = Json(json!({ "user": name, "role": stored.user.role, "must_change_password": stored.user.must_change_password })).into_response();
    res.headers_mut().insert(header::SET_COOKIE, HeaderValue::from_str(&auth::session_cookie(&token, max_age, st.secure_cookies())).unwrap());
    Ok(res)
}

async fn logout(State(st): State<Arc<AppState>>, Extension(user): Extension<CurrentUser>, req: Request) -> ApiResult<Response> {
    if let Some(token) = auth::cookie_token(req.headers()) {
        st.store.delete_session(&auth::token_hash(&token))?;
    }
    audit::record(&st, &By::user(&user), "auth.sign_out", None, None);
    let mut res = StatusCode::NO_CONTENT.into_response();
    res.headers_mut().insert(header::SET_COOKIE, HeaderValue::from_str(&auth::clear_cookie(st.secure_cookies())).unwrap());
    Ok(res)
}

async fn me(State(st): State<Arc<AppState>>, Extension(user): Extension<CurrentUser>) -> ApiResult<Json<serde_json::Value>> {
    let must_change = st.store.user(&user.user)?.is_some_and(|u| u.user.must_change_password);
    Ok(Json(json!({ "user": user.user, "role": user.role, "token": user.token, "must_change_password": must_change })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PasswordInput {
    current_password: String,
    new_password: String,
}

/// Change your own password. Signs you out everywhere else.
async fn change_password(State(st): State<Arc<AppState>>, Extension(user): Extension<CurrentUser>, body: String) -> ApiResult<StatusCode> {
    let input: PasswordInput = serde_json::from_str(&body).map_err(|e| bad(format!("Invalid request: {e}")))?;
    let mut stored = st.store.user(&user.user)?.ok_or_else(|| ApiError::not_found("User"))?;
    let (hash, current) = (stored.password_hash.clone(), input.current_password.clone());
    if !tokio::task::spawn_blocking(move || auth::verify_password(&hash, &current)).await.unwrap_or(false) {
        return Err(ApiError::new(StatusCode::FORBIDDEN, "The current password is wrong."));
    }
    validate_password(&input.new_password).map_err(bad)?;
    if input.new_password == input.current_password {
        return Err(bad("Choose a password different from the current one.".into()));
    }
    let new = input.new_password.clone();
    stored.password_hash = tokio::task::spawn_blocking(move || auth::hash_password(&new)).await.map_err(|e| anyhow::anyhow!("{e}"))??;
    stored.user.must_change_password = false;
    stored.user.password_changed_at = Some(now_ms());
    st.store.put_user(&stored)?;
    st.store.delete_sessions_of(&user.user, user.session.as_deref())?;
    tracing::info!(user = %user.user, "password changed");
    audit::record(&st, &By::user(&user), "auth.password_change", Some(AuditTarget::new("user", &user.user, &user.user)), None);
    Ok(StatusCode::NO_CONTENT)
}
