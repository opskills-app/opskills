//! Workspaces and their members. Server admins create and delete workspaces; a
//! workspace's Admins rename it and choose its members from the existing accounts.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Json, Router};
use serde::Deserialize;
use serde_json::json;

use crate::audit::{self, By};
use crate::model::{AuditTarget, DEFAULT_WORKSPACE, Role, Workspace, now_ms, validate_slug, validate_workspace_description, validate_workspace_name};
use crate::web::AppState;
use super::audit::{AuditQuery, audit_page};
use super::{Access, ApiError, ApiResult, CurrentUser, Scope, bad};

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/workspaces", get(list_workspaces).post(create_workspace))
        .route("/workspaces/{slug}", get(get_workspace).put(update_workspace).delete(delete_workspace))
        .route("/workspaces/{slug}/members", get(list_members))
        .route("/workspaces/{slug}/addable-users", get(addable_users))
        .route("/workspaces/{slug}/members/{name}", axum::routing::put(set_member).delete(remove_member))
        .route("/workspaces/{slug}/audit", get(workspace_audit))
}

pub(super) const ACCESS: &[(&str, &str, Access)] = &[
    ("GET", "/workspaces", Access::User),
    ("POST", "/workspaces", Access::ServerAdmin),
    ("GET", "/workspaces/{slug}", Access::Role(Role::Viewer, Scope::Slug)),
    ("PUT", "/workspaces/{slug}", Access::Role(Role::Admin, Scope::Slug)),
    ("DELETE", "/workspaces/{slug}", Access::ServerAdmin),
    ("GET", "/workspaces/{slug}/members", Access::Role(Role::Viewer, Scope::Slug)),
    ("GET", "/workspaces/{slug}/addable-users", Access::SessionRole(Role::Admin, Scope::Slug)),
    ("PUT", "/workspaces/{slug}/members/{name}", Access::SessionRole(Role::Admin, Scope::Slug)),
    ("DELETE", "/workspaces/{slug}/members/{name}", Access::SessionRole(Role::Admin, Scope::Slug)),
    ("GET", "/workspaces/{slug}/audit", Access::Role(Role::Admin, Scope::Slug)),
];

/// The workspaces the caller can see, with their role in each: all of them for server admins.
async fn list_workspaces(State(st): State<Arc<AppState>>, Extension(me): Extension<CurrentUser>) -> ApiResult<Json<Vec<serde_json::Value>>> {
    let user = st.store.user(&me.user)?.ok_or_else(|| ApiError::not_found("User"))?.user;
    Ok(Json(
        st.store
            .workspaces()?
            .into_iter()
            .filter_map(|w| user.role_in(&w.slug).map(|role| json!({ "slug": w.slug, "name": w.name, "description": w.description, "created_at": w.created_at, "role": role })))
            .collect(),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewWorkspace {
    slug: String,
    name: String,
    #[serde(default)]
    description: String,
}

async fn create_workspace(State(st): State<Arc<AppState>>, Extension(me): Extension<CurrentUser>, body: String) -> ApiResult<Response> {
    let input: NewWorkspace = serde_json::from_str(&body).map_err(|e| bad(format!("Invalid workspace: {e}")))?;
    let slug = validate_slug(&input.slug).map_err(bad)?;
    let name = validate_workspace_name(&input.name).map_err(bad)?;
    let description = validate_workspace_description(&input.description).map_err(bad)?;
    if st.store.workspace(&slug)?.is_some() {
        return Err(ApiError::new(StatusCode::CONFLICT, format!("There is already a workspace {slug:?}.")));
    }
    let ws = Workspace { slug, name, description, created_at: now_ms(), tag_rules: vec![] };
    st.store.put_workspace(&ws)?;
    tracing::info!(workspace = %ws.slug, by = %me.user, "workspace created");
    audit::record(&st, &By::user(&me), Some(&ws.slug), "workspace.create", Some(AuditTarget::new("workspace", &ws.slug, &ws.name)), None);
    Ok((StatusCode::CREATED, Json(ws)).into_response())
}

/// A workspace with how much is in it.
async fn get_workspace(State(st): State<Arc<AppState>>, Path(slug): Path<String>) -> ApiResult<Json<serde_json::Value>> {
    let ws = st.store.workspace(&slug)?.ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "No such workspace."))?;
    let runbooks = st.store.runbooks()?.iter().filter(|r| r.workspace == slug).count();
    let hosts = st.store.hosts()?.iter().filter(|h| h.workspaces.contains(&slug)).count();
    let members = st.store.users()?.iter().filter(|u| u.workspaces.contains_key(&slug)).count();
    Ok(Json(json!({ "workspace": ws, "runbooks": runbooks, "hosts": hosts, "members": members })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceUpdate {
    name: String,
    #[serde(default)]
    description: String,
}

async fn update_workspace(State(st): State<Arc<AppState>>, Extension(me): Extension<CurrentUser>, Path(slug): Path<String>, body: String) -> ApiResult<Json<Workspace>> {
    let input: WorkspaceUpdate = serde_json::from_str(&body).map_err(|e| bad(format!("Invalid workspace: {e}")))?;
    let mut ws = st.store.workspace(&slug)?.ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "No such workspace."))?;
    let name = validate_workspace_name(&input.name).map_err(bad)?;
    let description = validate_workspace_description(&input.description).map_err(bad)?;
    let renamed = (name != ws.name).then(|| format!("renamed from {:?}", ws.name));
    ws.name = name;
    ws.description = description;
    st.store.put_workspace(&ws)?;
    audit::record(&st, &By::user(&me), Some(&slug), "workspace.update", Some(AuditTarget::new("workspace", &slug, &ws.name)), renamed);
    Ok(Json(ws))
}

/// Delete an empty workspace: no runbooks, no secrets, no hosts only it uses. Its run
/// history goes with it; shared hosts and members just leave it.
async fn delete_workspace(State(st): State<Arc<AppState>>, Extension(me): Extension<CurrentUser>, Path(slug): Path<String>) -> ApiResult<StatusCode> {
    if slug == DEFAULT_WORKSPACE {
        return Err(ApiError::new(StatusCode::CONFLICT, "The Default workspace can't be deleted."));
    }
    let ws = st.store.workspace(&slug)?.ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "No such workspace."))?;
    let conflict = |m: String| ApiError::new(StatusCode::CONFLICT, m);
    let runbooks = st.store.runbooks()?.iter().filter(|r| r.workspace == slug).count();
    if runbooks > 0 {
        return Err(conflict(format!("{} still has {runbooks} runbook(s). Delete them first.", ws.name)));
    }
    let secrets = st.store.secrets(&slug)?.len();
    if secrets > 0 {
        return Err(conflict(format!("{} still has {secrets} secret(s) in its vault. Delete them first.", ws.name)));
    }
    let only_here: Vec<String> = st.store.hosts()?.into_iter().filter(|h| h.workspaces == [slug.clone()]).map(|h| h.name).collect();
    if !only_here.is_empty() {
        return Err(conflict(format!("These hosts are only in {}: {}. Remove them, or add them to another workspace first.", ws.name, only_here.join(", "))));
    }
    let runs = st.store.delete_workspace(&slug)?;
    tracing::info!(workspace = %slug, runs, by = %me.user, "workspace deleted");
    audit::record(&st, &By::user(&me), Some(&slug), "workspace.delete", Some(AuditTarget::new("workspace", &slug, &ws.name)), Some(format!("{runs} run(s) of history removed")));
    Ok(StatusCode::NO_CONTENT)
}

