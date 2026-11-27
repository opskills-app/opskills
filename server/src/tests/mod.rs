//! End-to-end tests through the HTTP router with a real store and executor.

use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

use crate::config::Config;
use crate::{firststart, web};

mod basics;
mod vault;
mod hosts;
mod inputs;
mod steps;
mod rollouts;
mod tokens;
mod schedules;
mod webhooks;
mod notifications;
mod users;
mod audit;
mod workspaces;
mod vault_paths;
mod sign_in;
mod inventory;

struct TestServer {
    app: Router,
    state: std::sync::Arc<web::AppState>,
    password: String,
    dir: std::path::PathBuf,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn server() -> TestServer {
    server_with(|_| {})
}

fn server_with(adjust: impl FnOnce(&mut Config)) -> TestServer {
    let dir = std::env::temp_dir().join(format!("opskills-it-{}", uuid::Uuid::now_v7()));
    let mut cfg = Config::default();
    adjust(&mut cfg);
    cfg.data.dir = dir.clone();
    let fs = firststart::prepare(&cfg).unwrap();
    let password = fs.initial_password.clone().unwrap();
    let state = web::build_state(cfg, &fs).unwrap();
    // Most tests act as an admin who has already chosen a password; `fresh_server` doesn't.
    set_must_change(&state, "admin", false);
    TestServer { app: web::router(state.clone()), state, password, dir }
}

/// A server just after its first start: the admin still has the generated password.
fn fresh_server() -> TestServer {
    let s = server();
    set_must_change(&s.state, "admin", true);
    s
}

fn set_must_change(state: &web::AppState, name: &str, on: bool) {
    let mut u = state.store.user(name).unwrap().unwrap();
    u.user.must_change_password = on;
    state.store.put_user(&u).unwrap();
}

struct Reply {
    status: StatusCode,
    cookie: Option<String>,
    body: String,
}

impl Reply {
    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or(Value::Null)
    }
}

async fn call(app: &Router, method: &str, path: &str, body: Option<Value>, cookie: Option<&str>, csrf: bool) -> Reply {
    let mut req = Request::builder().method(method).uri(path);
    if let Some(c) = cookie {
        req = req.header(header::COOKIE, c);
    }
    if csrf {
        req = req.header("x-opskills-request", "1");
    }
    let req = match body {
        Some(b) => req.header(header::CONTENT_TYPE, "application/json").body(Body::from(b.to_string())),
        None => req.body(Body::empty()),
    }
    .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let cookie = res.headers().get(header::SET_COOKIE).map(|v| v.to_str().unwrap().split(';').next().unwrap().to_string());
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    Reply { status, cookie, body: String::from_utf8_lossy(&bytes).into() }
}

async fn sign_in(s: &TestServer) -> String {
    let r = call(&s.app, "POST", "/api/v1/auth/login", Some(json!({"username": "admin", "password": s.password})), None, true).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    r.cookie.expect("session cookie")
}

async fn create(s: &TestServer, cookie: &str, body: Value) -> String {
    let r = call(&s.app, "POST", "/api/v1/runbooks", Some(body), Some(cookie), true).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
    r.json()["id"].as_str().unwrap().to_string()
}

async fn start(s: &TestServer, cookie: &str, runbook: &str) -> String {
    let r = call(&s.app, "POST", &format!("/api/v1/runbooks/{runbook}/runs"), None, Some(cookie), true).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
    r.json()["id"].as_str().unwrap().to_string()
}

