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
