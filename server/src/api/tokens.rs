//! API tokens for scripts and CI.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Json, Router};
use serde::Deserialize;
use serde_json::json;

use crate::auth;
use crate::model::now_ms;
use crate::web::AppState;
use super::{ApiError, ApiResult, CurrentUser, bad};

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/me/tokens", get(list_tokens).post(create_token))
        .route("/me/tokens/{id}", axum::routing::delete(delete_token))
}

const MAX_TOKENS: usize = 50;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TokenInput {
    name: String,
    /// Days until the token stops working; null or absent for never.
    #[serde(default)]
    expires_in_days: Option<u32>,
}

async fn list_tokens(State(st): State<Arc<AppState>>) -> ApiResult<Json<Vec<crate::model::ApiToken>>> {
    Ok(Json(st.store.api_tokens()?))
}

async fn create_token(State(st): State<Arc<AppState>>, Extension(user): Extension<CurrentUser>, body: String) -> ApiResult<Response> {
    let input: TokenInput = serde_json::from_str(&body).map_err(|e| bad(format!("Invalid token request: {e}")))?;
    let name = input.name.trim().to_string();
    if name.is_empty() || name.chars().count() > 60 {
        return Err(bad("Give the token a name of 1 to 60 characters, like \"CI deploy\".".into()));
    }
    if input.expires_in_days.is_some_and(|d| d == 0 || d > 3650) {
        return Err(bad("A token can last 1 to 3650 days, or never expire.".into()));
    }
    let existing = st.store.api_tokens()?;
    if existing.iter().any(|t| t.name.eq_ignore_ascii_case(&name)) {
        return Err(ApiError::new(StatusCode::CONFLICT, format!("There is already a token named {name:?}.")));
    }
    if existing.len() >= MAX_TOKENS {
        return Err(ApiError::new(StatusCode::CONFLICT, format!("You can have at most {MAX_TOKENS} tokens. Revoke one first.")));
    }
    let (id, secret, whole) = auth::new_api_token();
    let now = now_ms();
    let token = crate::model::ApiToken {
        id,
        name,
        user: user.user.clone(),
        created_at: now,
        expires_at: input.expires_in_days.map(|d| now + u64::from(d) * 86_400_000),
        last_used_at: None,
    };
    st.store.put_api_token(&crate::model::StoredApiToken { token: token.clone(), secret_hash: auth::token_hash(&secret) })?;
    tracing::info!(token_id = %token.id, name = %token.name, "API token created");
    // The only time the token itself is ever shown.
    Ok((StatusCode::CREATED, Json(json!({ "token": token, "value": whole }))).into_response())
}

async fn delete_token(State(st): State<Arc<AppState>>, Path(id): Path<String>) -> ApiResult<StatusCode> {
    if st.store.delete_api_token(&id)? {
        tracing::info!(token_id = %id, "API token revoked");
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found("Token"))
    }
}
