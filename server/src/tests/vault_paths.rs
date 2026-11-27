//! Vault folders, moves, folders picked by a choice input, and shared secrets.

use super::*;

async fn new_workspace(s: &TestServer, admin: &str, slug: &str) {
    let r = call(&s.app, "POST", "/api/v1/workspaces", Some(json!({"slug": slug, "name": slug})), Some(admin), true).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
}

/// A step that succeeds only when `$var` starts with `prefix`, without printing it.
fn check_value(var: &str, prefix: &str) -> Value {
    json!({"name": "check", "run": format!("case \"${var}\" in {prefix}*) echo value ok;; *) echo wrong value; exit 1;; esac")})
}

async fn run_ok(s: &TestServer, c: &str, rb: &str, body: Value) -> Value {
    let r = start_with(s, c, rb, body).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
    let done = wait_finished(s, c, r.json()["id"].as_str().unwrap()).await;
    assert_eq!(done["status"], "succeeded", "{done}");
    done
}

#[tokio::test]
async fn folders_hold_secrets_named_by_their_last_part() {
    let s = server();
    let c = sign_in(&s).await;
    for (name, value) in [("prod/db/DB_PASSWORD", "prod-value-1111"), ("staging/db/DB_PASSWORD", "staging-value-2222"), ("API_TOKEN", "flat-token-3333")] {
        let r = add_secret(&s, &c, name, value).await;
        assert_eq!(r.status, StatusCode::CREATED, "{name}: {}", r.body);
        assert_eq!(r.json()["name"], name);
        assert_eq!(r.json()["updated_by"], "admin");
    }
    let list = call(&s.app, "GET", "/api/v1/secrets", None, Some(&c), false).await.json();
    let names: Vec<&str> = list.as_array().unwrap().iter().map(|v| v["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["API_TOKEN", "prod/db/DB_PASSWORD", "staging/db/DB_PASSWORD"]);
    assert!(list.as_array().unwrap().iter().all(|v| v["shared"] == false));

    // A runbook names the path; the step sees the last part as the variable.
    let rb = create(&s, &c, json!({"name": "Prod check", "secrets": ["prod/db/DB_PASSWORD"], "steps": [check_value("DB_PASSWORD", "prod-value-")]})).await;
    let done = run_ok(&s, &c, &rb, json!({})).await;
    assert_eq!(done["secrets"], json!(["prod/db/DB_PASSWORD"]));

    // Two secrets that would be the same variable can't be used together.
    let both = call(&s.app, "POST", "/api/v1/runbooks", Some(json!({"name": "x", "secrets": ["prod/db/DB_PASSWORD", "staging/db/DB_PASSWORD"], "steps": [{"name": "a", "run": "true"}]})), Some(&c), true).await;
    assert_eq!(both.status, StatusCode::BAD_REQUEST);
    assert!(both.body.contains("would both be $DB_PASSWORD"), "{}", both.body);

    for bad in ["Prod/X_KEY", "shared/X_KEY", "a/b/c/d/e/f/X_KEY", "../X_KEY", "prod/x_key", "prod//X_KEY", "prod/"] {
        let r = add_secret(&s, &c, bad, "long-enough").await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{bad}: {}", r.body);
    }
    let r = add_secret(&s, &c, "shared/X_KEY", "long-enough").await;
    assert!(r.body.contains("can't be called shared"), "{}", r.body);

    // Paths work in the URL for changes and deletes.
    let r = call(&s.app, "PUT", "/api/v1/secrets/staging/db/DB_PASSWORD", Some(json!({"description": "staging db"})), Some(&c), true).await;
    assert_eq!((r.status, r.json()["description"].as_str()), (StatusCode::OK, Some("staging db")), "{}", r.body);
    assert_eq!(call(&s.app, "DELETE", "/api/v1/secrets/staging/db/DB_PASSWORD", None, Some(&c), true).await.status, StatusCode::NO_CONTENT);
    let used = call(&s.app, "DELETE", "/api/v1/secrets/prod/db/DB_PASSWORD", None, Some(&c), true).await;
    assert_eq!(used.status, StatusCode::CONFLICT);
    assert!(used.body.contains("Prod check"), "{}", used.body);
}

#[tokio::test]
async fn secrets_move_while_unused_and_keep_their_value() {
    let s = server();
    let c = sign_in(&s).await;
    add_secret(&s, &c, "DB_PASSWORD", "moving-value-4444").await;
    let rb = create(&s, &c, json!({"name": "Uses it", "secrets": ["DB_PASSWORD"], "steps": [{"name": "a", "run": "true"}]})).await;
    let r = call(&s.app, "PUT", "/api/v1/secrets/DB_PASSWORD", Some(json!({"name": "prod/DB_PASSWORD"})), Some(&c), true).await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    assert!(r.body.contains("Uses it"), "{}", r.body);
    assert_eq!(call(&s.app, "DELETE", &format!("/api/v1/runbooks/{rb}"), None, Some(&c), true).await.status, StatusCode::NO_CONTENT);

    let r = call(&s.app, "PUT", "/api/v1/secrets/DB_PASSWORD", Some(json!({"name": "prod/DB_PASSWORD"})), Some(&c), true).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(r.json()["name"], "prod/DB_PASSWORD");
    let list = call(&s.app, "GET", "/api/v1/secrets", None, Some(&c), false).await.json();
    assert_eq!(list.as_array().unwrap().len(), 1);
    // The value was sealed again under its new name and still opens.
    let rb = create(&s, &c, json!({"name": "Moved", "secrets": ["prod/DB_PASSWORD"], "steps": [check_value("DB_PASSWORD", "moving-value-")]})).await;
    run_ok(&s, &c, &rb, json!({})).await;
    // A move onto an existing name, or into the shared folder, is refused.
    add_secret(&s, &c, "OTHER", "other-value-5555").await;
    assert_eq!(call(&s.app, "PUT", "/api/v1/secrets/OTHER", Some(json!({"name": "prod/DB_PASSWORD"})), Some(&c), true).await.status, StatusCode::CONFLICT);
    assert_eq!(call(&s.app, "PUT", "/api/v1/secrets/OTHER", Some(json!({"name": "shared/OTHER"})), Some(&c), true).await.status, StatusCode::BAD_REQUEST);
    let audit = call(&s.app, "GET", "/api/v1/audit?action=secret.move", None, Some(&c), false).await.json();
    assert_eq!(audit["events"][0]["detail"], "from DB_PASSWORD");
}

#[tokio::test]
async fn a_choice_input_can_pick_the_folder() {
    let s = server();
    let c = sign_in(&s).await;
    add_secret(&s, &c, "prod/DB_PASSWORD", "prod-value-1111").await;
    add_secret(&s, &c, "staging/DB_PASSWORD", "staging-value-2222").await;
    let env = json!([{"name": "env", "type": "choice", "choices": ["prod", "staging"], "required": true}]);
    let rb = create(&s, &c, json!({"name": "Per env", "inputs": env, "secrets": ["{{ inputs.env }}/DB_PASSWORD"], "steps": [
        {"name": "check", "run": "case \"$DB_PASSWORD\" in \"$OPSKILLS_INPUT_ENV\"-value-*) echo picked ok;; *) exit 1;; esac"}
    ]})).await;
    for e in ["staging", "prod"] {
        let done = run_ok(&s, &c, &rb, json!({"inputs": {"env": e}})).await;
        assert_eq!(done["secrets"], json!([format!("{e}/DB_PASSWORD")]));
    }
    // Both secrets show the runbook as a user, and can't be deleted.
    let list = call(&s.app, "GET", "/api/v1/secrets", None, Some(&c), false).await.json();
    assert!(list.as_array().unwrap().iter().all(|v| v["used_by"][0]["name"] == "Per env"), "{list}");
    assert_eq!(call(&s.app, "DELETE", "/api/v1/secrets/staging/DB_PASSWORD", None, Some(&c), true).await.status, StatusCode::CONFLICT);

    let make = |inputs: Value, secret: &str| json!({"name": "x", "inputs": inputs, "secrets": [secret], "steps": [{"name": "a", "run": "true"}]});
    for (body, expect) in [
        (make(json!([{"name": "env", "type": "text"}]), "{{ inputs.env }}/DB_PASSWORD"), "must be a choice input"),
        (make(json!([{"name": "env", "type": "choice", "choices": ["prod", "qa"]}]), "{{ inputs.env }}/DB_PASSWORD"), "qa/DB_PASSWORD"),
        (make(env.clone(), "prod/{{ inputs.env }}"), "only a folder can"),
        (make(env.clone(), "{{ inputs.region }}/DB_PASSWORD"), "no input named region"),
        (make(env.clone(), "x{{ inputs.env }}/DB_PASSWORD"), "only be a whole folder"),
        (make(json!([{"name": "env", "type": "choice", "choices": ["Prod"]}]), "{{ inputs.env }}/DB_PASSWORD"), "as Prod/DB_PASSWORD: Folder"),
    ] {
        let r = call(&s.app, "POST", "/api/v1/runbooks", Some(body), Some(&c), true).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{expect}: {}", r.body);
        assert!(r.body.contains(expect), "{expect}: {}", r.body);
    }
}

#[tokio::test]
async fn shared_secrets_reach_only_the_workspaces_they_are_shared_with() {
    let s = server();
    let admin = sign_in(&s).await;
    new_workspace(&s, &admin, "payments").await;
    new_workspace(&s, &admin, "tmp").await;
    let olly = user_with(&s, &admin, json!({"name": "olly", "workspaces": {"payments": "admin"}})).await;

    // Only server admins manage them, not a workspace's Admins.
    let body = json!({"name": "smtp/SMTP_PASSWORD", "kind": "password", "value": "shared-value-6666", "workspaces": ["payments", "tmp"]});
    let r = call(&s.app, "POST", "/api/v1/shared-secrets", Some(body.clone()), Some(&olly), true).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN, "{}", r.body);
    let r = call(&s.app, "POST", "/api/v1/shared-secrets", Some(body), Some(&admin), true).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
    assert!(!r.body.contains("shared-value-6666"));
    assert_eq!(r.json()["workspaces"], json!(["payments", "tmp"]));
    let bad = call(&s.app, "POST", "/api/v1/shared-secrets", Some(json!({"name": "X_KEY", "value": "long-enough", "workspaces": ["nowhere"]})), Some(&admin), true).await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST, "{}", bad.body);

    // Payments sees it, read-only; Default doesn't.
    let list = call(&s.app, "GET", "/api/v1/secrets?workspace=payments", None, Some(&olly), false).await.json();
    assert_eq!(list[0]["name"], "shared/smtp/SMTP_PASSWORD");
    assert_eq!(list[0]["shared"], true);
    let r = call(&s.app, "DELETE", "/api/v1/secrets/shared/smtp/SMTP_PASSWORD?workspace=payments", None, Some(&olly), true).await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    assert!(r.body.contains("server admins"), "{}", r.body);
    let default_list = call(&s.app, "GET", "/api/v1/secrets", None, Some(&admin), false).await.json();
    assert_eq!(default_list, json!([]));

    let steps = json!([check_value("SMTP_PASSWORD", "shared-value-")]);
    let r = call(&s.app, "POST", "/api/v1/runbooks", Some(json!({"name": "In default", "secrets": ["shared/smtp/SMTP_PASSWORD"], "steps": steps})), Some(&admin), true).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert!(r.body.contains("isn't shared with this workspace"), "{}", r.body);
    let r = call(&s.app, "POST", "/api/v1/runbooks?workspace=payments", Some(json!({"name": "Mailer", "secrets": ["shared/smtp/SMTP_PASSWORD"], "steps": steps})), Some(&olly), true).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
    let mailer = r.json()["id"].as_str().unwrap().to_string();
    run_ok(&s, &olly, &mailer, json!({})).await;

    // Deleting a workspace takes it off the list.
    assert_eq!(call(&s.app, "DELETE", "/api/v1/workspaces/tmp", None, Some(&admin), true).await.status, StatusCode::NO_CONTENT);
    let shared = call(&s.app, "GET", "/api/v1/shared-secrets", None, Some(&admin), false).await.json();
    assert_eq!(shared[0]["workspaces"], json!(["payments"]));
    assert_eq!(shared[0]["used_by"][0]["workspace"], "payments");

    // A workspace whose runbooks use it can't lose it, and it can't be deleted or moved.
    let path = "/api/v1/shared-secrets/smtp/SMTP_PASSWORD";
    let r = call(&s.app, "PUT", path, Some(json!({"workspaces": []})), Some(&admin), true).await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    assert!(r.body.contains("Mailer (in payments)"), "{}", r.body);
    assert_eq!(call(&s.app, "DELETE", path, None, Some(&admin), true).await.status, StatusCode::CONFLICT);
    assert_eq!(call(&s.app, "PUT", path, Some(json!({"name": "mail/SMTP_PASSWORD"})), Some(&admin), true).await.status, StatusCode::CONFLICT);

    // Sharing with all workspaces reaches Default too; a new value reaches every run.
    let r = call(&s.app, "PUT", path, Some(json!({"all_workspaces": true, "value": "shared-value-7777"})), Some(&admin), true).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!((r.json()["all_workspaces"].clone(), r.json()["workspaces"].clone()), (json!(true), json!([])));
    let rb = create(&s, &admin, json!({"name": "In default", "secrets": ["shared/smtp/SMTP_PASSWORD"], "steps": [check_value("SMTP_PASSWORD", "shared-value-7777")]})).await;
    run_ok(&s, &admin, &rb, json!({})).await;
    // Back to payments only is refused while Default's runbook uses it.
    let r = call(&s.app, "PUT", path, Some(json!({"all_workspaces": false, "workspaces": ["payments"]})), Some(&admin), true).await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    assert!(r.body.contains("In default (in default)"), "{}", r.body);

    let audit = call(&s.app, "GET", "/api/v1/audit?action=shared_secret.", None, Some(&admin), false).await.json();
    let actions: Vec<&str> = audit["events"].as_array().unwrap().iter().map(|e| e["action"].as_str().unwrap()).collect();
    assert_eq!(actions, ["shared_secret.update", "shared_secret.share", "shared_secret.create"]);
    assert_eq!(audit["events"][1]["detail"], "with all workspaces");
    assert!(audit["events"][2]["workspace"].is_null());
}

