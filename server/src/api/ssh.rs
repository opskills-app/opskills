//! Hosts reached over SSH: test a connection, see the keys a host presents, and accept a
//! changed host key. For the workspace's Admins.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use serde_json::json;

use crate::audit::{self, By};
use crate::inventory::{InvHost, Reach};
use crate::model::{AuditTarget, Role, fingerprint, now_ms};
use crate::ssh::Contact;
use crate::web::AppState;
use super::{Access, ApiError, ApiResult, CurrentUser, Scope, bad};

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/hosts/{id}/ssh/test", post(test_connection))
        .route("/hosts/{id}/ssh/keys", get(host_keys))
        .route("/hosts/{id}/ssh/accept", post(accept_key))
}

pub(super) const ACCESS: &[(&str, &str, Access)] = &[
    ("POST", "/hosts/{id}/ssh/test", Access::Role(Role::Admin, Scope::SourceHost)),
    ("GET", "/hosts/{id}/ssh/keys", Access::Role(Role::Admin, Scope::SourceHost)),
    ("POST", "/hosts/{id}/ssh/accept", Access::Role(Role::Admin, Scope::SourceHost)),
];

/// The SSH host `id` and its workspace.
fn ssh_host(st: &AppState, id: &str) -> ApiResult<(String, InvHost, crate::model::SshTarget)> {
    let ws = crate::inventory::source_host_workspace(st, id)?.ok_or_else(|| ApiError::not_found("Host"))?;
    let host = crate::inventory::workspace_hosts(st, &ws)?.into_iter().find(|h| h.host.id == id).ok_or_else(|| ApiError::not_found("Host"))?;
    match (&host.reach, host.ssh.clone()) {
        (Reach::Ssh, Some(t)) => Ok((ws, host, t)),
        _ => Err(ApiError::new(StatusCode::CONFLICT, format!("{} isn't reached over SSH. Give its inventory source SSH settings first.", host.host.name))),
    }
}

/// Connect and run a harmless command, pinning the host key on first contact.
async fn test_connection(State(st): State<Arc<AppState>>, Extension(user): Extension<CurrentUser>, Path(id): Path<String>) -> ApiResult<Json<serde_json::Value>> {
    let (ws, host, target) = ssh_host(&st, &id)?;
    let name = host.host.name.clone();
    let credential = crate::secret_refs::value(&st.store, &st.vault, &ws, &target.secret).map_err(|e| ApiError::new(StatusCode::CONFLICT, format!("The source's SSH secret: {e}.")))?;
    let (keys, pinned) = match crate::ssh::trusted_keys(&st.cfg.ssh, &st.store, &id, &target).await {
        Ok(k) => k,
        Err(e) => {
            st.store.update_ssh_host(&id, |s| s.last_error = Some(e.clone()))?;
            return Ok(Json(json!({ "ok": false, "error": format!("Could not reach {name} over SSH ({}): {e}.", target.label()) })));
        }
    };
    if let Some(fps) = &pinned {
        audit::record(&st, &By::user(&user), Some(&ws), "host.ssh_key_pin", Some(AuditTarget::new("host", &id, &name)), Some(fps.join(", ")));
    }
    let script = crate::ssh::remote_script(&crate::ssh::Step {
        run_id: &format!("test-{}", uuid::Uuid::now_v7()),
        number: 1,
        command: "echo \"Connected as $(id -un) on $(hostname 2>/dev/null || uname -n)\"",
        env: &[],
        last: true,
        keep_on_failure: false,
    });
    let (_never, mut cancel) = tokio::sync::watch::channel(false);
    let mut lines: Vec<String> = Vec::new();
    let (end, contact) = crate::ssh::run_step(&st.cfg.ssh, &st.cfg.data.dir.join("work"), &id, &target, &keys, &credential, &script, Duration::from_secs(60), &mut cancel, |_, l| {
        if lines.len() < 20 {
            lines.push(l.replace(credential.as_str(), crate::model::MASK));
        }
    })
    .await;
    let fps: Vec<String> = keys.iter().filter_map(|k| fingerprint(k)).collect();
    let ok = contact == Contact::Ran && end.exit_code == Some(0);
    let error = match &contact {
        Contact::Ran if ok => None,
        Contact::Ran => Some(format!("The test command exited with code {}.", end.exit_code.unwrap_or(-1))),
        Contact::KeyChanged => Some(format!("{name}'s SSH host key is no longer the one pinned for it ({}). Check why, then accept the new key if the change is expected.", fps.join(", "))),
        Contact::Failed(_) | Contact::Stopped => end.error.clone(),
    };
    let mut first_change = false;
    st.store.update_ssh_host(&id, |s| match &contact {
        Contact::Ran => {
            s.last_ok = Some(now_ms());
            s.last_error = None;
        }
        Contact::KeyChanged if s.keys == keys => {
            first_change = s.key_changed_at.is_none();
            s.key_changed_at.get_or_insert(now_ms());
            s.last_error = Some("The host key changed since it was pinned.".into());
        }
        Contact::Failed(why) => s.last_error = Some(why.clone()),
        _ => {}
    })?;
    if first_change {
        audit::record(&st, &By::user(&user), Some(&ws), "host.ssh_key_changed", Some(AuditTarget::new("host", &id, &name)), Some(format!("pinned {}", fps.join(", "))));
    }
    Ok(Json(json!({ "ok": ok, "output": lines, "error": error, "pinned_now": pinned, "fingerprints": fps })))
}