/// The members of a workspace and their roles. Server admins can act in every workspace
/// without being members.
async fn list_members(State(st): State<Arc<AppState>>, Path(slug): Path<String>) -> ApiResult<Json<serde_json::Value>> {
    let users = st.store.users()?;
    let members: Vec<serde_json::Value> = users.iter().filter_map(|u| u.workspaces.get(&slug).map(|role| json!({ "name": u.name, "role": role }))).collect();
    let server_admins: Vec<&str> = users.iter().filter(|u| u.admin).map(|u| u.name.as_str()).collect();
    Ok(Json(json!({ "members": members, "server_admins": server_admins })))
}

/// Accounts that aren't members yet, for an Admin choosing whom to add.
async fn addable_users(State(st): State<Arc<AppState>>, Path(slug): Path<String>) -> ApiResult<Json<Vec<String>>> {
    Ok(Json(st.store.users()?.into_iter().filter(|u| !u.workspaces.contains_key(&slug)).map(|u| u.name).collect()))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Membership {
    role: Role,
}

/// Add an existing user to the workspace, or change their role there.
async fn set_member(State(st): State<Arc<AppState>>, Extension(me): Extension<CurrentUser>, Path((slug, name)): Path<(String, String)>, body: String) -> ApiResult<Json<serde_json::Value>> {
    let input: Membership = serde_json::from_str(&body).map_err(|e| bad(format!("Invalid membership: {e}")))?;
    if name == me.user {
        return Err(ApiError::new(StatusCode::CONFLICT, "You can't change your own membership. Ask another Admin."));
    }
    let mut stored = st.store.user(&name)?.ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "No such user. A server admin creates accounts."))?;
    let before = stored.user.workspaces.insert(slug.clone(), input.role);
    if before != Some(input.role) {
        st.store.put_user(&stored)?;
        let (action, detail) = match before {
            Some(old) => ("member.role_change", format!("role {} to {}", old.as_str(), input.role.as_str())),
            None => ("member.add", format!("role {}", input.role.as_str())),
        };
        tracing::info!(workspace = %slug, user = %name, role = input.role.as_str(), by = %me.user, "{action}");
        audit::record(&st, &By::user(&me), Some(&slug), action, Some(AuditTarget::new("user", &name, &name)), Some(detail));
    }
    Ok(Json(json!({ "name": name, "role": input.role })))
}

async fn remove_member(State(st): State<Arc<AppState>>, Extension(me): Extension<CurrentUser>, Path((slug, name)): Path<(String, String)>) -> ApiResult<StatusCode> {
    if name == me.user {
        return Err(ApiError::new(StatusCode::CONFLICT, "You can't change your own membership. Ask another Admin."));
    }
    let mut stored = st.store.user(&name)?.filter(|u| u.user.workspaces.contains_key(&slug)).ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "Not a member of this workspace."))?;
    let role = stored.user.workspaces.remove(&slug).map(|r| r.as_str()).unwrap_or_default();
    st.store.put_user(&stored)?;
    tracing::info!(workspace = %slug, user = %name, by = %me.user, "member removed");
    audit::record(&st, &By::user(&me), Some(&slug), "member.remove", Some(AuditTarget::new("user", &name, &name)), Some(format!("was {role}")));
    Ok(StatusCode::NO_CONTENT)
}

/// The workspace's own audit events, for its Admins.
async fn workspace_audit(State(st): State<Arc<AppState>>, Path(slug): Path<String>, Query(q): Query<AuditQuery>) -> ApiResult<Json<serde_json::Value>> {
    audit_page(&st, &q, Some(&slug))
}
