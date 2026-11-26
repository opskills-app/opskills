//! HTTP: health probes, the API, and the embedded web app.

use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use axum::extract::State;
use axum::http::{HeaderValue, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use rust_embed::RustEmbed;
use serde_json::json;
use tower_http::compression::CompressionLayer;
use tower_http::trace::TraceLayer;

use crate::config::Config;
use crate::executor::Executor;
use crate::hub::Hub;
use crate::store::Store;
use crate::vault::Vault;
use crate::{api, firststart};

/// The built web app (ui/dist), compiled into the binary.
#[derive(RustEmbed)]
#[folder = "../ui/dist"]
struct WebApp;

pub struct AppState {
    pub cfg: Config,
    pub server_id: String,
    pub started: Instant,
    pub started_unix: u64,
    pub store: Arc<Store>,
    pub executor: Arc<Executor>,
    pub hub: Arc<Hub>,
    pub vault: Arc<Vault>,
}

impl AppState {
    pub fn secure_cookies(&self) -> bool {
        self.cfg.server.public_url.as_deref().is_some_and(|u| u.starts_with("https://"))
    }
}

/// Open the store and build the shared state. Must run inside the async runtime.
pub fn build_state(cfg: Config, fs: &firststart::State) -> Result<Arc<AppState>> {
    let store = Arc::new(Store::open(&cfg.data.dir.join("opskills.redb")).context("opening the database")?);
    let interrupted = store.fail_interrupted_runs()?;
    if interrupted > 0 {
        tracing::warn!(runs = interrupted, "marked runs interrupted by a restart as failed");
    }
    if fs.initial_password.is_some() {
        // A new admin password invalidates every existing session.
        store.clear_sessions()?;
    }
    let work_root = cfg.data.dir.join("work");
    let _ = std::fs::remove_dir_all(&work_root);
    std::fs::create_dir_all(&work_root).context("creating the work directory")?;
    let vault = Arc::new(Vault::from_key_hex(&fs.master_key).context("loading the vault key")?);
    let hub = Hub::new();
    let executor = Executor::new(store.clone(), hub.clone(), vault.clone(), work_root, cfg.runs.max_concurrent);
    Ok(Arc::new(AppState {
        cfg,
        server_id: fs.server_id.clone(),
        started: Instant::now(),
        started_unix: SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
        store,
        executor,
        hub,
        vault,
    }))
}

pub async fn serve(cfg: Config, fs: firststart::State) -> Result<()> {
    let listen = cfg.server.listen.clone();
    let state = build_state(cfg, &fs)?;
    crate::schedule::spawn(state.clone());
    let app = router(state);
    let listener = tokio::net::TcpListener::bind(&listen).await.with_context(|| format!("listening on {listen}"))?;
    tracing::info!(listen = %listen, version = crate::VERSION, "opskills is ready");
    axum::serve(listener, app).with_graceful_shutdown(shutdown_signal()).await?;
    tracing::info!("stopped");
    Ok(())
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/readyz", get(readyz))
        .route("/downloads/{file}", get(download))
        .route("/install-agent.sh", get(install_script))
        .nest("/api/v1", api::router(state.clone()))
        .fallback(static_file)
        .with_state(state)
        .layer(CompressionLayer::new().compress_when(NotEventStream))
        .layer(TraceLayer::new_for_http())
}

/// Never compress Server-Sent Events: compression buffers them and breaks live output.
#[derive(Clone, Copy)]
struct NotEventStream;

impl tower_http::compression::predicate::Predicate for NotEventStream {
    fn should_compress<B>(&self, response: &axum::http::Response<B>) -> bool
    where
        B: axum::body::HttpBody,
    {
        let is_sse = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.starts_with("text/event-stream"));
        !is_sse && tower_http::compression::predicate::DefaultPredicate::new().should_compress(response)
    }
}

async fn readyz(State(st): State<Arc<AppState>>) -> Response {
    match tokio::fs::metadata(&st.cfg.data.dir).await {
        Ok(m) if m.is_dir() => (StatusCode::OK, "ready").into_response(),
        _ => (StatusCode::SERVICE_UNAVAILABLE, "data directory unavailable").into_response(),
    }
}

/// Agent packages: served from the packages folder when present, else from the GitHub release.
async fn download(State(st): State<Arc<AppState>>, axum::extract::Path(file): axum::extract::Path<String>) -> Response {
    let Some(pkg) = api::packages(&st).into_iter().find(|p| p.file == file) else {
        return (StatusCode::NOT_FOUND, "No such package for this OpSkills version.").into_response();
    };
    if !pkg.bundled {
        return axum::response::Redirect::temporary(&api::release_url(&pkg.file)).into_response();
    }
    match tokio::fs::read(st.cfg.hosts.packages_dir.join(&pkg.file)).await {
        Ok(bytes) => {
            let mime = if pkg.format == "deb" { "application/vnd.debian.binary-package" } else { "application/x-rpm" };
            let disposition = format!("attachment; filename=\"{}\"", pkg.file);
            let mut res = (StatusCode::OK, bytes).into_response();
            let h = res.headers_mut();
            h.insert(header::CONTENT_TYPE, HeaderValue::from_static(mime));
            if let Ok(v) = HeaderValue::from_str(&disposition) {
                h.insert(header::CONTENT_DISPOSITION, v);
            }
            h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
            h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
            res
        }
        Err(_) => axum::response::Redirect::temporary(&api::release_url(&pkg.file)).into_response(),
    }
}

/// One-line agent installer, for this server's version.
async fn install_script() -> Response {
    let script = include_str!("../packaging/agent/install-agent.sh").replace("@VERSION@", crate::VERSION);
    let mut res = (StatusCode::OK, script).into_response();
    let h = res.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/x-shellscript; charset=utf-8"));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    res
}

async fn static_file(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    if path.starts_with("api/") {
        return (StatusCode::NOT_FOUND, Json(json!({ "title": "Not Found", "status": 404 }))).into_response();
    }
    if let Some(file) = (!path.is_empty()).then(|| WebApp::get(path)).flatten() {
        return asset_response(file, path.starts_with("assets/"));
    }
    // Single-page app: any other path gets index.html.
    match WebApp::get("index.html") {
        Some(file) => asset_response(file, false),
        None => (StatusCode::NOT_FOUND, "web app not built").into_response(),
    }
}

fn asset_response(file: rust_embed::EmbeddedFile, immutable: bool) -> Response {
    let mime = file.metadata.mimetype().to_string();
    let cache = if immutable { "public, max-age=31536000, immutable" } else { "no-cache" };
    let mut res = (StatusCode::OK, file.data.into_owned()).into_response();
    let headers = res.headers_mut();
    if let Ok(v) = HeaderValue::from_str(&mime) {
        headers.insert(header::CONTENT_TYPE, v);
    }
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    headers.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(header::REFERRER_POLICY, HeaderValue::from_static("same-origin"));
    res
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    let term = async {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            s.recv().await;
        }
    };
    tokio::select! { _ = ctrl_c => {}, _ = term => {} }
    tracing::info!("shutting down");
}
