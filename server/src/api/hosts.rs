//! Hosts, enrollment tokens and agent packages.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use serde::Deserialize;
use serde_json::json;

use crate::auth;
use crate::filter::HostFilter;
use crate::model::{
    EnrollmentToken, Host, LOCAL_HOST_ID, LOCAL_HOST_NAME, Runbook, normalize_tag, now_ms,
};
use crate::model::Role;
use crate::audit::{self, By};
use crate::model::AuditTarget;
use crate::web::AppState;
use super::{Access, ApiError, ApiResult, CurrentUser, InWorkspace, Scope, bad};

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/hosts", get(list_hosts))
        .route("/hosts/packages", get(host_packages))
        .route("/hosts/enrollment-tokens", post(create_enrollment_token))
        .route("/hosts/{id}", axum::routing::put(update_host).delete(delete_host))
        .route("/hosts/{id}/workspaces/{slug}", axum::routing::put(add_to_workspace).delete(remove_from_workspace))
}

pub(super) const ACCESS: &[(&str, &str, Access)] = &[
    ("GET", "/hosts", Access::Role(Role::Viewer, Scope::Query)),
    ("GET", "/hosts/packages", Access::User),
    ("POST", "/hosts/enrollment-tokens", Access::Role(Role::Admin, Scope::Query)),
    ("PUT", "/hosts/{id}", Access::Role(Role::Admin, Scope::Host)),
    ("DELETE", "/hosts/{id}", Access::Role(Role::Admin, Scope::Host)),
    ("PUT", "/hosts/{id}/workspaces/{slug}", Access::Role(Role::Admin, Scope::HostAndSlug)),
    ("DELETE", "/hosts/{id}/workspaces/{slug}", Access::Role(Role::Admin, Scope::Slug)),
];

#[derive(serde::Serialize)]
struct HostView {
    /// `host.workspaces` lists only the workspaces the caller belongs to.
    #[serde(flatten)]
    host: Host,
    online: bool,
    /// The OpSkills server itself: in every workspace, can't be edited or removed.
    builtin: bool,
    /// The host is also in workspaces the caller doesn't belong to.
    shared_elsewhere: bool,
    /// Runbooks of the current workspace that run on it.
    used_by: Vec<serde_json::Value>,
    /// How runs reach it: `agent`, or `listed` (only an inventory source lists it).
    reach: crate::inventory::Reach,
    /// Inventory sources that list it.
    sources: Vec<serde_json::Value>,
    /// Tags it has from the workspace's tag rules (also in `tags`).
    rule_tags: Vec<String>,
    /// A source that lists it failed its last refresh.
    stale: bool,
    /// What an Admin set on an enrolled host, without what sources and rules add.
    own_tags: Vec<String>,
    own_attributes: std::collections::BTreeMap<String, String>,
    /// For a host reached over SSH: where, how, its pinned keys and its last connection.
    #[serde(skip_serializing_if = "Option::is_none")]
    ssh: Option<serde_json::Value>,
}

fn targets_host(rb: &Runbook, host: &Host) -> bool {
    if host.id == LOCAL_HOST_ID {
        return rb.targets.is_server() || rb.targets.hosts.iter().any(|h| h == LOCAL_HOST_ID);
    }
    rb.targets.includes(host)
}

fn local_host(st: &AppState) -> Host {
    let info = crate::agent::machine_info();
    let started = st.started_unix * 1000;
    Host {
        id: LOCAL_HOST_ID.into(),
        name: LOCAL_HOST_NAME.into(),
        hostname: info.hostname,
        os: info.os,
        arch: info.arch,
        agent_version: crate::VERSION.into(),
        tags: vec![],
        workspaces: vec![],
        enrolled_at: started,
        last_seen: now_ms(),
        attributes: Default::default(),
    }
}

