//! Workspaces: what members see, secrets per workspace, members, shared hosts, deletion.

use super::*;

async fn new_workspace(s: &TestServer, admin: &str, slug: &str, name: &str) {
    let r = call(&s.app, "POST", "/api/v1/workspaces", Some(json!({"slug": slug, "name": name})), Some(admin), true).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
}

fn names(list: &Value) -> Vec<String> {
    list.as_array().unwrap().iter().map(|v| v["name"].as_str().unwrap().to_string()).collect()
}

#[tokio::test]
async fn members_see_only_their_workspaces() {
    let s = server();
    let admin = sign_in(&s).await;
    new_workspace(&s, &admin, "ops", "Ops").await;
    let olly = user_with(&s, &admin, json!({"name": "olly", "workspaces": {"ops": "editor"}})).await;
    let in_default = create(&s, &admin, json!({"name": "Default book", "steps": [{"name": "a", "run": "true"}]})).await;

    // Outside a workspace, it doesn't exist: no names or contents leak.
    for path in ["/api/v1/runbooks", "/api/v1/runbooks?workspace=default", "/api/v1/runs", "/api/v1/secrets", "/api/v1/runbooks?workspace=nowhere"] {
        let r = call(&s.app, "GET", path, None, Some(&olly), false).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{path}: {}", r.body);
        assert!(r.body.contains("No such workspace"), "{path}: {}", r.body);
    }
    let r = call(&s.app, "GET", &format!("/api/v1/runbooks/{in_default}"), None, Some(&olly), false).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    assert!(r.body.contains("Runbook not found"), "{}", r.body);
    assert_eq!(call(&s.app, "GET", "/api/v1/runbooks?workspace=nowhere", None, Some(&admin), false).await.status, StatusCode::NOT_FOUND);
    let me = call(&s.app, "GET", "/api/v1/auth/me", None, Some(&olly), false).await.json();
    assert_eq!(me["workspaces"], json!([{"slug": "ops", "name": "Ops", "role": "editor"}]));
    assert_eq!(me["admin"], false);

    // Inside it, the usual roles apply, and lists show only that workspace.
    let r = call(&s.app, "POST", "/api/v1/runbooks?workspace=ops", Some(json!({"name": "Ops book", "steps": [{"name": "a", "run": "true"}]})), Some(&olly), true).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
    assert_eq!(r.json()["workspace"], "ops");
    assert_eq!(names(&call(&s.app, "GET", "/api/v1/runbooks?workspace=ops", None, Some(&olly), false).await.json()), ["Ops book"]);
    assert_eq!(names(&call(&s.app, "GET", "/api/v1/runbooks", None, Some(&admin), false).await.json()), ["Default book"]);

    // The same secret name in two workspaces holds two values.
    assert_eq!(add_secret(&s, &admin, "TOKEN", "default-value").await.status, StatusCode::CREATED);
    let r = call(&s.app, "POST", "/api/v1/secrets?workspace=ops", Some(json!({"name": "TOKEN", "kind": "token", "description": "", "value": "ops-value"})), Some(&olly), true).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
    let check = |want: &str| json!({"name": "Check token", "secrets": ["TOKEN"], "steps": [{"name": "a", "run": format!("test \"$TOKEN\" = {want}")}]});
    let r = call(&s.app, "POST", "/api/v1/runbooks?workspace=ops", Some(check("ops-value")), Some(&olly), true).await;
    let ops_book = r.json()["id"].as_str().unwrap().to_string();
    let default_book = create(&s, &admin, check("default-value")).await;
    for (who, book) in [(&olly, &ops_book), (&admin, &default_book)] {
        let run = start(&s, who, book).await;
        assert_eq!(wait_finished(&s, who, &run).await["status"], "succeeded", "{book}");
    }
    let ops_runs = call(&s.app, "GET", "/api/v1/runs?workspace=ops", None, Some(&olly), false).await.json();
    assert_eq!(ops_runs.as_array().unwrap().len(), 1);
    assert_eq!(ops_runs[0]["workspace"], "ops");

    // Calls and host targets don't cross workspaces.
    let call_out = json!({"name": "Caller", "steps": [{"name": "a", "call": {"runbook": in_default}}]});
    let r = call(&s.app, "POST", "/api/v1/runbooks?workspace=ops", Some(call_out), Some(&olly), true).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert!(r.body.contains("no runbook with id"), "{}", r.body);
    let (host, _) = enroll(&s, &admin, "web-1", json!([])).await;
    let r = call(&s.app, "POST", "/api/v1/runbooks?workspace=ops", Some(json!({"name": "Pinned", "steps": [{"name": "a", "run": "true"}], "targets": {"hosts": [host]}})), Some(&olly), true).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert!(r.body.contains("no host with id"), "{}", r.body);

    // Each workspace has its own audit log; server admins see all of them.
    let ops_log = call(&s.app, "GET", "/api/v1/audit?workspace=ops", None, Some(&admin), false).await.json();
    let events = ops_log["events"].as_array().unwrap();
    assert!(events.iter().all(|e| e["workspace"] == "ops"), "{ops_log}");
    assert!(events.iter().any(|e| e["action"] == "runbook.create" && e["actor"] == "olly"));
    assert_eq!(call(&s.app, "GET", "/api/v1/workspaces/ops/audit", None, Some(&olly), false).await.status, StatusCode::FORBIDDEN);
    assert_eq!(call(&s.app, "GET", "/api/v1/audit", None, Some(&olly), false).await.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn workspace_admins_manage_members_not_accounts() {
    let s = server();
    let admin = sign_in(&s).await;
    new_workspace(&s, &admin, "ops", "Ops").await;
    let wsa = user_with(&s, &admin, json!({"name": "wsa", "workspaces": {"ops": "admin"}})).await;
    let bob = user_with(&s, &admin, json!({"name": "bob"})).await;
    assert_eq!(call(&s.app, "GET", "/api/v1/workspaces", None, Some(&bob), false).await.json(), json!([]));
    assert_eq!(call(&s.app, "GET", "/api/v1/runbooks?workspace=ops", None, Some(&bob), false).await.status, StatusCode::NOT_FOUND);

    let addable = call(&s.app, "GET", "/api/v1/workspaces/ops/addable-users", None, Some(&wsa), false).await.json();
    assert!(addable.as_array().unwrap().contains(&json!("bob")) && !addable.as_array().unwrap().contains(&json!("wsa")), "{addable}");
    let r = call(&s.app, "PUT", "/api/v1/workspaces/ops/members/bob", Some(json!({"role": "viewer"})), Some(&wsa), true).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(call(&s.app, "GET", "/api/v1/runbooks?workspace=ops", None, Some(&bob), false).await.status, StatusCode::OK);
    let members = call(&s.app, "GET", "/api/v1/workspaces/ops/members", None, Some(&bob), false).await.json();
    assert_eq!(members["members"], json!([{"name": "bob", "role": "viewer"}, {"name": "wsa", "role": "admin"}]));
    assert_eq!(members["server_admins"], json!(["admin"]));

    let refused = [
        ("PUT", "/api/v1/workspaces/ops/members/nobody", Some(json!({"role": "viewer"})), StatusCode::NOT_FOUND),
        ("PUT", "/api/v1/workspaces/ops/members/wsa", Some(json!({"role": "viewer"})), StatusCode::CONFLICT),
        ("PUT", "/api/v1/workspaces/default/members/bob", Some(json!({"role": "viewer"})), StatusCode::NOT_FOUND),
        ("POST", "/api/v1/users", Some(json!({"name": "carol"})), StatusCode::FORBIDDEN),
        ("DELETE", "/api/v1/workspaces/ops", None, StatusCode::FORBIDDEN),
        ("POST", "/api/v1/workspaces", Some(json!({"slug": "mine", "name": "Mine"})), StatusCode::FORBIDDEN),
    ];
    for (method, path, body, status) in refused {
        let r = call(&s.app, method, path, body, Some(&wsa), true).await;
        assert_eq!(r.status, status, "{method} {path}: {}", r.body);
    }
    let r = call(&s.app, "PUT", "/api/v1/workspaces/ops", Some(json!({"name": "Operations"})), Some(&wsa), true).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);

    // Removing a member ends their access at once.
    assert_eq!(call(&s.app, "DELETE", "/api/v1/workspaces/ops/members/bob", None, Some(&wsa), true).await.status, StatusCode::NO_CONTENT);
    assert_eq!(call(&s.app, "GET", "/api/v1/runbooks?workspace=ops", None, Some(&bob), false).await.status, StatusCode::NOT_FOUND);
    let log = call(&s.app, "GET", "/api/v1/workspaces/ops/audit", None, Some(&wsa), false).await.json();
    let actions: Vec<&str> = log["events"].as_array().unwrap().iter().map(|e| e["action"].as_str().unwrap()).collect();
    for want in ["member.add", "member.remove", "workspace.update", "workspace.create"] {
        assert!(actions.contains(&want), "{want} missing from {actions:?}");
    }
}

#[tokio::test]
async fn hosts_can_be_shared_between_workspaces() {
    let s = server();
    let admin = sign_in(&s).await;
    new_workspace(&s, &admin, "ops", "Ops").await;
    let wsa = user_with(&s, &admin, json!({"name": "wsa", "workspaces": {"ops": "admin"}})).await;
    let (host, _) = enroll(&s, &admin, "shared-1", json!([])).await;

    // Sharing needs Admin in every workspace involved.
    let r = call(&s.app, "PUT", &format!("/api/v1/hosts/{host}/workspaces/ops"), None, Some(&wsa), true).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN, "{}", r.body);
    assert!(r.body.contains("workspace you don't belong to"), "{}", r.body);
    let r = call(&s.app, "PUT", &format!("/api/v1/hosts/{host}/workspaces/ops"), None, Some(&admin), true).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(r.json()["workspaces"], json!(["default", "ops"]));

    // In ops it's a host like any other, without revealing where else it is.
    let hosts = call(&s.app, "GET", "/api/v1/hosts?workspace=ops", None, Some(&wsa), false).await.json();
    let shared = hosts.as_array().unwrap().iter().find(|h| h["id"] == host.as_str()).expect("the shared host is listed").clone();
    assert_eq!(shared["workspaces"], json!(["ops"]));
    assert_eq!(shared["shared_elsewhere"], true);
    let r = call(&s.app, "PUT", &format!("/api/v1/hosts/{host}"), Some(json!({"tags": ["x"]})), Some(&wsa), true).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN, "changing a shared host needs Admin in all its workspaces");
    let r = call(&s.app, "POST", "/api/v1/runbooks?workspace=ops", Some(json!({"name": "On shared", "steps": [{"name": "a", "run": "true"}], "targets": {"hosts": [host]}})), Some(&wsa), true).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
    let rb = r.json()["id"].as_str().unwrap().to_string();
    let run = start(&s, &wsa, &rb).await;
    let r = call(&s.app, "GET", &format!("/api/v1/runs/{run}"), None, Some(&wsa), false).await.json();
    assert_eq!(r["hosts"][0]["id"], host.as_str());
    call(&s.app, "POST", &format!("/api/v1/runs/{run}/cancel"), None, Some(&wsa), true).await;

    // Taking it out of ops: not while ops runbooks target it, and never from its last workspace.
    let r = call(&s.app, "DELETE", &format!("/api/v1/hosts/{host}/workspaces/ops"), None, Some(&wsa), true).await;
    assert_eq!(r.status, StatusCode::CONFLICT, "{}", r.body);
    assert_eq!(call(&s.app, "DELETE", &format!("/api/v1/runbooks/{rb}"), None, Some(&wsa), true).await.status, StatusCode::NO_CONTENT);
    assert_eq!(call(&s.app, "DELETE", &format!("/api/v1/hosts/{host}/workspaces/ops"), None, Some(&wsa), true).await.status, StatusCode::NO_CONTENT);
    let r = call(&s.app, "DELETE", &format!("/api/v1/hosts/{host}/workspaces/default"), None, Some(&admin), true).await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    assert!(r.body.contains("only workspace"), "{}", r.body);

    // A host enrolls into the workspace its token was made in.
    let t = call(&s.app, "POST", "/api/v1/hosts/enrollment-tokens?workspace=ops", None, Some(&wsa), true).await;
    assert_eq!(t.status, StatusCode::CREATED, "{}", t.body);
    let token = t.json()["token"].as_str().unwrap().to_string();
    let r = agent_call(&s.app, "/api/v1/agent/enroll", json!({"token": token, "info": info(), "name": "ops-1"}), None).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
    let ops_hosts = call(&s.app, "GET", "/api/v1/hosts?workspace=ops", None, Some(&wsa), false).await.json();
    assert!(ops_hosts.as_array().unwrap().iter().any(|h| h["name"] == "ops-1" && h["workspaces"] == json!(["ops"])), "{ops_hosts}");
    let default_hosts = call(&s.app, "GET", "/api/v1/hosts", None, Some(&admin), false).await.json();
    assert!(!default_hosts.as_array().unwrap().iter().any(|h| h["name"] == "ops-1"));
}

