//! Inbound webhooks.

use super::*;

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
