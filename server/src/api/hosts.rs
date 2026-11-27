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
use crate::web::AppState;
use super::{Access, ApiError, ApiResult, CurrentUser, bad};

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/hosts", get(list_hosts))
        .route("/hosts/packages", get(host_packages))
        .route("/hosts/enrollment-tokens", post(create_enrollment_token))
        .route("/hosts/{id}", axum::routing::put(update_host).delete(delete_host))
}

pub(super) const ACCESS: &[(&str, &str, Access)] = &[
    ("GET", "/hosts", Access::Role(Role::Viewer)),
    ("GET", "/hosts/packages", Access::Role(Role::Viewer)),
    ("POST", "/hosts/enrollment-tokens", Access::Role(Role::Admin)),
    ("PUT", "/hosts/{id}", Access::Role(Role::Admin)),
    ("DELETE", "/hosts/{id}", Access::Role(Role::Admin)),
];

#[derive(serde::Serialize)]
struct HostView {
    #[serde(flatten)]
    host: Host,
    online: bool,
    /// The OpSkills server itself: always there, can't be edited or removed.
    builtin: bool,
    used_by: Vec<serde_json::Value>,
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
        enrolled_at: started,
        last_seen: now_ms(),
    }
}

fn host_views(st: &AppState) -> ApiResult<Vec<HostView>> {
    let runbooks = st.store.runbooks()?;
    let used = |h: &Host| runbooks.iter().filter(|r| targets_host(r, h)).map(|r| json!({ "id": r.id, "name": r.name })).collect();
    let local = local_host(st);
    let mut list = vec![HostView { used_by: used(&local), host: local, online: true, builtin: true }];
    for mut h in st.store.hosts()? {
        h.last_seen = h.last_seen.max(st.hub.last_seen(&h.id).unwrap_or(0));
        list.push(HostView { online: st.hub.is_online(&h.id, h.last_seen), used_by: used(&h), host: h, builtin: false });
    }
    Ok(list)
}

#[derive(Deserialize)]
struct HostsQuery {
    /// Only enrolled hosts this filter expression matches.
    filter: Option<String>,
}

async fn list_hosts(State(st): State<Arc<AppState>>, Query(q): Query<HostsQuery>) -> ApiResult<Json<Vec<HostView>>> {
    let views = host_views(&st)?;
    let Some(text) = q.filter.filter(|f| !f.trim().is_empty()) else { return Ok(Json(views)) };
    let filter = HostFilter::parse(&text).map_err(|msg| ApiError::new(StatusCode::BAD_REQUEST, msg))?;
    Ok(Json(views.into_iter().filter(|v| !v.builtin && filter.matches(&v.host)).collect()))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HostUpdate {
    name: Option<String>,
    tags: Option<Vec<String>>,
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

async fn update_host(State(st): State<Arc<AppState>>, Path(id): Path<String>, body: String) -> ApiResult<Json<HostView>> {
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
    st.store.put_host(&stored)?;
    host_views(&st)?.into_iter().find(|v| v.host.id == id).map(Json).ok_or_else(|| ApiError::not_found("Host"))
}

async fn delete_host(State(st): State<Arc<AppState>>, Path(id): Path<String>) -> ApiResult<StatusCode> {
    if id == LOCAL_HOST_ID {
        return Err(ApiError::new(StatusCode::CONFLICT, "The OpSkills server can't be removed."));
    }
    let users: Vec<String> = st.store.runbooks()?.into_iter().filter(|r| r.targets.hosts.contains(&id)).map(|r| r.name).collect();
    if !users.is_empty() {
        return Err(ApiError::new(StatusCode::CONFLICT, format!("This host is a target of: {}. Remove it from those runbooks first.", users.join(", "))));
    }
    if st.store.delete_host(&id)? {
        st.hub.forget(&id);
        tracing::info!(host_id = %id, "host removed");
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found("Host"))
    }
}

const ENROLLMENT_HOURS: u64 = 24;

async fn create_enrollment_token(State(st): State<Arc<AppState>>, Extension(user): Extension<CurrentUser>) -> ApiResult<Response> {
    st.store.purge_expired_enrollment_tokens()?;
    let token = auth::new_token();
    let now = now_ms();
    let t = EnrollmentToken { created_at: now, expires_at: now + ENROLLMENT_HOURS * 3_600_000, created_by: user.label() };
    st.store.put_enrollment_token(&auth::token_hash(&token), &t)?;
    tracing::info!("enrollment token created");
    Ok((StatusCode::CREATED, Json(json!({ "token": token, "expires_at": t.expires_at }))).into_response())
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