#[tokio::test]
async fn workspaces_are_deleted_only_when_empty() {
    let s = server();
    let admin = sign_in(&s).await;
    for (body, status) in [(json!({"slug": "Bad Slug", "name": "X"}), StatusCode::BAD_REQUEST), (json!({"slug": "default", "name": "Again"}), StatusCode::CONFLICT)] {
        assert_eq!(call(&s.app, "POST", "/api/v1/workspaces", Some(body), Some(&admin), true).await.status, status);
    }
    new_workspace(&s, &admin, "tmp", "Temporary").await;
    user_with(&s, &admin, json!({"name": "tia", "workspaces": {"tmp": "viewer"}})).await;
    let r = call(&s.app, "POST", "/api/v1/runbooks?workspace=tmp", Some(json!({"name": "Temp", "steps": [{"name": "a", "run": "true"}]})), Some(&admin), true).await;
    let rb = r.json()["id"].as_str().unwrap().to_string();
    let run = start(&s, &admin, &rb).await;
    wait_finished(&s, &admin, &run).await;
    call(&s.app, "POST", "/api/v1/secrets?workspace=tmp", Some(json!({"name": "TMP", "kind": "other", "description": "", "value": "x-value"})), Some(&admin), true).await;
    let t = call(&s.app, "POST", "/api/v1/hosts/enrollment-tokens?workspace=tmp", None, Some(&admin), true).await;
    let r = agent_call(&s.app, "/api/v1/agent/enroll", json!({"token": t.json()["token"], "info": info(), "name": "tmp-1"}), None).await;
    let host = r.json()["host_id"].as_str().unwrap().to_string();

    let delete = || call(&s.app, "DELETE", "/api/v1/workspaces/tmp", None, Some(&admin), true);
    for (blocker, cleanup) in [("runbook", format!("/api/v1/runbooks/{rb}")), ("secret", "/api/v1/secrets/TMP?workspace=tmp".to_string()), ("tmp-1", format!("/api/v1/hosts/{host}"))] {
        let r = delete().await;
        assert_eq!(r.status, StatusCode::CONFLICT, "{}", r.body);
        assert!(r.body.contains(blocker), "{blocker}: {}", r.body);
        assert_eq!(call(&s.app, "DELETE", &cleanup, None, Some(&admin), true).await.status, StatusCode::NO_CONTENT, "{cleanup}");
    }
    assert_eq!(delete().await.status, StatusCode::NO_CONTENT);
    // Its run history and memberships went with it.
    assert_eq!(call(&s.app, "GET", &format!("/api/v1/runs/{run}"), None, Some(&admin), false).await.status, StatusCode::NOT_FOUND);
    assert!(s.state.store.user("tia").unwrap().unwrap().user.workspaces.is_empty());
    let r = call(&s.app, "DELETE", "/api/v1/workspaces/default", None, Some(&admin), true).await;
    assert_eq!(r.status, StatusCode::CONFLICT);
}
