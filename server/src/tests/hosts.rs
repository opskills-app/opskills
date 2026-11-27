//! Hosts and the agent protocol.

use super::*;

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
