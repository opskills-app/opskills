//! Inbound webhooks that start runs.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;

use crate::auth;
use crate::model::{
    Run, Runbook, now_ms,
};
use crate::vault::Sealed;
use crate::model::Role;
use crate::web::AppState;
use super::runs::start_runbook;
use super::{Access, ApiError, ApiResult, bad};

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/runbooks/{id}/webhooks", get(list_webhooks).post(create_webhook))
        .route("/webhooks/{id}", axum::routing::put(update_webhook).delete(delete_webhook))
        .route("/webhooks/{id}/rotate", post(rotate_webhook))
        .route("/hooks/{token}", post(inbound_hook))
}

pub(super) const ACCESS: &[(&str, &str, Access)] = &[
    ("GET", "/runbooks/{id}/webhooks", Access::Role(Role::Viewer)),
    ("POST", "/runbooks/{id}/webhooks", Access::Role(Role::Editor)),
    ("PUT", "/webhooks/{id}", Access::Role(Role::Editor)),
    ("DELETE", "/webhooks/{id}", Access::Role(Role::Editor)),
    ("POST", "/webhooks/{id}/rotate", Access::Role(Role::Editor)),
    ("POST", "/hooks/{token}", Access::OwnCredential),
];

const MAX_WEBHOOKS: usize = 20;
const MAX_HOOK_BODY: usize = 256 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WebhookInput {
    name: String,
    #[serde(default = "enabled_by_default")]
    enabled: bool,
    #[serde(default)]
    inputs: std::collections::BTreeMap<String, String>,
    /// Require an HMAC-SHA256 signature. Turning it on creates a signing secret,
    /// returned once.
    #[serde(default)]
    signed: bool,
}

fn enabled_by_default() -> bool {
    true
}

fn check_webhook_input(st: &AppState, runbook_id: &str, input: &WebhookInput, except: Option<&str>) -> ApiResult<(Runbook, String)> {
    let rb = st.store.runbook(runbook_id)?.ok_or_else(|| ApiError::not_found("Runbook"))?;
    let name = input.name.trim().to_string();
    if name.is_empty() || name.chars().count() > 60 {
        return Err(bad("Give the webhook a name of 1 to 60 characters, like \"GitHub push\".".into()));
    }
    crate::model::validate_hook_inputs(&input.inputs, &rb.inputs).map_err(bad)?;
    if st.store.webhooks_of(runbook_id)?.iter().any(|h| Some(h.id.as_str()) != except && h.name.eq_ignore_ascii_case(&name)) {
        return Err(ApiError::new(StatusCode::CONFLICT, format!("This runbook already has a webhook named {name:?}.")));
    }
    Ok((rb, name))
}

/// A new signing secret, sealed for storage.
fn new_signing_secret(st: &AppState, hook_id: &str) -> ApiResult<(String, String, String)> {
    let secret = hex::encode(crate::firststart::random_bytes::<32>());
    let sealed = st.vault.seal(&format!("webhook:{hook_id}"), secret.as_bytes())?;
    Ok((secret, sealed.nonce, sealed.ciphertext))
}

async fn list_webhooks(State(st): State<Arc<AppState>>, Path(id): Path<String>) -> ApiResult<Json<Vec<crate::model::Webhook>>> {
    st.store.runbook(&id)?.ok_or_else(|| ApiError::not_found("Runbook"))?;
    Ok(Json(st.store.webhooks_of(&id)?))
}

async fn create_webhook(State(st): State<Arc<AppState>>, Path(id): Path<String>, body: String) -> ApiResult<Response> {
    let input: WebhookInput = serde_json::from_str(&body).map_err(|e| bad(format!("Invalid webhook: {e}")))?;
    let (rb, name) = check_webhook_input(&st, &id, &input, None)?;
    if st.store.webhooks_of(&rb.id)?.len() >= MAX_WEBHOOKS {
        return Err(ApiError::new(StatusCode::CONFLICT, format!("A runbook can have at most {MAX_WEBHOOKS} webhooks.")));
    }
    let (hook_id, secret, token) = auth::new_webhook_token();
    let mut stored = crate::model::StoredWebhook {
        hook: crate::model::Webhook {
            id: hook_id.clone(),
            runbook_id: rb.id.clone(),
            name,
            enabled: input.enabled,
            inputs: input.inputs,
            signed: input.signed,
            created_at: now_ms(),
            deliveries: vec![],
        },
        token_hash: auth::token_hash(&secret),
        signing_nonce: None,
        signing_ciphertext: None,
    };
    let mut signing_secret = None;
    if input.signed {
        let (s, nonce, ct) = new_signing_secret(&st, &hook_id)?;
        stored.signing_nonce = Some(nonce);
        stored.signing_ciphertext = Some(ct);
        signing_secret = Some(s);
    }
    st.store.put_webhook(&stored)?;
    tracing::info!(webhook_id = %hook_id, runbook = %rb.name, "webhook created");
    // The token (part of the URL) and the signing secret are shown only now.
    Ok((StatusCode::CREATED, Json(json!({ "webhook": stored.hook, "token": token, "signing_secret": signing_secret }))).into_response())
}

