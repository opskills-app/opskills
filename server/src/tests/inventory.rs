//! Inventory: attributes, tag rules, and static, URL and program sources.

use std::sync::{Arc, Mutex};

use super::*;

async fn add_source(s: &TestServer, c: &str, body: Value) -> Reply {
    call(&s.app, "POST", "/api/v1/inventory/sources", Some(body), Some(c), true).await
}

async fn hosts(s: &TestServer, c: &str, filter: Option<&str>) -> Vec<Value> {
    let path = match filter {
        Some(f) => format!("/api/v1/hosts?filter={}", f.replace(' ', "%20")),
        None => "/api/v1/hosts".into(),
    };
    let r = call(&s.app, "GET", &path, None, Some(c), false).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    r.json().as_array().unwrap().iter().filter(|h| h["builtin"] != true).cloned().collect()
}

fn named<'a>(list: &'a [Value], name: &str) -> &'a Value {
    list.iter().find(|h| h["name"] == name).unwrap_or_else(|| panic!("no host {name} in {list:?}"))
}

fn names(list: &[Value]) -> Vec<String> {
    list.iter().map(|h| h["name"].as_str().unwrap().to_string()).collect()
}

#[tokio::test]
async fn a_static_source_enriches_enrolled_hosts_and_lists_others() {
    let s = server();
    let c = sign_in(&s).await;
    let (web, _) = enroll(&s, &c, "web-1", json!(["web"])).await;
    let r = add_source(&s, &c, json!({"name": "CMDB", "type": "static", "hosts": [
        {"name": "web-1", "hostname": "web-1.internal", "tags": ["frontend"], "attributes": {"env": "prod", "region": "eu"}},
        {"name": "legacy-1", "hostname": "10.0.0.9", "tags": ["Legacy"], "attributes": {"env": "prod"}},
    ]})).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
    assert_eq!((r.json()["state"]["host_count"].as_u64(), r.json()["state"]["error"].clone()), (Some(2), Value::Null));

    let list = hosts(&s, &c, None).await;
    let w = named(&list, "web-1");
    assert_eq!((w["reach"].as_str(), w["id"].as_str()), (Some("agent"), Some(web.as_str())));
    assert_eq!(w["tags"], json!(["web", "frontend"]));
    assert_eq!(w["own_tags"], json!(["web"]));
    assert_eq!(w["attributes"], json!({"env": "prod", "region": "eu"}));
    assert_eq!(w["sources"][0]["name"], "CMDB");
    let l = named(&list, "legacy-1");
    assert_eq!((l["reach"].as_str(), l["online"].as_bool()), (Some("listed"), Some(false)));
    assert_eq!(l["tags"], json!(["legacy"]));

    // An Admin's attribute wins over a source's; filters see both.
    let r = call(&s.app, "PUT", &format!("/api/v1/hosts/{web}"), Some(json!({"attributes": {"env": "staging"}})), Some(&c), true).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(r.json()["attributes"], json!({"env": "staging", "region": "eu"}));
    assert_eq!(names(&hosts(&s, &c, Some("env:prod")).await), ["legacy-1"]);
    assert_eq!(names(&hosts(&s, &c, Some("region:eu")).await), ["web-1"]);

    // Tag rules add tags to the hosts their filter matches.
    let r = call(&s.app, "PUT", "/api/v1/workspaces/default/tag-rules", Some(json!({"rules": [{"filter": "env:staging", "tags": ["Canary"]}]})), Some(&c), true).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    let w = named(&hosts(&s, &c, None).await, "web-1").clone();
    assert_eq!((w["tags"].clone(), w["rule_tags"].clone()), (json!(["web", "frontend", "canary"]), json!(["canary"])));
    assert_eq!(names(&hosts(&s, &c, Some("tags:canary")).await), ["web-1"]);

    // Runs reach agent hosts, record their attributes, and leave out listed-only ones.
    let rb = create(&s, &c, json!({"name": "Everywhere", "targets": {"hosts": [], "tags": [], "filter": "env:*"},
        "steps": [{"name": "a", "run": "echo {{ host.attr.region }} [{{ host.attr.nope }}]"}]})).await;
    let r = start_with(&s, &c, &rb, json!({})).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
    assert_eq!(r.json()["skipped_hosts"], json!(["legacy-1"]));
    assert_eq!(r.json()["hosts"], json!([{"id": web, "name": "web-1", "attributes": {"env": "staging", "region": "eu"}}]));
    call(&s.app, "POST", &format!("/api/v1/runs/{}/cancel", r.json()["id"].as_str().unwrap()), None, Some(&c), true).await;
    let only_legacy = create(&s, &c, json!({"name": "Legacy only", "targets": {"hosts": [], "tags": ["legacy"], "filter": ""}, "steps": [{"name": "a", "run": "true"}]})).await;
    let r = start_with(&s, &c, &only_legacy, json!({})).await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    assert!(r.body.contains("only listed by an inventory source"), "{}", r.body);

    // Removing the source removes what it added.
    let id = call(&s.app, "GET", "/api/v1/inventory/sources", None, Some(&c), false).await.json()[0]["id"].as_str().unwrap().to_string();
    assert_eq!(call(&s.app, "DELETE", &format!("/api/v1/inventory/sources/{id}"), None, Some(&c), true).await.status, StatusCode::NO_CONTENT);
    let list = hosts(&s, &c, None).await;
    assert_eq!(names(&list), ["web-1"]);
    assert_eq!(named(&list, "web-1")["attributes"], json!({"env": "staging"}));
}

