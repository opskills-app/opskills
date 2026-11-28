//! Runbooks: list, read, create, update and delete.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Json, Router};

use crate::model::{
    calls_of, DEFAULT_STEP_TIMEOUT, LOCAL_HOST_ID, Runbook, RunbookInput, now_ms,
};
use crate::model::Role;
use crate::audit::{self, By};
use crate::model::AuditTarget;
use crate::web::AppState;
use super::{Access, ApiError, ApiResult, CurrentUser, InWorkspace, Scope, bad};

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/runbooks", get(list_runbooks).post(create_runbook))
        .route("/runbooks/{id}", get(get_runbook).put(update_runbook).delete(delete_runbook))
}

pub(super) const ACCESS: &[(&str, &str, Access)] = &[
    ("GET", "/runbooks", Access::Role(Role::Viewer, Scope::Query)),
    ("POST", "/runbooks", Access::Role(Role::Editor, Scope::Query)),
    ("GET", "/runbooks/{id}", Access::Role(Role::Viewer, Scope::Runbook)),
    ("PUT", "/runbooks/{id}", Access::Role(Role::Editor, Scope::Runbook)),
    ("DELETE", "/runbooks/{id}", Access::Role(Role::Editor, Scope::Runbook)),
];

async fn list_runbooks(State(st): State<Arc<AppState>>, Extension(InWorkspace(ws)): Extension<InWorkspace>) -> ApiResult<Json<Vec<Runbook>>> {
    Ok(Json(st.store.runbooks()?.into_iter().filter(|r| r.workspace == ws).collect()))
}

async fn get_runbook(State(st): State<Arc<AppState>>, Path(id): Path<String>) -> ApiResult<Json<Runbook>> {
    st.store.runbook(&id)?.map(Json).ok_or_else(|| ApiError::not_found("Runbook"))
}

/// Parse and check a runbook for workspace `ws`: the secrets, hosts and runbooks it
/// names must all be in that workspace.
fn parse_input(st: &AppState, ws: &str, body: &str) -> ApiResult<RunbookInput> {
    let input: RunbookInput = serde_json::from_str(body).map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, format!("Invalid runbook: {e}")))?;
    let input = input.validate().map_err(|msg| ApiError::new(StatusCode::BAD_REQUEST, msg))?;
    // Every secret a reference can stand for must be there for this workspace, for
    // every choice of the inputs that pick its folders.
    for reference in &input.secrets {
        for path in crate::model::secret_ref_expansions(reference, &input.inputs).map_err(bad)? {
            if let Err(why) = crate::secret_refs::check(&st.store, ws, &path)? {
                return Err(bad(if path == *reference { why } else { format!("{reference} as {path}: {why}") }));
            }
        }
    }
    for (i, n) in input.notifications.iter().enumerate() {
        for name in n.secret_refs() {
            if let Err(why) = crate::secret_refs::check(&st.store, ws, &name)? {
                return Err(bad(format!("Notification {}: {why}", i + 1)));
            }
        }
    }
    // Enrolled hosts and hosts the workspace's inventory sources list.
    let known: Vec<String> = if input.targets.hosts.iter().any(|h| h != LOCAL_HOST_ID) {
        crate::inventory::workspace_hosts(st, ws)?.into_iter().map(|h| h.host.id).collect()
    } else {
        vec![]
    };
    for id in &input.targets.hosts {
        if id != LOCAL_HOST_ID && !known.contains(id) {
            return Err(ApiError::new(StatusCode::BAD_REQUEST, format!("There is no host with id {id}. It may have been removed.")));
        }
    }
    Ok(input)
}

/// Check the call steps of `input` (saved as `self_id`, if it exists yet) against the
/// stored runbooks: each called runbook exists, gets only inputs it has and every
/// required one it has no default for, and no chain of calls comes back around.
fn check_calls(st: &AppState, ws: &str, self_id: Option<&str>, input: &RunbookInput) -> ApiResult<()> {
    let bad = |m: String| ApiError::new(StatusCode::BAD_REQUEST, m);
    // Calls stay inside a workspace: runbooks elsewhere don't exist from here.
    let all: Vec<Runbook> = st.store.runbooks()?.into_iter().filter(|r| r.workspace == ws).collect();
    for (i, step) in input.steps.iter().enumerate() {
        let Some(call) = &step.call else { continue };
        let n = i + 1;
        if Some(call.runbook.as_str()) == self_id {
            return Err(bad(format!("Step {n}: a runbook can't call itself.")));
        }
        let target = all.iter().find(|r| r.id == call.runbook).ok_or_else(|| bad(format!("Step {n}: there is no runbook with id {}.", call.runbook)))?;
        if let Some(unknown) = call.inputs.keys().find(|k| !target.inputs.iter().any(|d| &d.name == *k)) {
            return Err(bad(format!("Step {n}: \"{}\" has no input named {unknown}.", target.name)));
        }
        if let Some(missing) = target.inputs.iter().find(|d| d.required && d.default.is_none() && !call.inputs.contains_key(&d.name)) {
            return Err(bad(format!("Step {n}: \"{}\" needs a value for its input {}.", target.name, missing.name)));
        }
    }
    // Follow calls from this runbook through the stored ones; reaching it again is a loop.
    if let Some(me) = self_id {
        let mut stack: Vec<String> = calls_of(&input.steps);
        let mut seen: Vec<String> = Vec::new();
        while let Some(id) = stack.pop() {
            if id == me {
                return Err(bad("These calls would loop back to this runbook. Remove the call that closes the loop.".into()));
            }
            if seen.contains(&id) {
                continue;
            }
            if let Some(rb) = all.iter().find(|r| r.id == id) {
                stack.extend(calls_of(&rb.steps));
            }
            seen.push(id);
        }
    }
    Ok(())
}

