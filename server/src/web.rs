//! HTTP: health probes, the system endpoint, and the embedded web app.

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
use crate::firststart;

/// The built web app (ui/dist), compiled into the binary.
#[derive(RustEmbed)]
#[folder = "../ui/dist"]
struct WebApp;

struct AppState {
    cfg: Config,
    server_id: String,
    started: Instant,
    started_unix: u64,
}

pub async fn serve(cfg: Config, fs_state: firststart::State) -> Result<()> {
    let listen = cfg.server.listen.clone();
    let state = Arc::new(AppState {
        cfg,
        server_id: fs_state.server_id,
        started: Instant::now(),
        started_unix: SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
    });

    let app = router(state);
    let listener = tokio::net::TcpListener::bind(&listen).await.with_context(|| format!("listening on {listen}"))?;
    tracing::info!(listen = %listen, version = crate::VERSION, "opskills is ready");
    axum::serve(listener, app).with_graceful_shutdown(shutdown_signal()).await?;
    tracing::info!("stopped");
    Ok(())
}

fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/readyz", get(readyz))
        .route("/api/v1/system", get(system))
        .fallback(static_file)
        .with_state(state)
        .layer(CompressionLayer::new())
        .layer(TraceLayer::new_for_http())
}

async fn readyz(State(st): State<Arc<AppState>>) -> Response {
    match tokio::fs::metadata(&st.cfg.data.dir).await {
        Ok(m) if m.is_dir() => (StatusCode::OK, "ready").into_response(),
        _ => (StatusCode::SERVICE_UNAVAILABLE, "data directory unavailable").into_response(),
    }
}

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
