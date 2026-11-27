//! Vault secrets: write-only values, encryption at rest, and masking in run output.

use super::*;

const SECRET_VALUE: &str = "s3cr3t-Value-8472";

#[tokio::test]
async fn vault_values_are_write_only_and_encrypted_at_rest() {
    let s = server();
    let c = sign_in(&s).await;
    let created = add_secret(&s, &c, "DB_PASSWORD", SECRET_VALUE).await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    assert!(!created.body.contains(SECRET_VALUE));
    let list = call(&s.app, "GET", "/api/v1/secrets", None, Some(&c), false).await;
    assert!(list.body.contains("DB_PASSWORD") && !list.body.contains(SECRET_VALUE), "{}", list.body);
    assert!(list.json()[0].get("value").is_none() && list.json()[0].get("ciphertext").is_none());
    // Duplicate names are refused.
    assert_eq!(add_secret(&s, &c, "DB_PASSWORD", "another-value").await.status, StatusCode::CONFLICT);
    // The database file never contains the plaintext.
    let bytes = std::fs::read(s.dir.join("opskills.redb")).unwrap();
    assert!(!bytes.windows(SECRET_VALUE.len()).any(|w| w == SECRET_VALUE.as_bytes()), "plaintext found on disk");
}

#[tokio::test]
async fn secret_validation() {
    let s = server();
    let c = sign_in(&s).await;
    for (name, value) in [("db_password", "long-enough"), ("PATH", "long-enough"), ("OPSKILLS_KEY", "long-enough"), ("GOOD_NAME", "abc")] {
        assert_eq!(add_secret(&s, &c, name, value).await.status, StatusCode::BAD_REQUEST, "{name}");
    }
    let bad_kind = call(&s.app, "POST", "/api/v1/secrets", Some(json!({"name": "X_TOKEN", "kind": "magic", "value": "long-enough"})), Some(&c), true).await;
    assert_eq!(bad_kind.status, StatusCode::BAD_REQUEST);
    // A runbook can't reference a secret that isn't in the vault.
    let missing = call(&s.app, "POST", "/api/v1/runbooks", Some(json!({"name": "x", "secrets": ["NOPE"], "steps": [{"name": "a", "run": "true"}]})), Some(&c), true).await;
    assert_eq!(missing.status, StatusCode::BAD_REQUEST);
    assert!(missing.json()["detail"].as_str().unwrap().contains("NOPE"));
}

#[tokio::test]
async fn runs_get_listed_secrets_as_env_and_output_is_masked() {
    let s = server();
    let c = sign_in(&s).await;
    add_secret(&s, &c, "DB_PASSWORD", SECRET_VALUE).await;
    add_secret(&s, &c, "OTHER_TOKEN", "unrelated-token-123").await;
    let id = create(&s, &c, json!({"name": "Uses secret", "secrets": ["DB_PASSWORD"], "steps": [
        // Checks the value without writing it out, so the command itself holds no secret.
        {"name": "check", "run": "test \"${#DB_PASSWORD}\" -eq 17 && case \"$DB_PASSWORD\" in s3cr3t-*8472) echo value matches;; *) exit 1;; esac"},
        {"name": "leak", "run": "echo \"the password is $DB_PASSWORD\"; echo \"$DB_PASSWORD\" >&2"},
        {"name": "isolation", "run": "test -z \"$OTHER_TOKEN\" && echo other secret not exposed"}
    ]})).await;
    let done = wait_finished(&s, &c, &start(&s, &c, &id).await).await;
    assert_eq!(done["status"], "succeeded", "{done}");
    assert_eq!(done["secrets"], json!(["DB_PASSWORD"]));
    let run = done["id"].as_str().unwrap();
    let logs = call(&s.app, "GET", &format!("/api/v1/runs/{run}/logs"), None, Some(&c), false).await.body;
    assert!(logs.contains("value matches"), "{logs}");
    assert!(logs.contains("the password is ••••••"), "{logs}");
    assert!(logs.contains("other secret not exposed"));
    assert!(!logs.contains(SECRET_VALUE), "secret leaked into stored output");
    let events = call(&s.app, "GET", &format!("/api/v1/runs/{run}/events"), None, Some(&c), false).await.body;
    assert!(!events.contains(SECRET_VALUE), "secret leaked into live events");
    // Neither the database nor the run record holds the plaintext.
    let bytes = std::fs::read(s.dir.join("opskills.redb")).unwrap();
    assert!(!bytes.windows(SECRET_VALUE.len()).any(|w| w == SECRET_VALUE.as_bytes()));
    // The secrets list shows where it's used.
    let list = call(&s.app, "GET", "/api/v1/secrets", None, Some(&c), false).await.json();
    let db = list.as_array().unwrap().iter().find(|x| x["name"] == "DB_PASSWORD").unwrap().clone();
    assert_eq!(db["used_by"][0]["name"], "Uses secret");
}

