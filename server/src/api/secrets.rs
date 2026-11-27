//! Vault secrets of a workspace, by path (`prod/db/DB_PASSWORD`). Values are write-only:
//! no response ever contains one. The list also shows the shared secrets the workspace
//! can use, which server admins manage under `/shared-secrets`.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Json, Router};
use serde_json::json;

use crate::audit::{self, By};
use crate::model::{
    AuditTarget, DEFAULT_WORKSPACE, Role, SHARED_PREFIX, Secret, SecretInput, SecretUpdate, StoredSecret, now_ms, secret_binding, validate_secret_description, validate_secret_kind, validate_secret_value,
    validate_workspace_secret_path,
};
use crate::vault::Sealed;
use crate::web::AppState;
use super::{Access, ApiError, ApiResult, CurrentUser, InWorkspace, Scope, bad};

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/secrets", get(list_secrets).post(create_secret))
        .route("/secrets/{*name}", axum::routing::put(update_secret).delete(delete_secret))
}

pub(super) const ACCESS: &[(&str, &str, Access)] = &[
    ("GET", "/secrets", Access::Role(Role::Editor, Scope::Query)),
    ("POST", "/secrets", Access::Role(Role::Editor, Scope::Query)),
    ("PUT", "/secrets/{*name}", Access::Role(Role::Editor, Scope::Query)),
    ("DELETE", "/secrets/{*name}", Access::Role(Role::Editor, Scope::Query)),
];

#[derive(serde::Serialize)]
struct SecretView {
    /// The full name: `prod/db/DB_PASSWORD`, or `shared/<path>` for a shared secret.
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    workspace: Option<String>,
    kind: String,
    description: String,
    created_at: u64,
    updated_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    updated_by: Option<String>,
    /// A shared secret: usable here, managed by server admins.
    shared: bool,
    used_by: Vec<serde_json::Value>,
}

/// The runbooks of `ws` that use `reference`, with their secret paths worked out once.
struct Usage(Vec<(String, String, Vec<String>)>);

impl Usage {
    fn of(st: &AppState, ws: &str) -> ApiResult<Usage> {
        Ok(Usage(st.store.runbooks()?.into_iter().filter(|r| r.workspace == ws).map(|r| { let p = r.secret_paths(); (r.id, r.name, p) }).collect()))
    }

    fn runbooks(&self, reference: &str) -> Vec<serde_json::Value> {
        self.0.iter().filter(|(_, _, p)| p.iter().any(|x| x == reference)).map(|(id, name, _)| json!({ "id": id, "name": name })).collect()
    }
}

fn workspace_view(meta: Secret, usage: &Usage) -> SecretView {
    let used_by = usage.runbooks(&meta.name);
    SecretView { name: meta.name, workspace: Some(meta.workspace), kind: meta.kind, description: meta.description, created_at: meta.created_at, updated_at: meta.updated_at, updated_by: meta.updated_by, shared: false, used_by }
}

/// The secrets of workspace `ws` and the shared secrets it can use, each with the
/// runbooks here that use it.
fn views(st: &AppState, ws: &str) -> ApiResult<Vec<SecretView>> {
    let usage = Usage::of(st, ws)?;
    let mut list: Vec<SecretView> = st.store.secrets(ws)?.into_iter().map(|meta| workspace_view(meta, &usage)).collect();
    for s in st.store.shared_secrets()?.into_iter().filter(|s| s.available_to(ws)) {
        let name = format!("{SHARED_PREFIX}{}", s.name);
        let used_by = usage.runbooks(&name);
        list.push(SecretView { name, workspace: None, kind: s.kind, description: s.description, created_at: s.created_at, updated_at: s.updated_at, updated_by: s.updated_by, shared: true, used_by });
    }
    list.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(list)
}

/// What stops a secret of `ws` from being moved or deleted: runbooks using it, or the
/// email settings (whose password can be a secret of the default workspace).
fn users_of(st: &AppState, ws: &str, name: &str) -> ApiResult<Vec<String>> {
    let mut users: Vec<String> = Usage::of(st, ws)?.runbooks(name).iter().filter_map(|r| r["name"].as_str().map(str::to_string)).collect();
    if ws == DEFAULT_WORKSPACE && st.store.email_settings()?.is_some_and(|e| e.password_secret.as_deref() == Some(name)) {
        users.push("Settings > Email".into());
    }
    Ok(users)
}

async fn list_secrets(State(st): State<Arc<AppState>>, Extension(InWorkspace(ws)): Extension<InWorkspace>) -> ApiResult<Json<Vec<SecretView>>> {
    Ok(Json(views(&st, &ws)?))
}

