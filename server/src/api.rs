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
use crate::hub::{AgentInfo, JobEnd, OutLine, POLL_WAIT};
use crate::model::{
    DEFAULT_STEP_TIMEOUT, EnrollmentToken, Host, LOCAL_HOST_ID, LOCAL_HOST_NAME, Run, RunHost, Runbook, RunbookInput, Secret, StoredHost, normalize_tag, SecretInput, SecretUpdate, StoredSecret, now_ms, validate_secret_description, validate_secret_kind,
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
        .route("/hosts", get(list_hosts))
        .route("/hosts/packages", get(host_packages))
        .route("/hosts/enrollment-tokens", post(create_enrollment_token))
        .route("/hosts/{id}", axum::routing::put(update_host).delete(delete_host))
        .route("/agent/enroll", post(agent_enroll))
        .route("/agent/poll", post(agent_poll))
        .route("/agent/whoami", get(agent_whoami))
        .route("/agent/jobs/{id}/output", post(agent_output))
        .route("/agent/jobs/{id}/finish", post(agent_finish))
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
    if path.starts_with("/agent/") || path.starts_with("/api/v1/agent/") {
        // Agents authenticate with their own credential in each handler; no cookies.
        return next.run(req).await;
    }
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
    for id in &input.targets.hosts {
        if id != LOCAL_HOST_ID && st.store.stored_host(id)?.is_none() {
            return Err(ApiError::new(StatusCode::BAD_REQUEST, format!("There is no host with id {id}. It may have been removed.")));
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
        targets: input.targets,
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
        targets: input.targets,
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
    let hosts = run_hosts(&st, &rb)?;
    let run = st.executor.start(Run::new(&rb, &user.0, hosts), env)?;
    Ok((StatusCode::CREATED, Json(run)).into_response())
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
        if rb.targets.hosts.contains(&h.id) || h.tags.iter().any(|t| rb.targets.tags.contains(t)) {
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

// Hosts

#[derive(serde::Serialize)]
struct HostView {
    #[serde(flatten)]
    host: Host,
    online: bool,
    /// The OpSkills server itself: always there, can't be edited or removed.
    builtin: bool,
    used_by: Vec<serde_json::Value>,
}

fn targets_host(rb: &Runbook, host: &Host) -> bool {
    if host.id == LOCAL_HOST_ID {
        return rb.targets.is_server() || rb.targets.hosts.iter().any(|h| h == LOCAL_HOST_ID);
    }
    rb.targets.hosts.contains(&host.id) || host.tags.iter().any(|t| rb.targets.tags.contains(t))
}

fn local_host(st: &AppState) -> Host {
    let info = crate::agent::machine_info();
    let started = st.started_unix * 1000;
    Host {
        id: LOCAL_HOST_ID.into(),
        name: LOCAL_HOST_NAME.into(),
        hostname: info.hostname,
        os: info.os,
        arch: info.arch,
        agent_version: crate::VERSION.into(),
        tags: vec![],
        enrolled_at: started,
        last_seen: now_ms(),
    }
}

fn host_views(st: &AppState) -> ApiResult<Vec<HostView>> {
    let runbooks = st.store.runbooks()?;
    let used = |h: &Host| runbooks.iter().filter(|r| targets_host(r, h)).map(|r| json!({ "id": r.id, "name": r.name })).collect();
    let local = local_host(st);
    let mut list = vec![HostView { used_by: used(&local), host: local, online: true, builtin: true }];
    for mut h in st.store.hosts()? {
        h.last_seen = h.last_seen.max(st.hub.last_seen(&h.id).unwrap_or(0));
        list.push(HostView { online: st.hub.is_online(&h.id, h.last_seen), used_by: used(&h), host: h, builtin: false });
    }
    Ok(list)
}

async fn list_hosts(State(st): State<Arc<AppState>>) -> ApiResult<Json<Vec<HostView>>> {
    Ok(Json(host_views(&st)?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HostUpdate {
    name: Option<String>,
    tags: Option<Vec<String>>,
}

fn validate_host_name(name: &str) -> Result<String, String> {
    let name = name.trim().to_string();
    if name.is_empty() || name.chars().count() > 80 || name.chars().any(char::is_control) {
        return Err("A host name needs 1 to 80 characters.".into());
    }
    if name.eq_ignore_ascii_case(LOCAL_HOST_NAME) {
        return Err(format!("{LOCAL_HOST_NAME} is reserved for the server itself."));
    }
    Ok(name)
}

fn clean_tags(tags: &[String]) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    for t in tags {
        if let Some(t) = normalize_tag(t)?
            && !out.contains(&t) {
                out.push(t);
            }
    }
    if out.len() > 20 {
        return Err("A host can have at most 20 tags.".into());
    }
    Ok(out)
}

async fn update_host(State(st): State<Arc<AppState>>, Path(id): Path<String>, body: String) -> ApiResult<Json<HostView>> {
    if id == LOCAL_HOST_ID {
        return Err(ApiError::new(StatusCode::CONFLICT, "The OpSkills server can't be renamed or tagged."));
    }
    let mut stored = st.store.stored_host(&id)?.ok_or_else(|| ApiError::not_found("Host"))?;
    let update: HostUpdate = serde_json::from_str(&body).map_err(|e| bad(format!("Invalid update: {e}")))?;
    if let Some(name) = update.name {
        let name = validate_host_name(&name).map_err(bad)?;
        if st.store.hosts()?.iter().any(|h| h.id != id && h.name.eq_ignore_ascii_case(&name)) {
            return Err(ApiError::new(StatusCode::CONFLICT, format!("Another host is already called {name}.")));
        }
        stored.host.name = name;
    }
    if let Some(tags) = update.tags {
        stored.host.tags = clean_tags(&tags).map_err(bad)?;
    }
    st.store.put_host(&stored)?;
    host_views(&st)?.into_iter().find(|v| v.host.id == id).map(Json).ok_or_else(|| ApiError::not_found("Host"))
}

async fn delete_host(State(st): State<Arc<AppState>>, Path(id): Path<String>) -> ApiResult<StatusCode> {
    if id == LOCAL_HOST_ID {
        return Err(ApiError::new(StatusCode::CONFLICT, "The OpSkills server can't be removed."));
    }
    let users: Vec<String> = st.store.runbooks()?.into_iter().filter(|r| r.targets.hosts.contains(&id)).map(|r| r.name).collect();
    if !users.is_empty() {
        return Err(ApiError::new(StatusCode::CONFLICT, format!("This host is a target of: {}. Remove it from those runbooks first.", users.join(", "))));
    }
    if st.store.delete_host(&id)? {
        st.hub.forget(&id);
        tracing::info!(host_id = %id, "host removed");
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found("Host"))
    }
}

const ENROLLMENT_HOURS: u64 = 24;

async fn create_enrollment_token(State(st): State<Arc<AppState>>, Extension(user): Extension<CurrentUser>) -> ApiResult<Response> {
    st.store.purge_expired_enrollment_tokens()?;
    let token = auth::new_token();
    let now = now_ms();
    let t = EnrollmentToken { created_at: now, expires_at: now + ENROLLMENT_HOURS * 3_600_000, created_by: user.0 };
    st.store.put_enrollment_token(&auth::token_hash(&token), &t)?;
    tracing::info!("enrollment token created");
    Ok((StatusCode::CREATED, Json(json!({ "token": token, "expires_at": t.expires_at }))).into_response())
}

/// One agent package: where the server offers it and where it comes from.
#[derive(serde::Serialize)]
pub struct Package {
    pub format: &'static str,
    pub arch: &'static str,
    pub file: String,
    /// Served from this server's packages folder (else redirected to the GitHub release).
    pub bundled: bool,
}

pub fn packages(st: &AppState) -> Vec<Package> {
    let v = crate::VERSION;
    let mut out = Vec::new();
    for (arch, rpm_arch) in [("amd64", "x86_64"), ("arm64", "aarch64")] {
        for (format, file) in [("deb", format!("opskills-agent_{v}_{arch}.deb")), ("rpm", format!("opskills-agent-{v}-1.{rpm_arch}.rpm"))] {
            let bundled = st.cfg.hosts.packages_dir.join(&file).is_file();
            out.push(Package { format, arch, file, bundled });
        }
    }
    out
}

pub fn release_url(file: &str) -> String {
    format!("https://github.com/opskills-app/opskills/releases/download/v{}/{file}", crate::VERSION)
}

async fn host_packages(State(st): State<Arc<AppState>>) -> Json<serde_json::Value> {
    Json(json!({ "version": crate::VERSION, "packages": packages(&st) }))
}

// Agent endpoints. Each agent sends `Authorization: Bearer <host id>.<secret>`.

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
