//! Inventory sources of a workspace, and its tag rules. A workspace's Admins manage
//! static lists and URLs; program sources run code on the server, so only server admins
//! add or change them.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Extension, Json, Router};
use serde_json::json;

use crate::audit::{self, By};
use crate::model::{
    AuditTarget, InventorySource, MAX_SOURCES, Role, SourceInput, SourceKind, SourceState, TagRule, check_source_fields, now_ms, validate_interval, validate_source_name, validate_tag_rules,
};
use crate::web::AppState;
use super::{Access, ApiError, ApiResult, CurrentUser, InWorkspace, Scope, bad};

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/inventory/sources", get(list_sources).post(create_source))
        .route("/inventory/sources/{id}", get(get_source).put(update_source).delete(delete_source))
        .route("/inventory/sources/{id}/refresh", post(refresh_source))
        .route("/workspaces/{slug}/tag-rules", put(set_tag_rules))
}

pub(super) const ACCESS: &[(&str, &str, Access)] = &[
    ("GET", "/inventory/sources", Access::Role(Role::Admin, Scope::Query)),
    ("POST", "/inventory/sources", Access::Role(Role::Admin, Scope::Query)),
    ("GET", "/inventory/sources/{id}", Access::Role(Role::Admin, Scope::Source)),
    ("PUT", "/inventory/sources/{id}", Access::Role(Role::Admin, Scope::Source)),
    ("DELETE", "/inventory/sources/{id}", Access::Role(Role::Admin, Scope::Source)),
    ("POST", "/inventory/sources/{id}/refresh", Access::Role(Role::Admin, Scope::Source)),
    ("PUT", "/workspaces/{slug}/tag-rules", Access::Role(Role::Admin, Scope::Slug)),
];

/// A source with what its last refreshes found; with `hosts`, the hosts too.
fn view(src: InventorySource, state: SourceState, hosts: bool) -> serde_json::Value {
    let mut v = json!(src);
    v["state"] = json!({
        "last_attempt": state.last_attempt,
        "last_success": state.last_success,
        "error": state.error,
        "host_count": state.hosts.len(),
    });
    if hosts {
        v["hosts"] = json!(state.hosts);
    }
    v
}

/// Program sources run code on the server: only server admins add, change or remove them.
fn check_program(user: &CurrentUser, kind: &SourceKind) -> ApiResult<()> {
    if matches!(kind, SourceKind::Program { .. }) && !user.admin {
        return Err(ApiError::new(StatusCode::FORBIDDEN, "A program source runs code on the server, so only server admins can set one up."));
    }
    Ok(())
}

/// Secrets the source names must be there for its workspace.
fn check_secrets(st: &AppState, ws: &str, kind: &SourceKind) -> ApiResult<()> {
    let refs: Vec<&String> = match kind {
        SourceKind::Url { secret: Some(s), .. } => vec![s],
        SourceKind::Program { secrets, .. } => secrets.iter().collect(),
        _ => vec![],
    };
    for r in refs {
        if let Err(why) = crate::secret_refs::check(&st.store, ws, r)? {
            return Err(bad(why));
        }
    }
    Ok(())
}

async fn list_sources(State(st): State<Arc<AppState>>, Extension(InWorkspace(ws)): Extension<InWorkspace>) -> ApiResult<Json<Vec<serde_json::Value>>> {
    let mut out = Vec::new();
    for s in st.store.inventory_sources()?.into_iter().filter(|s| s.workspace == ws) {
        let state = st.store.source_state(&s.id)?;
        out.push(view(s, state, false));
    }
    Ok(Json(out))
}

async fn get_source(State(st): State<Arc<AppState>>, Path(id): Path<String>) -> ApiResult<Json<serde_json::Value>> {
    let s = st.store.inventory_source(&id)?.ok_or_else(|| ApiError::not_found("Inventory source"))?;
    let state = st.store.source_state(&id)?;
    Ok(Json(view(s, state, true)))
}

/// Refresh now, on a blocking thread, and return the source with what it found.
async fn refresh_now(st: &Arc<AppState>, src: InventorySource, by: By, accept_empty: bool) -> ApiResult<serde_json::Value> {
    let st2 = st.clone();
    let s2 = src.clone();
    let state = tokio::task::spawn_blocking(move || crate::inventory::refresh(&st2, &s2, by, accept_empty)).await.map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(view(src, state, true))
}

/// Parse a source from the API, refusing unknown fields.
fn parse_input(body: &str) -> ApiResult<SourceInput> {
    let v: serde_json::Value = serde_json::from_str(body).map_err(|e| bad(format!("Invalid inventory source: {e}")))?;
    check_source_fields(&v).map_err(bad)?;
    serde_json::from_value(v).map_err(|e| bad(format!("Invalid inventory source: {e}")))
}

