//! Schedules: status and preview.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;

use crate::model::now_ms;
use crate::web::AppState;
use super::{ApiError, ApiResult, bad};

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/runbooks/{id}/schedules", get(schedule_status))
        .route("/schedules/preview", post(preview_schedule))
}

/// What the scheduler knows about each of a runbook's schedules.
async fn schedule_status(State(st): State<Arc<AppState>>, Path(id): Path<String>) -> ApiResult<Json<Vec<serde_json::Value>>> {
    let rb = st.store.runbook(&id)?.ok_or_else(|| ApiError::not_found("Runbook"))?;
    let now = now_ms();
    let mut out = Vec::new();
    for s in &rb.schedules {
        let state = st.store.schedule_state(&crate::schedule::state_key(&rb.id, &s.id))?.unwrap_or_default();
        out.push(json!({
            "id": s.id,
            "next": crate::schedule::upcoming(s, now, 3),
            "last_fired_at": state.last_fired_at,
            "last_run_id": state.last_run_id,
            "last_error": state.last_error,
        }));
    }
    Ok(Json(out))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PreviewInput {
    cron: String,
    #[serde(default)]
    timezone: Option<String>,
}

/// The next times a cron expression fires, for the editor to show while you type.
async fn preview_schedule(body: String) -> ApiResult<Json<serde_json::Value>> {
    let input: PreviewInput = serde_json::from_str(&body).map_err(|e| bad(format!("Invalid preview request: {e}")))?;
    let cron = crate::schedule::parse_cron(&input.cron).map_err(bad)?;
    let tz = crate::schedule::parse_tz(input.timezone.as_deref().unwrap_or("UTC")).map_err(bad)?;
    Ok(Json(json!({ "next": crate::schedule::next_times(&cron, tz, now_ms(), 5) })))
}
