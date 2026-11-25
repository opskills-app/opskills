//! The /api/v1 HTTP API. The web app uses exactly this API, nothing private.

use std::convert::Infallible;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use axum::extract::{Path, Query, Request, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use futures_util::stream::{self, Stream, StreamExt};
use serde::Deserialize;
use serde_json::json;
use tokio_stream::wrappers::BroadcastStream;

use crate::auth;
use crate::executor::Event;
use crate::model::{
    DEFAULT_STEP_TIMEOUT, Run, Runbook, RunbookInput, Secret, SecretInput, SecretUpdate, StoredSecret, now_ms, validate_secret_description, validate_secret_kind,
    validate_secret_name, validate_secret_value,
};
use crate::vault::Sealed;
use crate::store::Session;
use crate::web::AppState;

pub fn router(state: Arc<AppState>) -> Router<Arc<AppState>> {
    Router::new()
        .route("/system", get(system))
        .route("/auth/login", post(login))
        .route("/auth/logout", post(logout))
        .route("/auth/me", get(me))
        .route("/runbooks", get(list_runbooks).post(create_runbook))
        .route("/runbooks/{id}", get(get_runbook).put(update_runbook).delete(delete_runbook))
        .route("/runbooks/{id}/runs", post(start_run))
        .route("/runs", get(list_runs))
        .route("/runs/{id}", get(get_run))
        .route("/runs/{id}/logs", get(run_logs))
        .route("/runs/{id}/events", get(run_events))
        .route("/runs/{id}/cancel", post(cancel_run))
        .route("/secrets", get(list_secrets).post(create_secret))
        .route("/secrets/{name}", axum::routing::put(update_secret).delete(delete_secret))
        .fallback(|| async { ApiError::new(StatusCode::NOT_FOUND, "No such API endpoint.") })
        .layer(middleware::from_fn_with_state(state, guard))
}

// Errors

pub struct ApiError {
    status: StatusCode,
    detail: String,
}

impl ApiError {
    pub fn new(status: StatusCode, detail: impl Into<String>) -> Self {
        ApiError { status, detail: detail.into() }
    }
    fn not_found(what: &str) -> Self {
        Self::new(StatusCode::NOT_FOUND, format!("{what} not found."))
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(err: anyhow::Error) -> Self {
        tracing::error!(error = %format!("{err:#}"), "request failed");
        ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Something went wrong on the server.")
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = json!({ "title": self.status.canonical_reason().unwrap_or("Error"), "status": self.status.as_u16(), "detail": self.detail });
        (self.status, [(header::CONTENT_TYPE, "application/problem+json")], body.to_string()).into_response()
    }
}

type ApiResult<T> = Result<T, ApiError>;

#[derive(Clone)]
pub struct CurrentUser(pub String);

// Guard: CSRF header on writes, and a session everywhere except a few public endpoints.

async fn guard(State(st): State<Arc<AppState>>, mut req: Request, next: Next) -> Response {
    let path = req.uri().path().to_string();
    let safe = matches!(*req.method(), Method::GET | Method::HEAD);
    if !safe && req.headers().get(auth::CSRF_HEADER).and_then(|v| v.to_str().ok()) != Some("1") {
        return ApiError::new(StatusCode::FORBIDDEN, "Missing the X-OpSkills-Request header.").into_response();
    }
    if path.ends_with("/system") || path.ends_with("/auth/login") {
        return next.run(req).await;
    }
    let user = auth::cookie_token(req.headers()).and_then(|t| st.store.session(&auth::token_hash(&t)).ok().flatten());
    match user {
        Some(session) => {
            req.extensions_mut().insert(CurrentUser(session.user));
            next.run(req).await
        }
        None => ApiError::new(StatusCode::UNAUTHORIZED, "Sign in to continue.").into_response(),
    }
}

// System and auth

async fn system(State(st): State<Arc<AppState>>) -> Json<serde_json::Value> {
    Json(json!({
        "name": "OpSkills",
        "version": crate::VERSION,
        "server_id": st.server_id,
        "started_at_unix": st.started_unix,
        "uptime_seconds": st.started.elapsed().as_secs(),
        "public_url": st.cfg.server.public_url,
    }))
}

#[derive(Deserialize)]
struct LoginInput {
    username: String,
    password: String,
}

static FAILED_LOGINS: AtomicU64 = AtomicU64::new(0);

async fn login(State(st): State<Arc<AppState>>, Json(input): Json<LoginInput>) -> ApiResult<Response> {
    let hash_file = st.cfg.data.dir.join("admin.hash");
    let password = input.password.clone();
    let ok = input.username.trim() == auth::ADMIN_USER
        && tokio::task::spawn_blocking(move || auth::verify_admin_password(&hash_file, &password)).await.unwrap_or(false);
    if !ok {
        // Slow down guessing; the delay grows with recent failures, up to 5 seconds.
        let fails = FAILED_LOGINS.fetch_add(1, Ordering::Relaxed).min(9) + 1;
        tokio::time::sleep(Duration::from_millis(500 * fails)).await;
        tracing::warn!(username = %input.username, "failed sign-in");
        return Err(ApiError::new(StatusCode::UNAUTHORIZED, "Wrong username or password."));
    }
    FAILED_LOGINS.store(0, Ordering::Relaxed);
    st.store.purge_expired_sessions()?;
    let token = auth::new_token();
    let max_age = st.cfg.auth.session_hours * 3600;
    let now = now_ms();
    st.store.put_session(&auth::token_hash(&token), &Session { user: auth::ADMIN_USER.into(), created_at: now, expires_at: now + max_age * 1000 })?;
    tracing::info!(user = auth::ADMIN_USER, "signed in");
    let mut res = Json(json!({ "user": auth::ADMIN_USER })).into_response();
    res.headers_mut().insert(header::SET_COOKIE, HeaderValue::from_str(&auth::session_cookie(&token, max_age, st.secure_cookies())).unwrap());
    Ok(res)
}

async fn logout(State(st): State<Arc<AppState>>, req: Request) -> ApiResult<Response> {
    if let Some(token) = auth::cookie_token(req.headers()) {
        st.store.delete_session(&auth::token_hash(&token))?;
    }
    let mut res = StatusCode::NO_CONTENT.into_response();
    res.headers_mut().insert(header::SET_COOKIE, HeaderValue::from_str(&auth::clear_cookie(st.secure_cookies())).unwrap());
    Ok(res)
}

async fn me(Extension(user): Extension<CurrentUser>) -> Json<serde_json::Value> {
    Json(json!({ "user": user.0 }))
}

// Runbooks

async fn list_runbooks(State(st): State<Arc<AppState>>) -> ApiResult<Json<Vec<Runbook>>> {
    Ok(Json(st.store.runbooks()?))
}

async fn get_runbook(State(st): State<Arc<AppState>>, Path(id): Path<String>) -> ApiResult<Json<Runbook>> {
    st.store.runbook(&id)?.map(Json).ok_or_else(|| ApiError::not_found("Runbook"))
}

fn parse_input(st: &AppState, body: &str) -> ApiResult<RunbookInput> {
    let input: RunbookInput = serde_json::from_str(body).map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, format!("Invalid runbook: {e}")))?;
    let input = input.validate().map_err(|msg| ApiError::new(StatusCode::BAD_REQUEST, msg))?;
    for name in &input.secrets {
        if st.store.secret(name)?.is_none() {
            return Err(ApiError::new(StatusCode::BAD_REQUEST, format!("The vault has no secret named {name}. Add it in the vault first.")));
        }
    }
    Ok(input)
}

