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
    TestServer { app: web::router(state.clone()), state, password, dir }
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

#[tokio::test]
async fn sign_in_csrf_and_sign_out() {
    let s = server();
    assert_eq!(call(&s.app, "GET", "/api/v1/runbooks", None, None, false).await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(call(&s.app, "GET", "/api/v1/system", None, None, false).await.status, StatusCode::OK);
    let no_csrf = call(&s.app, "POST", "/api/v1/auth/login", Some(json!({"username": "admin", "password": s.password})), None, false).await;
    assert_eq!(no_csrf.status, StatusCode::FORBIDDEN);
    let wrong = call(&s.app, "POST", "/api/v1/auth/login", Some(json!({"username": "admin", "password": "nope"})), None, true).await;
    assert_eq!(wrong.status, StatusCode::UNAUTHORIZED);
    assert!(wrong.cookie.is_none());

    let cookie = sign_in(&s).await;
    let me = call(&s.app, "GET", "/api/v1/auth/me", None, Some(&cookie), false).await;
    assert_eq!(me.json()["user"], "admin");
    // Writes need the CSRF header even with a valid session.
    let w = call(&s.app, "POST", "/api/v1/runbooks", Some(json!({"name": "x", "steps": [{"name": "a", "run": "true"}]})), Some(&cookie), false).await;
    assert_eq!(w.status, StatusCode::FORBIDDEN);

    assert_eq!(call(&s.app, "POST", "/api/v1/auth/logout", None, Some(&cookie), true).await.status, StatusCode::NO_CONTENT);
    assert_eq!(call(&s.app, "GET", "/api/v1/auth/me", None, Some(&cookie), false).await.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn runbook_crud_and_validation() {
    let s = server();
    let c = sign_in(&s).await;
    let bad = call(&s.app, "POST", "/api/v1/runbooks", Some(json!({"name": "", "steps": []})), Some(&c), true).await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST);
    assert!(bad.json()["detail"].as_str().unwrap().contains("Name"));
    let unknown = call(&s.app, "POST", "/api/v1/runbooks", Some(json!({"name": "x", "steps": [{"name": "a", "run": "true"}], "extra": 1})), Some(&c), true).await;
    assert_eq!(unknown.status, StatusCode::BAD_REQUEST);

    let id = create(&s, &c, json!({"name": "Check disk", "tags": ["Ops"], "steps": [{"name": "df", "run": "df -h"}]})).await;
    let got = call(&s.app, "GET", &format!("/api/v1/runbooks/{id}"), None, Some(&c), false).await.json();
    assert_eq!(got["name"], "Check disk");
    assert_eq!(got["tags"], json!(["ops"]));
    assert_eq!(got["timeout_seconds"], 600);

    let upd = call(&s.app, "PUT", &format!("/api/v1/runbooks/{id}"), Some(json!({"name": "Check disks", "steps": [{"name": "df", "run": "df -h /"}], "timeout_seconds": 30})), Some(&c), true).await;
    assert_eq!(upd.status, StatusCode::OK);
    assert_eq!(upd.json()["timeout_seconds"], 30);
    assert_eq!(call(&s.app, "GET", "/api/v1/runbooks", None, Some(&c), false).await.json().as_array().unwrap().len(), 1);

    assert_eq!(call(&s.app, "DELETE", &format!("/api/v1/runbooks/{id}"), None, Some(&c), true).await.status, StatusCode::NO_CONTENT);
    assert_eq!(call(&s.app, "GET", &format!("/api/v1/runbooks/{id}"), None, Some(&c), false).await.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn run_success_failure_and_skip() {
    let s = server();
    let c = sign_in(&s).await;
    let id = create(&s, &c, json!({"name": "Mixed", "steps": [
        {"name": "hello", "run": "echo hello from $OPSKILLS_STEP; echo warning >&2"},
        {"name": "fail", "run": "echo about to fail; exit 3"},
        {"name": "never", "run": "echo should not run"}
    ]})).await;
    let run = start(&s, &c, &id).await;
    let done = wait_finished(&s, &c, &run).await;
    assert_eq!(done["status"], "failed");
    let steps = done["steps"].as_array().unwrap();
    assert_eq!(steps[0]["status"], "succeeded");
    assert_eq!(steps[0]["exit_code"], 0);
    assert_eq!(steps[1]["status"], "failed");
    assert_eq!(steps[1]["exit_code"], 3);
    assert_eq!(steps[2]["status"], "skipped");
    assert_eq!(done["started_by"], "admin");

    let logs = call(&s.app, "GET", &format!("/api/v1/runs/{run}/logs"), None, Some(&c), false).await.json();
    let lines: Vec<(String, String)> = logs["lines"].as_array().unwrap().iter().map(|l| (l["stream"].as_str().unwrap().into(), l["line"].as_str().unwrap().into())).collect();
    assert!(lines.contains(&("stdout".into(), "hello from 1".into())), "{lines:?}");
    assert!(lines.contains(&("stderr".into(), "warning".into())));
    assert!(lines.iter().any(|(s, l)| s == "system" && l.contains("exit code 3")));
    assert!(!lines.iter().any(|(_, l)| l.contains("should not run")));

    let list = call(&s.app, "GET", &format!("/api/v1/runs?runbook_id={id}"), None, Some(&c), false).await.json();
    assert_eq!(list.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn step_timeout_stops_the_process() {
    let s = server();
    let c = sign_in(&s).await;
    let id = create(&s, &c, json!({"name": "Slow", "timeout_seconds": 1, "steps": [{"name": "sleep", "run": "echo start; sleep 30"}]})).await;
    let t0 = Instant::now();
    let done = wait_finished(&s, &c, &start(&s, &c, &id).await).await;
    assert_eq!(done["status"], "failed");
    assert!(t0.elapsed() < Duration::from_secs(10), "timeout was not enforced");
    let logs = call(&s.app, "GET", &format!("/api/v1/runs/{}/logs", done["id"].as_str().unwrap()), None, Some(&c), false).await.body;
    assert!(logs.contains("timed out after 1 second and"), "{logs}");
}

#[tokio::test]
async fn cancel_a_running_run() {
    let s = server();
    let c = sign_in(&s).await;
    let id = create(&s, &c, json!({"name": "Long", "steps": [{"name": "sleep", "run": "sleep 30"}, {"name": "after", "run": "true"}]})).await;
    let run = start(&s, &c, &id).await;
    let t0 = Instant::now();
    loop {
        let r = call(&s.app, "GET", &format!("/api/v1/runs/{run}"), None, Some(&c), false).await.json();
        if r["steps"][0]["status"] == "running" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(call(&s.app, "POST", &format!("/api/v1/runs/{run}/cancel"), None, Some(&c), true).await.status, StatusCode::ACCEPTED);
    let done = wait_finished(&s, &c, &run).await;
    assert_eq!(done["status"], "cancelled");
    assert_eq!(done["steps"][1]["status"], "skipped");
    assert!(t0.elapsed() < Duration::from_secs(10));
    assert_eq!(call(&s.app, "POST", &format!("/api/v1/runs/{run}/cancel"), None, Some(&c), true).await.status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn live_events_stream_output_and_end() {
    let s = server();
    let c = sign_in(&s).await;
    let id = create(&s, &c, json!({"name": "Stream", "steps": [{"name": "count", "run": "for i in 1 2 3; do echo line$i; sleep 0.1; done"}]})).await;
    let run = start(&s, &c, &id).await;
    let events = call(&s.app, "GET", &format!("/api/v1/runs/{run}/events"), None, Some(&c), false).await;
    assert_eq!(events.status, StatusCode::OK);
    for expected in ["event: run", "line1", "line2", "line3", "event: end", "\"status\":\"succeeded\""] {
        assert!(events.body.contains(expected), "missing {expected} in:\n{}", events.body);
    }
    // Each line appears exactly once even though the subscriber joined mid-run.
    assert_eq!(events.body.matches("\"line\":\"line2\"").count(), 1);
    // A finished run replays its stored output.
    let replay = call(&s.app, "GET", &format!("/api/v1/runs/{run}/events"), None, Some(&c), false).await.body;
    assert!(replay.contains("line3") && replay.contains("event: end"));
}

#[tokio::test]
async fn interrupted_runs_are_marked_failed() {
    let dir = std::env::temp_dir().join(format!("opskills-it-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("opskills.redb");
    {
        let store = crate::store::Store::open(&path).unwrap();
        let rb = crate::model::Runbook {
            id: "rb".into(),
            name: "x".into(),
            description: String::new(),
            tags: vec![],
            inputs: vec![],
            steps: vec![crate::model::Step { name: "a".into(), run: "true".into(), ..Default::default() }],
            on_step_failure: crate::model::FailurePolicy::Stop,
            output_limit: crate::model::OutputLimit::default(),
            rollout: crate::model::Rollout::default(),
            schedules: vec![],
            notifications: vec![],
            secrets: vec![],
            targets: crate::model::Targets::default(),
            timeout_seconds: 60,
            created_at: 0,
            updated_at: 0,
        };
        let mut run = crate::model::Run::new(&rb, "admin", vec![crate::model::RunHost { id: "local".into(), name: "OpSkills server".into() }], &[]);
        run.status = crate::model::Status::Running;
        run.steps[0].status = crate::model::Status::Running;
        store.put_run(&run).unwrap();
        assert_eq!(store.fail_interrupted_runs().unwrap(), 1);
        let after = store.run(&run.id).unwrap().unwrap();
        assert_eq!(after.status, crate::model::Status::Failed);
        assert!(after.error.unwrap().contains("server stopped"));
    }
    std::fs::remove_dir_all(dir).unwrap();
}

const SECRET_VALUE: &str = "s3cr3t-Value-8472";

async fn add_secret(s: &TestServer, c: &str, name: &str, value: &str) -> Reply {
    call(&s.app, "POST", "/api/v1/secrets", Some(json!({"name": name, "kind": "password", "description": "test", "value": value})), Some(c), true).await
}

#[tokio::test]
async fn vault_values_are_write_only_and_encrypted_at_rest() {
    let s = server();
    let c = sign_in(&s).await;
    let created = add_secret(&s, &c, "DB_PASSWORD", SECRET_VALUE).await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    assert!(!created.body.contains(SECRET_VALUE));
    let list = call(&s.app, "GET", "/api/v1/secrets", None, Some(&c), false).await;
    assert!(list.body.contains("DB_PASSWORD") && !list.body.contains(SECRET_VALUE), "{}", list.body);
    assert!(list.json()[0].get("value").is_none() && list.json()[0].get("ciphertext").is_none());
    // Duplicate names are refused.
    assert_eq!(add_secret(&s, &c, "DB_PASSWORD", "another-value").await.status, StatusCode::CONFLICT);
    // The database file never contains the plaintext.
    let bytes = std::fs::read(s.dir.join("opskills.redb")).unwrap();
    assert!(!bytes.windows(SECRET_VALUE.len()).any(|w| w == SECRET_VALUE.as_bytes()), "plaintext found on disk");
}

#[tokio::test]
async fn secret_validation() {
    let s = server();
    let c = sign_in(&s).await;
    for (name, value) in [("db_password", "long-enough"), ("PATH", "long-enough"), ("OPSKILLS_KEY", "long-enough"), ("GOOD_NAME", "abc")] {
        assert_eq!(add_secret(&s, &c, name, value).await.status, StatusCode::BAD_REQUEST, "{name}");
    }
    let bad_kind = call(&s.app, "POST", "/api/v1/secrets", Some(json!({"name": "X_TOKEN", "kind": "magic", "value": "long-enough"})), Some(&c), true).await;
    assert_eq!(bad_kind.status, StatusCode::BAD_REQUEST);
    // A runbook can't reference a secret that isn't in the vault.
    let missing = call(&s.app, "POST", "/api/v1/runbooks", Some(json!({"name": "x", "secrets": ["NOPE"], "steps": [{"name": "a", "run": "true"}]})), Some(&c), true).await;
    assert_eq!(missing.status, StatusCode::BAD_REQUEST);
    assert!(missing.json()["detail"].as_str().unwrap().contains("NOPE"));
}

#[tokio::test]
async fn runs_get_listed_secrets_as_env_and_output_is_masked() {
    let s = server();
    let c = sign_in(&s).await;
    add_secret(&s, &c, "DB_PASSWORD", SECRET_VALUE).await;
    add_secret(&s, &c, "OTHER_TOKEN", "unrelated-token-123").await;
    let id = create(&s, &c, json!({"name": "Uses secret", "secrets": ["DB_PASSWORD"], "steps": [
        // Checks the value without writing it out, so the command itself holds no secret.
        {"name": "check", "run": "test \"${#DB_PASSWORD}\" -eq 17 && case \"$DB_PASSWORD\" in s3cr3t-*8472) echo value matches;; *) exit 1;; esac"},
        {"name": "leak", "run": "echo \"the password is $DB_PASSWORD\"; echo \"$DB_PASSWORD\" >&2"},
        {"name": "isolation", "run": "test -z \"$OTHER_TOKEN\" && echo other secret not exposed"}
    ]})).await;
    let done = wait_finished(&s, &c, &start(&s, &c, &id).await).await;
    assert_eq!(done["status"], "succeeded", "{done}");
    assert_eq!(done["secrets"], json!(["DB_PASSWORD"]));
    let run = done["id"].as_str().unwrap();
    let logs = call(&s.app, "GET", &format!("/api/v1/runs/{run}/logs"), None, Some(&c), false).await.body;
    assert!(logs.contains("value matches"), "{logs}");
    assert!(logs.contains("the password is ••••••"), "{logs}");
    assert!(logs.contains("other secret not exposed"));
    assert!(!logs.contains(SECRET_VALUE), "secret leaked into stored output");
    let events = call(&s.app, "GET", &format!("/api/v1/runs/{run}/events"), None, Some(&c), false).await.body;
    assert!(!events.contains(SECRET_VALUE), "secret leaked into live events");
    // Neither the database nor the run record holds the plaintext.
    let bytes = std::fs::read(s.dir.join("opskills.redb")).unwrap();
    assert!(!bytes.windows(SECRET_VALUE.len()).any(|w| w == SECRET_VALUE.as_bytes()));
    // The secrets list shows where it's used.
    let list = call(&s.app, "GET", "/api/v1/secrets", None, Some(&c), false).await.json();
    let db = list.as_array().unwrap().iter().find(|x| x["name"] == "DB_PASSWORD").unwrap().clone();
    assert_eq!(db["used_by"][0]["name"], "Uses secret");
}

#[tokio::test]
async fn replacing_and_deleting_secrets() {
    let s = server();
    let c = sign_in(&s).await;
    add_secret(&s, &c, "API_TOKEN", "first-value-111").await;
    let id = create(&s, &c, json!({"name": "Token check", "secrets": ["API_TOKEN"], "steps": [{"name": "check", "run": "test \"$API_TOKEN\" = second-value-222"}]})).await;
    let upd = call(&s.app, "PUT", "/api/v1/secrets/API_TOKEN", Some(json!({"value": "second-value-222", "description": "rotated"})), Some(&c), true).await;
    assert_eq!(upd.status, StatusCode::OK, "{}", upd.body);
    assert_eq!(upd.json()["description"], "rotated");
    assert!(!upd.body.contains("second-value-222"));
    let done = wait_finished(&s, &c, &start(&s, &c, &id).await).await;
    assert_eq!(done["status"], "succeeded", "the new value was not used");
    // In use: deleting is refused and names the runbook.
    let refused = call(&s.app, "DELETE", "/api/v1/secrets/API_TOKEN", None, Some(&c), true).await;
    assert_eq!(refused.status, StatusCode::CONFLICT);
    assert!(refused.json()["detail"].as_str().unwrap().contains("Token check"));
    // After the runbook stops using it, it can be deleted.
    let r = call(&s.app, "PUT", &format!("/api/v1/runbooks/{id}"), Some(json!({"name": "Token check", "steps": [{"name": "check", "run": "true"}]})), Some(&c), true).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(call(&s.app, "DELETE", "/api/v1/secrets/API_TOKEN", None, Some(&c), true).await.status, StatusCode::NO_CONTENT);
    assert_eq!(call(&s.app, "PUT", "/api/v1/secrets/API_TOKEN", Some(json!({"value": "x-value"})), Some(&c), true).await.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn multi_line_secrets_are_masked_line_by_line() {
    let s = server();
    let c = sign_in(&s).await;
    let key = "-----BEGIN KEY-----\nAAAAB3NzaC1yc2EAAAADAQABAAABgQ\nZZZZtopsecretlineZZZZ\n-----END KEY-----";
    let r = call(&s.app, "POST", "/api/v1/secrets", Some(json!({"name": "DEPLOY_KEY", "kind": "ssh_key", "value": key})), Some(&c), true).await;
    assert_eq!(r.status, StatusCode::CREATED);
    let id = create(&s, &c, json!({"name": "Key", "secrets": ["DEPLOY_KEY"], "steps": [{"name": "print", "run": "printf '%s\\n' \"$DEPLOY_KEY\""}]})).await;
    let done = wait_finished(&s, &c, &start(&s, &c, &id).await).await;
    let logs = call(&s.app, "GET", &format!("/api/v1/runs/{}/logs", done["id"].as_str().unwrap()), None, Some(&c), false).await.body;
    assert!(!logs.contains("ZZZZtopsecretlineZZZZ") && !logs.contains("AAAAB3NzaC1yc2EAAAADAQABAAABgQ"), "{logs}");
}

// Hosts and the agent protocol

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

fn info() -> Value {
    json!({"hostname": "web-1.internal", "os": "Debian GNU/Linux 13 (trixie)", "arch": "amd64", "version": "test"})
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

#[tokio::test]
async fn enrollment_tokens_work_once_and_agents_need_their_credential() {
    let s = server();
    let c = sign_in(&s).await;
    let t = call(&s.app, "POST", "/api/v1/hosts/enrollment-tokens", None, Some(&c), true).await;
    let token = t.json()["token"].as_str().unwrap().to_string();
    let first = agent_call(&s.app, "/api/v1/agent/enroll", json!({"token": token, "info": info(), "tags": ["Web", "web", "prod"]}), None).await;
    assert_eq!(first.status, StatusCode::CREATED, "{}", first.body);
    let again = agent_call(&s.app, "/api/v1/agent/enroll", json!({"token": token, "info": info()}), None).await;
    assert_eq!(again.status, StatusCode::UNAUTHORIZED);

    // Without a session the token endpoint is closed; without a credential the agent API is.
    assert_eq!(call(&s.app, "POST", "/api/v1/hosts/enrollment-tokens", None, None, true).await.status, StatusCode::UNAUTHORIZED);
    let id = first.json()["host_id"].as_str().unwrap().to_string();
    assert_eq!(agent_call(&s.app, "/api/v1/agent/poll", info(), None).await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(agent_call(&s.app, "/api/v1/agent/poll", info(), Some(&format!("Bearer {id}.wrong"))).await.status, StatusCode::UNAUTHORIZED);

    // The host is listed with its tags (lower-cased, no duplicates), after the server itself.
    let hosts = call(&s.app, "GET", "/api/v1/hosts", None, Some(&c), false).await.json();
    assert_eq!(hosts[0]["builtin"], true);
    assert_eq!(hosts[1]["name"], "web-1.internal");
    assert_eq!(hosts[1]["tags"], json!(["web", "prod"]));
    assert_eq!(hosts[1]["online"], true);
    assert!(hosts[1].get("secret_hash").is_none());
}

#[tokio::test]
async fn runs_on_an_enrolled_host_through_the_agent() {
    let s = server();
    let c = sign_in(&s).await;
    add_secret(&s, &c, "DEPLOY_KEY", "super-secret-1").await;
    let (_, auth) = enroll(&s, &c, "web-1", json!(["web"])).await;
    let rb = create(&s, &c, json!({"name": "On web", "steps": [{"name": "Hello", "run": "echo hi"}], "secrets": ["DEPLOY_KEY"], "targets": {"tags": ["web"]}})).await;
    let run = start(&s, &c, &rb).await;

    let job = agent_call(&s.app, "/api/v1/agent/poll", info(), Some(&auth)).await;
    assert_eq!(job.status, StatusCode::OK, "{}", job.body);
    let job = job.json();
    assert_eq!(job["command"], "echo hi");
    let env = job["env"].as_array().unwrap();
    assert!(env.contains(&json!(["DEPLOY_KEY", "super-secret-1"])), "{env:?}");
    assert!(env.contains(&json!(["OPSKILLS_HOST_NAME", "web-1"])), "{env:?}");
    assert!(env.contains(&json!(["OPSKILLS_RUNBOOK_NAME", "On web"])), "{env:?}");
    let id = job["id"].as_str().unwrap();
    let out = agent_call(&s.app, &format!("/api/v1/agent/jobs/{id}/output"), json!({"lines": [{"stream": "stdout", "line": "hi"}, {"stream": "stdout", "line": "key super-secret-1"}]}), Some(&auth)).await;
    assert_eq!(out.json()["cancel"], false);
    let fin = agent_call(&s.app, &format!("/api/v1/agent/jobs/{id}/finish"), json!({"end": "exited", "exit_code": 0}), Some(&auth)).await;
    assert_eq!(fin.status, StatusCode::NO_CONTENT);

    let done = wait_finished(&s, &c, &run).await;
    assert_eq!(done["status"], "succeeded");
    assert_eq!(done["hosts"][0]["name"], "web-1");
    assert_eq!(done["steps"][0]["host_name"], "web-1");
    let logs = call(&s.app, "GET", &format!("/api/v1/runs/{run}/logs"), None, Some(&c), false).await.json();
    let lines: Vec<(String, String)> = logs["lines"].as_array().unwrap().iter().map(|l| (l["host"].as_str().unwrap().into(), l["line"].as_str().unwrap().into())).collect();
    assert!(lines.contains(&("web-1".into(), "hi".into())), "{lines:?}");
    assert!(lines.contains(&("web-1".into(), format!("key {}", crate::executor::MASK))), "{lines:?}");

    // Another host's credential can't touch this host's steps.
    let (_, other) = enroll(&s, &c, "db-1", json!([])).await;
    assert_eq!(agent_call(&s.app, &format!("/api/v1/agent/jobs/{id}/output"), json!({"lines": []}), Some(&other)).await.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn cancelling_a_run_reaches_the_agent() {
    let s = server();
    let c = sign_in(&s).await;
    let (host, auth) = enroll(&s, &c, "web-1", json!([])).await;
    let rb = create(&s, &c, json!({"name": "Long", "steps": [{"name": "Wait", "run": "sleep 60"}], "targets": {"hosts": [host]}})).await;
    let run = start(&s, &c, &rb).await;
    let job = agent_call(&s.app, "/api/v1/agent/poll", info(), Some(&auth)).await.json();
    let id = job["id"].as_str().unwrap().to_string();
    let output = format!("/api/v1/agent/jobs/{id}/output");
    assert_eq!(agent_call(&s.app, &output, json!({"lines": []}), Some(&auth)).await.json()["cancel"], false);
    assert_eq!(call(&s.app, "POST", &format!("/api/v1/runs/{run}/cancel"), None, Some(&c), true).await.status, StatusCode::ACCEPTED);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if agent_call(&s.app, &output, json!({"lines": []}), Some(&auth)).await.json()["cancel"] == true {
            break;
        }
        assert!(Instant::now() < deadline, "the agent was never told to cancel");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    agent_call(&s.app, &format!("/api/v1/agent/jobs/{id}/finish"), json!({"end": "cancelled", "exit_code": 143}), Some(&auth)).await;
    assert_eq!(wait_finished(&s, &c, &run).await["status"], "cancelled");
}

#[tokio::test]
async fn host_targets_are_checked_and_targeted_hosts_are_protected() {
    let s = server();
    let c = sign_in(&s).await;
    let bad = call(&s.app, "POST", "/api/v1/runbooks", Some(json!({"name": "X", "steps": [{"name": "a", "run": "true"}], "targets": {"hosts": ["no-such-host"]}})), Some(&c), true).await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST);
    let none = create(&s, &c, json!({"name": "Nobody", "steps": [{"name": "a", "run": "true"}], "targets": {"tags": ["empty"]}})).await;
    assert_eq!(call(&s.app, "POST", &format!("/api/v1/runbooks/{none}/runs"), None, Some(&c), true).await.status, StatusCode::CONFLICT);

    let (host, _) = enroll(&s, &c, "web-1", json!([])).await;
    let rb = create(&s, &c, json!({"name": "Pinned", "steps": [{"name": "a", "run": "true"}], "targets": {"hosts": [host]}})).await;
    assert_eq!(call(&s.app, "DELETE", &format!("/api/v1/hosts/{host}"), None, Some(&c), true).await.status, StatusCode::CONFLICT);
    assert_eq!(call(&s.app, "DELETE", "/api/v1/hosts/local", None, Some(&c), true).await.status, StatusCode::CONFLICT);
    call(&s.app, "DELETE", &format!("/api/v1/runbooks/{rb}"), None, Some(&c), true).await;
    assert_eq!(call(&s.app, "DELETE", &format!("/api/v1/hosts/{host}"), None, Some(&c), true).await.status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn agents_can_check_their_credential() {
    let s = server();
    let c = sign_in(&s).await;
    let (host, auth) = enroll(&s, &c, "web-1", json!([])).await;
    let req = |a: &str| Request::builder().uri("/api/v1/agent/whoami").header(header::AUTHORIZATION, a).body(Body::empty()).unwrap();
    let ok = s.app.clone().oneshot(req(&auth)).await.unwrap();
    assert_eq!(ok.status(), StatusCode::OK);
    call(&s.app, "DELETE", &format!("/api/v1/hosts/{host}"), None, Some(&c), true).await;
    assert_eq!(s.app.clone().oneshot(req(&auth)).await.unwrap().status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn the_install_script_is_served_for_this_version() {
    let s = server();
    let r = call(&s.app, "GET", "/install-agent.sh", None, None, false).await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r.body.starts_with("#!/bin/sh"));
    assert!(r.body.contains(&format!("VERSION=\"{}\"", crate::VERSION)));
    assert!(!r.body.contains("@VERSION@"));
}

// Inputs, templates, retries and failure handling

async fn start_with(s: &TestServer, cookie: &str, runbook: &str, body: Value) -> Reply {
    call(&s.app, "POST", &format!("/api/v1/runbooks/{runbook}/runs"), Some(body), Some(cookie), true).await
}

async fn log_lines(s: &TestServer, cookie: &str, run: &str) -> Vec<String> {
    let logs = call(&s.app, "GET", &format!("/api/v1/runs/{run}/logs"), None, Some(cookie), false).await.json();
    logs["lines"].as_array().unwrap().iter().map(|l| l["line"].as_str().unwrap().to_string()).collect()
}

#[tokio::test]
async fn inputs_reach_steps_as_quoted_words_and_env_vars() {
    let s = server();
    let c = sign_in(&s).await;
    let rb = create(&s, &c, json!({
        "name": "Restart",
        "inputs": [
            {"name": "service", "type": "text", "required": true},
            {"name": "count", "type": "number", "default": "3"},
            {"name": "token", "type": "secret"},
            {"name": "region", "type": "choice", "choices": ["eu", "us"], "required": true}
        ],
        "steps": [{"name": "show", "run": "printf '[%s]\\n' {{ inputs.service }} \"$OPSKILLS_INPUT_COUNT\" {{ inputs.region }} {{ host.name }} \"token=$OPSKILLS_INPUT_TOKEN\""}]
    }))
    .await;

    let missing = start_with(&s, &c, &rb, json!({"inputs": {"service": "web"}})).await;
    assert_eq!(missing.status, StatusCode::BAD_REQUEST, "{}", missing.body);
    let wrong = start_with(&s, &c, &rb, json!({"inputs": {"service": "web", "region": "asia"}})).await;
    assert_eq!(wrong.status, StatusCode::BAD_REQUEST, "{}", wrong.body);
    let unknown = start_with(&s, &c, &rb, json!({"inputs": {"service": "web", "region": "eu", "extra": 1}})).await;
    assert_eq!(unknown.status, StatusCode::BAD_REQUEST, "{}", unknown.body);

    let r = start_with(&s, &c, &rb, json!({"inputs": {"service": "web app; echo INJECTED", "region": "eu", "token": "tok-9921"}})).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
    let run = r.json()["id"].as_str().unwrap().to_string();
    let done = wait_finished(&s, &c, &run).await;
    assert_eq!(done["status"], "succeeded", "{done}");
    assert_eq!(done["inputs"]["service"], "web app; echo INJECTED");
    assert_eq!(done["inputs"]["count"], "3");
    assert_eq!(done["inputs"]["token"], crate::executor::MASK, "secret inputs are not stored");
    let lines = log_lines(&s, &c, &run).await;
    for want in ["[web app; echo INJECTED]", "[3]", "[eu]", "[OpSkills server]"] {
        assert!(lines.iter().any(|l| l == want), "{want} in {lines:?}");
    }
    assert!(!lines.iter().any(|l| l == "INJECTED"), "an input value ran as a command: {lines:?}");
    assert!(lines.iter().any(|l| l == &format!("[token={}]", crate::executor::MASK)), "{lines:?}");
    assert!(!lines.iter().any(|l| l.contains("tok-9921")), "{lines:?}");
}

#[tokio::test]
async fn retries_and_continue_on_error() {
    let s = server();
    let c = sign_in(&s).await;
    let rb = create(&s, &c, json!({
        "name": "Flaky",
        "steps": [
            {"name": "third time lucky", "run": "n=$(cat tries 2>/dev/null || echo 0); n=$((n+1)); echo $n > tries; echo try $n; [ $n -ge 3 ]", "retries": 3},
            {"name": "optional", "run": "exit 4", "continue_on_error": true},
            {"name": "after", "run": "echo after"}
        ]
    }))
    .await;
    let done = wait_finished(&s, &c, &start(&s, &c, &rb).await).await;
    assert_eq!(done["status"], "succeeded", "{done}");
    let steps = done["steps"].as_array().unwrap();
    assert_eq!((steps[0]["status"].as_str(), steps[0]["attempts"].as_u64()), (Some("succeeded"), Some(3)));
    assert_eq!((steps[1]["status"].as_str(), steps[1]["exit_code"].as_i64()), (Some("failed"), Some(4)));
    assert_eq!(steps[2]["status"], "succeeded");
    let lines = log_lines(&s, &c, done["id"].as_str().unwrap()).await;
    assert!(lines.iter().any(|l| l == "Attempt 3 of 4."), "{lines:?}");
    assert!(lines.iter().any(|l| l.contains("continues on error")), "{lines:?}");

    let always = create(&s, &c, json!({"name": "Always fails", "steps": [{"name": "no", "run": "exit 2", "retries": 1, "retry_delay_seconds": "1s"}]})).await;
    let t0 = Instant::now();
    let done = wait_finished(&s, &c, &start(&s, &c, &always).await).await;
    assert_eq!((done["status"].as_str(), done["steps"][0]["attempts"].as_u64()), (Some("failed"), Some(2)));
    assert!(t0.elapsed() >= Duration::from_secs(1), "retry delay was not applied");
}

#[tokio::test]
async fn finish_then_fail_runs_the_remaining_steps() {
    let s = server();
    let c = sign_in(&s).await;
    let body = |policy: &str| json!({"name": "Policy", "on_step_failure": policy, "steps": [{"name": "breaks", "run": "exit 1"}, {"name": "cleanup", "run": "echo cleaned"}]});
    let finish = create(&s, &c, body("finish_then_fail")).await;
    let done = wait_finished(&s, &c, &start(&s, &c, &finish).await).await;
    assert_eq!(done["status"], "failed");
    assert_eq!(done["steps"][1]["status"], "succeeded");
    let stop = create(&s, &c, body("stop")).await;
    let done = wait_finished(&s, &c, &start(&s, &c, &stop).await).await;
    assert_eq!((done["status"].as_str(), done["steps"][1]["status"].as_str()), (Some("failed"), Some("skipped")));
    let bad = call(&s.app, "POST", "/api/v1/runbooks", Some(body("sometimes")), Some(&c), true).await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn an_output_limit_can_stop_the_run() {
    let s = server();
    let c = sign_in(&s).await;
    let rb = create(&s, &c, json!({
        "name": "Chatty",
        "output_limit": {"lines": 50, "action": "fail"},
        "steps": [{"name": "spam", "run": "seq 1 100000; sleep 30"}, {"name": "next", "run": "echo next"}]
    }))
    .await;
    let t0 = Instant::now();
    let done = wait_finished(&s, &c, &start(&s, &c, &rb).await).await;
    assert!(t0.elapsed() < Duration::from_secs(15), "the step was not stopped");
    assert_eq!((done["status"].as_str(), done["steps"][0]["status"].as_str(), done["steps"][1]["status"].as_str()), (Some("failed"), Some("failed"), Some("skipped")));
    assert!(done["log_lines"].as_u64().unwrap() <= 52, "{done}");
    let lines = log_lines(&s, &c, done["id"].as_str().unwrap()).await;
    assert!(lines.iter().any(|l| l.contains("output limit of 50 lines")), "{lines:?}");

    let truncating = create(&s, &c, json!({"name": "Quiet", "output_limit": {"lines": 20}, "steps": [{"name": "spam", "run": "seq 1 1000"}]})).await;
    let done = wait_finished(&s, &c, &start(&s, &c, &truncating).await).await;
    assert_eq!(done["status"], "succeeded");
    assert!(done["log_lines"].as_u64().unwrap() <= 21, "{done}");
}

#[tokio::test]
async fn templates_must_name_known_values() {
    let s = server();
    let c = sign_in(&s).await;
    let r = call(&s.app, "POST", "/api/v1/runbooks", Some(json!({"name": "Typo", "steps": [{"name": "a", "run": "echo {{ inputs.servce }}"}]})), Some(&c), true).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert!(r.json()["detail"].as_str().unwrap().contains("inputs.servce"), "{}", r.body);
    // Other uses of double braces, like Go templates, are left alone.
    let id = create(&s, &c, json!({"name": "Go template", "steps": [{"name": "a", "run": "echo '{{.Names}}'"}]})).await;
    let done = wait_finished(&s, &c, &start(&s, &c, &id).await).await;
    assert!(log_lines(&s, &c, done["id"].as_str().unwrap()).await.iter().any(|l| l == "{{.Names}}"));
}

#[tokio::test]
async fn retried_steps_on_a_host_keep_their_work_folder() {
    let s = server();
    let c = sign_in(&s).await;
    let (_, auth) = enroll(&s, &c, "web-1", json!(["web"])).await;
    let rb = create(&s, &c, json!({"name": "Remote retry", "targets": {"tags": ["web"]}, "steps": [{"name": "flaky", "run": "true", "retries": 1}]})).await;
    let run = start(&s, &c, &rb).await;
    let first = agent_call(&s.app, "/api/v1/agent/poll", info(), Some(&auth)).await.json();
    assert_eq!((first["last"].as_bool(), first["keep_on_failure"].as_bool()), (Some(true), Some(true)), "{first}");
    let id = first["id"].as_str().unwrap();
    agent_call(&s.app, &format!("/api/v1/agent/jobs/{id}/output"), json!({"lines": []}), Some(&auth)).await;
    agent_call(&s.app, &format!("/api/v1/agent/jobs/{id}/finish"), json!({"end": "exited", "exit_code": 1}), Some(&auth)).await;
    let second = agent_call(&s.app, "/api/v1/agent/poll", info(), Some(&auth)).await.json();
    assert_eq!(second["keep_on_failure"].as_bool(), Some(false), "the last try can clean up: {second}");
    let id = second["id"].as_str().unwrap();
    agent_call(&s.app, &format!("/api/v1/agent/jobs/{id}/output"), json!({"lines": []}), Some(&auth)).await;
    agent_call(&s.app, &format!("/api/v1/agent/jobs/{id}/finish"), json!({"end": "exited", "exit_code": 0}), Some(&auth)).await;
    let done = wait_finished(&s, &c, &run).await;
    assert_eq!((done["status"].as_str(), done["steps"][0]["attempts"].as_u64()), (Some("succeeded"), Some(2)));
}

// Copy and call steps

#[tokio::test]
async fn copy_steps_write_files_with_their_mode() {
    let s = server();
    let c = sign_in(&s).await;
    let rb = create(&s, &c, json!({
        "name": "Config",
        "inputs": [{"name": "who", "type": "text", "default": "world"}],
        "steps": [
            {"name": "write", "copy": {"to": "conf/app.conf", "content": "hello {{ inputs.who }}\nhost {{ host.name }}\n", "mode": "640"}},
            {"name": "check", "run": "cat conf/app.conf; find conf/app.conf -perm 0640 | grep -q . && echo mode-ok; echo leftovers=$(ls -a conf | grep -c opskills-copy)"}
        ]
    }))
    .await;
    let got = call(&s.app, "GET", &format!("/api/v1/runbooks/{rb}"), None, Some(&c), false).await.json();
    assert_eq!(got["steps"][0]["copy"]["mode"], "0640", "modes are normalized");
    let done = wait_finished(&s, &c, &start(&s, &c, &rb).await).await;
    assert_eq!(done["status"], "succeeded", "{done}");
    let lines = log_lines(&s, &c, done["id"].as_str().unwrap()).await;
    for want in ["hello world", "host OpSkills server", "mode-ok", "leftovers=0"] {
        assert!(lines.iter().any(|l| l == want), "{want} in {lines:?}");
    }
    assert!(lines.iter().any(|l| l == "Wrote 33 bytes to conf/app.conf (mode 0640)"), "{lines:?}");
}

#[tokio::test]
async fn copy_steps_can_write_a_vault_secret() {
    let s = server();
    let c = sign_in(&s).await;
    add_secret(&s, &c, "TLS_KEY", "-----KEY 77213-----").await;
    let bad = [
        json!({"name": "a", "copy": {"to": "k", "secret": "TLS_KEY"}}),
        json!({"name": "a", "copy": {"to": "k", "content": "x", "secret": "TLS_KEY"}}),
        json!({"name": "a", "copy": {"to": "k", "content": "x", "mode": "999"}}),
        json!({"name": "a", "copy": {"to": "", "content": "x"}}),
        json!({"name": "a", "run": "true", "copy": {"to": "k", "content": "x"}}),
    ];
    for step in bad {
        let r = call(&s.app, "POST", "/api/v1/runbooks", Some(json!({"name": "Bad", "steps": [step]})), Some(&c), true).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{step}: {}", r.body);
    }
    let rb = create(&s, &c, json!({
        "name": "Install key",
        "secrets": ["TLS_KEY"],
        "steps": [
            {"name": "write", "copy": {"to": "key.pem", "secret": "TLS_KEY"}},
            {"name": "check", "run": "cat key.pem; echo; find key.pem -perm 0600 | grep -q . && echo private"}
        ]
    }))
    .await;
    let done = wait_finished(&s, &c, &start(&s, &c, &rb).await).await;
    assert_eq!(done["status"], "succeeded", "{done}");
    let lines = log_lines(&s, &c, done["id"].as_str().unwrap()).await;
    assert!(lines.iter().any(|l| l == crate::executor::MASK), "the value is masked: {lines:?}");
    assert!(lines.iter().any(|l| l == "private"), "secrets default to 0600: {lines:?}");
    assert!(!lines.iter().any(|l| l.contains("77213")), "{lines:?}");
}

#[tokio::test]
async fn copy_steps_reach_agents_without_the_content_on_the_command_line() {
    let s = server();
    let c = sign_in(&s).await;
    let (_, auth) = enroll(&s, &c, "web-1", json!(["web"])).await;
    let rb = create(&s, &c, json!({"name": "Remote copy", "targets": {"tags": ["web"]}, "steps": [{"name": "w", "copy": {"to": "/etc/motd", "content": "welcome to {{ host.name }}"}}]})).await;
    let run = start(&s, &c, &rb).await;
    let job = agent_call(&s.app, "/api/v1/agent/poll", info(), Some(&auth)).await.json();
    let command = job["command"].as_str().unwrap();
    assert!(command.contains("dest='/etc/motd'") && command.contains("mktemp"), "{command}");
    assert!(!command.contains("welcome"), "{command}");
    assert!(job["env"].as_array().unwrap().contains(&json!(["OPSKILLS_COPY_CONTENT", "welcome to web-1"])), "{job}");
    let id = job["id"].as_str().unwrap();
    agent_call(&s.app, &format!("/api/v1/agent/jobs/{id}/output"), json!({"lines": []}), Some(&auth)).await;
    agent_call(&s.app, &format!("/api/v1/agent/jobs/{id}/finish"), json!({"end": "exited", "exit_code": 0}), Some(&auth)).await;
    assert_eq!(wait_finished(&s, &c, &run).await["status"], "succeeded");
}

#[tokio::test]
async fn call_steps_run_another_runbook_on_the_same_host() {
    // One run slot: a parent waiting for its child must not block the child.
    let s = server_with(|cfg| cfg.runs.max_concurrent = 1);
    let c = sign_in(&s).await;
    let child = create(&s, &c, json!({
        "name": "Drain",
        "inputs": [{"name": "target", "type": "text", "required": true}],
        "steps": [{"name": "drain", "run": "echo child got {{ inputs.target }} on {{ host.name }}"}]
    }))
    .await;
    let parent = create(&s, &c, json!({
        "name": "Rolling restart",
        "inputs": [{"name": "svc", "type": "text", "required": true}],
        "steps": [
            {"name": "drain first", "call": {"runbook": child, "inputs": {"target": "{{ inputs.svc }}-x"}}},
            {"name": "after", "run": "echo after"}
        ]
    }))
    .await;
    let r = start_with(&s, &c, &parent, json!({"inputs": {"svc": "api"}})).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
    let done = wait_finished(&s, &c, r.json()["id"].as_str().unwrap()).await;
    assert_eq!(done["status"], "succeeded", "{done}");
    assert_eq!(done["steps"][1]["status"], "succeeded");
    let child_id = done["steps"][0]["child_run_id"].as_str().expect("child run id").to_string();
    let child_run = call(&s.app, "GET", &format!("/api/v1/runs/{child_id}"), None, Some(&c), false).await.json();
    assert_eq!((child_run["trigger"].as_str(), child_run["depth"].as_u64()), (Some("runbook"), Some(1)), "{child_run}");
    assert_eq!(child_run["parent"]["run_id"], done["id"]);
    assert_eq!(child_run["inputs"]["target"], "api-x");
    assert!(log_lines(&s, &c, &child_id).await.iter().any(|l| l == "child got api-x on OpSkills server"));
    assert!(log_lines(&s, &c, done["id"].as_str().unwrap()).await.iter().any(|l| l.starts_with("Running \"Drain\" as run #")));
}

#[tokio::test]
async fn call_steps_are_checked_and_failures_pass_up() {
    let s = server();
    let c = sign_in(&s).await;
    let needs = create(&s, &c, json!({"name": "Needs input", "inputs": [{"name": "x", "type": "text", "required": true}], "steps": [{"name": "a", "run": "true"}]})).await;
    let fails = create(&s, &c, json!({"name": "Fails", "steps": [{"name": "a", "run": "exit 3"}]})).await;
    for step in [
        json!({"name": "a", "call": {"runbook": "no-such-id"}}),
        json!({"name": "a", "call": {"runbook": needs}}),
        json!({"name": "a", "call": {"runbook": needs, "inputs": {"x": "1", "y": "2"}}}),
        json!({"name": "a", "call": {"runbook": needs, "inputs": {"x": "{{ inputs.missing }}"}}}),
    ] {
        let r = call(&s.app, "POST", "/api/v1/runbooks", Some(json!({"name": "Bad", "steps": [step]})), Some(&c), true).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{step}: {}", r.body);
    }

    let parent = create(&s, &c, json!({"name": "Parent", "steps": [{"name": "call", "call": {"runbook": fails}}, {"name": "next", "run": "echo next"}]})).await;
    let done = wait_finished(&s, &c, &start(&s, &c, &parent).await).await;
    assert_eq!((done["status"].as_str(), done["steps"][0]["status"].as_str(), done["steps"][1]["status"].as_str()), (Some("failed"), Some("failed"), Some("skipped")));
    let lines = log_lines(&s, &c, done["id"].as_str().unwrap()).await;
    assert!(lines.iter().any(|l| l.starts_with("The called run #") && l.ends_with("failed.")), "{lines:?}");

    // Parent calls Fails; Fails calling Parent would loop, and so would Parent calling itself.
    let looped = call(&s.app, "PUT", &format!("/api/v1/runbooks/{fails}"), Some(json!({"name": "Fails", "steps": [{"name": "a", "call": {"runbook": parent}}]})), Some(&c), true).await;
    assert_eq!(looped.status, StatusCode::BAD_REQUEST, "{}", looped.body);
    let itself = call(&s.app, "PUT", &format!("/api/v1/runbooks/{parent}"), Some(json!({"name": "Parent", "steps": [{"name": "a", "call": {"runbook": parent}}]})), Some(&c), true).await;
    assert_eq!(itself.status, StatusCode::BAD_REQUEST, "{}", itself.body);
    // A called runbook can't be deleted while something calls it.
    let del = call(&s.app, "DELETE", &format!("/api/v1/runbooks/{fails}"), None, Some(&c), true).await;
    assert_eq!(del.status, StatusCode::CONFLICT, "{}", del.body);
    assert!(del.json()["detail"].as_str().unwrap().contains("Parent"));
}

#[tokio::test]
async fn cancelling_a_parent_cancels_its_child() {
    let s = server();
    let c = sign_in(&s).await;
    let slow = create(&s, &c, json!({"name": "Slow", "steps": [{"name": "sleep", "run": "echo started; sleep 30"}]})).await;
    let parent = create(&s, &c, json!({"name": "Waits", "steps": [{"name": "call", "call": {"runbook": slow}}]})).await;
    let run = start(&s, &c, &parent).await;
    let t0 = Instant::now();
    let child = loop {
        let r = call(&s.app, "GET", &format!("/api/v1/runs/{run}"), None, Some(&c), false).await.json();
        if let Some(id) = r["steps"][0]["child_run_id"].as_str() {
            let cr = call(&s.app, "GET", &format!("/api/v1/runs/{id}"), None, Some(&c), false).await.json();
            if cr["status"] == "running" {
                break id.to_string();
            }
        }
        assert!(t0.elapsed() < Duration::from_secs(10), "child never started: {r}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(call(&s.app, "POST", &format!("/api/v1/runs/{run}/cancel"), None, Some(&c), true).await.status, StatusCode::ACCEPTED);
    let done = wait_finished(&s, &c, &run).await;
    assert_eq!(done["status"], "cancelled");
    assert_eq!(wait_finished(&s, &c, &child).await["status"], "cancelled");
    assert!(t0.elapsed() < Duration::from_secs(15));
}

// Rollout across hosts

/// Enroll `names` with tag `fleet`; returns their Authorization headers by name.
async fn fleet(s: &TestServer, c: &str, names: &[&str]) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    for n in names {
        let (_, auth) = enroll(s, c, n, json!(["fleet"])).await;
        out.insert(n.to_string(), auth);
    }
    out
}

async fn run_json(s: &TestServer, c: &str, run: &str) -> Value {
    call(&s.app, "GET", &format!("/api/v1/runs/{run}"), None, Some(c), false).await.json()
}

/// Status of each host's single step, by host name.
async fn host_states(s: &TestServer, c: &str, run: &str) -> std::collections::BTreeMap<String, String> {
    run_json(s, c, run).await["steps"].as_array().unwrap().iter().map(|st| (st["host_name"].as_str().unwrap().to_string(), st["status"].as_str().unwrap().to_string())).collect()
}

/// Wait until exactly `running` hosts are running (and their steps are queued for their agents).
async fn wait_running(s: &TestServer, c: &str, run: &str, running: &[&str]) -> std::collections::BTreeMap<String, String> {
    let t0 = Instant::now();
    loop {
        let states = host_states(s, c, run).await;
        let now: Vec<&str> = states.iter().filter(|(_, v)| *v == "running").map(|(k, _)| k.as_str()).collect();
        if now == running {
            return states;
        }
        assert!(t0.elapsed() < Duration::from_secs(10), "expected {running:?} running, got {states:?}");
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
}

/// Take the waiting step on a host's agent and finish it with `code`.
async fn finish_on(s: &TestServer, auth: &str, code: i32) {
    let job = agent_call(&s.app, "/api/v1/agent/poll", info(), Some(auth)).await;
    assert_eq!(job.status, StatusCode::OK, "{}", job.body);
    let id = job.json()["id"].as_str().unwrap().to_string();
    agent_call(&s.app, &format!("/api/v1/agent/jobs/{id}/output"), json!({"lines": []}), Some(auth)).await;
    agent_call(&s.app, &format!("/api/v1/agent/jobs/{id}/finish"), json!({"end": "exited", "exit_code": code}), Some(auth)).await;
}

#[tokio::test]
async fn parallel_rollouts_run_hosts_at_once_up_to_the_cap() {
    let s = server();
    let c = sign_in(&s).await;
    let auth = fleet(&s, &c, &["a1", "a2", "a3"]).await;
    let all = create(&s, &c, json!({"name": "All at once", "targets": {"tags": ["fleet"]}, "rollout": {"mode": "parallel"}, "steps": [{"name": "s", "run": "true"}]})).await;
    let run = start(&s, &c, &all).await;
    wait_running(&s, &c, &run, &["a1", "a2", "a3"]).await;
    for n in ["a1", "a2", "a3"] {
        finish_on(&s, &auth[n], 0).await;
    }
    assert_eq!(wait_finished(&s, &c, &run).await["status"], "succeeded");

    let capped = create(&s, &c, json!({"name": "Two at a time", "targets": {"tags": ["fleet"]}, "rollout": {"mode": "parallel", "max_parallel": 2}, "steps": [{"name": "s", "run": "true"}]})).await;
    let run = start(&s, &c, &capped).await;
    let states = wait_running(&s, &c, &run, &["a1", "a2"]).await;
    assert_eq!(states["a3"], "pending");
    finish_on(&s, &auth["a2"], 0).await;
    wait_running(&s, &c, &run, &["a1", "a3"]).await;
    finish_on(&s, &auth["a1"], 0).await;
    finish_on(&s, &auth["a3"], 0).await;
    assert_eq!(wait_finished(&s, &c, &run).await["status"], "succeeded");
}

#[tokio::test]
async fn batches_follow_a_canary_with_a_pause() {
    let s = server();
    let c = sign_in(&s).await;
    let auth = fleet(&s, &c, &["b1", "b2", "b3", "b4"]).await;
    let rb = create(&s, &c, json!({
        "name": "Canary then halves",
        "targets": {"tags": ["fleet"]},
        "rollout": {"mode": "batches", "batch_size": "50%", "canary": 1, "pause_seconds": "1s"},
        "steps": [{"name": "s", "run": "true"}]
    }))
    .await;
    let got = call(&s.app, "GET", &format!("/api/v1/runbooks/{rb}"), None, Some(&c), false).await.json();
    assert_eq!(got["rollout"]["batch_size"], "50%");
    let run = start(&s, &c, &rb).await;
    let states = wait_running(&s, &c, &run, &["b1"]).await;
    assert!(["b2", "b3", "b4"].iter().all(|h| states[*h] == "pending"), "{states:?}");
    finish_on(&s, &auth["b1"], 0).await;
    let t0 = Instant::now();
    wait_running(&s, &c, &run, &["b2", "b3"]).await;
    assert!(t0.elapsed() >= Duration::from_millis(900), "the pause was skipped");
    finish_on(&s, &auth["b2"], 0).await;
    finish_on(&s, &auth["b3"], 0).await;
    wait_running(&s, &c, &run, &["b4"]).await;
    finish_on(&s, &auth["b4"], 0).await;
    let done = wait_finished(&s, &c, &run).await;
    assert_eq!(done["status"], "succeeded");
    let lines = log_lines(&s, &c, done["id"].as_str().unwrap()).await;
    for want in ["Canary: b1", "Batch 1 of 2: b2, b3", "Batch 2 of 2: b4", "Waiting 1 second before the next hosts."] {
        assert!(lines.iter().any(|l| l == want), "{want} in {lines:?}");
    }
}

#[tokio::test]
async fn a_failed_host_stops_or_lets_the_rest_continue() {
    let s = server();
    let c = sign_in(&s).await;
    let auth = fleet(&s, &c, &["c1", "c2", "c3"]).await;
    // Default: a runbook that stops at a failed step also stops starting hosts.
    let stop = create(&s, &c, json!({"name": "Stops", "targets": {"tags": ["fleet"]}, "steps": [{"name": "s", "run": "true"}]})).await;
    let run = start(&s, &c, &stop).await;
    wait_running(&s, &c, &run, &["c1"]).await;
    finish_on(&s, &auth["c1"], 1).await;
    let done = wait_finished(&s, &c, &run).await;
    assert_eq!(done["status"], "failed");
    assert_eq!(host_states(&s, &c, &run).await.into_values().collect::<Vec<_>>(), vec!["failed", "skipped", "skipped"]);
    assert!(log_lines(&s, &c, &run).await.iter().any(|l| l == "c1 failed, so hosts that haven't started are skipped."));

    // Continue: every host runs, and the run still fails.
    let cont = create(&s, &c, json!({"name": "Continues", "targets": {"tags": ["fleet"]}, "rollout": {"on_host_failure": "continue"}, "steps": [{"name": "s", "run": "true"}]})).await;
    let run = start(&s, &c, &cont).await;
    wait_running(&s, &c, &run, &["c1"]).await;
    finish_on(&s, &auth["c1"], 1).await;
    wait_running(&s, &c, &run, &["c2"]).await;
    finish_on(&s, &auth["c2"], 0).await;
    wait_running(&s, &c, &run, &["c3"]).await;
    finish_on(&s, &auth["c3"], 0).await;
    assert_eq!(wait_finished(&s, &c, &run).await["status"], "failed");
    assert_eq!(host_states(&s, &c, &run).await.into_values().collect::<Vec<_>>(), vec!["failed", "succeeded", "succeeded"]);

    // In parallel, hosts already running finish even when one fails and stops the rollout.
    let par = create(&s, &c, json!({"name": "Parallel stop", "targets": {"tags": ["fleet"]}, "rollout": {"mode": "parallel", "max_parallel": 2, "on_host_failure": "stop"}, "steps": [{"name": "s", "run": "true"}]})).await;
    let run = start(&s, &c, &par).await;
    wait_running(&s, &c, &run, &["c1", "c2"]).await;
    finish_on(&s, &auth["c1"], 1).await;
    finish_on(&s, &auth["c2"], 0).await;
    assert_eq!(wait_finished(&s, &c, &run).await["status"], "failed");
    assert_eq!(host_states(&s, &c, &run).await.into_values().collect::<Vec<_>>(), vec!["failed", "succeeded", "skipped"]);
}

#[tokio::test]
async fn cancelling_a_parallel_run_reaches_every_running_host() {
    let s = server();
    let c = sign_in(&s).await;
    let auth = fleet(&s, &c, &["d1", "d2"]).await;
    let rb = create(&s, &c, json!({"name": "Both", "targets": {"tags": ["fleet"]}, "rollout": {"mode": "parallel"}, "steps": [{"name": "s", "run": "sleep 60"}]})).await;
    let run = start(&s, &c, &rb).await;
    wait_running(&s, &c, &run, &["d1", "d2"]).await;
    let mut jobs = Vec::new();
    for n in ["d1", "d2"] {
        let job = agent_call(&s.app, "/api/v1/agent/poll", info(), Some(&auth[n])).await.json();
        let id = job["id"].as_str().unwrap().to_string();
        agent_call(&s.app, &format!("/api/v1/agent/jobs/{id}/output"), json!({"lines": []}), Some(&auth[n])).await;
        jobs.push((n, id));
    }
    assert_eq!(call(&s.app, "POST", &format!("/api/v1/runs/{run}/cancel"), None, Some(&c), true).await.status, StatusCode::ACCEPTED);
    for (n, id) in &jobs {
        // The cancel reaches each agent asynchronously, in its next output reply.
        let deadline = Instant::now() + Duration::from_secs(5);
        while agent_call(&s.app, &format!("/api/v1/agent/jobs/{id}/output"), json!({"lines": []}), Some(&auth[*n])).await.json()["cancel"] != true {
            assert!(Instant::now() < deadline, "{n} was never told to cancel");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        agent_call(&s.app, &format!("/api/v1/agent/jobs/{id}/finish"), json!({"end": "cancelled", "exit_code": null}), Some(&auth[*n])).await;
    }
    assert_eq!(wait_finished(&s, &c, &run).await["status"], "cancelled");
}

#[tokio::test]
async fn rollouts_are_validated_on_save() {
    let s = server();
    let c = sign_in(&s).await;
    for rollout in [json!({"mode": "batches"}), json!({"mode": "batches", "batch_size": "0%"}), json!({"mode": "parallel", "batch_size": 2}), json!({"mode": "nope"})] {
        let r = call(&s.app, "POST", "/api/v1/runbooks", Some(json!({"name": "Bad", "rollout": rollout, "steps": [{"name": "a", "run": "true"}]})), Some(&c), true).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{rollout}: {}", r.body);
    }
}

// Host filters

#[tokio::test]
async fn host_filters_pick_the_hosts_a_run_goes_to() {
    let s = server();
    let c = sign_in(&s).await;
    for (name, tags) in [("web-1", json!(["web", "prod"])), ("web-2", json!(["web", "staging"])), ("db-1", json!(["db", "prod"]))] {
        enroll(&s, &c, name, tags).await;
    }
    let preview = call(&s.app, "GET", "/api/v1/hosts?filter=tags:prod%20!name:db-*", None, Some(&c), false).await;
    assert_eq!(preview.status, StatusCode::OK, "{}", preview.body);
    let names: Vec<String> = preview.json().as_array().unwrap().iter().map(|h| h["name"].as_str().unwrap().to_string()).collect();
    assert_eq!(names, vec!["web-1"]);
    assert_eq!(call(&s.app, "GET", "/api/v1/hosts?filter=color:blue", None, Some(&c), false).await.status, StatusCode::BAD_REQUEST);
    assert_eq!(call(&s.app, "GET", "/api/v1/hosts", None, Some(&c), false).await.json().as_array().unwrap().len(), 4, "no filter lists every host");

    let bad = call(&s.app, "POST", "/api/v1/runbooks", Some(json!({"name": "Bad", "targets": {"filter": "tags:"}, "steps": [{"name": "a", "run": "true"}]})), Some(&c), true).await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST, "{}", bad.body);

    // Union of an explicit host, a tag and a filter; the filter never matches the server.
    let rb = create(&s, &c, json!({"name": "Prod web", "targets": {"filter": "tags:web  !tags:staging", "tags": ["db"]}, "steps": [{"name": "a", "run": "true"}]})).await;
    let got = call(&s.app, "GET", &format!("/api/v1/runbooks/{rb}"), None, Some(&c), false).await.json();
    assert_eq!(got["targets"]["filter"], "tags:web !tags:staging", "spaces are tidied");
    let run = start(&s, &c, &rb).await;
    let hosts: Vec<String> = run_json(&s, &c, &run).await["hosts"].as_array().unwrap().iter().map(|h| h["name"].as_str().unwrap().to_string()).collect();
    assert_eq!(hosts, vec!["db-1", "web-1"]);
    call(&s.app, "POST", &format!("/api/v1/runs/{run}/cancel"), None, Some(&c), true).await;
    let used: Vec<String> = call(&s.app, "GET", "/api/v1/hosts", None, Some(&c), false).await.json().as_array().unwrap().iter().filter(|h| !h["used_by"].as_array().unwrap().is_empty()).map(|h| h["name"].as_str().unwrap().to_string()).collect();
    assert_eq!(used, vec!["db-1", "web-1"], "the Hosts page shows which runbooks target each host");

    let none = create(&s, &c, json!({"name": "Nobody", "targets": {"filter": "tags:mars"}, "steps": [{"name": "a", "run": "true"}]})).await;
    assert_eq!(call(&s.app, "POST", &format!("/api/v1/runbooks/{none}/runs"), None, Some(&c), true).await.status, StatusCode::CONFLICT);
}

// Retrying unfinished hosts

#[tokio::test]
async fn retry_runs_only_the_hosts_that_didnt_finish() {
    let s = server();
    let c = sign_in(&s).await;
    let auth = fleet(&s, &c, &["r1", "r2", "r3"]).await;
    let rb = create(&s, &c, json!({"name": "Fleet", "targets": {"tags": ["fleet"]}, "steps": [{"name": "s", "run": "true"}]})).await;
    let run = start(&s, &c, &rb).await;
    let retry = |id: String| format!("/api/v1/runs/{id}/retry");
    assert_eq!(call(&s.app, "POST", &retry(run.clone()), None, Some(&c), true).await.status, StatusCode::CONFLICT, "still running");
    wait_running(&s, &c, &run, &["r1"]).await;
    finish_on(&s, &auth["r1"], 0).await;
    wait_running(&s, &c, &run, &["r2"]).await;
    finish_on(&s, &auth["r2"], 1).await;
    assert_eq!(wait_finished(&s, &c, &run).await["status"], "failed");

    let r = call(&s.app, "POST", &retry(run.clone()), None, Some(&c), true).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
    let again = r.json();
    let names: Vec<&str> = again["hosts"].as_array().unwrap().iter().map(|h| h["name"].as_str().unwrap()).collect();
    assert_eq!(names, vec!["r2", "r3"], "the failed host and the one it caused to be skipped");
    assert_eq!((again["trigger"].as_str(), again["retry_of"].as_str()), (Some("retry"), Some(run.as_str())));
    let again_id = again["id"].as_str().unwrap().to_string();
    wait_running(&s, &c, &again_id, &["r2"]).await;
    finish_on(&s, &auth["r2"], 0).await;
    wait_running(&s, &c, &again_id, &["r3"]).await;
    finish_on(&s, &auth["r3"], 0).await;
    assert_eq!(wait_finished(&s, &c, &again_id).await["status"], "succeeded");
    let nothing = call(&s.app, "POST", &retry(again_id), None, Some(&c), true).await;
    assert_eq!(nothing.status, StatusCode::CONFLICT);
    assert!(nothing.json()["detail"].as_str().unwrap().contains("nothing to retry"));
}

#[tokio::test]
async fn retry_reuses_inputs_but_asks_for_secrets_again() {
    let s = server();
    let c = sign_in(&s).await;
    let inputs = json!([{"name": "svc", "type": "text", "required": true}, {"name": "tok", "type": "secret", "required": true}]);
    let rb = create(&s, &c, json!({"name": "Deploy", "inputs": inputs, "steps": [{"name": "s", "run": "exit 1"}]})).await;
    let r = start_with(&s, &c, &rb, json!({"inputs": {"svc": "api", "tok": "tok-4410"}})).await;
    let first = r.json()["id"].as_str().unwrap().to_string();
    assert_eq!(wait_finished(&s, &c, &first).await["status"], "failed");
    // Fix the runbook; a retry uses it as it is now.
    let upd = call(&s.app, "PUT", &format!("/api/v1/runbooks/{rb}"), Some(json!({"name": "Deploy", "inputs": inputs, "steps": [{"name": "s", "run": "echo svc=$OPSKILLS_INPUT_SVC tok=$OPSKILLS_INPUT_TOK"}]})), Some(&c), true).await;
    assert_eq!(upd.status, StatusCode::OK, "{}", upd.body);
    let url = format!("/api/v1/runs/{first}/retry");
    let missing = call(&s.app, "POST", &url, None, Some(&c), true).await;
    assert_eq!(missing.status, StatusCode::BAD_REQUEST, "{}", missing.body);
    let ok = call(&s.app, "POST", &url, Some(json!({"inputs": {"tok": "tok-5520"}})), Some(&c), true).await;
    assert_eq!(ok.status, StatusCode::CREATED, "{}", ok.body);
    let again = wait_finished(&s, &c, ok.json()["id"].as_str().unwrap()).await;
    assert_eq!(again["status"], "succeeded");
    assert_eq!(again["inputs"]["svc"], "api");
    let lines = log_lines(&s, &c, again["id"].as_str().unwrap()).await;
    assert!(lines.iter().any(|l| l == &format!("svc=api tok={}", crate::executor::MASK)), "{lines:?}");
}

// API tokens

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

#[tokio::test]
async fn api_tokens_work_without_a_session_and_can_be_revoked() {
    let s = server();
    let c = sign_in(&s).await;
    let made = call(&s.app, "POST", "/api/v1/me/tokens", Some(json!({"name": "CI deploy", "expires_in_days": 90})), Some(&c), true).await;
    assert_eq!(made.status, StatusCode::CREATED, "{}", made.body);
    let value = made.json()["value"].as_str().unwrap().to_string();
    let id = made.json()["token"]["id"].as_str().unwrap().to_string();
    assert!(value.starts_with("osk_") && value.len() == 4 + 12 + 1 + 64, "{value}");
    assert!(made.json()["token"]["expires_at"].as_u64().is_some());

    // Listing never shows the token or its hash.
    let list = call(&s.app, "GET", "/api/v1/me/tokens", None, Some(&c), false).await;
    assert!(!list.body.contains(&value[16..]) && !list.body.contains("secret_hash"), "{}", list.body);
    assert_eq!(list.json()[0]["name"], "CI deploy");
    assert!(list.json()[0]["last_used_at"].is_null());

    // A token works for reads and writes, with no cookie and no CSRF header.
    assert_eq!(bearer_call(&s.app, "GET", "/api/v1/runbooks", None, &value).await.status, StatusCode::OK);
    let me = bearer_call(&s.app, "GET", "/api/v1/auth/me", None, &value).await.json();
    assert_eq!((me["user"].as_str(), me["token"].as_str()), (Some("admin"), Some("CI deploy")));
    let rb = create(&s, &c, json!({"name": "Ping", "steps": [{"name": "a", "run": "echo pong"}]})).await;
    let started = bearer_call(&s.app, "POST", &format!("/api/v1/runbooks/{rb}/runs"), None, &value).await;
    assert_eq!(started.status, StatusCode::CREATED, "{}", started.body);
    let done = wait_finished(&s, &c, started.json()["id"].as_str().unwrap()).await;
    assert_eq!((done["status"].as_str(), done["started_by"].as_str()), (Some("succeeded"), Some("admin (token CI deploy)")));
    assert!(call(&s.app, "GET", "/api/v1/me/tokens", None, Some(&c), false).await.json()[0]["last_used_at"].as_u64().is_some());

    // Tokens can't manage tokens or sign-ins.
    assert_eq!(bearer_call(&s.app, "GET", "/api/v1/me/tokens", None, &value).await.status, StatusCode::FORBIDDEN);
    assert_eq!(bearer_call(&s.app, "POST", "/api/v1/me/tokens", Some(json!({"name": "escalate"})), &value).await.status, StatusCode::FORBIDDEN);
    assert_eq!(bearer_call(&s.app, "POST", "/api/v1/auth/logout", None, &value).await.status, StatusCode::FORBIDDEN);

    // Wrong, malformed and revoked tokens are refused.
    let wrong = format!("{}{}", &value[..value.len() - 1], if value.ends_with('0') { '1' } else { '0' });
    for bad in [wrong.as_str(), "osk_nope", "not-a-token"] {
        assert_eq!(bearer_call(&s.app, "GET", "/api/v1/runbooks", None, bad).await.status, StatusCode::UNAUTHORIZED, "{bad}");
    }
    assert_eq!(call(&s.app, "DELETE", &format!("/api/v1/me/tokens/{id}"), None, Some(&c), true).await.status, StatusCode::NO_CONTENT);
    assert_eq!(bearer_call(&s.app, "GET", "/api/v1/runbooks", None, &value).await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(call(&s.app, "DELETE", &format!("/api/v1/me/tokens/{id}"), None, Some(&c), true).await.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn token_requests_are_checked() {
    let s = server();
    let c = sign_in(&s).await;
    for bad in [json!({"name": ""}), json!({"name": "x".repeat(61)}), json!({"name": "a", "expires_in_days": 0}), json!({"name": "a", "expires_in_days": 4000}), json!({"name": "a", "scope": "all"})] {
        let r = call(&s.app, "POST", "/api/v1/me/tokens", Some(bad.clone()), Some(&c), true).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{bad}: {}", r.body);
    }
    assert_eq!(call(&s.app, "POST", "/api/v1/me/tokens", Some(json!({"name": "Nightly"})), Some(&c), true).await.status, StatusCode::CREATED);
    let dup = call(&s.app, "POST", "/api/v1/me/tokens", Some(json!({"name": "nightly"})), Some(&c), true).await;
    assert_eq!(dup.status, StatusCode::CONFLICT);
    assert!(call(&s.app, "GET", "/api/v1/me/tokens", None, Some(&c), false).await.json()[0]["expires_at"].is_null(), "no expiry by default");
    // Sessions still need the CSRF header for writes.
    assert_eq!(call(&s.app, "POST", "/api/v1/me/tokens", Some(json!({"name": "x"})), Some(&c), false).await.status, StatusCode::FORBIDDEN);
}

// Schedules

#[tokio::test]
async fn schedules_are_checked_and_previewed() {
    let s = server();
    let c = sign_in(&s).await;
    let future = crate::model::now_ms() + 3_600_000;
    let inputs = json!([{"name": "env", "type": "choice", "choices": ["staging", "prod"], "required": true}]);
    let body = |schedules: Value| json!({"name": "Nightly", "inputs": inputs, "schedules": schedules, "steps": [{"name": "a", "run": "true"}]});
    for bad in [
        json!([{"cron": "0 3 * *", "inputs": {"env": "prod"}}]),
        json!([{"cron": "0 3 * * *", "timezone": "Mars/Base", "inputs": {"env": "prod"}}]),
        json!([{"cron": "0 3 * * *", "at": future, "inputs": {"env": "prod"}}]),
        json!([{"at": 1000, "inputs": {"env": "prod"}}]),
        json!([{"cron": "0 3 * * *"}]),
        json!([{"cron": "0 3 * * *", "inputs": {"env": "qa"}}]),
        json!([{"cron": "0 3 * * *", "inputs": {"env": "prod", "extra": "1"}}]),
    ] {
        let r = call(&s.app, "POST", "/api/v1/runbooks", Some(body(bad.clone())), Some(&c), true).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{bad}: {}", r.body);
    }
    let secret = json!({"name": "Secret", "inputs": [{"name": "tok", "type": "secret", "required": true}], "schedules": [{"cron": "@daily"}], "steps": [{"name": "a", "run": "true"}]});
    let r = call(&s.app, "POST", "/api/v1/runbooks", Some(secret), Some(&c), true).await;
    assert!(r.json()["detail"].as_str().unwrap().contains("can't hold secrets"), "{}", r.body);

    let rb = create(&s, &c, body(json!([
        {"name": "weekday mornings", "cron": "30  9 * * 1-5", "timezone": "Europe/Berlin", "inputs": {"env": "prod"}},
        {"at": future, "inputs": {"env": "staging"}}
    ]))).await;
    let got = call(&s.app, "GET", &format!("/api/v1/runbooks/{rb}"), None, Some(&c), false).await.json();
    let saved = got["schedules"].as_array().unwrap();
    assert_eq!(saved[0]["cron"], "30 9 * * 1-5", "spaces are tidied");
    assert!(saved.iter().all(|x| x["id"].as_str().is_some_and(|id| !id.is_empty())), "ids are assigned");
    let status = call(&s.app, "GET", &format!("/api/v1/runbooks/{rb}/schedules"), None, Some(&c), false).await.json();
    assert_eq!(status[0]["next"].as_array().unwrap().len(), 3);
    assert_eq!(status[1]["next"], json!([future]));
    // Saving again keeps the ids.
    let again = call(&s.app, "PUT", &format!("/api/v1/runbooks/{rb}"), Some(json!({"name": "Nightly", "inputs": inputs, "schedules": got["schedules"], "steps": [{"name": "a", "run": "true"}]})), Some(&c), true).await;
    assert_eq!(again.json()["schedules"][0]["id"], saved[0]["id"]);

    let preview = call(&s.app, "POST", "/api/v1/schedules/preview", Some(json!({"cron": "0 * * * *", "timezone": "UTC"})), Some(&c), true).await.json();
    let next = preview["next"].as_array().unwrap();
    assert_eq!(next.len(), 5);
    assert_eq!(next[1].as_u64().unwrap() - next[0].as_u64().unwrap(), 3_600_000);
    assert_eq!(call(&s.app, "POST", "/api/v1/schedules/preview", Some(json!({"cron": "nope"})), Some(&c), true).await.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn the_scheduler_fires_due_runs_once() {
    let s = server();
    let c = sign_in(&s).await;
    let now = crate::model::now_ms();
    let rb = create(&s, &c, json!({
        "name": "Every minute",
        "inputs": [{"name": "who", "type": "text", "default": "nobody"}],
        "schedules": [{"name": "minutely", "cron": "* * * * *", "inputs": {"who": "the scheduler"}}],
        "steps": [{"name": "a", "run": "echo hello {{ inputs.who }}"}]
    }))
    .await;
    let st = s.state.clone();
    // Ten minutes behind: one run for the latest minute, not ten.
    let fired = crate::schedule::tick(&st, now - 600_000, now);
    assert_eq!(fired.len(), 1);
    let run = wait_finished(&s, &c, &fired[0]).await;
    assert_eq!((run["trigger"].as_str(), run["started_by"].as_str(), run["status"].as_str()), (Some("schedule"), Some("schedule minutely"), Some("succeeded")));
    assert!(log_lines(&s, &c, &fired[0]).await.iter().any(|l| l == "hello the scheduler"));
    assert!(crate::schedule::tick(&st, now - 600_000, now).is_empty(), "the same minute never fires twice");
    assert_eq!(crate::schedule::tick(&st, now - 600_000, now + 60_000).len(), 1, "the next minute fires");
    let status = call(&s.app, "GET", &format!("/api/v1/runbooks/{rb}/schedules"), None, Some(&c), false).await.json();
    assert!(status[0]["last_run_id"].as_str().is_some() && status[0]["last_error"].is_null(), "{status}");
}

#[tokio::test]
async fn one_time_schedules_fire_once_and_failures_are_recorded() {
    let s = server();
    let c = sign_in(&s).await;
    let now = crate::model::now_ms();
    let at = now + 3_600_000;
    create(&s, &c, json!({"name": "Once", "schedules": [{"at": at}], "steps": [{"name": "a", "run": "true"}]})).await;
    let st = s.state.clone();
    assert!(crate::schedule::tick(&st, now, now).is_empty(), "not due yet");
    assert_eq!(crate::schedule::tick(&st, now, at + 1000).len(), 1);
    assert!(crate::schedule::tick(&st, now, at + 120_000).is_empty(), "a one-time schedule fires once");
    // Missed while the server was down: the scheduler started after it was due.
    assert!(crate::schedule::tick(&st, at + 10, at + 20).is_empty());

    let nowhere = create(&s, &c, json!({"name": "No hosts", "targets": {"tags": ["mars"]}, "schedules": [{"cron": "* * * * *"}], "steps": [{"name": "a", "run": "true"}]})).await;
    assert!(crate::schedule::tick(&st, now - 120_000, now).is_empty());
    let status = call(&s.app, "GET", &format!("/api/v1/runbooks/{nowhere}/schedules"), None, Some(&c), false).await.json();
    assert!(status[0]["last_error"].as_str().unwrap().contains("No hosts match"), "{status}");
}

// Inbound webhooks

async fn hook_call(app: &Router, token: &str, body: &str, headers: &[(&str, String)]) -> Reply {
    let mut req = Request::builder().method("POST").uri(format!("/api/v1/hooks/{token}")).header(header::CONTENT_TYPE, "application/json");
    for (k, v) in headers {
        req = req.header(*k, v.as_str());
    }
    let res = app.clone().oneshot(req.body(Body::from(body.to_string())).unwrap()).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    Reply { status, cookie: None, body: String::from_utf8_lossy(&bytes).into() }
}

fn sign(secret: &str, body: &str) -> String {
    use hmac::{KeyInit, Mac};
    let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(body.as_bytes());
    format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
}

#[tokio::test]
async fn webhooks_start_runs_with_values_from_the_payload() {
    let s = server();
    let c = sign_in(&s).await;
    let rb = create(&s, &c, json!({
        "name": "Deploy",
        "inputs": [{"name": "repo", "type": "text", "required": true}, {"name": "sha", "type": "text", "default": "none"}],
        "steps": [{"name": "a", "run": "echo deploying {{ inputs.repo }} at {{ inputs.sha }}"}]
    }))
    .await;
    let bad = call(&s.app, "POST", &format!("/api/v1/runbooks/{rb}/webhooks"), Some(json!({"name": "x", "inputs": {"nope": "a"}})), Some(&c), true).await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST, "{}", bad.body);
    let bad_path = call(&s.app, "POST", &format!("/api/v1/runbooks/{rb}/webhooks"), Some(json!({"name": "x", "inputs": {"repo": "a..b"}})), Some(&c), true).await;
    assert_eq!(bad_path.status, StatusCode::BAD_REQUEST);

    let made = call(&s.app, "POST", &format!("/api/v1/runbooks/{rb}/webhooks"), Some(json!({"name": "CI push", "inputs": {"repo": "repository.name", "sha": "commits.0.id"}})), Some(&c), true).await;
    assert_eq!(made.status, StatusCode::CREATED, "{}", made.body);
    let token = made.json()["token"].as_str().unwrap().to_string();
    let hook_id = made.json()["webhook"]["id"].as_str().unwrap().to_string();
    assert!(token.starts_with("owh_") && made.json()["signing_secret"].is_null());
    let list = call(&s.app, "GET", &format!("/api/v1/runbooks/{rb}/webhooks"), None, Some(&c), false).await;
    assert!(!list.body.contains(&token[17..]) && !list.body.contains("token_hash"), "{}", list.body);

    // No session or CSRF header: the token in the URL is the credential.
    let payload = r#"{"repository": {"name": "shop"}, "commits": [{"id": "abc123"}]}"#;
    let r = hook_call(&s.app, &token, payload, &[]).await;
    assert_eq!(r.status, StatusCode::ACCEPTED, "{}", r.body);
    let run = wait_finished(&s, &c, r.json()["run_id"].as_str().unwrap()).await;
    assert_eq!((run["trigger"].as_str(), run["started_by"].as_str(), run["status"].as_str()), (Some("webhook"), Some("webhook CI push"), Some("succeeded")));
    assert!(log_lines(&s, &c, run["id"].as_str().unwrap()).await.iter().any(|l| l == "deploying shop at abc123"));

    // A payload without a required value, and one that isn't JSON, are refused and recorded.
    assert_eq!(hook_call(&s.app, &token, r#"{"other": 1}"#, &[]).await.status, StatusCode::BAD_REQUEST);
    assert_eq!(hook_call(&s.app, &token, "not json", &[]).await.status, StatusCode::BAD_REQUEST);
    let hooks = call(&s.app, "GET", &format!("/api/v1/runbooks/{rb}/webhooks"), None, Some(&c), false).await.json();
    let statuses: Vec<u64> = hooks[0]["deliveries"].as_array().unwrap().iter().map(|d| d["status"].as_u64().unwrap()).collect();
    assert_eq!(statuses, vec![400, 400, 202], "newest first");
    assert!(hooks[0]["deliveries"][0]["error"].as_str().unwrap().contains("JSON"));

    // Wrong tokens look like no webhook at all.
    let wrong = format!("{}{}", &token[..token.len() - 1], if token.ends_with('0') { '1' } else { '0' });
    for t in [wrong.as_str(), "owh_nope", "anything"] {
        assert_eq!(hook_call(&s.app, t, payload, &[]).await.status, StatusCode::NOT_FOUND, "{t}");
    }
    // Turned off, then rotated: the old URL stops working.
    let off = call(&s.app, "PUT", &format!("/api/v1/webhooks/{hook_id}"), Some(json!({"name": "CI push", "enabled": false, "inputs": {"repo": "repository.name"}})), Some(&c), true).await;
    assert_eq!(off.status, StatusCode::OK, "{}", off.body);
    assert_eq!(hook_call(&s.app, &token, payload, &[]).await.status, StatusCode::CONFLICT);
    call(&s.app, "PUT", &format!("/api/v1/webhooks/{hook_id}"), Some(json!({"name": "CI push", "inputs": {"repo": "repository.name"}})), Some(&c), true).await;
    let rotated = call(&s.app, "POST", &format!("/api/v1/webhooks/{hook_id}/rotate"), None, Some(&c), true).await.json()["token"].as_str().unwrap().to_string();
    assert_eq!(hook_call(&s.app, &token, payload, &[]).await.status, StatusCode::NOT_FOUND);
    assert_eq!(hook_call(&s.app, &rotated, payload, &[]).await.status, StatusCode::ACCEPTED);

    // Deleting the runbook deletes its webhooks.
    assert_eq!(call(&s.app, "DELETE", &format!("/api/v1/runbooks/{rb}"), None, Some(&c), true).await.status, StatusCode::NO_CONTENT);
    assert_eq!(hook_call(&s.app, &rotated, payload, &[]).await.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn signed_webhooks_check_the_body_signature() {
    let s = server();
    let c = sign_in(&s).await;
    let rb = create(&s, &c, json!({"name": "Signed", "steps": [{"name": "a", "run": "true"}]})).await;
    let made = call(&s.app, "POST", &format!("/api/v1/runbooks/{rb}/webhooks"), Some(json!({"name": "GitHub", "signed": true})), Some(&c), true).await.json();
    let (token, secret) = (made["token"].as_str().unwrap().to_string(), made["signing_secret"].as_str().unwrap().to_string());
    assert_eq!(secret.len(), 64);
    let body = r#"{"ref": "refs/heads/main"}"#;
    assert_eq!(hook_call(&s.app, &token, body, &[]).await.status, StatusCode::UNAUTHORIZED, "no signature");
    assert_eq!(hook_call(&s.app, &token, body, &[("x-opskills-signature", sign("wrong", body))]).await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(hook_call(&s.app, &token, body, &[("x-opskills-signature", sign(&secret, "a different body"))]).await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(hook_call(&s.app, &token, body, &[("x-opskills-signature", sign(&secret, body))]).await.status, StatusCode::ACCEPTED);
    // GitHub's header works too.
    assert_eq!(hook_call(&s.app, &token, body, &[("x-hub-signature-256", sign(&secret, body))]).await.status, StatusCode::ACCEPTED);
    // The secret is never listed.
    let list = call(&s.app, "GET", &format!("/api/v1/runbooks/{rb}/webhooks"), None, Some(&c), false).await.body;
    assert!(!list.contains(&secret) && !list.contains("signing_ciphertext"), "{list}");
}

// Notifications

/// A local HTTP receiver that records each POST. `/flaky` answers 503 once, then 200;
/// `/gone` always answers 404.
async fn receiver() -> (String, std::sync::Arc<std::sync::Mutex<Vec<(String, String, Value)>>>) {
    use std::sync::{Arc, Mutex};
    type Seen = Arc<Mutex<Vec<(String, String, Value)>>>;
    let seen: Seen = Arc::new(Mutex::new(vec![]));
    let flaky = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (s2, f2) = (seen.clone(), flaky.clone());
    let app = Router::new().fallback(move |uri: axum::http::Uri, headers: axum::http::HeaderMap, body: String| {
        let (seen, flaky) = (s2.clone(), f2.clone());
        async move {
            let path = uri.path().to_string();
            if path == "/gone" {
                return StatusCode::NOT_FOUND;
            }
            if path == "/flaky" && !flaky.swap(true, std::sync::atomic::Ordering::SeqCst) {
                return StatusCode::SERVICE_UNAVAILABLE;
            }
            let event = headers.get("x-opskills-event").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
            seen.lock().unwrap().push((path, event, serde_json::from_str(&body).unwrap_or(Value::Null)));
            StatusCode::OK
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), seen)
}

async fn wait_for<T>(mut f: impl FnMut() -> Option<T>, what: &str) -> T {
    for _ in 0..300 {
        if let Some(v) = f() {
            return v;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("timed out waiting for {what}");
}

async fn notices(s: &TestServer, c: &str, run: &str, n: usize) -> Vec<Value> {
    for _ in 0..300 {
        let list = call(&s.app, "GET", &format!("/api/v1/runs/{run}/notifications"), None, Some(c), false).await.json();
        if list.as_array().unwrap().len() >= n {
            return list.as_array().unwrap().clone();
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("timed out waiting for {n} notices on {run}");
}

#[tokio::test]
async fn notification_rules_are_checked() {
    let s = server();
    let c = sign_in(&s).await;
    let rb = |n: Value| json!({"name": "N", "steps": [{"name": "a", "run": "true"}], "notifications": [n]});
    for bad in [
        json!({"on": [], "webhook": "https://hooks.example.com/x"}),
        json!({"on": ["failed"], "email": ["not-an-address"]}),
        json!({"on": ["failed"], "email": ["a@example.com"], "webhook": "https://hooks.example.com/x"}),
        json!({"on": ["failed"]}),
        json!({"on": ["failed"], "webhook": "ftp://example.com/x"}),
        json!({"on": ["slow"], "webhook": "https://hooks.example.com/x"}),
        json!({"on": ["exploded"], "webhook": "https://hooks.example.com/x"}),
    ] {
        let r = call(&s.app, "POST", "/api/v1/runbooks", Some(rb(bad.clone())), Some(&c), true).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{bad} -> {}", r.body);
    }
    let ok = call(&s.app, "POST", "/api/v1/runbooks", Some(rb(json!({"on": ["failed", "slow", "failed"], "slow_after_seconds": 600, "email": [" ops@example.com "]}))), Some(&c), true).await;
    assert_eq!(ok.status, StatusCode::CREATED, "{}", ok.body);
    let n = &ok.json()["notifications"][0];
    assert_eq!((n["on"].clone(), n["email"][0].as_str(), n["id"].as_str().unwrap().len()), (json!(["failed", "slow"]), Some("ops@example.com"), 12));
    // No SMTP server configured: email can't be tested, and the editor is told so.
    let settings = call(&s.app, "GET", "/api/v1/notifications/settings", None, Some(&c), false).await.json();
    assert_eq!(settings["email"], json!(false));
    let t = call(&s.app, "POST", "/api/v1/notifications/test", Some(json!({"email": ["ops@example.com"]})), Some(&c), true).await;
    assert_eq!(t.status, StatusCode::CONFLICT, "{}", t.body);
}

#[tokio::test]
async fn webhook_notifications_report_starts_failures_and_retries() {
    let (base, seen) = receiver().await;
    let s = server();
    let c = sign_in(&s).await;
    let rb = create(&s, &c, json!({
        "name": "Migrate",
        "steps": [{"name": "prepare", "run": "echo preparing"}, {"name": "migrate", "run": "echo 'table locked' >&2; exit 3"}],
        "notifications": [
            {"on": ["started", "failed", "succeeded"], "webhook": format!("{base}/chat/T000/B000/secretpart")},
            {"on": ["failed"], "webhook": format!("{base}/flaky")},
            {"on": ["failed"], "webhook": format!("{base}/gone")},
            {"on": ["failed"], "webhook": format!("{base}/off"), "enabled": false}
        ]
    }))
    .await;
    let run = call(&s.app, "POST", &format!("/api/v1/runbooks/{rb}/runs"), None, Some(&c), true).await.json();
    let id = run["id"].as_str().unwrap().to_string();
    wait_finished(&s, &c, &id).await;
    let list = notices(&s, &c, &id, 4).await;
    let got = wait_for(|| { let v = seen.lock().unwrap().clone(); (v.len() >= 3).then_some(v) }, "3 webhook calls").await;
    let main: Vec<&(String, String, Value)> = got.iter().filter(|(p, _, _)| p.starts_with("/chat")).collect();
    assert_eq!(main.iter().map(|(_, e, _)| e.as_str()).collect::<Vec<_>>(), vec!["started", "failed"]);
    let failed = &main[1].2;
    let text = failed["text"].as_str().unwrap();
    assert!(text.starts_with("Migrate failed: run #") && text.contains("step \"migrate\" on OpSkills server"), "{text}");
    assert_eq!(failed["content"], failed["text"]);
    assert_eq!(failed["run"]["failed_step"]["exit_code"], json!(3));
    assert!(failed["output_tail"].as_array().unwrap().iter().any(|l| l.as_str().unwrap().ends_with("table locked")), "{failed}");
    assert!(main[0].2["text"].as_str().unwrap().contains("started by admin"));
    // The flaky receiver got it on the second try; the 404 one was not retried.
    assert_eq!(got.iter().filter(|(p, _, _)| p == "/flaky").count(), 1);
    assert!(!got.iter().any(|(p, _, _)| p == "/off"));
    let by_target = |t: &str| list.iter().filter(|n| n["target"].as_str() == Some(t)).cloned().collect::<Vec<_>>();
    let host = format!("webhook {}", base.trim_start_matches("http://").split(':').next().unwrap());
    assert!(by_target(&host).iter().filter(|n| n["ok"] == json!(true)).count() >= 3, "{list:?}");
    let gone = list.iter().find(|n| n["ok"] == json!(false)).expect("the 404 is recorded");
    assert!(gone["error"].as_str().unwrap().contains("404"), "{gone}");
    // Webhook URLs are often secret: only the host is ever shown.
    assert!(!serde_json::to_string(&list).unwrap().contains("secretpart"));
}

#[tokio::test]
async fn slow_runs_notify_once_and_fast_ones_do_not() {
    let (base, seen) = receiver().await;
    let s = server();
    let c = sign_in(&s).await;
    let rb = create(&s, &c, json!({
        "name": "Backup",
        "inputs": [{"name": "nap", "type": "text", "default": "0"}],
        "steps": [{"name": "copy", "run": "sleep {{ inputs.nap }}"}],
        "notifications": [{"on": ["slow", "succeeded"], "slow_after_seconds": 1, "webhook": format!("{base}/slow")}]
    }))
    .await;
    let fast = call(&s.app, "POST", &format!("/api/v1/runbooks/{rb}/runs"), None, Some(&c), true).await.json();
    let fast_id = fast["id"].as_str().unwrap().to_string();
    wait_finished(&s, &c, &fast_id).await;
    notices(&s, &c, &fast_id, 1).await;
    let slow = call(&s.app, "POST", &format!("/api/v1/runbooks/{rb}/runs"), Some(json!({"inputs": {"nap": "2"}})), Some(&c), true).await.json();
    let slow_id = slow["id"].as_str().unwrap().to_string();
    wait_finished(&s, &c, &slow_id).await;
    let list = notices(&s, &c, &slow_id, 2).await;
    let events: Vec<&str> = list.iter().map(|n| n["event"].as_str().unwrap()).collect();
    assert_eq!(events, vec!["slow", "succeeded"]);
    // Give a stray timer from the fast run time to fire, then check it didn't.
    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
    let got = seen.lock().unwrap().clone();
    let slow_calls: Vec<&Value> = got.iter().filter(|(_, e, _)| e == "slow").map(|(_, _, b)| b).collect();
    assert_eq!(slow_calls.len(), 1, "{got:?}");
    assert!(slow_calls[0]["text"].as_str().unwrap().starts_with("Backup is still running after 1 second"), "{:?}", slow_calls[0]);
    assert_eq!(slow_calls[0]["run"]["id"].as_str(), Some(slow_id.as_str()));
}

/// A tiny SMTP server that accepts every message and keeps its text.
async fn fake_smtp() -> (u16, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
    let (port, mail, _) = fake_smtp_with_auth().await;
    (port, mail)
}

/// Like `fake_smtp`, and also keeps each AUTH command it was sent.
#[allow(clippy::type_complexity)]
async fn fake_smtp_with_auth() -> (u16, std::sync::Arc<std::sync::Mutex<Vec<String>>>, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let mail = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let auths = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (m, a) = (mail.clone(), auths.clone());
    tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = listener.accept().await else { return };
            let (m, a) = (m.clone(), a.clone());
            tokio::spawn(async move {
                let (r, mut w) = sock.into_split();
                let mut lines = BufReader::new(r).lines();
                w.write_all(b"220 test ESMTP\r\n").await.unwrap();
                let (mut in_data, mut msg) = (false, String::new());
                while let Ok(Some(line)) = lines.next_line().await {
                    if in_data {
                        if line == "." {
                            in_data = false;
                            m.lock().unwrap().push(std::mem::take(&mut msg));
                            w.write_all(b"250 queued\r\n").await.unwrap();
                        } else {
                            msg.push_str(&line);
                            msg.push('\n');
                        }
                        continue;
                    }
                    let cmd = line.to_ascii_uppercase();
                    let reply: &[u8] = if cmd.starts_with("EHLO") {
                        b"250-test\r\n250 AUTH PLAIN LOGIN\r\n"
                    } else if cmd.starts_with("AUTH") {
                        a.lock().unwrap().push(line.clone());
                        b"235 2.7.0 accepted\r\n"
                    } else if cmd.starts_with("DATA") {
                        in_data = true;
                        b"354 go ahead\r\n"
                    } else if cmd.starts_with("QUIT") {
                        let _ = w.write_all(b"221 bye\r\n").await;
                        return;
                    } else {
                        b"250 ok\r\n"
                    };
                    w.write_all(reply).await.unwrap();
                }
            });
        }
    });
    (port, mail, auths)
}

#[tokio::test]
async fn email_notifications_go_through_the_smtp_server() {
    let (port, mail) = fake_smtp().await;
    let s = server_with(|cfg| {
        cfg.email.smtp_host = Some("127.0.0.1".into());
        cfg.email.smtp_port = port;
        cfg.email.security = "none".into();
        cfg.email.from = Some("OpSkills <ops@example.com>".into());
        cfg.server.public_url = Some("https://ops.example.com/".into());
    });
    let c = sign_in(&s).await;
    let settings = call(&s.app, "GET", "/api/v1/notifications/settings", None, Some(&c), false).await.json();
    assert_eq!((settings["email"].clone(), settings["from"].as_str()), (json!(true), Some("OpSkills <ops@example.com>")));
    let t = call(&s.app, "POST", "/api/v1/notifications/test", Some(json!({"email": ["oncall@example.com"]})), Some(&c), true).await;
    assert_eq!(t.status, StatusCode::OK, "{}", t.body);
    let rb = create(&s, &c, json!({
        "name": "Rotate certs",
        "inputs": [{"name": "env", "type": "text", "default": "prod"}],
        "steps": [{"name": "rotate", "run": "echo rotated"}],
        "notifications": [{"on": ["succeeded"], "email": ["oncall@example.com", "lead@example.com"]}]
    }))
    .await;
    let run = call(&s.app, "POST", &format!("/api/v1/runbooks/{rb}/runs"), None, Some(&c), true).await.json();
    let id = run["id"].as_str().unwrap().to_string();
    wait_finished(&s, &c, &id).await;
    let list = notices(&s, &c, &id, 1).await;
    assert_eq!((list[0]["ok"].clone(), list[0]["target"].as_str()), (json!(true), Some("oncall@example.com, lead@example.com")), "{list:?}");
    let got = mail.lock().unwrap().clone();
    assert_eq!(got.len(), 2);
    assert!(got[0].contains("Subject: [OpSkills] Test notification"));
    // The body is quoted-printable: undo soft line breaks and the encoded "=".
    let m = &got[1].replace("=\n", "").replace("=3D", "=");
    assert!(m.contains("Subject: [OpSkills] Rotate certs succeeded (run #"), "{m}");
    assert!(m.contains("To: oncall@example.com, lead@example.com") || m.contains("To: oncall@example.com,"), "{m}");
    assert!(m.contains(&format!("https://ops.example.com/#/runs/{id}")) && m.contains("env = prod"), "{m}");
}

#[tokio::test]
async fn email_can_be_set_up_on_the_settings_page_with_a_vault_password() {
    let (port, mail, auths) = fake_smtp_with_auth().await;
    let s = server();
    let c = sign_in(&s).await;
    let get = || call(&s.app, "GET", "/api/v1/settings/email", None, Some(&c), false);
    assert_eq!(get().await.json(), json!({"source": "off", "settings": null}));
    let settings = json!({"smtp_host": "127.0.0.1", "smtp_port": port, "security": "none", "username": "ops", "password_secret": "SMTP_PASSWORD", "from": "OpSkills <ops@example.com>"});
    let r = call(&s.app, "PUT", "/api/v1/settings/email", Some(settings.clone()), Some(&c), true).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "the secret isn't in the vault yet: {}", r.body);
    for bad in [json!({"smtp_host": "", "from": "a@example.com"}), json!({"smtp_host": "smtp.example.com", "from": "nobody"}), json!({"smtp_host": "smtp.example.com", "from": "a@example.com", "username": "ops"}), json!({"smtp_host": "smtp.example.com", "from": "a@example.com", "security": "ssl"})] {
        assert_eq!(call(&s.app, "PUT", "/api/v1/settings/email", Some(bad.clone()), Some(&c), true).await.status, StatusCode::BAD_REQUEST, "{bad}");
    }
    add_secret(&s, &c, "SMTP_PASSWORD", "mail-pass-123").await;
    let r = call(&s.app, "PUT", "/api/v1/settings/email", Some(settings), Some(&c), true).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    let shown = get().await;
    assert_eq!(shown.json()["source"], json!("settings"));
    assert!(!shown.body.contains("mail-pass-123"), "{}", shown.body);
    let ns = call(&s.app, "GET", "/api/v1/notifications/settings", None, Some(&c), false).await.json();
    assert_eq!((ns["email"].clone(), ns["source"].as_str()), (json!(true), Some("settings")));

    let t = call(&s.app, "POST", "/api/v1/notifications/test", Some(json!({"email": ["oncall@example.com"]})), Some(&c), true).await;
    assert_eq!(t.status, StatusCode::OK, "{}", t.body);
    assert_eq!(mail.lock().unwrap().len(), 1);
    // It signed in with the password from the vault (AUTH PLAIN of "\0ops\0mail-pass-123").
    let auth = auths.lock().unwrap().clone();
    assert!(auth.iter().any(|l| l.contains("AG9wcwBtYWlsLXBhc3MtMTIz")), "{auth:?}");

    // The password secret can't be deleted while email uses it.
    let del = call(&s.app, "DELETE", "/api/v1/secrets/SMTP_PASSWORD", None, Some(&c), true).await;
    assert_eq!(del.status, StatusCode::CONFLICT, "{}", del.body);
    assert_eq!(call(&s.app, "DELETE", "/api/v1/settings/email", None, Some(&c), true).await.status, StatusCode::NO_CONTENT);
    assert_eq!(get().await.json()["source"], json!("off"));
    assert_eq!(call(&s.app, "DELETE", "/api/v1/secrets/SMTP_PASSWORD", None, Some(&c), true).await.status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn email_in_the_config_file_wins_and_is_read_only() {
    let s = server_with(|cfg| {
        cfg.email.smtp_host = Some("smtp.example.com".into());
        cfg.email.from = Some("OpSkills <ops@example.com>".into());
        cfg.email.username = Some("ops".into());
        cfg.email.password = Some("from-config-file".into());
    });
    let c = sign_in(&s).await;
    let got = call(&s.app, "GET", "/api/v1/settings/email", None, Some(&c), false).await;
    assert_eq!((got.json()["source"].as_str(), got.json()["settings"]["smtp_host"].as_str()), (Some("config"), Some("smtp.example.com")));
    assert!(!got.body.contains("from-config-file"));
    let put = call(&s.app, "PUT", "/api/v1/settings/email", Some(json!({"smtp_host": "other.example.com", "from": "a@example.com"})), Some(&c), true).await;
    assert_eq!(put.status, StatusCode::CONFLICT);
}

/// Records the headers of each POST.
async fn header_receiver() -> (String, std::sync::Arc<std::sync::Mutex<Vec<axum::http::HeaderMap>>>) {
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let s2 = seen.clone();
    let app = Router::new().fallback(move |headers: axum::http::HeaderMap| {
        let seen = s2.clone();
        async move {
            seen.lock().unwrap().push(headers);
            StatusCode::OK
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}/events"), seen)
}

#[tokio::test]
async fn webhook_notifications_send_custom_headers_and_sign_in() {
    let (url, seen) = header_receiver().await;
    let s = server();
    let c = sign_in(&s).await;
    add_secret(&s, &c, "API_KEY", "k-123").await;
    add_secret(&s, &c, "HOOK_PASS", "p@ss:word").await;
    let rb = |n: Value| json!({"name": "H", "steps": [{"name": "a", "run": "true"}], "notifications": [n]});
    for bad in [
        json!({"on": ["failed"], "email": ["a@example.com"], "headers": [{"name": "X-A", "value": "b"}]}),
        json!({"on": ["failed"], "webhook": url, "headers": [{"name": "Content-Type", "value": "text/plain"}]}),
        json!({"on": ["failed"], "webhook": url, "headers": [{"name": "X Bad", "value": "b"}]}),
        json!({"on": ["failed"], "webhook": url, "headers": [{"name": "X-A", "value": "{{ inputs.x }}"}]}),
        json!({"on": ["failed"], "webhook": url, "headers": [{"name": "X-A", "value": "{{ secrets.NOT_THERE }}"}]}),
        json!({"on": ["failed"], "webhook": url, "headers": [{"name": "Authorization", "value": "x"}], "auth": {"type": "bearer", "secret": "API_KEY"}}),
        json!({"on": ["failed"], "webhook": url, "auth": {"type": "basic", "username": "a:b", "secret": "HOOK_PASS"}}),
    ] {
        let r = call(&s.app, "POST", "/api/v1/runbooks", Some(rb(bad.clone())), Some(&c), true).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{bad} -> {}", r.body);
    }
    let made = call(&s.app, "POST", "/api/v1/runbooks", Some(rb(json!({
        "on": ["succeeded"], "webhook": url,
        "headers": [{"name": "X-Api-Key", "value": "key-{{ secrets.API_KEY }}"}, {"name": "X-Team", "value": "ops"}],
        "auth": {"type": "basic", "username": "opskills", "secret": "HOOK_PASS"}
    }))), Some(&c), true).await;
    assert_eq!(made.status, StatusCode::CREATED, "{}", made.body);
    assert!(!made.body.contains("k-123"), "only the reference is stored");
    let rb_id = made.json()["id"].as_str().unwrap().to_string();
    let run = call(&s.app, "POST", &format!("/api/v1/runbooks/{rb_id}/runs"), None, Some(&c), true).await.json();
    let id = run["id"].as_str().unwrap().to_string();
    wait_finished(&s, &c, &id).await;
    let list = notices(&s, &c, &id, 1).await;
    assert_eq!(list[0]["ok"], json!(true), "{list:?}");
    let h = wait_for(|| seen.lock().unwrap().first().cloned(), "the webhook call").await;
    let get = |k: &str| h.get(k).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    assert_eq!((get("x-api-key"), get("x-team")), ("key-k-123".to_string(), "ops".to_string()));
    // base64("opskills:p@ss:word")
    assert_eq!(get("authorization"), "Basic b3Bza2lsbHM6cEBzczp3b3Jk");

    // Bearer, through the test endpoint.
    let t = call(&s.app, "POST", "/api/v1/notifications/test", Some(json!({"webhook": url, "auth": {"type": "bearer", "secret": "API_KEY"}})), Some(&c), true).await;
    assert_eq!(t.status, StatusCode::OK, "{}", t.body);
    let h = wait_for(|| seen.lock().unwrap().get(1).cloned(), "the test call").await;
    assert_eq!(h.get("authorization").unwrap(), "Bearer k-123");

    // Secrets a notification uses can't be deleted.
    let del = call(&s.app, "DELETE", "/api/v1/secrets/HOOK_PASS", None, Some(&c), true).await;
    assert_eq!(del.status, StatusCode::CONFLICT, "{}", del.body);
}

#[tokio::test]
async fn webhook_notifications_can_send_a_custom_body() {
    let (base, seen) = receiver().await;
    let s = server();
    let c = sign_in(&s).await;
    add_secret(&s, &c, "ALERT_ROUTING_KEY", "R0UT1NG").await;
    let rb = |n: Value| json!({"name": "B", "steps": [{"name": "a", "run": "true"}], "notifications": [n]});
    for bad in [
        json!({"on": ["failed"], "webhook": base, "body": "{not json"}),
        json!({"on": ["failed"], "webhook": base, "body": "\"just a string\""}),
        json!({"on": ["failed"], "webhook": base, "body": "{\"a\": \"{{ run.nope }}\"}"}),
        json!({"on": ["failed"], "webhook": base, "body": "{\"a\": \"{{ secrets.MISSING }}\"}"}),
        json!({"on": ["failed"], "email": ["a@example.com"], "body": "{}"}),
    ] {
        let r = call(&s.app, "POST", "/api/v1/runbooks", Some(rb(bad.clone())), Some(&c), true).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{bad} -> {}", r.body);
    }
    let template = r#"{
      "routing_key": "{{ secrets.ALERT_ROUTING_KEY }}",
      "event_action": "trigger",
      "dedup_key": "opskills-{{ runbook.id }}",
      "payload": {
        "summary": "{{ text }}",
        "source": "opskills",
        "severity": "error",
        "custom_details": {
          "exit_code": "{{ run.failed_step.exit_code }}",
          "env": "{{ run.inputs.env }}",
          "output": "{{ output_tail }}",
          "output_text": "last lines: {{ output_tail }}",
          "error": "{{ run.error }}"
        }
      }
    }"#;
    let made = call(&s.app, "POST", "/api/v1/runbooks", Some(json!({
        "name": "Payments",
        "inputs": [{"name": "env", "type": "text", "default": "prod"}],
        "steps": [{"name": "charge", "run": "echo 'card declined'; exit 4"}],
        "notifications": [{"on": ["failed"], "webhook": format!("{base}/v2/enqueue"), "body": template}]
    })), Some(&c), true).await;
    assert_eq!(made.status, StatusCode::CREATED, "{}", made.body);
    let rb_id = made.json()["id"].as_str().unwrap().to_string();
    let run = call(&s.app, "POST", &format!("/api/v1/runbooks/{rb_id}/runs"), None, Some(&c), true).await.json();
    let id = run["id"].as_str().unwrap().to_string();
    wait_finished(&s, &c, &id).await;
    assert_eq!(notices(&s, &c, &id, 1).await[0]["ok"], json!(true));
    let got = wait_for(|| seen.lock().unwrap().iter().find(|(p, _, _)| p == "/v2/enqueue").cloned(), "the custom body").await;
    let b = got.2;
    assert_eq!(b["routing_key"], json!("R0UT1NG"));
    assert_eq!(b["dedup_key"], json!(format!("opskills-{rb_id}")));
    assert!(b["payload"]["summary"].as_str().unwrap().starts_with("Payments failed: run #"), "{b}");
    let d = &b["payload"]["custom_details"];
    assert_eq!(d["exit_code"], json!(4), "a lone placeholder keeps its type");
    assert_eq!(d["env"], json!("prod"));
    assert!(d["output"].as_array().unwrap().iter().any(|l| l == "card declined"), "{d}");
    assert!(d["output_text"].as_str().unwrap().starts_with("last lines: ") && d["output_text"].as_str().unwrap().contains("card declined\n"), "{d}");
    assert_eq!(d["error"], Value::Null);
    assert!(b.get("text").is_none(), "only the custom body is sent");

    // The test button renders the template with example values.
    let t = call(&s.app, "POST", "/api/v1/notifications/test", Some(json!({"webhook": format!("{base}/test"), "body": "{\"summary\": \"{{ text }}\", \"code\": \"{{ run.failed_step.exit_code }}\"}"})), Some(&c), true).await;
    assert_eq!(t.status, StatusCode::OK, "{}", t.body);
    let tb = wait_for(|| seen.lock().unwrap().iter().find(|(p, _, _)| p == "/test").cloned(), "the test body").await.2;
    assert_eq!(tb["code"], json!(1));
    assert!(tb["summary"].as_str().unwrap().contains("test notification"));
    // A secret used in a body can't be deleted.
    assert_eq!(call(&s.app, "DELETE", "/api/v1/secrets/ALERT_ROUTING_KEY", None, Some(&c), true).await.status, StatusCode::CONFLICT);
}
