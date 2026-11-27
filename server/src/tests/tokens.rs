//! API tokens.

use super::*;

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