async fn create_runbook(State(st): State<Arc<AppState>>, body: String) -> ApiResult<Response> {
    let input = parse_input(&st, &body)?;
    let now = now_ms();
    let rb = Runbook {
        id: uuid::Uuid::now_v7().to_string(),
        name: input.name,
        description: input.description,
        tags: input.tags,
        steps: input.steps,
        secrets: input.secrets,
        timeout_seconds: input.timeout_seconds.unwrap_or(DEFAULT_STEP_TIMEOUT),
        created_at: now,
        updated_at: now,
    };
    st.store.put_runbook(&rb)?;
    tracing::info!(runbook_id = %rb.id, name = %rb.name, "runbook created");
    Ok((StatusCode::CREATED, Json(rb)).into_response())
}

async fn update_runbook(State(st): State<Arc<AppState>>, Path(id): Path<String>, body: String) -> ApiResult<Json<Runbook>> {
    let existing = st.store.runbook(&id)?.ok_or_else(|| ApiError::not_found("Runbook"))?;
    let input = parse_input(&st, &body)?;
    let rb = Runbook {
        name: input.name,
        description: input.description,
        tags: input.tags,
        steps: input.steps,
        secrets: input.secrets,
        timeout_seconds: input.timeout_seconds.unwrap_or(existing.timeout_seconds),
        updated_at: now_ms(),
        ..existing
    };
    st.store.put_runbook(&rb)?;
    Ok(Json(rb))
}

async fn delete_runbook(State(st): State<Arc<AppState>>, Path(id): Path<String>) -> ApiResult<StatusCode> {
    if st.store.delete_runbook(&id)? { Ok(StatusCode::NO_CONTENT) } else { Err(ApiError::not_found("Runbook")) }
}

// Runs