/// The hosts of workspace `ws` (the server first), as `viewer` may see them.
fn host_views(st: &AppState, viewer: &CurrentUser, ws: &str) -> ApiResult<Vec<HostView>> {
    let me = st.store.user(&viewer.user)?.map(|u| u.user);
    let visible = |w: &String| me.as_ref().is_some_and(|u| u.role_in(w).is_some());
    let runbooks: Vec<Runbook> = st.store.runbooks()?.into_iter().filter(|r| r.workspace == ws).collect();
    let used = |h: &Host| runbooks.iter().filter(|r| targets_host(r, h)).map(|r| json!({ "id": r.id, "name": r.name })).collect();
    let mut local = local_host(st);
    local.workspaces = vec![ws.to_string()];
    let mut list = vec![HostView {
        used_by: used(&local),
        host: local,
        online: true,
        builtin: true,
        shared_elsewhere: false,
        reach: crate::inventory::Reach::Agent,
        sources: vec![],
        rule_tags: vec![],
        stale: false,
        own_tags: vec![],
        own_attributes: Default::default(),
        ssh: None,
    }];
    let own: std::collections::HashMap<String, Host> = st.store.hosts()?.into_iter().map(|h| (h.id.clone(), h)).collect();
    for inv in crate::inventory::workspace_hosts(st, ws)? {
        let mut h = inv.host;
        let enrolled = inv.reach == crate::inventory::Reach::Agent;
        if enrolled {
            h.last_seen = h.last_seen.max(st.hub.last_seen(&h.id).unwrap_or(0));
        }
        let shared_elsewhere = h.workspaces.iter().any(|w| !visible(w));
        let used_by = used(&h);
        h.workspaces.retain(|w| visible(w));
        let (own_tags, own_attributes) = own.get(&h.id).map(|o| (o.tags.clone(), o.attributes.clone())).unwrap_or_default();
        let mut online = enrolled && st.hub.is_online(&h.id, h.last_seen);
        let ssh = match &inv.ssh {
            Some(t) => {
                let s = st.store.ssh_host(&h.id)?;
                // "Online" for an SSH host: its last connection worked.
                online = s.last_ok.is_some() && s.last_error.is_none();
                h.last_seen = s.last_ok.unwrap_or(0);
                Some(json!({
                    "address": t.address,
                    "port": t.port,
                    "user": t.user,
                    "auth": t.auth,
                    "sudo": t.sudo && t.user != "root",
                    "fingerprints": s.fingerprints(),
                    "pinned_at": s.pinned_at,
                    "pinned_by": s.pinned_by,
                    "key_changed_at": s.key_changed_at,
                    "last_ok": s.last_ok,
                    "last_error": s.last_error,
                }))
            }
            None => None,
        };
        list.push(HostView {
            online,
            used_by,
            host: h,
            builtin: false,
            shared_elsewhere,
            reach: inv.reach,
            sources: inv.sources.into_iter().map(|(id, name)| json!({ "id": id, "name": name })).collect(),
            rule_tags: inv.rule_tags,
            stale: inv.stale,
            own_tags,
            own_attributes,
            ssh,
        });
    }
    Ok(list)
}

/// Runbooks in `workspaces` that name the host `id` as a target, as "name (workspace)".
fn runbooks_targeting(st: &AppState, id: &str, workspaces: &[String]) -> ApiResult<Vec<String>> {
    Ok(st
        .store
        .runbooks()?
        .into_iter()
        .filter(|r| workspaces.contains(&r.workspace) && r.targets.hosts.iter().any(|h| h == id))
        .map(|r| if workspaces.len() > 1 { format!("{} ({})", r.name, r.workspace) } else { r.name })
        .collect())
}

#[derive(Deserialize)]
struct HostsQuery {
    /// Only enrolled hosts this filter expression matches.
    filter: Option<String>,
}

async fn list_hosts(State(st): State<Arc<AppState>>, Extension(user): Extension<CurrentUser>, Extension(InWorkspace(ws)): Extension<InWorkspace>, Query(q): Query<HostsQuery>) -> ApiResult<Json<Vec<HostView>>> {
    let views = host_views(&st, &user, &ws)?;
    let Some(text) = q.filter.filter(|f| !f.trim().is_empty()) else { return Ok(Json(views)) };
    let filter = HostFilter::parse(&text).map_err(|msg| ApiError::new(StatusCode::BAD_REQUEST, msg))?;
    Ok(Json(views.into_iter().filter(|v| !v.builtin && filter.matches(&v.host)).collect()))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HostUpdate {
    name: Option<String>,
    tags: Option<Vec<String>>,
    /// Attributes like env or region; replaces the ones an Admin set before.
    #[serde(default)]
    attributes: Option<std::collections::BTreeMap<String, String>>,
}

pub(super) fn validate_host_name(name: &str) -> Result<String, String> {
    let name = name.trim().to_string();
    if name.is_empty() || name.chars().count() > 80 || name.chars().any(char::is_control) {
        return Err("A host name needs 1 to 80 characters.".into());
    }
    if name.eq_ignore_ascii_case(LOCAL_HOST_NAME) {
        return Err(format!("{LOCAL_HOST_NAME} is reserved for the server itself."));
    }
    Ok(name)
}

pub(super) fn clean_tags(tags: &[String]) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    for t in tags {
        if let Some(t) = normalize_tag(t)?
            && !out.contains(&t) {
                out.push(t);
            }
    }
    if out.len() > 20 {
        return Err("A host can have at most 20 tags.".into());
    }
    Ok(out)
}