/// A server for a URL source: serves `doc`, or `status` when it isn't 200, and wants
/// the header `x-api-key: k3y-value-1234`.
async fn doc_server() -> (String, Arc<Mutex<(u16, String)>>) {
    let doc = Arc::new(Mutex::new((200u16, String::new())));
    let d2 = doc.clone();
    let app = Router::new().route("/moved", axum::routing::get(|| async { axum::response::Redirect::temporary("/hosts.json") })).route(
        "/hosts.json",
        axum::routing::get(move |headers: axum::http::HeaderMap| {
            let d = d2.clone();
            async move {
                if headers.get("x-api-key").and_then(|v| v.to_str().ok()) != Some("k3y-value-1234") {
                    return (StatusCode::UNAUTHORIZED, String::new());
                }
                let (code, body) = d.lock().unwrap().clone();
                (StatusCode::from_u16(code).unwrap(), body)
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/hosts.json", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (url, doc)
}

#[tokio::test]
async fn a_url_source_refreshes_and_keeps_its_last_good_hosts_when_it_fails() {
    let s = server();
    let c = sign_in(&s).await;
    add_secret(&s, &c, "inv/API_KEY", "k3y-value-1234").await;
    let (url, doc) = doc_server().await;
    *doc.lock().unwrap() = (200, json!({"hosts": [{"name": "api-1", "tags": ["api"], "extra": "ignored"}]}).to_string());
    let r = add_source(&s, &c, json!({"name": "Registry", "type": "url", "url": url, "header": "X-Api-Key", "secret": "inv/API_KEY", "interval_seconds": 60})).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
    let id = r.json()["id"].as_str().unwrap().to_string();
    let first_id = named(&hosts(&s, &c, None).await, "api-1")["id"].clone();
    let path = format!("/api/v1/inventory/sources/{id}/refresh");
    let refresh = |body: Value| call(&s.app, "POST", &path, Some(body), Some(&c), true);

    *doc.lock().unwrap() = (200, json!({"hosts": [{"name": "api-1"}, {"name": "api-2"}]}).to_string());
    let r = refresh(json!({})).await;
    assert_eq!(r.json()["state"]["host_count"], 2, "{}", r.body);
    let list = hosts(&s, &c, None).await;
    assert_eq!(named(&list, "api-1")["id"], first_id, "a host keeps its id across refreshes");

    // A failure keeps the hosts, says why, and marks them stale; recovery clears it.
    *doc.lock().unwrap() = (500, String::new());
    let r = refresh(json!({})).await;
    let err = r.json()["state"]["error"].as_str().unwrap_or("").to_string();
    assert!(err.contains("answered 500"), "{}", r.body);
    assert!(!err.contains("k3y-value-1234"));
    let list = hosts(&s, &c, None).await;
    assert_eq!((names(&list).len(), named(&list, "api-1")["stale"].as_bool()), (2, Some(true)));
    *doc.lock().unwrap() = (200, "not json".into());
    assert!(refresh(json!({})).await.json()["state"]["error"].as_str().unwrap().contains("isn't {\"hosts\""));
    // Listing nobody after listing hosts is kept as a failure until accepted.
    *doc.lock().unwrap() = (200, json!({"hosts": []}).to_string());
    let r = refresh(json!({})).await;
    assert!(r.json()["state"]["error"].as_str().unwrap().contains("listed no hosts after listing 2"), "{}", r.body);
    assert_eq!(hosts(&s, &c, None).await.len(), 2);
    let r = refresh(json!({"accept_empty": true})).await;
    assert_eq!((r.json()["state"]["host_count"].as_u64(), r.json()["state"]["error"].clone()), (Some(0), Value::Null));
    assert!(hosts(&s, &c, None).await.is_empty());

    let audit = call(&s.app, "GET", "/api/v1/audit?action=inventory_source.", None, Some(&c), false).await.json();
    let actions: Vec<&str> = audit["events"].as_array().unwrap().iter().map(|e| e["action"].as_str().unwrap()).collect();
    assert_eq!(actions.iter().filter(|a| **a == "inventory_source.refresh_failed").count(), 1, "one event per failure streak: {actions:?}");
    assert!(actions.contains(&"inventory_source.recovered") && actions.contains(&"inventory_source.hosts_change"), "{actions:?}");

    // Redirects aren't followed, so the header's secret stays with the server it's for.
    let moved = url.replace("/hosts.json", "/moved");
    let r = add_source(&s, &c, json!({"name": "Moved", "type": "url", "url": moved, "header": "X-Api-Key", "secret": "inv/API_KEY"})).await;
    assert!(r.json()["state"]["error"].as_str().unwrap_or("").contains("redirects aren't followed"), "{}", r.body);
}

#[tokio::test]
async fn program_sources_are_for_server_admins_and_get_settings_and_secrets() {
    let s = server();
    let admin = sign_in(&s).await;
    let wsadmin = user_with_role(&s, &admin, "wsadmin", "admin").await;
    add_secret(&s, &admin, "API_TOKEN", "t0ken-value-5678").await;
    let script = s.dir.join("list-hosts.sh");
    std::fs::write(&script, "#!/bin/sh\nif [ \"$OPSKILLS_CONFIG_FAIL\" = yes ]; then echo \"boom $API_TOKEN\" >&2; exit 3; fi\nprintf '{\"hosts\": [{\"name\": \"%s-1\", \"attributes\": {\"token-length\": \"%s\", \"workspace\": \"%s\"}}]}' \"$OPSKILLS_CONFIG_PREFIX\" \"${#API_TOKEN}\" \"$OPSKILLS_WORKSPACE\"\n").unwrap();
    std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let body = json!({"name": "Cloud", "type": "program", "command": script.to_str().unwrap(), "config": {"PREFIX": "vm"}, "secrets": ["API_TOKEN"]});

    let r = add_source(&s, &wsadmin, body.clone()).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN, "{}", r.body);
    let r = add_source(&s, &admin, body.clone()).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
    let id = r.json()["id"].as_str().unwrap().to_string();
    let vm = named(&hosts(&s, &admin, None).await, "vm-1").clone();
    assert_eq!(vm["attributes"], json!({"token-length": "16", "workspace": "default"}));
    // A workspace Admin sees it but can't change or remove it.
    assert_eq!(call(&s.app, "GET", "/api/v1/inventory/sources", None, Some(&wsadmin), false).await.json()[0]["type"], "program");
    assert_eq!(call(&s.app, "DELETE", &format!("/api/v1/inventory/sources/{id}"), None, Some(&wsadmin), true).await.status, StatusCode::FORBIDDEN);

    let mut failing = body.clone();
    failing["config"]["FAIL"] = json!("yes");
    let r = call(&s.app, "PUT", &format!("/api/v1/inventory/sources/{id}"), Some(failing), Some(&admin), true).await;
    let err = r.json()["state"]["error"].as_str().unwrap_or("").to_string();
    assert!(err.contains("exited with") && err.contains("boom ••••••"), "the secret is hidden: {}", r.body);
    assert_eq!(names(&hosts(&s, &admin, None).await), ["vm-1"], "the last good hosts stay");
}

#[tokio::test]
async fn sources_tag_rules_and_attributes_are_checked() {
    let s = server();
    let c = sign_in(&s).await;
    let (web, _) = enroll(&s, &c, "web-1", json!([])).await;
    for (body, expect) in [
        (json!({"name": "x", "type": "static", "hosts": [], "url": "http://a.example"}), "has no field \\\"url\\\""),
        (json!({"name": "x", "type": "ldap"}), "not a source type"),
        (json!({"name": "x", "type": "url", "url": "ftp://a.example/h"}), "http or https"),
        (json!({"name": "x", "type": "url", "url": "https://a.example/h", "header": "X-Key"}), "both a header name and the vault secret"),
        (json!({"name": "x", "type": "url", "url": "https://a.example/h", "header": "X-Key", "secret": "MISSING"}), "no secret named MISSING"),
        (json!({"name": "x", "type": "program", "command": "list-hosts.sh"}), "absolute path"),
        (json!({"name": "x", "type": "static", "hosts": [{"name": "a"}, {"name": "A"}]}), "listed twice"),
        (json!({"name": "x", "type": "static", "hosts": [{"name": "a", "attributes": {"tags": "x"}}]}), "built into every host"),
        (json!({"name": "x", "type": "static", "hosts": [], "interval_seconds": 5}), "30 seconds"),
    ] {
        let r = add_source(&s, &c, body.clone()).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{body}: {}", r.body);
        assert!(r.body.contains(expect), "{expect}: {}", r.body);
    }
    for (rules, expect) in [(json!([{"filter": "", "tags": ["x"]}]), "give a host filter"), (json!([{"filter": "-bad:x", "tags": ["x"]}]), "not a host filter key"), (json!([{"filter": "env:x", "tags": []}]), "1 to 20 tags")] {
        let r = call(&s.app, "PUT", "/api/v1/workspaces/default/tag-rules", Some(json!({"rules": rules})), Some(&c), true).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{expect}: {}", r.body);
        assert!(r.body.contains(expect), "{expect}: {}", r.body);
    }
    for attrs in [json!({"Env": "x y\nz"}), json!({"name": "x"}), json!({"-x": "y"})] {
        let r = call(&s.app, "PUT", &format!("/api/v1/hosts/{web}"), Some(json!({"attributes": attrs})), Some(&c), true).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{attrs}: {}", r.body);
    }
    // A workspace's sources go when the workspace is deleted.
    call(&s.app, "POST", "/api/v1/workspaces", Some(json!({"slug": "tmp", "name": "Tmp"})), Some(&c), true).await;
    let r = call(&s.app, "POST", "/api/v1/inventory/sources?workspace=tmp", Some(json!({"name": "x", "type": "static", "hosts": [{"name": "a"}]})), Some(&c), true).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
    assert_eq!(call(&s.app, "DELETE", "/api/v1/workspaces/tmp", None, Some(&c), true).await.status, StatusCode::NO_CONTENT);
    assert!(s.state.store.inventory_sources().unwrap().is_empty());
}

#[test]
fn a_database_from_0_16_is_backed_up_and_opens_with_inventory() {
    let dir = std::env::temp_dir().join(format!("opskills-it-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("opskills.redb");
    let t = |name: &'static str| redb::TableDefinition::<&str, &str>::new(name);
    {
        let db = redb::Database::create(&path).unwrap();
        let w = db.begin_write().unwrap();
        w.open_table(t("meta")).unwrap().insert("schema_version", "3").unwrap();
        let ws = json!({"slug": "ops", "name": "Ops", "description": "", "created_at": 1});
        w.open_table(t("workspaces")).unwrap().insert("ops", ws.to_string().as_str()).unwrap();
        let host = json!({"id": "h1", "name": "web-1", "workspaces": ["ops"], "hostname": "web-1.internal", "os": "Debian", "arch": "amd64",
            "agent_version": "0.16.0", "tags": ["web"], "enrolled_at": 1, "last_seen": 2, "secret_hash": "x"});
        w.open_table(t("hosts")).unwrap().insert("h1", host.to_string().as_str()).unwrap();
        w.commit().unwrap();
    }
    let store = crate::store::Store::open(&path, &dir).unwrap();
    assert_eq!(store.schema_version().unwrap(), crate::store::SCHEMA_VERSION);
    let backups: Vec<String> = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().to_string()).filter(|n| n.contains("-backup-")).collect();
    assert!(backups.len() == 1 && backups[0].starts_with("opskills.redb.schema-3-backup-"), "{backups:?}");
    assert!(store.inventory_sources().unwrap().is_empty());
    assert!(store.workspace("ops").unwrap().unwrap().tag_rules.is_empty());
    let h = &store.hosts().unwrap()[0];
    assert_eq!((h.name.as_str(), h.attributes.len()), ("web-1", 0));
    drop(store);
    std::fs::remove_dir_all(dir).unwrap();
}
