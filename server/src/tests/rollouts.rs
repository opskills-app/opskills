//! Rollouts across hosts, host filters, and retrying unfinished hosts.

use super::*;

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
