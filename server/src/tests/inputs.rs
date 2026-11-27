//! Inputs, templates, retries and failure handling.

use super::*;

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
