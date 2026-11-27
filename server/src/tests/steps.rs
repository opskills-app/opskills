//! Copy and call steps.

use super::*;

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
