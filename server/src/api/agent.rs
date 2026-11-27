//! Agent endpoints. Each agent sends `Authorization: Bearer <host id>.<secret>`.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;

use crate::auth;
use crate::hub::{AgentInfo, JobEnd, OutLine, POLL_WAIT};
use crate::model::{
    Host, StoredHost, now_ms,
};
use crate::audit::{self, By};
use crate::model::AuditTarget;
use crate::web::AppState;
use super::hosts::{clean_tags, validate_host_name};
use super::{Access, ApiError, ApiResult, bad};

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/agent/enroll", post(agent_enroll))
        .route("/agent/poll", post(agent_poll))
        .route("/agent/whoami", get(agent_whoami))
        .route("/agent/jobs/{id}/output", post(agent_output))
        .route("/agent/jobs/{id}/finish", post(agent_finish))
}

pub(super) const ACCESS: &[(&str, &str, Access)] = &[
    ("POST", "/agent/enroll", Access::OwnCredential),
    ("POST", "/agent/poll", Access::OwnCredential),
    ("GET", "/agent/whoami", Access::OwnCredential),
    ("POST", "/agent/jobs/{id}/output", Access::OwnCredential),
    ("POST", "/agent/jobs/{id}/finish", Access::OwnCredential),
];

fn agent_host(st: &AppState, headers: &axum::http::HeaderMap) -> ApiResult<StoredHost> {
    let unauthorized = || ApiError::new(StatusCode::UNAUTHORIZED, "This host is not enrolled. Enroll it again with a new token.");
    let value = headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).ok_or_else(unauthorized)?;
    let (id, secret) = value.strip_prefix("Bearer ").and_then(|c| c.split_once('.')).ok_or_else(unauthorized)?;
    let host = st.store.stored_host(id)?.ok_or_else(unauthorized)?;
    if !auth::constant_time_eq(auth::token_hash(secret).as_bytes(), host.secret_hash.as_bytes()) {
        return Err(unauthorized());
    }
    Ok(host)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EnrollInput {
    token: String,
    info: AgentInfo,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
}

static FAILED_ENROLLMENTS: AtomicU64 = AtomicU64::new(0);

async fn agent_enroll(State(st): State<Arc<AppState>>, body: String) -> ApiResult<Response> {
    let input: EnrollInput = serde_json::from_str(&body).map_err(|e| bad(format!("Invalid enrollment: {e}")))?;
    let info = input.info.clean();
    let name = validate_host_name(input.name.as_deref().filter(|n| !n.trim().is_empty()).unwrap_or(&info.hostname)).map_err(bad)?;
    let tags = clean_tags(&input.tags).map_err(bad)?;
    if !st.store.take_enrollment_token(&auth::token_hash(input.token.trim()))? {
        let fails = FAILED_ENROLLMENTS.fetch_add(1, Ordering::Relaxed).min(9) + 1;
        tokio::time::sleep(Duration::from_millis(500 * fails)).await;
        tracing::warn!(hostname = %info.hostname, "enrollment with an invalid token");
        return Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "This enrollment token is not valid: it was already used or has expired. Create a new one on the Hosts page.",
        ));
    }
    FAILED_ENROLLMENTS.store(0, Ordering::Relaxed);
    // Keep names unique: web-1, web-1 (2), ...
    let taken: Vec<String> = st.store.hosts()?.into_iter().map(|h| h.name.to_lowercase()).collect();
    let mut unique = name.clone();
    let mut n = 2;
    while taken.contains(&unique.to_lowercase()) {
        unique = format!("{name} ({n})");
        n += 1;
    }
    let secret = auth::new_token();
    let now = now_ms();
    let host = Host {
        id: uuid::Uuid::now_v7().to_string(),
        name: unique,
        hostname: info.hostname,
        os: info.os,
        arch: info.arch,
        agent_version: info.version,
        tags,
        enrolled_at: now,
        last_seen: now,
    };
    st.store.put_host(&StoredHost { host: host.clone(), secret_hash: auth::token_hash(&secret) })?;
    st.hub.touch(&host.id);
    tracing::info!(host_id = %host.id, name = %host.name, "host enrolled");
    audit::record(&st, &By::system(format!("agent {}", host.name)), "host.enroll", Some(AuditTarget::new("host", &host.id, &host.name)), Some(format!("{} {}", host.os, host.arch)));
    Ok((StatusCode::CREATED, Json(json!({ "host_id": host.id, "name": host.name, "secret": secret }))).into_response())
}

/// Lets an agent check its credential without taking a step.
async fn agent_whoami(State(st): State<Arc<AppState>>, headers: axum::http::HeaderMap) -> ApiResult<Json<serde_json::Value>> {
    let host = agent_host(&st, &headers)?;
    Ok(Json(json!({ "host_id": host.host.id, "name": host.host.name, "server_version": crate::VERSION })))
}

async fn agent_poll(State(st): State<Arc<AppState>>, headers: axum::http::HeaderMap, body: String) -> ApiResult<Response> {
    let mut stored = agent_host(&st, &headers)?;
    let info: AgentInfo = serde_json::from_str::<AgentInfo>(&body).map_err(|e| bad(format!("Invalid poll: {e}")))?.clean();
    let h = &mut stored.host;
    let changed = h.hostname != info.hostname || h.os != info.os || h.arch != info.arch || h.agent_version != info.version;
    let now = now_ms();
    if changed || now.saturating_sub(h.last_seen) > 30_000 {
        h.hostname = info.hostname;
        h.os = info.os;
        h.arch = info.arch;
        h.agent_version = info.version;
        h.last_seen = now;
        st.store.put_host(&stored)?;
    }
    match st.hub.next_job(&stored.host.id, POLL_WAIT).await {
        Some(job) => Ok(Json(job).into_response()),
        None => Ok(StatusCode::NO_CONTENT.into_response()),
    }
}

#[derive(Deserialize)]
struct OutputBatch {
    lines: Vec<OutLine>,
}

async fn agent_output(State(st): State<Arc<AppState>>, headers: axum::http::HeaderMap, Path(id): Path<String>, body: String) -> ApiResult<Json<serde_json::Value>> {
    let host = agent_host(&st, &headers)?;
    let batch: OutputBatch = serde_json::from_str(&body).map_err(|e| bad(format!("Invalid output: {e}")))?;
    let lines = batch.lines.into_iter().take(5000).collect();
    match st.hub.output(&host.host.id, &id, lines) {
        Some(cancel) => Ok(Json(json!({ "cancel": cancel }))),
        None => Err(ApiError::new(StatusCode::NOT_FOUND, "This step is no longer running on the server; stop it.")),
    }
}

async fn agent_finish(State(st): State<Arc<AppState>>, headers: axum::http::HeaderMap, Path(id): Path<String>, body: String) -> ApiResult<StatusCode> {
    let host = agent_host(&st, &headers)?;
    let end: JobEnd = serde_json::from_str(&body).map_err(|e| bad(format!("Invalid result: {e}")))?;
    st.hub.finish(&host.host.id, &id, end);
    Ok(StatusCode::NO_CONTENT)
}