async fn wait_finished(s: &TestServer, cookie: &str, run: &str) -> Value {
    let started = Instant::now();
    loop {
        let r = call(&s.app, "GET", &format!("/api/v1/runs/{run}"), None, Some(cookie), false).await.json();
        if matches!(r["status"].as_str(), Some("succeeded" | "failed" | "cancelled")) {
            return r;
        }
        assert!(started.elapsed() < Duration::from_secs(20), "run did not finish: {r}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn add_secret(s: &TestServer, c: &str, name: &str, value: &str) -> Reply {
    call(&s.app, "POST", "/api/v1/secrets", Some(json!({"name": name, "kind": "password", "description": "test", "value": value})), Some(c), true).await
}

async fn agent_call(app: &Router, path: &str, body: Value, auth: Option<&str>) -> Reply {
    let mut req = Request::builder().method("POST").uri(path).header(header::CONTENT_TYPE, "application/json");
    if let Some(a) = auth {
        req = req.header(header::AUTHORIZATION, a);
    }
    let res = app.clone().oneshot(req.body(Body::from(body.to_string())).unwrap()).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    Reply { status, cookie: None, body: String::from_utf8_lossy(&bytes).into() }
}

/// Enroll a host; returns (host id, Authorization header value).
async fn enroll(s: &TestServer, c: &str, name: &str, tags: Value) -> (String, String) {
    let t = call(&s.app, "POST", "/api/v1/hosts/enrollment-tokens", None, Some(c), true).await;
    assert_eq!(t.status, StatusCode::CREATED, "{}", t.body);
    let token = t.json()["token"].as_str().unwrap().to_string();
    let r = agent_call(&s.app, "/api/v1/agent/enroll", json!({"token": token, "info": info(), "name": name, "tags": tags}), None).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
    let id = r.json()["host_id"].as_str().unwrap().to_string();
    let auth = format!("Bearer {id}.{}", r.json()["secret"].as_str().unwrap());
    (id, auth)
}

fn info() -> Value {
    json!({"hostname": "web-1.internal", "os": "Debian GNU/Linux 13 (trixie)", "arch": "amd64", "version": "test"})
}

async fn log_lines(s: &TestServer, cookie: &str, run: &str) -> Vec<String> {
    let logs = call(&s.app, "GET", &format!("/api/v1/runs/{run}/logs"), None, Some(cookie), false).await.json();
    logs["lines"].as_array().unwrap().iter().map(|l| l["line"].as_str().unwrap().to_string()).collect()
}

async fn start_with(s: &TestServer, cookie: &str, runbook: &str, body: Value) -> Reply {
    call(&s.app, "POST", &format!("/api/v1/runbooks/{runbook}/runs"), Some(body), Some(cookie), true).await
}

async fn bearer_call(app: &Router, method: &str, path: &str, body: Option<Value>, token: &str) -> Reply {
    let mut req = Request::builder().method(method).uri(path).header(header::AUTHORIZATION, format!("Bearer {token}"));
    let req = match body {
        Some(b) => {
            req = req.header(header::CONTENT_TYPE, "application/json");
            req.body(Body::from(b.to_string())).unwrap()
        }
        None => req.body(Body::empty()).unwrap(),
    };
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    Reply { status, cookie: None, body: String::from_utf8_lossy(&bytes).into() }
}

/// A password the tests use after replacing a generated one.
const CHOSEN: &str = "a-chosen-password-1";

async fn sign_in_as(s: &TestServer, name: &str, password: &str) -> Reply {
    call(&s.app, "POST", "/api/v1/auth/login", Some(json!({"username": name, "password": password})), None, true).await
}

/// Create a user with a role in the default workspace, sign them in and replace their
/// generated password. Returns their session cookie.
async fn user_with_role(s: &TestServer, admin: &str, name: &str, role: &str) -> String {
    user_with(s, admin, json!({"name": name, "workspaces": {"default": role}})).await
}

/// Create a user (as the server admin `admin`) from a `POST /users` body, sign them in
/// and replace their generated password. Returns their session cookie.
async fn user_with(s: &TestServer, admin: &str, body: Value) -> String {
    let name = body["name"].as_str().unwrap().to_string();
    let r = call(&s.app, "POST", "/api/v1/users", Some(body), Some(admin), true).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
    let generated = r.json()["password"].as_str().unwrap().to_string();
    let r = sign_in_as(s, &name, &generated).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(r.json()["must_change_password"], true);
    let cookie = r.cookie.unwrap();
    let r = call(&s.app, "POST", "/api/v1/auth/password", Some(json!({"current_password": generated, "new_password": CHOSEN})), Some(&cookie), true).await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{}", r.body);
    cookie
}
