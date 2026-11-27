//! Signing in, runbooks, and runs on the server: success, failure, timeouts, cancelling and live output.

use super::*;

#[tokio::test]
async fn sign_in_csrf_and_sign_out() {
    let s = server();
    assert_eq!(call(&s.app, "GET", "/api/v1/runbooks", None, None, false).await.status, StatusCode::UNAUTHORIZED);
    let system = call(&s.app, "GET", "/api/v1/system", None, None, false).await;
    assert_eq!(system.status, StatusCode::OK);
    let system = system.json();
    assert_eq!(system["license"], "AGPL-3.0-only");
    assert_eq!(system["source_url"], format!("https://github.com/opskills-app/opskills/tree/v{}", crate::VERSION));
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
        let store = crate::store::Store::open(&path, &dir).unwrap();
        let rb = crate::model::Runbook {
            id: "rb".into(),
            workspace: "default".into(),
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
