//! Notifications and the email settings they use.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use serde::Deserialize;
use serde_json::json;

use crate::model::Role;
use crate::audit::{self, By};
use crate::model::AuditTarget;
use crate::web::AppState;
use super::{Access, ApiError, ApiResult, CurrentUser, InWorkspace, Scope, bad};

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/notifications/settings", get(notification_settings))
        .route("/settings/email", get(get_email_settings).put(put_email_settings).delete(delete_email_settings))
        .route("/notifications/test", post(test_notification))
        .route("/runs/{id}/notifications", get(run_notices))
}

pub(super) const ACCESS: &[(&str, &str, Access)] = &[
    ("GET", "/notifications/settings", Access::User),
    ("GET", "/settings/email", Access::ServerAdmin),
    ("PUT", "/settings/email", Access::ServerAdmin),
    ("DELETE", "/settings/email", Access::ServerAdmin),
    ("POST", "/notifications/test", Access::Role(Role::Editor, Scope::Query)),
    ("GET", "/runs/{id}/notifications", Access::Role(Role::Viewer, Scope::Run)),
];

async fn notification_settings(State(st): State<Arc<AppState>>) -> Json<serde_json::Value> {
    Json(json!({ "email": st.notifier.email_enabled(), "from": st.notifier.email_from(), "source": st.notifier.email_source() }))
}

/// Email settings: from the config file (read-only here), from this page, or off.
async fn get_email_settings(State(st): State<Arc<AppState>>) -> ApiResult<Json<serde_json::Value>> {
    let source = st.notifier.email_source();
    let settings = match source {
        crate::notify::EmailSource::Config => st.notifier.config_email(),
        _ => st.store.email_settings()?.map(|s| json!(s)),
    };
    Ok(Json(json!({ "source": source, "settings": settings })))
}

async fn put_email_settings(State(st): State<Arc<AppState>>, Extension(user): Extension<CurrentUser>, body: String) -> ApiResult<Json<serde_json::Value>> {
    if st.notifier.email_source() == crate::notify::EmailSource::Config {
        return Err(ApiError::new(StatusCode::CONFLICT, "Email is set in the server config file. Change it there, or remove it there to manage it here."));
    }
    let mut s: crate::model::EmailSettings = serde_json::from_str(&body).map_err(|e| bad(format!("Invalid email settings: {e}")))?;
    s.validate().map_err(bad)?;
    // Email is server-wide; its password is a secret of the default workspace.
    if let Some(name) = &s.password_secret
        && st.store.secret(crate::model::DEFAULT_WORKSPACE, name)?.is_none()
    {
        return Err(bad(format!("The Default workspace's vault has no secret named {name}. Add the password there first.")));
    }
    st.store.put_email_settings(Some(&s))?;
    tracing::info!(smtp_host = %s.smtp_host, "email settings saved");
    audit::record(&st, &By::user(&user), None, "settings.email_update", Some(AuditTarget::new("settings", "email", "Email")), Some(format!("SMTP {}:{}, from {}", s.smtp_host, s.smtp_port, s.from)));
    Ok(Json(json!({ "source": "settings", "settings": s })))
}

async fn delete_email_settings(State(st): State<Arc<AppState>>, Extension(user): Extension<CurrentUser>) -> ApiResult<StatusCode> {
    if st.notifier.email_source() == crate::notify::EmailSource::Config {
        return Err(ApiError::new(StatusCode::CONFLICT, "Email is set in the server config file. Remove it there."));
    }
    st.store.put_email_settings(None)?;
    audit::record(&st, &By::user(&user), None, "settings.email_delete", Some(AuditTarget::new("settings", "email", "Email")), Some("email turned off".into()));
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TestTarget {
    #[serde(default)]
    email: Vec<String>,
    #[serde(default)]
    webhook: Option<String>,
    #[serde(default)]
    headers: Vec<crate::model::WebhookHeader>,
    #[serde(default)]
    auth: Option<crate::model::WebhookAuth>,
    #[serde(default)]
    body: Option<String>,
}

/// Send a test message to a target, so people can check it before relying on it.
async fn test_notification(State(st): State<Arc<AppState>>, Extension(InWorkspace(ws)): Extension<InWorkspace>, body: String) -> ApiResult<Json<serde_json::Value>> {
    let t: TestTarget = serde_json::from_str(&body).map_err(|e| bad(format!("Invalid target: {e}")))?;
    let mut n = crate::model::Notification {
        id: String::new(),
        on: vec![crate::model::NotifyEvent::Started],
        slow_after_seconds: None,
        email: t.email,
        webhook: t.webhook,
        headers: t.headers,
        auth: t.auth,
        body: t.body,
        enabled: true,
    };
    n.validate().map_err(|e| bad(format!("{}{}", e[..1].to_uppercase(), &e[1..])))?;
    if n.webhook.is_none() && !st.notifier.email_enabled() {
        return Err(ApiError::new(StatusCode::CONFLICT, "Email isn't set up on this server yet. Set it up under Settings > Email."));
    }
    for name in n.secret_refs() {
        if st.store.secret(&ws, &name)?.is_none() {
            return Err(bad(format!("The vault has no secret named {name}.")));
        }
    }
    let notifier = st.notifier.clone();
    let target = n.target_label();
    match tokio::task::spawn_blocking(move || notifier.send_test(&n, &ws)).await.map_err(|e| anyhow::anyhow!("test send: {e}"))? {
        Ok(()) => Ok(Json(json!({ "sent": true, "target": target }))),
        Err(e) => Err(ApiError::new(StatusCode::BAD_GATEWAY, format!("Sending to {target} failed: {e}"))),
    }
}

async fn run_notices(State(st): State<Arc<AppState>>, Path(id): Path<String>) -> ApiResult<Json<Vec<crate::notify::Notice>>> {
    st.store.run(&id)?.ok_or_else(|| ApiError::not_found("Run"))?;
    Ok(Json(st.store.notices(&id)?))
}
