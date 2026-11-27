//! User accounts, for server admins. New users and password resets get a generated
//! password that is shown once; the user must replace it at their next sign-in. What a
//! user may do inside a workspace is set by its members (see `workspaces.rs`).

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use serde::Deserialize;
use serde_json::json;

use crate::audit::{self, By};
use crate::auth;
use crate::firststart::random_password;
use crate::model::{AuditTarget, Role, StoredUser, User, now_ms, validate_username};
use crate::web::AppState;
use super::{Access, ApiError, ApiResult, CurrentUser, bad};

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/users", get(list_users).post(create_user))
        .route("/users/{name}", axum::routing::put(update_user).delete(delete_user))
        .route("/users/{name}/reset-password", post(reset_password))
}

pub(super) const ACCESS: &[(&str, &str, Access)] = &[
    ("GET", "/users", Access::SessionServerAdmin),
    ("POST", "/users", Access::SessionServerAdmin),
    ("PUT", "/users/{name}", Access::SessionServerAdmin),
    ("DELETE", "/users/{name}", Access::SessionServerAdmin),
    ("POST", "/users/{name}/reset-password", Access::SessionServerAdmin),
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
    /// A server admin.
    #[serde(default)]
    admin: bool,
    /// Workspaces to add the user to, with their role in each.
    #[serde(default)]
    workspaces: BTreeMap<String, Role>,
    /// Links the account to the sign-in provider on its first sign-in there.
    #[serde(default)]
    email: Option<String>,
}

/// A checked email, or None for an empty one; refused if another user has it, since
/// sign-in links an account by its email.
fn check_email(users: &[User], email: Option<&str>, except: Option<&str>) -> ApiResult<Option<String>> {
    let Some(e) = email.map(str::trim).filter(|e| !e.is_empty()) else { return Ok(None) };
    let e = crate::model::normalize_email(e).map_err(bad)?;
    if let Some(other) = users.iter().find(|u| u.email.as_deref() == Some(e.as_str()) && Some(u.name.as_str()) != except) {
        return Err(ApiError::new(StatusCode::CONFLICT, format!("{} already has the email {e}.", other.name)));
    }
    Ok(Some(e))
}

/// A generated password and its hash.
async fn generated_password() -> ApiResult<(String, String)> {
    let password = random_password(GENERATED_PASSWORD_LENGTH);
    let p = password.clone();
    let hash = tokio::task::spawn_blocking(move || auth::hash_password(&p)).await.map_err(|e| anyhow::anyhow!("{e}"))??;
    Ok((password, hash))
}

/// "server admin", or "editor in ops, viewer in payments".
fn describe(u: &User) -> String {
    let mut parts: Vec<String> = Vec::new();
    if u.admin {
        parts.push("server admin".into());
    }
    parts.extend(u.workspaces.iter().map(|(w, r)| format!("{} in {w}", r.as_str())));
    if parts.is_empty() { "no workspaces yet".into() } else { parts.join(", ") }
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
    for slug in input.workspaces.keys() {
        if st.store.workspace(slug)?.is_none() {
            return Err(bad(format!("There is no workspace {slug:?}.")));
        }
    }
    let email = check_email(&users, input.email.as_deref(), None)?;
    let (password, password_hash) = generated_password().await?;
    let user = User { name, admin: input.admin, workspaces: input.workspaces, created_at: now_ms(), must_change_password: true, last_sign_in_at: None, password_changed_at: None, email, oidc: None };
    st.store.put_user(&StoredUser { user: user.clone(), password_hash })?;
    tracing::info!(user = %user.name, admin = user.admin, by = %me.user, "user created");
    audit::record(&st, &By::user(&me), None, "user.create", Some(AuditTarget::new("user", &user.name, &user.name)), Some(describe(&user)));
    for (slug, role) in &user.workspaces {
        audit::record(&st, &By::user(&me), Some(slug), "member.add", Some(AuditTarget::new("user", &user.name, &user.name)), Some(format!("role {}", role.as_str())));
    }
    // The only time the password is shown.
    Ok((StatusCode::CREATED, Json(json!({ "user": user, "password": password }))).into_response())
}

