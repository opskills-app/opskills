//! The audit log: what gets recorded, by whom and how, search and paging, and retention.

use super::*;
use crate::audit::{self, By};

async fn audit_log(s: &TestServer, c: &str, query: &str) -> Value {
    let r = call(&s.app, "GET", &format!("/api/v1/audit{query}"), None, Some(c), false).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    r.json()
}

fn actions(log: &Value) -> Vec<String> {
    log["events"].as_array().unwrap().iter().map(|e| e["action"].as_str().unwrap().to_string()).collect()
}

#[tokio::test]
async fn changes_are_recorded_with_who_and_how() {
    let s = server();
    assert_eq!(sign_in_as(&s, "admin", "not the password").await.status, StatusCode::UNAUTHORIZED);
    let c = sign_in(&s).await;
    let rb = create(&s, &c, json!({"name": "Deploy", "steps": [{"name": "a", "run": "true"}]})).await;
    let r = call(&s.app, "PUT", &format!("/api/v1/runbooks/{rb}"), Some(json!({"name": "Deploy web", "steps": [{"name": "a", "run": "true"}]})), Some(&c), true).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    let run = start(&s, &c, &rb).await;
    wait_finished(&s, &c, &run).await;
    assert_eq!(add_secret(&s, &c, "DB_PASSWORD", "very-secret-value-1").await.status, StatusCode::CREATED);
    let r = call(&s.app, "PUT", "/api/v1/secrets/DB_PASSWORD", Some(json!({"value": "very-secret-value-2"})), Some(&c), true).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    let r = call(&s.app, "POST", "/api/v1/me/tokens", Some(json!({"name": "ci"})), Some(&c), true).await;
    let token = r.json()["value"].as_str().unwrap().to_string();
    let r = bearer_call(&s.app, "POST", &format!("/api/v1/runbooks/{rb}/runs"), None, &token).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
    user_with_role(&s, &c, "ops", "operator").await;
    let r = call(&s.app, "PUT", "/api/v1/workspaces/default/members/ops", Some(json!({"role": "viewer"})), Some(&c), true).await;
    assert_eq!(r.status, StatusCode::OK);

    let log = audit_log(&s, &c, "").await;
    let got = actions(&log);
    for want in [
        "auth.sign_in_failed", "auth.sign_in", "runbook.create", "runbook.update", "run.start", "secret.create", "secret.update",
        "token.create", "user.create", "member.add", "auth.password_change", "member.role_change",
    ] {
        assert!(got.iter().any(|a| a == want), "{want} missing from {got:?}");
    }
    // Newest first.
    assert_eq!(got[0], "member.role_change");
    let events = log["events"].as_array().unwrap();
    let find = |action: &str| events.iter().find(|e| e["action"] == action).unwrap().clone();
    assert_eq!(find("member.role_change")["detail"], "role operator to viewer");
    assert_eq!(find("member.role_change")["workspace"], "default");
    assert_eq!(find("user.create")["detail"], "operator in default");
    assert!(find("auth.sign_in").get("workspace").is_none(), "sign-ins are server-wide");
    assert_eq!(find("runbook.update")["detail"], "renamed from \"Deploy\"");
    assert_eq!(find("auth.sign_in_failed")["detail"], "wrong password");
    let by_token = events.iter().find(|e| e["action"] == "run.start" && e["via"] == "token ci").expect("run started by the token");
    assert_eq!(by_token["actor"], "admin");
    assert_eq!(by_token["target"]["name"], "Deploy web");
    assert_eq!(find("secret.update")["detail"], "value replaced");
    // Secret values never reach the audit log.
    assert!(!log.to_string().contains("very-secret-value"), "{log}");
    assert_eq!(log["retention_days"], 90);
}

#[tokio::test]
async fn the_log_can_be_searched_and_paged() {
    let s = server();
    let c = sign_in(&s).await;
    for i in 0..5 {
        create(&s, &c, json!({"name": format!("Book {i}"), "steps": [{"name": "a", "run": "true"}]})).await;
    }
    let only = audit_log(&s, &c, "?action=runbook.").await;
    assert_eq!(actions(&only), vec!["runbook.create"; 5]);
    let found = audit_log(&s, &c, "?q=book%203").await;
    assert_eq!(found["events"].as_array().unwrap().len(), 1);
    assert_eq!(found["events"][0]["target"]["name"], "Book 3");
    assert_eq!(audit_log(&s, &c, "?actor=nobody").await["events"].as_array().unwrap().len(), 0);

    let page1 = audit_log(&s, &c, "?action=runbook.&limit=2").await;
    let next = page1["next"].as_str().expect("more pages").to_string();
    let page2 = audit_log(&s, &c, &format!("?action=runbook.&limit=2&before={next}")).await;
    let names = |p: &Value| p["events"].as_array().unwrap().iter().map(|e| e["target"]["name"].as_str().unwrap().to_string()).collect::<Vec<_>>();
    assert_eq!(names(&page1), ["Book 4", "Book 3"]);
    assert_eq!(names(&page2), ["Book 2", "Book 1"]);
}

#[tokio::test]
async fn schedules_and_webhooks_act_as_the_system() {
    let s = server();
    let c = sign_in(&s).await;
    let id = create(&s, &c, json!({"name": "Nightly", "steps": [{"name": "a", "run": "true"}]})).await;
    let rb = s.state.store.runbook(&id).unwrap().unwrap();
    let run = crate::api::start_runbook(&s.state, &rb, &serde_json::Map::new(), &By::system("schedule nightly"), "schedule").map_err(|e| e.detail()).unwrap();
    assert_eq!(run.started_by, "schedule nightly", "runs keep naming the schedule");
    let log = audit_log(&s, &c, "?action=run.start").await;
    assert_eq!(log["events"][0]["actor"], "system");
    assert_eq!(log["events"][0]["via"], "schedule nightly");
    assert_eq!(log["events"][0]["detail"], "schedule run");
    assert_eq!(By::system("webhook deploy").label(), "webhook deploy");
}

#[tokio::test]
async fn old_events_are_removed_after_the_retention_period() {
    let s = server_with(|cfg| cfg.audit.retention_days = 30);
    let c = sign_in(&s).await;
    let old = crate::model::AuditEvent {
        id: uuid::Uuid::now_v7().to_string(),
        at: crate::model::now_ms() - 31 * 86_400_000,
        actor: "admin".into(),
        via: None,
        action: "runbook.delete".into(),
        workspace: None,
        target: None,
        detail: None,
    };
    s.state.store.append_audit(&old).unwrap();
    assert!(actions(&audit_log(&s, &c, "").await).contains(&"runbook.delete".to_string()));
    audit::purge(&s.state);
    let after = actions(&audit_log(&s, &c, "").await);
    assert!(!after.contains(&"runbook.delete".to_string()), "{after:?}");
    assert!(after.contains(&"auth.sign_in".to_string()), "recent events stay");
}