#[tokio::test]
async fn replacing_and_deleting_secrets() {
    let s = server();
    let c = sign_in(&s).await;
    add_secret(&s, &c, "API_TOKEN", "first-value-111").await;
    let id = create(&s, &c, json!({"name": "Token check", "secrets": ["API_TOKEN"], "steps": [{"name": "check", "run": "test \"$API_TOKEN\" = second-value-222"}]})).await;
    let upd = call(&s.app, "PUT", "/api/v1/secrets/API_TOKEN", Some(json!({"value": "second-value-222", "description": "rotated"})), Some(&c), true).await;
    assert_eq!(upd.status, StatusCode::OK, "{}", upd.body);
    assert_eq!(upd.json()["description"], "rotated");
    assert!(!upd.body.contains("second-value-222"));
    let done = wait_finished(&s, &c, &start(&s, &c, &id).await).await;
    assert_eq!(done["status"], "succeeded", "the new value was not used");
    // In use: deleting is refused and names the runbook.
    let refused = call(&s.app, "DELETE", "/api/v1/secrets/API_TOKEN", None, Some(&c), true).await;
    assert_eq!(refused.status, StatusCode::CONFLICT);
    assert!(refused.json()["detail"].as_str().unwrap().contains("Token check"));
    // After the runbook stops using it, it can be deleted.
    let r = call(&s.app, "PUT", &format!("/api/v1/runbooks/{id}"), Some(json!({"name": "Token check", "steps": [{"name": "check", "run": "true"}]})), Some(&c), true).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(call(&s.app, "DELETE", "/api/v1/secrets/API_TOKEN", None, Some(&c), true).await.status, StatusCode::NO_CONTENT);
    assert_eq!(call(&s.app, "PUT", "/api/v1/secrets/API_TOKEN", Some(json!({"value": "x-value"})), Some(&c), true).await.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn multi_line_secrets_are_masked_line_by_line() {
    let s = server();
    let c = sign_in(&s).await;
    let key = "-----BEGIN KEY-----\nAAAAB3NzaC1yc2EAAAADAQABAAABgQ\nZZZZtopsecretlineZZZZ\n-----END KEY-----";
    let r = call(&s.app, "POST", "/api/v1/secrets", Some(json!({"name": "DEPLOY_KEY", "kind": "ssh_key", "value": key})), Some(&c), true).await;
    assert_eq!(r.status, StatusCode::CREATED);
    let id = create(&s, &c, json!({"name": "Key", "secrets": ["DEPLOY_KEY"], "steps": [{"name": "print", "run": "printf '%s\\n' \"$DEPLOY_KEY\""}]})).await;
    let done = wait_finished(&s, &c, &start(&s, &c, &id).await).await;
    let logs = call(&s.app, "GET", &format!("/api/v1/runs/{}/logs", done["id"].as_str().unwrap()), None, Some(&c), false).await.body;
    assert!(!logs.contains("ZZZZtopsecretlineZZZZ") && !logs.contains("AAAAB3NzaC1yc2EAAAADAQABAAABgQ"), "{logs}");
}
