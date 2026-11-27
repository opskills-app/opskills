//! Shared secrets, for server admins: one value that runbooks of chosen workspaces (or
//! all) use as `shared/<path>`. Values are write-only, as in every vault.

use std::collections::BTreeSet;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Json, Router};
use serde_json::json;

use crate::audit::{self, By};
use crate::model::{
    AuditTarget, SHARED_PREFIX, SharedSecret, SharedSecretInput, SharedSecretUpdate, StoredSharedSecret, now_ms, shared_secret_binding, validate_secret_description, validate_secret_kind,
    validate_secret_path, validate_secret_value,
};
use crate::vault::Sealed;
use crate::web::AppState;
use super::{Access, ApiError, ApiResult, CurrentUser, bad};

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/shared-secrets", get(list_shared).post(create_shared))
        .route("/shared-secrets/{*name}", axum::routing::put(update_shared).delete(delete_shared))
}

pub(super) const ACCESS: &[(&str, &str, Access)] = &[
    ("GET", "/shared-secrets", Access::ServerAdmin),
    ("POST", "/shared-secrets", Access::ServerAdmin),
    ("PUT", "/shared-secrets/{*name}", Access::ServerAdmin),
    ("DELETE", "/shared-secrets/{*name}", Access::ServerAdmin),
];

#[derive(serde::Serialize)]
struct SharedView {
    #[serde(flatten)]
    meta: SharedSecret,
    /// Runbooks that use it, in any workspace.
    used_by: Vec<serde_json::Value>,
    /// It's the email password (Settings > Email).
    email_password: bool,
}

/// Who uses `shared/<name>`: runbooks in any workspace, and the email settings.
struct Users {
    runbooks: Vec<(String, String, String)>,
    email: bool,
}

impl Users {
    fn of(st: &AppState, name: &str) -> ApiResult<Users> {
        let reference = format!("{SHARED_PREFIX}{name}");
        let runbooks = st.store.runbooks()?.into_iter().filter(|r| r.secret_paths().contains(&reference)).map(|r| (r.id, r.name, r.workspace)).collect();
        let email = st.store.email_settings()?.is_some_and(|e| e.password_secret.as_deref() == Some(reference.as_str()));
        Ok(Users { runbooks, email })
    }

    fn describe(&self) -> Vec<String> {
        let mut out: Vec<String> = self.runbooks.iter().map(|(_, name, ws)| format!("{name} (in {ws})")).collect();
        if self.email {
            out.push("Settings > Email".into());
        }
        out
    }
}

fn view(st: &AppState, meta: SharedSecret) -> ApiResult<SharedView> {
    let users = Users::of(st, &meta.name)?;
    let used_by = users.runbooks.iter().map(|(id, name, ws)| json!({ "id": id, "name": name, "workspace": ws })).collect();
    Ok(SharedView { meta, used_by, email_password: users.email })
}

/// Check and tidy who a shared secret is shared with.
fn sharing(st: &AppState, all: bool, workspaces: Vec<String>) -> ApiResult<Vec<String>> {
    if all {
        return Ok(vec![]);
    }
    let mut out = BTreeSet::new();
    for w in workspaces {
        let w = w.trim().to_string();
        if st.store.workspace(&w)?.is_none() {
            return Err(bad(format!("There's no workspace {w:?}.")));
        }
        out.insert(w);
    }
    Ok(out.into_iter().collect())
}

fn sharing_text(s: &SharedSecret) -> String {
    match (s.all_workspaces, s.workspaces.is_empty()) {
        (true, _) => "shared with all workspaces".into(),
        (false, true) => "not shared with any workspace yet".into(),
        (false, false) => format!("shared with {}", s.workspaces.join(", ")),
    }
}

async fn list_shared(State(st): State<Arc<AppState>>) -> ApiResult<Json<Vec<SharedView>>> {
    Ok(Json(st.store.shared_secrets()?.into_iter().map(|m| view(&st, m)).collect::<ApiResult<_>>()?))
}

async fn create_shared(State(st): State<Arc<AppState>>, Extension(user): Extension<CurrentUser>, body: String) -> ApiResult<Response> {
    let input: SharedSecretInput = serde_json::from_str(&body).map_err(|e| bad(format!("Invalid shared secret: {e}")))?;
    let name = input.name.trim().to_string();
    let kind = input.kind.unwrap_or_else(|| "other".into());
    validate_secret_path(&name).map_err(bad)?;
    validate_secret_kind(&kind).map_err(bad)?;
    validate_secret_value(&input.value).map_err(bad)?;
    let description = input.description.trim().to_string();
    validate_secret_description(&description).map_err(bad)?;
    let workspaces = sharing(&st, input.all_workspaces, input.workspaces)?;
    if st.store.shared_secret(&name)?.is_some() {
        return Err(ApiError::new(StatusCode::CONFLICT, format!("A shared secret named {name} already exists. Replace its value instead.")));
    }
    let sealed = st.vault.seal(&shared_secret_binding(&name), input.value.as_bytes())?;
    let now = now_ms();
    let meta = SharedSecret { name: name.clone(), kind, description, all_workspaces: input.all_workspaces, workspaces, created_at: now, updated_at: now, updated_by: Some(user.user.clone()) };
    st.store.put_shared_secret(&StoredSharedSecret { meta: meta.clone(), nonce: sealed.nonce, ciphertext: sealed.ciphertext })?;
    tracing::info!(secret = %name, "shared secret added");
    audit::record(&st, &By::user(&user), None, "shared_secret.create", Some(AuditTarget::new("shared_secret", &name, &name)), Some(sharing_text(&meta)));
    Ok((StatusCode::CREATED, Json(view(&st, meta)?)).into_response())
}