async fn update_webhook(State(st): State<Arc<AppState>>, Path(id): Path<String>, body: String) -> ApiResult<Json<serde_json::Value>> {
    let current = st.store.webhook(&id)?.ok_or_else(|| ApiError::not_found("Webhook"))?;
    let input: WebhookInput = serde_json::from_str(&body).map_err(|e| bad(format!("Invalid webhook: {e}")))?;
    let (_, name) = check_webhook_input(&st, &current.hook.runbook_id, &input, Some(&id))?;
    // A secret ready in case this turns signing on; kept only if it does.
    let fresh = if input.signed { Some(new_signing_secret(&st, &id)?) } else { None };
    let updated = st.store.update_webhook(&id, move |h| {
        let mut shown = None;
        match fresh {
            Some((secret, nonce, ct)) if !h.hook.signed => {
                h.signing_nonce = Some(nonce);
                h.signing_ciphertext = Some(ct);
                shown = Some(secret);
            }
            Some(_) => {}
            None => {
                h.signing_nonce = None;
                h.signing_ciphertext = None;
            }
        }
        h.hook.name = name;
        h.hook.enabled = input.enabled;
        h.hook.inputs = input.inputs;
        h.hook.signed = input.signed;
        (h.hook.clone(), shown)
    })?;
    let (hook, signing_secret) = updated.ok_or_else(|| ApiError::not_found("Webhook"))?;
    Ok(Json(json!({ "webhook": hook, "signing_secret": signing_secret })))
}

/// Give a webhook a new URL; the old one stops working at once.
async fn rotate_webhook(State(st): State<Arc<AppState>>, Path(id): Path<String>) -> ApiResult<Json<serde_json::Value>> {
    let secret = hex::encode(crate::firststart::random_bytes::<32>());
    let hash = auth::token_hash(&secret);
    st.store.update_webhook(&id, |h| h.token_hash = hash)?.ok_or_else(|| ApiError::not_found("Webhook"))?;
    Ok(Json(json!({ "token": format!("{}{id}_{secret}", auth::WEBHOOK_PREFIX) })))
}

async fn delete_webhook(State(st): State<Arc<AppState>>, Path(id): Path<String>) -> ApiResult<StatusCode> {
    if st.store.delete_webhook(&id)? { Ok(StatusCode::NO_CONTENT) } else { Err(ApiError::not_found("Webhook")) }
}

/// `POST /hooks/{token}` from another system: check the token and signature, map the
/// JSON body to the runbook's inputs and start a run. Every call is recorded.
async fn inbound_hook(State(st): State<Arc<AppState>>, Path(token): Path<String>, headers: axum::http::HeaderMap, body: axum::body::Bytes) -> ApiResult<Response> {
    let unknown = || ApiError::new(StatusCode::NOT_FOUND, "No such webhook.");
    let (id, secret) = auth::parse_webhook_token(&token).ok_or_else(unknown)?;
    let stored = st.store.webhook(id)?.ok_or_else(unknown)?;
    if !auth::constant_time_eq(auth::token_hash(secret).as_bytes(), stored.token_hash.as_bytes()) {
        return Err(unknown());
    }
    let result = deliver(&st, &stored, &headers, &body);
    let delivery = match &result {
        Ok(run) => crate::model::Delivery { at: now_ms(), status: 202, run_id: Some(run.id.clone()), error: None },
        Err(e) => crate::model::Delivery { at: now_ms(), status: e.status().as_u16(), run_id: None, error: Some(e.detail()) },
    };
    st.store.update_webhook(id, |h| {
        h.hook.deliveries.insert(0, delivery);
        h.hook.deliveries.truncate(crate::model::MAX_DELIVERIES);
    })?;
    let run = result?;
    Ok((StatusCode::ACCEPTED, Json(json!({ "run_id": run.id, "status": run.status }))).into_response())
}

fn deliver(st: &AppState, stored: &crate::model::StoredWebhook, headers: &axum::http::HeaderMap, body: &[u8]) -> ApiResult<Run> {
    let hook = &stored.hook;
    if !hook.enabled {
        return Err(ApiError::new(StatusCode::CONFLICT, "This webhook is turned off."));
    }
    if body.len() > MAX_HOOK_BODY {
        return Err(ApiError::new(StatusCode::PAYLOAD_TOO_LARGE, "The body is larger than 256 KB."));
    }
    if let (Some(nonce), Some(ct)) = (&stored.signing_nonce, &stored.signing_ciphertext) {
        let key = st.vault.open(&format!("webhook:{}", hook.id), &Sealed { nonce: nonce.clone(), ciphertext: ct.clone() })?;
        let given = ["x-opskills-signature", "x-hub-signature-256"]
            .iter()
            .find_map(|h| headers.get(*h))
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().strip_prefix("sha256="))
            .and_then(|v| hex::decode(v).ok());
        use hmac::{KeyInit, Mac};
        let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(&key).map_err(|e| anyhow::anyhow!("hmac key: {e}"))?;
        mac.update(body);
        if given.is_none_or(|sig| mac.verify_slice(&sig).is_err()) {
            return Err(ApiError::new(StatusCode::UNAUTHORIZED, "The signature is missing or doesn't match. Send X-OpSkills-Signature: sha256=<HMAC-SHA256 of the body, hex>."));
        }
    }
    let doc: serde_json::Value = if body.iter().all(u8::is_ascii_whitespace) {
        json!({})
    } else {
        serde_json::from_slice(body).map_err(|e| bad(format!("The body must be JSON: {e}")))?
    };
    let rb = st.store.runbook(&hook.runbook_id)?.ok_or_else(|| ApiError::new(StatusCode::CONFLICT, "The runbook of this webhook no longer exists."))?;
    let given = hook
        .inputs
        .iter()
        .filter_map(|(name, path)| crate::model::json_path(&doc, path).map(|v| (name.clone(), serde_json::Value::String(v))))
        .collect();
    start_runbook(st, &rb, &given, &format!("webhook {}", hook.name), "webhook")
}