async fn create_source(State(st): State<Arc<AppState>>, Extension(user): Extension<CurrentUser>, Extension(InWorkspace(ws)): Extension<InWorkspace>, body: String) -> ApiResult<Response> {
    let mut input = parse_input(&body)?;
    check_program(&user, &input.kind)?;
    let name = validate_source_name(&input.name).map_err(bad)?;
    input.kind.validate().map_err(bad)?;
    check_secrets(&st, &ws, &input.kind)?;
    let interval = validate_interval(input.interval_seconds.unwrap_or(crate::model::DEFAULT_INTERVAL)).map_err(bad)?;
    let existing = st.store.inventory_sources()?;
    if existing.iter().filter(|s| s.workspace == ws).count() >= MAX_SOURCES {
        return Err(ApiError::new(StatusCode::CONFLICT, format!("A workspace can have at most {MAX_SOURCES} inventory sources.")));
    }
    if existing.iter().any(|s| s.workspace == ws && s.name.eq_ignore_ascii_case(&name)) {
        return Err(ApiError::new(StatusCode::CONFLICT, format!("There's already a source called {name}.")));
    }
    let now = now_ms();
    let src = InventorySource {
        id: uuid::Uuid::now_v7().to_string(),
        workspace: ws.clone(),
        name,
        kind: input.kind,
        interval_seconds: interval,
        enabled: input.enabled.unwrap_or(true),
        created_at: now,
        updated_at: now,
        updated_by: Some(user.user.clone()),
    };
    st.store.put_inventory_source(&src)?;
    audit::record(&st, &By::user(&user), Some(&ws), "inventory_source.create", Some(AuditTarget::new("inventory_source", &src.id, &src.name)), Some(src.kind.label().into()));
    // The first refresh right away, so the hosts show up now.
    let v = if src.enabled { refresh_now(&st, src, By::user(&user), false).await? } else { view(src, SourceState::default(), true) };
    Ok((StatusCode::CREATED, Json(v)).into_response())
}

async fn update_source(State(st): State<Arc<AppState>>, Extension(user): Extension<CurrentUser>, Path(id): Path<String>, body: String) -> ApiResult<Json<serde_json::Value>> {
    let mut src = st.store.inventory_source(&id)?.ok_or_else(|| ApiError::not_found("Inventory source"))?;
    let mut input = parse_input(&body)?;
    check_program(&user, &src.kind)?;
    check_program(&user, &input.kind)?;
    let name = validate_source_name(&input.name).map_err(bad)?;
    input.kind.validate().map_err(bad)?;
    check_secrets(&st, &src.workspace, &input.kind)?;
    if st.store.inventory_sources()?.iter().any(|s| s.id != id && s.workspace == src.workspace && s.name.eq_ignore_ascii_case(&name)) {
        return Err(ApiError::new(StatusCode::CONFLICT, format!("There's already a source called {name}.")));
    }
    src.name = name;
    src.kind = input.kind;
    if let Some(i) = input.interval_seconds {
        src.interval_seconds = validate_interval(i).map_err(bad)?;
    }
    if let Some(e) = input.enabled {
        src.enabled = e;
    }
    src.updated_at = now_ms();
    src.updated_by = Some(user.user.clone());
    st.store.put_inventory_source(&src)?;
    audit::record(&st, &By::user(&user), Some(&src.workspace), "inventory_source.update", Some(AuditTarget::new("inventory_source", &id, &src.name)), Some(if src.enabled { "on" } else { "off" }.into()));
    let v = if src.enabled { refresh_now(&st, src, By::user(&user), false).await? } else { view(src, st.store.source_state(&id)?, true) };
    Ok(Json(v))
}

async fn delete_source(State(st): State<Arc<AppState>>, Extension(user): Extension<CurrentUser>, Path(id): Path<String>) -> ApiResult<StatusCode> {
    let src = st.store.inventory_source(&id)?.ok_or_else(|| ApiError::not_found("Inventory source"))?;
    check_program(&user, &src.kind)?;
    st.store.delete_inventory_source(&id)?;
    audit::record(&st, &By::user(&user), Some(&src.workspace), "inventory_source.delete", Some(AuditTarget::new("inventory_source", &id, &src.name)), None);
    Ok(StatusCode::NO_CONTENT)
}

#[derive(serde::Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RefreshInput {
    /// Take an empty list after the source listed hosts, instead of keeping them.
    #[serde(default)]
    accept_empty: bool,
}

async fn refresh_source(State(st): State<Arc<AppState>>, Extension(user): Extension<CurrentUser>, Path(id): Path<String>, body: String) -> ApiResult<Json<serde_json::Value>> {
    let src = st.store.inventory_source(&id)?.ok_or_else(|| ApiError::not_found("Inventory source"))?;
    let input: RefreshInput = if body.trim().is_empty() { RefreshInput::default() } else { serde_json::from_str(&body).map_err(|e| bad(format!("Invalid request: {e}")))? };
    Ok(Json(refresh_now(&st, src, By::user(&user), input.accept_empty).await?))
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct TagRulesInput {
    rules: Vec<TagRule>,
}

async fn set_tag_rules(State(st): State<Arc<AppState>>, Extension(user): Extension<CurrentUser>, Path(slug): Path<String>, body: String) -> ApiResult<Json<serde_json::Value>> {
    let mut input: TagRulesInput = serde_json::from_str(&body).map_err(|e| bad(format!("Invalid tag rules: {e}")))?;
    validate_tag_rules(&mut input.rules).map_err(bad)?;
    let mut ws = st.store.workspace(&slug)?.ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "No such workspace."))?;
    ws.tag_rules = input.rules;
    st.store.put_workspace(&ws)?;
    let detail = ws.tag_rules.iter().map(|r| format!("{} -> {}", r.filter, r.tags.join(", "))).collect::<Vec<_>>().join("; ");
    audit::record(&st, &By::user(&user), Some(&slug), "workspace.tag_rules", Some(AuditTarget::new("workspace", &slug, &ws.name)), Some(if detail.is_empty() { "no rules".into() } else { detail }));
    Ok(Json(json!({ "rules": ws.tag_rules })))
}