async fn create_runbook(State(st): State<Arc<AppState>>, Extension(user): Extension<CurrentUser>, Extension(InWorkspace(ws)): Extension<InWorkspace>, body: String) -> ApiResult<Response> {
    let input = parse_input(&st, &ws, &body)?;
    check_calls(&st, &ws, None, &input)?;
    let now = now_ms();
    let rb = Runbook {
        id: uuid::Uuid::now_v7().to_string(),
        workspace: ws,
        name: input.name,
        description: input.description,
        tags: input.tags,
        inputs: input.inputs,
        steps: input.steps,
        on_step_failure: input.on_step_failure,
        output_limit: input.output_limit.unwrap_or_default(),
        rollout: input.rollout,
        schedules: input.schedules,
        notifications: input.notifications,
        secrets: input.secrets,
        targets: input.targets,
        timeout_seconds: input.timeout_seconds.unwrap_or(DEFAULT_STEP_TIMEOUT),
        created_at: now,
        updated_at: now,
    };
    st.store.put_runbook(&rb)?;
    tracing::info!(runbook_id = %rb.id, name = %rb.name, "runbook created");
    audit::record(&st, &By::user(&user), Some(&rb.workspace), "runbook.create", Some(AuditTarget::new("runbook", &rb.id, &rb.name)), None);
    Ok((StatusCode::CREATED, Json(rb)).into_response())
}

async fn update_runbook(State(st): State<Arc<AppState>>, Extension(user): Extension<CurrentUser>, Path(id): Path<String>, body: String) -> ApiResult<Json<Runbook>> {
    let existing = st.store.runbook(&id)?.ok_or_else(|| ApiError::not_found("Runbook"))?;
    let input = parse_input(&st, &existing.workspace, &body)?;
    check_calls(&st, &existing.workspace, Some(&id), &input)?;
    let renamed = (existing.name != input.name).then(|| format!("renamed from {:?}", existing.name));
    let rb = Runbook {
        name: input.name,
        description: input.description,
        tags: input.tags,
        inputs: input.inputs,
        steps: input.steps,
        on_step_failure: input.on_step_failure,
        output_limit: input.output_limit.unwrap_or_default(),
        rollout: input.rollout,
        schedules: input.schedules,
        notifications: input.notifications,
        secrets: input.secrets,
        targets: input.targets,
        timeout_seconds: input.timeout_seconds.unwrap_or(existing.timeout_seconds),
        updated_at: now_ms(),
        ..existing
    };
    st.store.put_runbook(&rb)?;
    audit::record(&st, &By::user(&user), Some(&rb.workspace), "runbook.update", Some(AuditTarget::new("runbook", &rb.id, &rb.name)), renamed);
    Ok(Json(rb))
}

async fn delete_runbook(State(st): State<Arc<AppState>>, Extension(user): Extension<CurrentUser>, Path(id): Path<String>) -> ApiResult<StatusCode> {
    let callers: Vec<String> = st.store.runbooks()?.into_iter().filter(|r| r.id != id && calls_of(&r.steps).contains(&id)).map(|r| r.name).collect();
    if !callers.is_empty() {
        return Err(ApiError::new(StatusCode::CONFLICT, format!("This runbook is called by: {}. Remove those calls first.", callers.join(", "))));
    }
    let (name, ws) = st.store.runbook(&id)?.map(|r| (r.name, r.workspace)).unwrap_or_default();
    if st.store.delete_runbook(&id)? {
        for h in st.store.webhooks_of(&id)? {
            st.store.delete_webhook(&h.id)?;
        }
        audit::record(&st, &By::user(&user), Some(&ws), "runbook.delete", Some(AuditTarget::new("runbook", &id, &name)), None);
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found("Runbook"))
    }
}
