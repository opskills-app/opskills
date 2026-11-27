//! Schedules.

use super::*;

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