async fn update_host(State(st): State<Arc<AppState>>, Extension(user): Extension<CurrentUser>, Path(id): Path<String>, body: String) -> ApiResult<Json<HostView>> {
    if id == LOCAL_HOST_ID {
        return Err(ApiError::new(StatusCode::CONFLICT, "The OpSkills server can't be renamed or tagged."));
    }
    let mut stored = st.store.stored_host(&id)?.ok_or_else(|| ApiError::not_found("Host"))?;
    let update: HostUpdate = serde_json::from_str(&body).map_err(|e| bad(format!("Invalid update: {e}")))?;
    if let Some(name) = update.name {
        let name = validate_host_name(&name).map_err(bad)?;
        if st.store.hosts()?.iter().any(|h| h.id != id && h.name.eq_ignore_ascii_case(&name)) {
            return Err(ApiError::new(StatusCode::CONFLICT, format!("Another host is already called {name}.")));
        }
        stored.host.name = name;
    }
    if let Some(tags) = update.tags {
        stored.host.tags = clean_tags(&tags).map_err(bad)?;
    }
    if let Some(mut attrs) = update.attributes {
        crate::model::validate_attributes(&mut attrs).map_err(bad)?;
        stored.host.attributes = attrs;
    }
    st.store.put_host(&stored)?;
    let attrs: Vec<String> = stored.host.attributes.iter().map(|(k, v)| format!("{k}={v}")).collect();
    let detail = format!("name {}, tags [{}]{}", stored.host.name, stored.host.tags.join(", "), if attrs.is_empty() { String::new() } else { format!(", attributes {}", attrs.join(" ")) });
    // A shared host changes in each of its workspaces.
    for ws in &stored.host.workspaces {
        audit::record(&st, &By::user(&user), Some(ws), "host.update", Some(AuditTarget::new("host", &id, &stored.host.name)), Some(detail.clone()));
    }
    let first = stored.host.workspaces.first().cloned().unwrap_or_default();
    host_views(&st, &user, &first)?.into_iter().find(|v| v.host.id == id).map(Json).ok_or_else(|| ApiError::not_found("Host"))
}

async fn delete_host(State(st): State<Arc<AppState>>, Extension(user): Extension<CurrentUser>, Path(id): Path<String>) -> ApiResult<StatusCode> {
    if id == LOCAL_HOST_ID {
        return Err(ApiError::new(StatusCode::CONFLICT, "The OpSkills server can't be removed."));
    }
    let host = st.store.stored_host(&id)?.ok_or_else(|| ApiError::not_found("Host"))?.host;
    let users = runbooks_targeting(&st, &id, &host.workspaces)?;
    if !users.is_empty() {
        return Err(ApiError::new(StatusCode::CONFLICT, format!("This host is a target of: {}. Remove it from those runbooks first.", users.join(", "))));
    }
    if st.store.delete_host(&id)? {
        st.hub.forget(&id);
        for ws in &host.workspaces {
            audit::record(&st, &By::user(&user), Some(ws), "host.delete", Some(AuditTarget::new("host", &id, &host.name)), None);
        }
        tracing::info!(host_id = %id, "host removed");
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found("Host"))
    }
}

const ENROLLMENT_HOURS: u64 = 24;