async fn start_run(State(st): State<Arc<AppState>>, Extension(user): Extension<CurrentUser>, Path(id): Path<String>) -> ApiResult<Response> {
    let rb = st.store.runbook(&id)?.ok_or_else(|| ApiError::not_found("Runbook"))?;
    // Decrypt the secrets this runbook uses; values live only in memory for the run.
    let mut env = Vec::new();
    for name in &rb.secrets {
        let stored = st.store.secret(name)?.ok_or_else(|| {
            ApiError::new(StatusCode::CONFLICT, format!("This runbook uses the secret {name}, which is no longer in the vault. Add it again or edit the runbook."))
        })?;
        let value = st.vault.open(name, &Sealed { nonce: stored.nonce, ciphertext: stored.ciphertext })?;
        env.push((name.clone(), String::from_utf8_lossy(&value).into_owned()));
    }
    let run = st.executor.start(Run::new(&rb, &user.0), env)?;
    Ok((StatusCode::CREATED, Json(run)).into_response())
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

async fn cancel_run(State(st): State<Arc<AppState>>, Path(id): Path<String>) -> ApiResult<StatusCode> {
    let run = st.store.run(&id)?.ok_or_else(|| ApiError::not_found("Run"))?;
    if run.status.is_finished() || !st.executor.cancel(&id) {
        return Err(ApiError::new(StatusCode::CONFLICT, "This run has already finished."));
    }
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

// Vault secrets. Values are write-only: no response ever contains one.

#[derive(serde::Serialize)]
struct SecretView {
    #[serde(flatten)]
    meta: Secret,
    used_by: Vec<serde_json::Value>,
}

fn views(st: &AppState) -> ApiResult<Vec<SecretView>> {
    let runbooks = st.store.runbooks()?;
    let mut list: Vec<SecretView> = st
        .store
        .secrets()?
        .into_iter()
        .map(|meta| {
            let used_by = runbooks.iter().filter(|r| r.secrets.contains(&meta.name)).map(|r| json!({ "id": r.id, "name": r.name })).collect();
            SecretView { meta, used_by }
        })
        .collect();
    list.sort_by(|a, b| a.meta.name.cmp(&b.meta.name));
    Ok(list)
}

fn bad(msg: String) -> ApiError {
    ApiError::new(StatusCode::BAD_REQUEST, msg)
}

async fn list_secrets(State(st): State<Arc<AppState>>) -> ApiResult<Json<Vec<SecretView>>> {
    Ok(Json(views(&st)?))
}

async fn create_secret(State(st): State<Arc<AppState>>, body: String) -> ApiResult<Response> {
    let input: SecretInput = serde_json::from_str(&body).map_err(|e| bad(format!("Invalid secret: {e}")))?;
    let name = input.name.trim().to_string();
    let kind = input.kind.unwrap_or_else(|| "other".into());
    validate_secret_name(&name).map_err(bad)?;
    validate_secret_kind(&kind).map_err(bad)?;
    validate_secret_value(&input.value).map_err(bad)?;
    let description = input.description.trim().to_string();
    validate_secret_description(&description).map_err(bad)?;
    if st.store.secret(&name)?.is_some() {
        return Err(ApiError::new(StatusCode::CONFLICT, format!("A secret named {name} already exists. Replace its value instead.")));
    }
    let sealed = st.vault.seal(&name, input.value.as_bytes())?;
    let now = now_ms();
    let meta = Secret { name: name.clone(), kind, description, created_at: now, updated_at: now };
    st.store.put_secret(&StoredSecret { meta: meta.clone(), nonce: sealed.nonce, ciphertext: sealed.ciphertext })?;
    tracing::info!(secret = %name, "secret added");
    Ok((StatusCode::CREATED, Json(SecretView { meta, used_by: vec![] })).into_response())
}

async fn update_secret(State(st): State<Arc<AppState>>, Path(name): Path<String>, body: String) -> ApiResult<Json<SecretView>> {
    let mut stored = st.store.secret(&name)?.ok_or_else(|| ApiError::not_found("Secret"))?;
    let update: SecretUpdate = serde_json::from_str(&body).map_err(|e| bad(format!("Invalid update: {e}")))?;
    if let Some(kind) = update.kind {
        validate_secret_kind(&kind).map_err(bad)?;
        stored.meta.kind = kind;
    }
    if let Some(description) = update.description {
        let description = description.trim().to_string();
        validate_secret_description(&description).map_err(bad)?;
        stored.meta.description = description;
    }
    if let Some(value) = update.value {
        validate_secret_value(&value).map_err(bad)?;
        let sealed = st.vault.seal(&name, value.as_bytes())?;
        stored.nonce = sealed.nonce;
        stored.ciphertext = sealed.ciphertext;
        tracing::info!(secret = %name, "secret value replaced");
    }
    stored.meta.updated_at = now_ms();
    st.store.put_secret(&stored)?;
    views(&st)?.into_iter().find(|v| v.meta.name == name).map(Json).ok_or_else(|| ApiError::not_found("Secret"))
}

async fn delete_secret(State(st): State<Arc<AppState>>, Path(name): Path<String>) -> ApiResult<StatusCode> {
    let users: Vec<String> = st.store.runbooks()?.into_iter().filter(|r| r.secrets.contains(&name)).map(|r| r.name).collect();
    if !users.is_empty() {
        return Err(ApiError::new(StatusCode::CONFLICT, format!("{name} is used by: {}. Remove it from those runbooks first.", users.join(", "))));
    }
    if st.store.delete_secret(&name)? {
        tracing::info!(secret = %name, "secret deleted");
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found("Secret"))
    }
}