async fn update_shared(State(st): State<Arc<AppState>>, Extension(user): Extension<CurrentUser>, Path(name): Path<String>, body: String) -> ApiResult<Json<SharedView>> {
    let mut stored = st.store.shared_secret(&name)?.ok_or_else(|| ApiError::not_found("Shared secret"))?;
    let update: SharedSecretUpdate = serde_json::from_str(&body).map_err(|e| bad(format!("Invalid update: {e}")))?;
    let before = stored.meta.clone();
    let users = Users::of(&st, &name)?;

    let new_name = update.name.map(|n| n.trim().to_string()).filter(|n| *n != name);
    let mut moved_value = None;
    if let Some(to) = &new_name {
        validate_secret_path(to).map_err(bad)?;
        if st.store.shared_secret(to)?.is_some() {
            return Err(ApiError::new(StatusCode::CONFLICT, format!("A shared secret named {to} already exists.")));
        }
        if !users.describe().is_empty() {
            return Err(ApiError::new(StatusCode::CONFLICT, format!("{name} is used by: {}. A secret in use can't be moved; change those first.", users.describe().join(", "))));
        }
        moved_value = Some(st.vault.open(&shared_secret_binding(&name), &Sealed { nonce: stored.nonce.clone(), ciphertext: stored.ciphertext.clone() })?);
        stored.meta.name = to.clone();
    }

    // Sharing: a workspace whose runbooks use the secret can't lose it.
    let all = update.all_workspaces.unwrap_or(before.all_workspaces);
    let listed = update.workspaces.unwrap_or_else(|| before.workspaces.clone());
    stored.meta.workspaces = sharing(&st, all, listed)?;
    stored.meta.all_workspaces = all;
    let losing: Vec<String> = st.store.workspaces()?.into_iter().map(|w| w.slug).filter(|w| before.available_to(w) && !stored.meta.available_to(w)).collect();
    let blocked: Vec<String> = users.runbooks.iter().filter(|(_, _, ws)| losing.contains(ws)).map(|(_, rb, ws)| format!("{rb} (in {ws})")).collect();
    if !blocked.is_empty() {
        return Err(ApiError::new(StatusCode::CONFLICT, format!("{name} is used by: {}. Change those runbooks before taking the secret away from their workspace.", blocked.join(", "))));
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
    if let Some(plaintext) = update.value.map(String::into_bytes).or(moved_value) {
        let sealed = st.vault.seal(&shared_secret_binding(&stored.meta.name), &plaintext)?;
        stored.nonce = sealed.nonce;
        stored.ciphertext = sealed.ciphertext;
    }
    stored.meta.updated_at = now_ms();
    stored.meta.updated_by = Some(user.user.clone());
    let current = stored.meta.name.clone();
    let by = By::user(&user);
    let target = || Some(AuditTarget::new("shared_secret", &current, &current));
    match &new_name {
        Some(_) => {
            st.store.move_shared_secret(&name, &stored)?;
            tracing::info!(from = %name, to = %current, "shared secret moved");
            audit::record(&st, &by, None, "shared_secret.move", target(), Some(format!("from {name}")));
        }
        None => st.store.put_shared_secret(&stored)?,
    }
    let gained: Vec<String> = st.store.workspaces()?.into_iter().map(|w| w.slug).filter(|w| !before.available_to(w) && stored.meta.available_to(w)).collect();
    if (all && !before.all_workspaces) || !gained.is_empty() {
        let detail = if all { "with all workspaces".to_string() } else { format!("with {}", gained.join(", ")) };
        audit::record(&st, &by, None, "shared_secret.share", target(), Some(detail));
    }
    if (!all && before.all_workspaces) || !losing.is_empty() {
        let detail = if before.all_workspaces && !all { format!("no longer with all workspaces; {}", sharing_text(&stored.meta)) } else { format!("from {}", losing.join(", ")) };
        audit::record(&st, &by, None, "shared_secret.unshare", target(), Some(detail));
    }
    if replaced || details {
        let what = if replaced { "value replaced" } else { "details changed" };
        audit::record(&st, &by, None, "shared_secret.update", target(), Some(what.into()));
    }
    Ok(Json(view(&st, stored.meta)?))
}

async fn delete_shared(State(st): State<Arc<AppState>>, Extension(user): Extension<CurrentUser>, Path(name): Path<String>) -> ApiResult<StatusCode> {
    let users = Users::of(&st, &name)?.describe();
    if !users.is_empty() {
        return Err(ApiError::new(StatusCode::CONFLICT, format!("{name} is used by: {}. Remove it from those first.", users.join(", "))));
    }
    if st.store.delete_shared_secret(&name)? {
        tracing::info!(secret = %name, "shared secret deleted");
        audit::record(&st, &By::user(&user), None, "shared_secret.delete", Some(AuditTarget::new("shared_secret", &name, &name)), None);
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found("Shared secret"))
    }
}