/// A one-time token; the host that uses it joins the workspace it was made in.
async fn create_enrollment_token(State(st): State<Arc<AppState>>, Extension(user): Extension<CurrentUser>, Extension(InWorkspace(ws)): Extension<InWorkspace>) -> ApiResult<Response> {
    st.store.purge_expired_enrollment_tokens()?;
    let token = auth::new_token();
    let now = now_ms();
    let t = EnrollmentToken { workspace: ws.clone(), created_at: now, expires_at: now + ENROLLMENT_HOURS * 3_600_000, created_by: user.label() };
    st.store.put_enrollment_token(&auth::token_hash(&token), &t)?;
    tracing::info!(workspace = %ws, "enrollment token created");
    audit::record(&st, &By::user(&user), Some(&ws), "host.enrollment_token", None, Some("valid once, for 24 hours".into()));
    Ok((StatusCode::CREATED, Json(json!({ "token": token, "expires_at": t.expires_at }))).into_response())
}

/// Share a host with another workspace. Needs Admin in the host's workspaces and in that one.
async fn add_to_workspace(State(st): State<Arc<AppState>>, Extension(user): Extension<CurrentUser>, Path((id, slug)): Path<(String, String)>) -> ApiResult<Json<HostView>> {
    let ws = st.store.workspace(&slug)?.ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "No such workspace."))?;
    let mut stored = st.store.stored_host(&id)?.ok_or_else(|| ApiError::not_found("Host"))?;
    if !stored.host.workspaces.contains(&ws.slug) {
        let before = stored.host.workspaces.join(", ");
        stored.host.workspaces.push(ws.slug.clone());
        st.store.put_host(&stored)?;
        tracing::info!(host_id = %id, workspace = %ws.slug, "host shared");
        audit::record(&st, &By::user(&user), Some(&ws.slug), "host.share", Some(AuditTarget::new("host", &id, &stored.host.name)), Some(format!("also in {before}")));
    }
    host_views(&st, &user, &ws.slug)?.into_iter().find(|v| v.host.id == id).map(Json).ok_or_else(|| ApiError::not_found("Host"))
}

/// Take a host out of one of its workspaces. Needs Admin in that workspace.
async fn remove_from_workspace(State(st): State<Arc<AppState>>, Extension(user): Extension<CurrentUser>, Path((id, slug)): Path<(String, String)>) -> ApiResult<StatusCode> {
    let mut stored = st.store.stored_host(&id)?.filter(|h| h.host.workspaces.contains(&slug)).ok_or_else(|| ApiError::not_found("Host"))?;
    if stored.host.workspaces.len() == 1 {
        return Err(ApiError::new(StatusCode::CONFLICT, "This is the host's only workspace. Remove the host instead."));
    }
    let users = runbooks_targeting(&st, &id, std::slice::from_ref(&slug))?;
    if !users.is_empty() {
        return Err(ApiError::new(StatusCode::CONFLICT, format!("This host is a target of: {}. Remove it from those runbooks first.", users.join(", "))));
    }
    stored.host.workspaces.retain(|w| w != &slug);
    st.store.put_host(&stored)?;
    tracing::info!(host_id = %id, workspace = %slug, "host unshared");
    audit::record(&st, &By::user(&user), Some(&slug), "host.unshare", Some(AuditTarget::new("host", &id, &stored.host.name)), None);
    Ok(StatusCode::NO_CONTENT)
}

/// One agent package: where the server offers it and where it comes from.
#[derive(serde::Serialize)]
pub struct Package {
    pub format: &'static str,
    pub arch: &'static str,
    pub file: String,
    /// Served from this server's packages folder (else redirected to the GitHub release).
    pub bundled: bool,
}

pub fn packages(st: &AppState) -> Vec<Package> {
    let v = crate::VERSION;
    let mut out = Vec::new();
    for (arch, rpm_arch) in [("amd64", "x86_64"), ("arm64", "aarch64")] {
        for (format, file) in [("deb", format!("opskills-agent_{v}_{arch}.deb")), ("rpm", format!("opskills-agent-{v}-1.{rpm_arch}.rpm"))] {
            let bundled = st.cfg.hosts.packages_dir.join(&file).is_file();
            out.push(Package { format, arch, file, bundled });
        }
    }
    out
}

pub fn release_url(file: &str) -> String {
    format!("https://github.com/opskills-app/opskills/releases/download/v{}/{file}", crate::VERSION)
}

async fn host_packages(State(st): State<Arc<AppState>>) -> Json<serde_json::Value> {
    Json(json!({ "version": crate::VERSION, "packages": packages(&st) }))
}
