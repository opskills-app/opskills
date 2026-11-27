//! Users and roles, for admins. New users and password resets get a generated password
//! that is shown once; the user must replace it at their next sign-in.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use serde::Deserialize;
use serde_json::json;

use crate::auth;
use crate::firststart::random_password;
use crate::model::{Role, StoredUser, User, now_ms, validate_username};
use crate::audit::{self, By};
use crate::model::AuditTarget;
use crate::web::AppState;
use super::{Access, ApiError, ApiResult, CurrentUser, bad};

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/users", get(list_users).post(create_user))
        .route("/users/{name}", axum::routing::put(update_user).delete(delete_user))
        .route("/users/{name}/reset-password", post(reset_password))
}

pub(super) const ACCESS: &[(&str, &str, Access)] = &[
    ("GET", "/users", Access::SessionRole(Role::Admin)),
    ("POST", "/users", Access::SessionRole(Role::Admin)),
    ("PUT", "/users/{name}", Access::SessionRole(Role::Admin)),
    ("DELETE", "/users/{name}", Access::SessionRole(Role::Admin)),
    ("POST", "/users/{name}/reset-password", Access::SessionRole(Role::Admin)),
];

const MAX_USERS: usize = 500;
const GENERATED_PASSWORD_LENGTH: usize = 20;

async fn list_users(State(st): State<Arc<AppState>>) -> ApiResult<Json<Vec<User>>> {
    Ok(Json(st.store.users()?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewUser {
    name: String,
    role: Role,
}

/// A generated password and its hash.
async fn generated_password() -> ApiResult<(String, String)> {
    let password = random_password(GENERATED_PASSWORD_LENGTH);
    let p = password.clone();
    let hash = tokio::task::spawn_blocking(move || auth::hash_password(&p)).await.map_err(|e| anyhow::anyhow!("{e}"))??;
    Ok((password, hash))
}

async fn create_user(State(st): State<Arc<AppState>>, Extension(me): Extension<CurrentUser>, body: String) -> ApiResult<Response> {
    let input: NewUser = serde_json::from_str(&body).map_err(|e| bad(format!("Invalid user: {e}")))?;
    let name = validate_username(&input.name).map_err(bad)?;
    let users = st.store.users()?;
    if users.iter().any(|u| u.name == name) {
        return Err(ApiError::new(StatusCode::CONFLICT, format!("There is already a user named {name:?}.")));
    }
    if users.len() >= MAX_USERS {
        return Err(ApiError::new(StatusCode::CONFLICT, format!("There can be at most {MAX_USERS} users.")));
    }
    let (password, password_hash) = generated_password().await?;
    let user = User { name, role: input.role, created_at: now_ms(), must_change_password: true, last_sign_in_at: None, password_changed_at: None };
    st.store.put_user(&StoredUser { user: user.clone(), password_hash })?;
    tracing::info!(user = %user.name, role = user.role.as_str(), by = %me.user, "user created");
    audit::record(&st, &By::user(&me), "user.create", Some(AuditTarget::new("user", &user.name, &user.name)), Some(format!("role {}", user.role.as_str())));
    // The only time the password is shown.
    Ok((StatusCode::CREATED, Json(json!({ "user": user, "password": password }))).into_response())
}

/// Admins left if `name` stopped being one.
fn other_admins(users: &[User], name: &str) -> usize {
    users.iter().filter(|u| u.role == Role::Admin && u.name != name).count()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UserUpdate {
    role: Role,
}

async fn update_user(State(st): State<Arc<AppState>>, Extension(me): Extension<CurrentUser>, Path(name): Path<String>, body: String) -> ApiResult<Json<User>> {
    let input: UserUpdate = serde_json::from_str(&body).map_err(|e| bad(format!("Invalid change: {e}")))?;
    let mut stored = st.store.user(&name)?.ok_or_else(|| ApiError::not_found("User"))?;
    if input.role != stored.user.role {
        if name == me.user {
            return Err(ApiError::new(StatusCode::CONFLICT, "You can't change your own role. Ask another admin."));
        }
        if stored.user.role == Role::Admin && other_admins(&st.store.users()?, &name) == 0 {
            return Err(ApiError::new(StatusCode::CONFLICT, "There must always be at least one admin."));
        }
        tracing::info!(user = %name, from = stored.user.role.as_str(), to = input.role.as_str(), by = %me.user, "role changed");
        audit::record(&st, &By::user(&me), "user.role_change", Some(AuditTarget::new("user", &name, &name)), Some(format!("role {} to {}", stored.user.role.as_str(), input.role.as_str())));
        stored.user.role = input.role;
        st.store.put_user(&stored)?;
    }
    Ok(Json(stored.user))
}

async fn reset_password(State(st): State<Arc<AppState>>, Extension(me): Extension<CurrentUser>, Path(name): Path<String>) -> ApiResult<Json<serde_json::Value>> {
    if name == me.user {
        return Err(ApiError::new(StatusCode::CONFLICT, "To change your own password, use Settings > Your account."));
    }
    let mut stored = st.store.user(&name)?.ok_or_else(|| ApiError::not_found("User"))?;
    let (password, password_hash) = generated_password().await?;
    stored.password_hash = password_hash;
    stored.user.must_change_password = true;
    st.store.put_user(&stored)?;
    st.store.delete_sessions_of(&name, None)?;
    tracing::info!(user = %name, by = %me.user, "password reset");
    audit::record(&st, &By::user(&me), "user.password_reset", Some(AuditTarget::new("user", &name, &name)), Some("signed out; must choose a new password".into()));
    Ok(Json(json!({ "user": stored.user, "password": password })))
}

async fn delete_user(State(st): State<Arc<AppState>>, Extension(me): Extension<CurrentUser>, Path(name): Path<String>) -> ApiResult<StatusCode> {
    if name == me.user {
        return Err(ApiError::new(StatusCode::CONFLICT, "You can't remove yourself. Ask another admin."));
    }
    let stored = st.store.user(&name)?.ok_or_else(|| ApiError::not_found("User"))?;
    if stored.user.role == Role::Admin && other_admins(&st.store.users()?, &name) == 0 {
        return Err(ApiError::new(StatusCode::CONFLICT, "There must always be at least one admin."));
    }
    st.store.delete_user(&name)?;
    tracing::info!(user = %name, by = %me.user, "user removed, with their sessions and API tokens");
    audit::record(&st, &By::user(&me), "user.delete", Some(AuditTarget::new("user", &name, &name)), Some(format!("was {}; sessions and API tokens ended", stored.user.role.as_str())));
    Ok(StatusCode::NO_CONTENT)
}