async fn create_secret(State(st): State<Arc<AppState>>, Extension(user): Extension<CurrentUser>, Extension(InWorkspace(ws)): Extension<InWorkspace>, body: String) -> ApiResult<Response> {
    let input: SecretInput = serde_json::from_str(&body).map_err(|e| bad(format!("Invalid secret: {e}")))?;
    let name = input.name.trim().to_string();
    let kind = input.kind.unwrap_or_else(|| "other".into());
    validate_workspace_secret_path(&name).map_err(bad)?;
    validate_secret_kind(&kind).map_err(bad)?;
    validate_secret_value(&input.value).map_err(bad)?;
    let description = input.description.trim().to_string();
    validate_secret_description(&description).map_err(bad)?;
    if st.store.secret(&ws, &name)?.is_some() {
        return Err(ApiError::new(StatusCode::CONFLICT, format!("A secret named {name} already exists. Replace its value instead.")));
    }
    let sealed = st.vault.seal(&secret_binding(&ws, &name), input.value.as_bytes())?;
    let now = now_ms();
    let meta = Secret { name: name.clone(), workspace: ws.clone(), kind, description, created_at: now, updated_at: now, updated_by: Some(user.user.clone()) };
    st.store.put_secret(&StoredSecret { meta: meta.clone(), nonce: sealed.nonce, ciphertext: sealed.ciphertext })?;
    tracing::info!(secret = %name, "secret added");
    audit::record(&st, &By::user(&user), Some(&ws), "secret.create", Some(AuditTarget::new("secret", &name, &name)), None);
    Ok((StatusCode::CREATED, Json(workspace_view(meta, &Usage(vec![])))).into_response())
}

async fn update_secret(State(st): State<Arc<AppState>>, Extension(user): Extension<CurrentUser>, Extension(InWorkspace(ws)): Extension<InWorkspace>, Path(name): Path<String>, body: String) -> ApiResult<Json<SecretView>> {
    if name.starts_with(SHARED_PREFIX) {
        return Err(ApiError::new(StatusCode::CONFLICT, "Shared secrets are managed by server admins, under Vault > Shared."));
    }
    let mut stored = st.store.secret(&ws, &name)?.ok_or_else(|| ApiError::not_found("Secret"))?;
    let update: SecretUpdate = serde_json::from_str(&body).map_err(|e| bad(format!("Invalid update: {e}")))?;
    // A move re-seals the value under the new name, so check it before changing anything.
    let new_name = update.name.map(|n| n.trim().to_string()).filter(|n| *n != name);
    let mut moved_value = None;
    if let Some(to) = &new_name {
        validate_workspace_secret_path(to).map_err(bad)?;
        if st.store.secret(&ws, to)?.is_some() {
            return Err(ApiError::new(StatusCode::CONFLICT, format!("A secret named {to} already exists.")));
        }
        let users = users_of(&st, &ws, &name)?;
        if !users.is_empty() {
            return Err(ApiError::new(StatusCode::CONFLICT, format!("{name} is used by: {}. A secret in use can't be moved; change those first.", users.join(", "))));
        }
        moved_value = Some(st.vault.open(&secret_binding(&ws, &name), &Sealed { nonce: stored.nonce.clone(), ciphertext: stored.ciphertext.clone() })?);
        stored.meta.name = to.clone();
    }
    let mut details = false;
    if let Some(kind) = update.kind {
        validate_secret_kind(&kind).map_err(bad)?;
        details |= kind != stored.meta.kind;
        stored.meta.kind = kind;
    }
    if let Some(description) = update.description {
        let description = description.trim().to_string();
        validate_secret_description(&description).map_err(bad)?;
        details |= description != stored.meta.description;
        stored.meta.description = description;
    }
    let replaced = update.value.is_some();
    if let Some(value) = &update.value {
        validate_secret_value(value).map_err(bad)?;
    }
    let plaintext = update.value.map(String::into_bytes).or(moved_value);
    if let Some(plaintext) = plaintext {
        let sealed = st.vault.seal(&secret_binding(&ws, &stored.meta.name), &plaintext)?;
        stored.nonce = sealed.nonce;
        stored.ciphertext = sealed.ciphertext;
    }
    stored.meta.updated_at = now_ms();
    stored.meta.updated_by = Some(user.user.clone());
    let current = stored.meta.name.clone();
    match &new_name {
        Some(_) => {
            st.store.move_secret(&name, &stored)?;
            tracing::info!(from = %name, to = %current, "secret moved");
            audit::record(&st, &By::user(&user), Some(&ws), "secret.move", Some(AuditTarget::new("secret", &current, &current)), Some(format!("from {name}")));
        }
        None => st.store.put_secret(&stored)?,
    }
    if replaced || details {
        if replaced {
            tracing::info!(secret = %current, "secret value replaced");
        }
        let what = if replaced { "value replaced" } else { "details changed" };
        audit::record(&st, &By::user(&user), Some(&ws), "secret.update", Some(AuditTarget::new("secret", &current, &current)), Some(what.into()));
    }
    views(&st, &ws)?.into_iter().find(|v| !v.shared && v.name == current).map(Json).ok_or_else(|| ApiError::not_found("Secret"))
}

async fn delete_secret(State(st): State<Arc<AppState>>, Extension(user): Extension<CurrentUser>, Extension(InWorkspace(ws)): Extension<InWorkspace>, Path(name): Path<String>) -> ApiResult<StatusCode> {
    if name.starts_with(SHARED_PREFIX) {
        return Err(ApiError::new(StatusCode::CONFLICT, "Shared secrets are managed by server admins, under Vault > Shared."));
    }
    let users = users_of(&st, &ws, &name)?;
    if !users.is_empty() {
        return Err(ApiError::new(StatusCode::CONFLICT, format!("{name} is used by: {}. Remove it from those first.", users.join(", "))));
    }
    if st.store.delete_secret(&ws, &name)? {
        tracing::info!(secret = %name, "secret deleted");
        audit::record(&st, &By::user(&user), Some(&ws), "secret.delete", Some(AuditTarget::new("secret", &name, &name)), None);
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found("Secret"))
    }
}
