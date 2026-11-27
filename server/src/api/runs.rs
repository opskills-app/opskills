//! Runs: start, list, follow live, retry and cancel.

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use futures_util::stream::{self, Stream, StreamExt};
use serde::Deserialize;
use serde_json::json;
use tokio_stream::wrappers::BroadcastStream;

use crate::executor::{Event, StartError};
use crate::model::{
    LOCAL_HOST_ID, LOCAL_HOST_NAME, Run, RunHost, Runbook,
};
use crate::model::Role;
use crate::audit::{self, By};
use crate::model::AuditTarget;
use crate::web::AppState;
use super::{Access, ApiError, ApiResult, CurrentUser};

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/runbooks/{id}/runs", post(start_run))
        .route("/runs", get(list_runs))
        .route("/runs/{id}", get(get_run))
        .route("/runs/{id}/logs", get(run_logs))
        .route("/runs/{id}/events", get(run_events))
        .route("/runs/{id}/cancel", post(cancel_run))
        .route("/runs/{id}/retry", post(retry_run))
}

pub(super) const ACCESS: &[(&str, &str, Access)] = &[
    ("POST", "/runbooks/{id}/runs", Access::Role(Role::Operator)),
    ("GET", "/runs", Access::Role(Role::Viewer)),
    ("GET", "/runs/{id}", Access::Role(Role::Viewer)),
    ("GET", "/runs/{id}/logs", Access::Role(Role::Viewer)),
    ("GET", "/runs/{id}/events", Access::Role(Role::Viewer)),
    ("POST", "/runs/{id}/cancel", Access::Role(Role::Operator)),
    ("POST", "/runs/{id}/retry", Access::Role(Role::Operator)),
];

/// Optional body of `POST /runbooks/{id}/runs`.
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct StartRunInput {
    #[serde(default)]
    inputs: serde_json::Map<String, serde_json::Value>,
}

async fn start_run(State(st): State<Arc<AppState>>, Extension(user): Extension<CurrentUser>, Path(id): Path<String>, body: String) -> ApiResult<Response> {
    let rb = st.store.runbook(&id)?.ok_or_else(|| ApiError::not_found("Runbook"))?;
    let start: StartRunInput = if body.trim().is_empty() {
        StartRunInput::default()
    } else {
        serde_json::from_str(&body).map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, format!("Invalid run request: {e}")))?
    };
    let run = start_runbook(&st, &rb, &start.inputs, &By::user(&user), "manual")?;
    Ok((StatusCode::CREATED, Json(run)).into_response())
}

/// Start a run of `rb` on its targets: check the inputs, decrypt its secrets (held only
/// in memory for the run), pick the hosts and hand it to the executor. Used by the
/// API, schedules and webhooks.
pub fn start_runbook(st: &AppState, rb: &Runbook, given: &serde_json::Map<String, serde_json::Value>, by: &By, trigger: &str) -> ApiResult<Run> {
    let values = st.executor.prepare(rb, given).map_err(|e| match e {
        StartError::BadInput(m) => ApiError::new(StatusCode::BAD_REQUEST, m),
        StartError::Conflict(m) => ApiError::new(StatusCode::CONFLICT, m),
        StartError::Internal(e) => e.into(),
    })?;
    let hosts = run_hosts(st, rb)?;
    let mut run = Run::new(rb, &by.label(), hosts, &values.inputs);
    run.trigger = trigger.into();
    let run = st.executor.start(run, values)?;
    audit::record(st, by, "run.start", Some(AuditTarget::new("run", &run.id, &rb.name)), Some(format!("{trigger} run")));
    Ok(run)
}

/// The hosts a run of `rb` goes to, in order: the server first if targeted, then hosts by name.
fn run_hosts(st: &AppState, rb: &Runbook) -> ApiResult<Vec<RunHost>> {
    let local = RunHost { id: LOCAL_HOST_ID.into(), name: LOCAL_HOST_NAME.into() };
    if rb.targets.is_server() {
        return Ok(vec![local]);
    }
    let mut out = Vec::new();
    if rb.targets.hosts.iter().any(|h| h == LOCAL_HOST_ID) {
        out.push(local);
    }
    for h in st.store.hosts()? {
        if rb.targets.includes(&h) {
            out.push(RunHost { id: h.id, name: h.name });
        }
    }
    if out.is_empty() {
        return Err(ApiError::new(StatusCode::CONFLICT, "No hosts match this runbook's targets. Enroll a host or edit the runbook."));
    }
    Ok(out)
}

#[derive(Deserialize)]
struct RunsQuery {
    runbook_id: Option<String>,
    limit: Option<usize>,
}

async fn list_runs(State(st): State<Arc<AppState>>, Query(q): Query<RunsQuery>) -> ApiResult<Json<Vec<Run>>> {
    let limit = q.limit.unwrap_or(200).clamp(1, 1000);
    Ok(Json(st.store.runs(q.runbook_id.as_deref(), limit)?))
}

async fn get_run(State(st): State<Arc<AppState>>, Path(id): Path<String>) -> ApiResult<Json<Run>> {
    st.store.run(&id)?.map(Json).ok_or_else(|| ApiError::not_found("Run"))
}

#[derive(Deserialize)]
struct LogsQuery {
    after: Option<u64>,
    limit: Option<usize>,
}

async fn run_logs(State(st): State<Arc<AppState>>, Path(id): Path<String>, Query(q): Query<LogsQuery>) -> ApiResult<Json<serde_json::Value>> {
    st.store.run(&id)?.ok_or_else(|| ApiError::not_found("Run"))?;
    let lines = st.store.logs(&id, q.after.unwrap_or(0), q.limit.unwrap_or(5000).clamp(1, 20_000))?;
    Ok(Json(json!({ "lines": lines })))
}