/// Server admins left if `name` stopped being one.
fn other_server_admins(users: &[User], name: &str) -> usize {
    users.iter().filter(|u| u.admin && u.name != name).count()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UserUpdate {
    #[serde(default)]
    admin: Option<bool>,
    /// A new email; empty removes it.
    #[serde(default)]
    email: Option<String>,
    /// Forget the provider account the user signs in with, so the next sign-in links again.
    #[serde(default)]
    unlink_sign_in: bool,
}

/// Make a user a server admin or not, set their email, or unlink their provider account.
/// Everything is checked before anything changes.
async fn update_user(State(st): State<Arc<AppState>>, Extension(me): Extension<CurrentUser>, Path(name): Path<String>, body: String) -> ApiResult<Json<User>> {
    let input: UserUpdate = serde_json::from_str(&body).map_err(|e| bad(format!("Invalid change: {e}")))?;
    let mut stored = st.store.user(&name)?.ok_or_else(|| ApiError::not_found("User"))?;
    let users = st.store.users()?;
    let admin = input.admin.filter(|a| *a != stored.user.admin);
    if admin.is_some() {
        if name == me.user {
            return Err(ApiError::new(StatusCode::CONFLICT, "You can't change whether you're a server admin. Ask another server admin."));
        }
        if stored.user.admin && other_server_admins(&users, &name) == 0 {
            return Err(ApiError::new(StatusCode::CONFLICT, "There must always be at least one server admin."));
        }
    }
    let email = match &input.email {
        Some(e) => Some(check_email(&users, Some(e), Some(&name))?),
        None => None,
    };
    let by = By::user(&me);
    let target = || Some(AuditTarget::new("user", &name, &name));
    if let Some(email) = email.filter(|e| *e != stored.user.email) {
        let detail = match &email {
            Some(e) => format!("email {e}"),
            None => "email removed".into(),
        };
        audit::record(&st, &by, None, "user.email_change", target(), Some(detail));
        stored.user.email = email;
    }
    if input.unlink_sign_in && stored.user.oidc.take().is_some() {
        audit::record(&st, &by, None, "user.sign_in_unlink", target(), None);
    }
    if let Some(admin) = admin {
        let what = if admin { "made a server admin" } else { "no longer a server admin" };
        tracing::info!(user = %name, admin, by = %me.user, "server admin changed");
        audit::record(&st, &by, None, "user.admin_change", target(), Some(what.into()));
        stored.user.admin = admin;
    }
    st.store.put_user(&stored)?;
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
    audit::record(&st, &By::user(&me), None, "user.password_reset", Some(AuditTarget::new("user", &name, &name)), Some("signed out; must choose a new password".into()));
    Ok(Json(json!({ "user": stored.user, "password": password })))
}

async fn delete_user(State(st): State<Arc<AppState>>, Extension(me): Extension<CurrentUser>, Path(name): Path<String>) -> ApiResult<StatusCode> {
    if name == me.user {
        return Err(ApiError::new(StatusCode::CONFLICT, "You can't remove yourself. Ask another server admin."));
    }
    let stored = st.store.user(&name)?.ok_or_else(|| ApiError::not_found("User"))?;
    if stored.user.admin && other_server_admins(&st.store.users()?, &name) == 0 {
        return Err(ApiError::new(StatusCode::CONFLICT, "There must always be at least one server admin."));
    }
    st.store.delete_user(&name)?;
    tracing::info!(user = %name, by = %me.user, "user removed, with their sessions and API tokens");
    audit::record(&st, &By::user(&me), None, "user.delete", Some(AuditTarget::new("user", &name, &name)), Some(format!("was {}; sessions and API tokens ended", describe(&stored.user))));
    Ok(StatusCode::NO_CONTENT)
}
