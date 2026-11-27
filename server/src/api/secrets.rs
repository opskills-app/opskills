//! Vault secrets. Values are write-only: no response ever contains one.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Json, Router};
use serde_json::json;

use crate::model::{
    DEFAULT_WORKSPACE, Secret, SecretInput, SecretUpdate, StoredSecret, now_ms, secret_binding, validate_secret_description, validate_secret_kind, validate_secret_name, validate_secret_value,
};
use crate::model::Role;
use crate::audit::{self, By};
use crate::model::AuditTarget;
use crate::web::AppState;
use super::{Access, ApiError, ApiResult, CurrentUser, InWorkspace, Scope, bad};

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/secrets", get(list_secrets).post(create_secret))
        .route("/secrets/{name}", axum::routing::put(update_secret).delete(delete_secret))
}

pub(super) const ACCESS: &[(&str, &str, Access)] = &[
    ("GET", "/secrets", Access::Role(Role::Editor, Scope::Query)),
    ("POST", "/secrets", Access::Role(Role::Editor, Scope::Query)),
    ("PUT", "/secrets/{name}", Access::Role(Role::Editor, Scope::Query)),
    ("DELETE", "/secrets/{name}", Access::Role(Role::Editor, Scope::Query)),
];

#[derive(serde::Serialize)]
struct SecretView {
    #[serde(flatten)]
    meta: Secret,
    used_by: Vec<serde_json::Value>,
}

/// The secrets of workspace `ws`, each with the runbooks there that use it.
fn views(st: &AppState, ws: &str) -> ApiResult<Vec<SecretView>> {
    let runbooks: Vec<_> = st.store.runbooks()?.into_iter().filter(|r| r.workspace == ws).collect();
    let mut list: Vec<SecretView> = st
        .store
        .secrets(ws)?
        .into_iter()
        .map(|meta| {
            let used_by = runbooks.iter().filter(|r| r.secrets.contains(&meta.name)).map(|r| json!({ "id": r.id, "name": r.name })).collect();
            SecretView { meta, used_by }
        })
        .collect();
    list.sort_by(|a, b| a.meta.name.cmp(&b.meta.name));
    Ok(list)
}

async fn list_secrets(State(st): State<Arc<AppState>>, Extension(InWorkspace(ws)): Extension<InWorkspace>) -> ApiResult<Json<Vec<SecretView>>> {
    Ok(Json(views(&st, &ws)?))
}

async fn create_secret(State(st): State<Arc<AppState>>, Extension(user): Extension<CurrentUser>, Extension(InWorkspace(ws)): Extension<InWorkspace>, body: String) -> ApiResult<Response> {
    let input: SecretInput = serde_json::from_str(&body).map_err(|e| bad(format!("Invalid secret: {e}")))?;
    let name = input.name.trim().to_string();
    let kind = input.kind.unwrap_or_else(|| "other".into());
    validate_secret_name(&name).map_err(bad)?;
    validate_secret_kind(&kind).map_err(bad)?;
    validate_secret_value(&input.value).map_err(bad)?;
    let description = input.description.trim().to_string();
    validate_secret_description(&description).map_err(bad)?;
    if st.store.secret(&ws, &name)?.is_some() {
        return Err(ApiError::new(StatusCode::CONFLICT, format!("A secret named {name} already exists. Replace its value instead.")));
    }
    let sealed = st.vault.seal(&secret_binding(&ws, &name), input.value.as_bytes())?;
    let now = now_ms();
    let meta = Secret { name: name.clone(), workspace: ws.clone(), kind, description, created_at: now, updated_at: now };
    st.store.put_secret(&StoredSecret { meta: meta.clone(), nonce: sealed.nonce, ciphertext: sealed.ciphertext })?;
    tracing::info!(secret = %name, "secret added");
    audit::record(&st, &By::user(&user), Some(&ws), "secret.create", Some(AuditTarget::new("secret", &name, &name)), None);
    Ok((StatusCode::CREATED, Json(SecretView { meta, used_by: vec![] })).into_response())
}

async fn update_secret(State(st): State<Arc<AppState>>, Extension(user): Extension<CurrentUser>, Extension(InWorkspace(ws)): Extension<InWorkspace>, Path(name): Path<String>, body: String) -> ApiResult<Json<SecretView>> {
    let mut stored = st.store.secret(&ws, &name)?.ok_or_else(|| ApiError::not_found("Secret"))?;
    let update: SecretUpdate = serde_json::from_str(&body).map_err(|e| bad(format!("Invalid update: {e}")))?;
    if let Some(kind) = update.kind {
        validate_secret_kind(&kind).map_err(bad)?;
        stored.meta.kind = kind;
    }
    if let Some(description) = update.description {
        let description = description.trim().to_string();
        validate_secret_description(&description).map_err(bad)?;
        stored.meta.description = description;
    }
    let replaced = update.value.is_some();
    if let Some(value) = update.value {
        validate_secret_value(&value).map_err(bad)?;
        let sealed = st.vault.seal(&secret_binding(&ws, &name), value.as_bytes())?;
        stored.nonce = sealed.nonce;
        stored.ciphertext = sealed.ciphertext;
        tracing::info!(secret = %name, "secret value replaced");
    }
    stored.meta.updated_at = now_ms();
    st.store.put_secret(&stored)?;
    let what = if replaced { "value replaced" } else { "details changed" };
    audit::record(&st, &By::user(&user), Some(&ws), "secret.update", Some(AuditTarget::new("secret", &name, &name)), Some(what.into()));
    views(&st, &ws)?.into_iter().find(|v| v.meta.name == name).map(Json).ok_or_else(|| ApiError::not_found("Secret"))
}

async fn delete_secret(State(st): State<Arc<AppState>>, Extension(user): Extension<CurrentUser>, Extension(InWorkspace(ws)): Extension<InWorkspace>, Path(name): Path<String>) -> ApiResult<StatusCode> {
    // The email password is a secret of the default workspace.
    if ws == DEFAULT_WORKSPACE && st.store.email_settings()?.is_some_and(|e| e.password_secret.as_deref() == Some(name.as_str())) {
        return Err(ApiError::new(StatusCode::CONFLICT, format!("{name} is the email password. Change it under Settings > Email first.")));
    }
    let users: Vec<String> = st
        .store
        .runbooks()?
        .into_iter()
        .filter(|r| r.workspace == ws)
        .filter(|r| r.secrets.contains(&name) || r.notifications.iter().any(|n| n.secret_refs().contains(&name)))
        .map(|r| r.name)
        .collect();
    if !users.is_empty() {
        return Err(ApiError::new(StatusCode::CONFLICT, format!("{name} is used by: {}. Remove it from those runbooks first.", users.join(", "))));
    }
    if st.store.delete_secret(&ws, &name)? {
        tracing::info!(secret = %name, "secret deleted");
        audit::record(&st, &By::user(&user), Some(&ws), "secret.delete", Some(AuditTarget::new("secret", &name, &name)), None);
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found("Secret"))
    }
}