/// Start a new run of the same runbook on the hosts of run `id` that didn't finish
/// well. It uses the runbook as it is now, and the earlier run's input values unless
/// the body gives new ones; secret inputs are never stored, so they must be given again.
async fn retry_run(State(st): State<Arc<AppState>>, Extension(user): Extension<CurrentUser>, Path(id): Path<String>, body: String) -> ApiResult<Response> {
    let conflict = |m: &str| ApiError::new(StatusCode::CONFLICT, m.to_string());
    let earlier = st.store.run(&id)?.ok_or_else(|| ApiError::not_found("Run"))?;
    if !earlier.status.is_finished() {
        return Err(conflict("This run is still going. Retry it after it ends."));
    }
    let rb = st.store.runbook(&earlier.runbook_id)?.ok_or_else(|| conflict("The runbook of this run was deleted, so it can't be retried."))?;
    let mut hosts = earlier.unfinished_hosts();
    if hosts.is_empty() {
        return Err(conflict("Every host of this run succeeded; there is nothing to retry."));
    }
    let mut kept = Vec::new();
    for h in hosts.drain(..) {
        if h.id == LOCAL_HOST_ID || st.store.stored_host(&h.id)?.is_some() {
            kept.push(h);
        }
    }
    if kept.is_empty() {
        return Err(conflict("The hosts to retry have all been removed."));
    }
    let start: StartRunInput = if body.trim().is_empty() {
        StartRunInput::default()
    } else {
        serde_json::from_str(&body).map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, format!("Invalid retry request: {e}")))?
    };
    // The earlier values (secret ones were masked, so they're left out), then any given now.
    let mut given = serde_json::Map::new();
    for def in &rb.inputs {
        match earlier.inputs.get(&def.name) {
            Some(v) if def.kind != crate::model::InputKind::Secret && v != crate::model::MASK => {
                given.insert(def.name.clone(), serde_json::Value::String(v.clone()));
            }
            _ => {}
        }
    }
    given.extend(start.inputs);
    let values = st.executor.prepare(&rb, &given).map_err(|e| match e {
        StartError::BadInput(m) => ApiError::new(StatusCode::BAD_REQUEST, m),
        StartError::Conflict(m) => ApiError::new(StatusCode::CONFLICT, m),
        StartError::Internal(e) => e.into(),
    })?;
    let mut run = Run::new(&rb, &user.label(), kept, &values.inputs);
    run.trigger = "retry".into();
    run.retry_of = Some(earlier.id.clone());
    let run = st.executor.start(run, values)?;
    tracing::info!(run_id = %run.id, retry_of = %earlier.id, hosts = run.hosts.len(), "retrying unfinished hosts");
    audit::record(&st, &By::user(&user), "run.retry", Some(AuditTarget::new("run", &run.id, &rb.name)), Some(format!("{} host(s) of run {}", run.hosts.len(), earlier.id)));
    Ok((StatusCode::CREATED, Json(run)).into_response())
}

async fn cancel_run(State(st): State<Arc<AppState>>, Extension(user): Extension<CurrentUser>, Path(id): Path<String>) -> ApiResult<StatusCode> {
    let run = st.store.run(&id)?.ok_or_else(|| ApiError::not_found("Run"))?;
    if run.status.is_finished() || !st.executor.cancel(&id) {
        return Err(ApiError::new(StatusCode::CONFLICT, "This run has already finished."));
    }
    audit::record(&st, &By::user(&user), "run.cancel", Some(AuditTarget::new("run", &id, &run.runbook_name)), None);
    Ok(StatusCode::ACCEPTED)
}

fn sse(event: &str, data: impl serde::Serialize) -> Result<SseEvent, Infallible> {
    Ok(SseEvent::default().event(event).data(serde_json::to_string(&data).unwrap_or_default()))
}

/// Live run updates: the run, all output so far, then new output and status changes,
/// and finally an `end` event. Finished runs get their stored output and `end`.
async fn run_events(State(st): State<Arc<AppState>>, Path(id): Path<String>) -> ApiResult<Sse<impl Stream<Item = Result<SseEvent, Infallible>>>> {
    let run = st.store.run(&id)?.ok_or_else(|| ApiError::not_found("Run"))?;
    let live = st.executor.subscribe(&id);
    let (backlog, rx) = match live {
        Some((lines, rx)) => (lines, Some(rx)),
        None => (st.store.logs(&id, 0, 20_000)?, None),
    };
    let mut initial = vec![sse("run", &run)];
    initial.extend(backlog.iter().map(|l| sse("log", l)));

    let updates = match rx {
        Some(rx) => BroadcastStream::new(rx)
            .filter_map(|item| async move {
                match item {
                    Ok(Event::Log(line)) => Some(sse("log", &line)),
                    Ok(Event::Run(run)) => Some(sse("run", &*run)),
                    Err(_) => None,
                }
            })
            .boxed(),
        None => stream::empty().boxed(),
    };
    let store = st.store.clone();
    let end = stream::once(async move {
        let last = store.run(&id).ok().flatten();
        Ok::<_, Infallible>(SseEvent::default().event("end").data(serde_json::to_string(&last).unwrap_or_default()))
    });
    let all = stream::iter(initial).chain(updates).chain(end);
    Ok(Sse::new(all).keep_alive(KeepAlive::new().interval(Duration::from_secs(15))))
}