/// A database written by 0.14 (schema 2) gets the shared secrets table after a backup,
/// and its secrets, with and without workspaces, still open.
#[test]
fn a_database_from_0_14_is_backed_up_and_its_secrets_still_open() {
    let dir = std::env::temp_dir().join(format!("opskills-it-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("opskills.redb");
    let t = |name: &'static str| redb::TableDefinition::<&str, &str>::new(name);
    let vault = crate::vault::Vault::from_key_hex(&hex::encode([9u8; 32])).unwrap();
    let in_default = vault.seal("API_KEY", b"default-value").unwrap();
    let in_ops = vault.seal("ops/DB_PASSWORD", b"ops-value").unwrap();
    {
        let db = redb::Database::create(&path).unwrap();
        let w = db.begin_write().unwrap();
        w.open_table(t("meta")).unwrap().insert("schema_version", "2").unwrap();
        let mut secrets = w.open_table(t("secrets")).unwrap();
        for (key, ws, name, sealed) in [("default/API_KEY", "default", "API_KEY", &in_default), ("ops/DB_PASSWORD", "ops", "DB_PASSWORD", &in_ops)] {
            let v = json!({"name": name, "workspace": ws, "kind": "password", "description": "", "created_at": 0, "updated_at": 0, "nonce": sealed.nonce, "ciphertext": sealed.ciphertext});
            secrets.insert(key, v.to_string().as_str()).unwrap();
        }
        drop(secrets);
        w.commit().unwrap();
    }
    let store = crate::store::Store::open(&path, &dir).unwrap();
    assert_eq!(store.schema_version().unwrap(), 3);
    assert!(store.shared_secrets().unwrap().is_empty());
    let backups: Vec<String> = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().to_string()).filter(|n| n.contains("-backup-")).collect();
    assert!(backups.len() == 1 && backups[0].starts_with("opskills.redb.schema-2-backup-"), "{backups:?}");
    for (ws, name, want) in [("default", "API_KEY", &b"default-value"[..]), ("ops", "DB_PASSWORD", &b"ops-value"[..])] {
        let s = store.secret(ws, name).unwrap().unwrap();
        let opened = vault.open(&crate::model::secret_binding(ws, name), &crate::vault::Sealed { nonce: s.nonce, ciphertext: s.ciphertext }).unwrap();
        assert_eq!(opened, want);
        assert!(s.meta.updated_by.is_none());
    }
    drop(store);
    std::fs::remove_dir_all(dir).unwrap();
}