/// The keys pinned for the host and the ones it presents now.
async fn host_keys(State(st): State<Arc<AppState>>, Path(id): Path<String>) -> ApiResult<Json<serde_json::Value>> {
    let (_, _, target) = ssh_host(&st, &id)?;
    let pinned = st.store.ssh_host(&id)?.fingerprints();
    let presented = crate::ssh::keyscan(&st.cfg.ssh, &target.address, target.port).await.map_err(|e| ApiError::new(StatusCode::BAD_GATEWAY, format!("{e}.")))?;
    let presented: Vec<serde_json::Value> = presented
        .iter()
        .map(|k| json!({ "type": k.split_whitespace().next().unwrap_or(""), "fingerprint": fingerprint(k), "pinned": fingerprint(k).is_some_and(|f| pinned.contains(&f)) }))
        .collect();
    Ok(Json(json!({ "pinned": pinned, "presented": presented })))
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct AcceptInput {
    /// Fingerprints from `GET /hosts/{id}/ssh/keys` to pin; the host must present them now.
    fingerprints: Vec<String>,
}

/// Pin the keys the host presents now whose fingerprints the Admin confirmed.
async fn accept_key(State(st): State<Arc<AppState>>, Extension(user): Extension<CurrentUser>, Path(id): Path<String>, body: String) -> ApiResult<Json<serde_json::Value>> {
    let input: AcceptInput = serde_json::from_str(&body).map_err(|e| bad(format!("Invalid request: {e}")))?;
    if input.fingerprints.is_empty() {
        return Err(bad("Give the fingerprints to accept, from the keys the host presents.".into()));
    }
    let (ws, host, target) = ssh_host(&st, &id)?;
    let presented = crate::ssh::keyscan(&st.cfg.ssh, &target.address, target.port).await.map_err(|e| ApiError::new(StatusCode::BAD_GATEWAY, format!("{e}.")))?;
    let keep: Vec<String> = presented.into_iter().filter(|k| fingerprint(k).is_some_and(|f| input.fingerprints.contains(&f))).collect();
    if keep.is_empty() {
        return Err(ApiError::new(StatusCode::CONFLICT, "The host doesn't present a key with those fingerprints now. Look at its keys again."));
    }
    if !target.expected_keys.is_empty() && !keep.iter().any(|k| crate::model::expected(k, &target.expected_keys)) {
        return Err(ApiError::new(StatusCode::CONFLICT, "Its inventory source gives other host keys for it. Change them there first."));
    }
    let fps: Vec<String> = keep.iter().filter_map(|k| fingerprint(k)).collect();
    let before = st.store.ssh_host(&id)?.fingerprints();
    st.store.update_ssh_host(&id, |s| {
        s.keys = keep.clone();
        s.pinned_at = Some(now_ms());
        s.pinned_by = Some(user.user.clone());
        s.key_changed_at = None;
        s.last_error = None;
    })?;
    let detail = if before.is_empty() { fps.join(", ") } else { format!("{} (was {})", fps.join(", "), before.join(", ")) };
    audit::record(&st, &By::user(&user), Some(&ws), "host.ssh_key_accept", Some(AuditTarget::new("host", &id, &host.host.name)), Some(detail));
    Ok(Json(json!({ "pinned": fps })))
}
