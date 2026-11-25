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
    password: String,
    dir: std::path::PathBuf,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn server() -> TestServer {
    let dir = std::env::temp_dir().join(format!("opskills-it-{}", uuid::Uuid::now_v7()));
    let mut cfg = Config::default();
    cfg.data.dir = dir.clone();
    let fs = firststart::prepare(&cfg).unwrap();
    let password = fs.initial_password.clone().unwrap();
    let state = web::build_state(cfg, &fs).unwrap();
    TestServer { app: web::router(state), password, dir }
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
    assert!(logs.contains("timed out after 1 seconds"), "{logs}");
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
            steps: vec![crate::model::Step { name: "a".into(), run: "true".into() }],
            secrets: vec![],
            timeout_seconds: 60,
            created_at: 0,
            updated_at: 0,
        };
        let mut run = crate::model::Run::new(&rb, "admin");
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
