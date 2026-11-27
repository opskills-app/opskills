//! The audit log, for admins: who changed what, and when, newest first.

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;

use crate::web::AppState;
use super::{Access, ApiResult};

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/audit", get(list_audit))
}

pub(super) const ACCESS: &[(&str, &str, Access)] = &[
    ("GET", "/audit", Access::ServerAdmin),
];

/// Events looked at per request at most, so a search through a long log stays quick.
const SCAN: usize = 20_000;

#[derive(Deserialize)]
pub(super) struct AuditQuery {
    /// Continue after this event id (from `next` of the previous page).
    before: Option<String>,
    limit: Option<usize>,
    /// Only actions starting with this, like "runbook." or "auth.sign_in_failed".
    action: Option<String>,
    /// Only this user (or "system").
    actor: Option<String>,
    /// Text anywhere in the actor, how, action, target or note; case doesn't matter.
    q: Option<String>,
    /// Only events in this workspace (`GET /audit`); empty or missing for all.
    workspace: Option<String>,
}

/// Everything, for server admins; `workspace` narrows it to one workspace.
async fn list_audit(State(st): State<Arc<AppState>>, Query(q): Query<AuditQuery>) -> ApiResult<Json<serde_json::Value>> {
    let only = q.workspace.clone().filter(|w| !w.is_empty());
    audit_page(&st, &q, only.as_deref())
}

/// A page of events, newest first, optionally only those of one workspace.
pub(super) fn audit_page(st: &AppState, q: &AuditQuery, workspace: Option<&str>) -> ApiResult<Json<serde_json::Value>> {
    let limit = q.limit.unwrap_or(100).clamp(1, 500);
    let text = q.q.as_deref().map(str::trim).filter(|t| !t.is_empty()).map(str::to_lowercase);
    let (events, next) = st.store.audit(q.before.as_deref(), limit, SCAN, |e| {
        workspace.is_none_or(|w| e.workspace.as_deref() == Some(w))
            && q.action.as_deref().is_none_or(|a| e.action.starts_with(a))
            && q.actor.as_deref().is_none_or(|a| e.actor == a)
            && text.as_deref().is_none_or(|t| {
                let target = e.target.as_ref().map(|t| format!("{} {} {}", t.kind, t.name, t.id)).unwrap_or_default();
                format!("{} {} {} {} {} {}", e.actor, e.via.as_deref().unwrap_or(""), e.action, e.workspace.as_deref().unwrap_or(""), target, e.detail.as_deref().unwrap_or("")).to_lowercase().contains(t)
            })
    })?;
    Ok(Json(json!({ "events": events, "next": next, "retention_days": st.cfg.audit.retention_days })))
}
