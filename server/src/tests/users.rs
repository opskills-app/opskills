//! Users and roles: first sign-in, the access rule of every route, tokens, user management,
//! the database migrations and the password reset through `admin.hash`.

use std::collections::HashMap;

use super::*;
use crate::api::{Access, Scope, access_rules};
use crate::model::Role;

#[tokio::test]
async fn the_first_sign_in_asks_for_a_new_password() {
    let s = fresh_server();
    let system = call(&s.app, "GET", "/api/v1/system", None, None, false).await.json();
    assert_eq!(system["setup_done"], false, "the sign-in page shows where the password is");
    let r = sign_in_as(&s, "admin", &s.password).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json()["must_change_password"], true);
    assert_eq!(r.json()["admin"], true);
    let c = r.cookie.unwrap();

    // Nothing else works until the password is replaced.
    let blocked = call(&s.app, "GET", "/api/v1/runbooks", None, Some(&c), false).await;
    assert_eq!(blocked.status, StatusCode::FORBIDDEN);
    assert!(blocked.body.contains("Choose a new password first"), "{}", blocked.body);
    let me = call(&s.app, "GET", "/api/v1/auth/me", None, Some(&c), false).await;
    assert_eq!(me.json()["must_change_password"], true);

    let change = |current: &str, new: &str| json!({"current_password": current, "new_password": new});
    let r = call(&s.app, "POST", "/api/v1/auth/password", Some(change("wrong", CHOSEN)), Some(&c), true).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let r = call(&s.app, "POST", "/api/v1/auth/password", Some(change(&s.password, "short")), Some(&c), true).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert!(r.body.contains("at least 12"), "{}", r.body);
    let r = call(&s.app, "POST", "/api/v1/auth/password", Some(change(&s.password, &s.password)), Some(&c), true).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = call(&s.app, "POST", "/api/v1/auth/password", Some(change(&s.password, CHOSEN)), Some(&c), true).await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{}", r.body);

    // The same session goes on, now with access; the sign-in page stops explaining.
    assert_eq!(call(&s.app, "GET", "/api/v1/runbooks", None, Some(&c), false).await.status, StatusCode::OK);
    let me = call(&s.app, "GET", "/api/v1/auth/me", None, Some(&c), false).await.json();
    assert_eq!(me["workspaces"], json!([{"slug": "default", "name": "Default", "role": "admin"}]));
    let system = call(&s.app, "GET", "/api/v1/system", None, None, false).await.json();
    assert_eq!(system["setup_done"], true);
    assert_eq!(sign_in_as(&s, "admin", &s.password).await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(sign_in_as(&s, "admin", CHOSEN).await.status, StatusCode::OK);
    assert_eq!(sign_in_as(&s, "nobody", CHOSEN).await.status, StatusCode::UNAUTHORIZED);
}

/// The value to put in for each `{param}` of a route, from fixtures that exist.
fn fill_route(route: &str, ids: &HashMap<&str, String>) -> String {
    let parts: Vec<&str> = route.split('/').collect();
    let mut out = Vec::new();
    for (i, part) in parts.iter().enumerate() {
        let prev = if i > 0 { parts[i - 1] } else { "" };
        out.push(match (*part, prev) {
            ("{id}", "runbooks") => ids["runbook"].clone(),
            ("{id}", "runs") => ids["run"].clone(),
            ("{id}", "webhooks") => ids["webhook"].clone(),
            ("{id}", "hosts") => ids["host"].clone(),
            ("{id}", _) | ("{token}", _) => "x".into(),
            ("{slug}", _) => "default".into(),
            ("{*name}", "secrets") => "FIXTURE".into(),
            ("{*name}", "shared-secrets") => "SHARED_FIXTURE".into(),
            ("{name}", "members") => "member-x".into(),
            ("{name}", _) => "nobody".into(),
            (other, _) => other.to_string(),
        });
    }
    format!("/api/v1{}", out.join("/"))
}

/// Every route refuses callers below its rule and admits those who meet it.
#[tokio::test]
async fn every_route_enforces_its_rule() {
    let s = server();
    let admin = sign_in(&s).await;
    let mut cookies: HashMap<&str, String> = HashMap::new();
    for role in ["viewer", "operator", "editor"] {
        cookies.insert(role, user_with_role(&s, &admin, role, role).await);
    }
    // Admin of the default workspace, but not a server admin.
    cookies.insert("admin", user_with_role(&s, &admin, "wsadmin", "admin").await);
    user_with_role(&s, &admin, "member-x", "viewer").await;
    let mut ids: HashMap<&str, String> = HashMap::new();
    ids.insert("runbook", create(&s, &admin, json!({"name": "Fixture", "steps": [{"name": "a", "run": "true"}]})).await);
    let run = start(&s, &admin, &ids["runbook"]).await;
    wait_finished(&s, &admin, &run).await;
    ids.insert("run", run);
    let hook = call(&s.app, "POST", &format!("/api/v1/runbooks/{}/webhooks", ids["runbook"]), Some(json!({"name": "fixture"})), Some(&admin), true).await;
    ids.insert("webhook", hook.json()["webhook"]["id"].as_str().unwrap().to_string());
    assert_eq!(add_secret(&s, &admin, "FIXTURE", "fixture-value").await.status, StatusCode::CREATED);
    ids.insert("host", enroll(&s, &admin, "fixture-host", json!([])).await.0);

    let below = |r: Role| match r {
        Role::Viewer => None,
        Role::Operator => Some("viewer"),
        Role::Editor => Some("operator"),
        Role::Admin => Some("editor"),
    };
    let rules: Vec<_> = access_rules().collect();
    // Refusals first: they change nothing, so every fixture is still there.
    let mut checked = 0;
    for (method, route, access) in &rules {
        let path = fill_route(route, &ids);
        let body = (*method != "GET").then(|| json!({}));
        let (who, expect) = match access {
            Access::Role(min, _) | Access::SessionRole(min, _) => match below(*min) {
                Some(lower) => (lower, "role doesn't allow"),
                None => continue,
            },
            Access::ServerAdmin | Access::SessionServerAdmin => ("admin", "Only server admins"),
            _ => continue,
        };
        let r = call(&s.app, method, &path, body, Some(&cookies[who]), true).await;
        assert_eq!(r.status, StatusCode::FORBIDDEN, "{who} on {method} {route}: {}", r.body);
        assert!(r.body.contains(expect), "{who} on {method} {route}: {}", r.body);
        checked += 1;
    }
    assert!(checked > 30, "checked only {checked} refusals");
    // Then the callers who meet each rule (these may change or delete fixtures).
    for (method, route, access) in &rules {
        let path = fill_route(route, &ids);
        let body = (*method != "GET").then(|| json!({}));
        let who = match access {
            Access::Role(min, _) | Access::SessionRole(min, _) => min.as_str(),
            Access::User | Access::SessionUser => "viewer",
            _ => continue,
        };
        let r = call(&s.app, method, &path, body, Some(&cookies[who]), true).await;
        assert!(!r.body.contains("role doesn't allow") && !r.body.contains("Only server admins"), "{who} on {method} {route}: {}", r.body);
    }
}

/// The role table agreed for 4a, per workspace since 4c.
#[test]
fn the_role_table_is_the_agreed_one() {
    let rule = |m: &str, r: &str| access_rules().find(|(mm, rr, _)| *mm == m && *rr == r).map(|(_, _, a)| a).unwrap();
    use Access::{Role as R, SessionRole as SR};
    use Scope::*;
    for (m, r, want) in [
        ("GET", "/runbooks", R(Role::Viewer, Query)),
        ("GET", "/runs/{id}/logs", R(Role::Viewer, Run)),
        ("GET", "/hosts", R(Role::Viewer, Query)),
        ("POST", "/runbooks/{id}/runs", R(Role::Operator, Runbook)),
        ("POST", "/runs/{id}/cancel", R(Role::Operator, Run)),
        ("POST", "/runs/{id}/retry", R(Role::Operator, Run)),
        ("POST", "/runbooks", R(Role::Editor, Query)),
        ("PUT", "/runbooks/{id}", R(Role::Editor, Runbook)),
        ("POST", "/runbooks/{id}/webhooks", R(Role::Editor, Runbook)),
        ("GET", "/secrets", R(Role::Editor, Query)),
        ("PUT", "/secrets/{*name}", R(Role::Editor, Query)),
        ("PUT", "/shared-secrets/{*name}", Access::ServerAdmin),
        ("PUT", "/settings/sign-in", Access::ServerAdmin),
        ("POST", "/settings/sign-in/test", Access::ServerAdmin),
        ("GET", "/auth/oidc/start", Access::Public),
        ("GET", "/auth/oidc/callback", Access::Public),
        ("POST", "/hosts/enrollment-tokens", R(Role::Admin, Query)),
        ("PUT", "/hosts/{id}", R(Role::Admin, Host)),
        ("PUT", "/hosts/{id}/workspaces/{slug}", R(Role::Admin, HostAndSlug)),
        ("PUT", "/workspaces/{slug}/members/{name}", SR(Role::Admin, Slug)),
        ("GET", "/workspaces/{slug}/audit", R(Role::Admin, Slug)),
        ("POST", "/workspaces", Access::ServerAdmin),
        ("DELETE", "/workspaces/{slug}", Access::ServerAdmin),
        ("PUT", "/settings/email", Access::ServerAdmin),
        ("GET", "/audit", Access::ServerAdmin),
        ("GET", "/users", Access::SessionServerAdmin),
        ("GET", "/tokens", Access::SessionServerAdmin),
        ("POST", "/me/tokens", Access::SessionUser),
        ("POST", "/auth/password", Access::Account),
        ("GET", "/system", Access::Public),
    ] {
        assert_eq!(rule(m, r), want, "{m} {r}");
    }
}

#[tokio::test]
async fn tokens_act_with_their_owners_current_role() {
    let s = server();
    let admin = sign_in(&s).await;
    let editor = user_with_role(&s, &admin, "erin", "editor").await;
    let r = call(&s.app, "POST", "/api/v1/me/tokens", Some(json!({"name": "ci"})), Some(&editor), true).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
    let token = r.json()["value"].as_str().unwrap().to_string();
    let rb = json!({"name": "Made by a token", "steps": [{"name": "a", "run": "true"}]});
    assert_eq!(bearer_call(&s.app, "POST", "/api/v1/runbooks", Some(rb.clone()), &token).await.status, StatusCode::CREATED);

    // Tokens are personal: others don't see them, server admins see everyone's.
    let mine = call(&s.app, "GET", "/api/v1/me/tokens", None, Some(&admin), false).await.json();
    assert_eq!(mine.as_array().unwrap().len(), 0);
    let all = call(&s.app, "GET", "/api/v1/tokens", None, Some(&admin), false).await.json();
    assert_eq!(all[0]["user"], "erin");
    let id = all[0]["id"].as_str().unwrap().to_string();
    assert_eq!(call(&s.app, "DELETE", &format!("/api/v1/me/tokens/{id}"), None, Some(&admin), true).await.status, StatusCode::NOT_FOUND);
    // Tokens can't manage users, tokens or members.
    assert_eq!(bearer_call(&s.app, "GET", "/api/v1/me/tokens", None, &token).await.status, StatusCode::FORBIDDEN);
    let r = bearer_call(&s.app, "PUT", "/api/v1/workspaces/default/members/admin", Some(json!({"role": "viewer"})), &token).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);

    // A lower role applies to the token at once; removing the user ends it.
    let r = call(&s.app, "PUT", "/api/v1/workspaces/default/members/erin", Some(json!({"role": "viewer"})), Some(&admin), true).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    let r = bearer_call(&s.app, "POST", "/api/v1/runbooks", Some(rb), &token).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN, "{}", r.body);
    assert_eq!(bearer_call(&s.app, "GET", "/api/v1/runbooks", None, &token).await.status, StatusCode::OK);
    assert_eq!(call(&s.app, "DELETE", "/api/v1/users/erin", None, Some(&admin), true).await.status, StatusCode::NO_CONTENT);
    assert_eq!(bearer_call(&s.app, "GET", "/api/v1/runbooks", None, &token).await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(call(&s.app, "GET", "/api/v1/runbooks", None, Some(&editor), false).await.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn server_admins_manage_accounts_but_not_themselves() {
    let s = server();
    let admin = sign_in(&s).await;
    for bad in [json!({"name": "Bad Name"}), json!({"name": "ops", "workspaces": {"default": "root"}}), json!({"name": "ops", "workspaces": {"nowhere": "viewer"}}), json!({"name": "ops", "role": "admin"})] {
        let r = call(&s.app, "POST", "/api/v1/users", Some(bad.clone()), Some(&admin), true).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{bad}: {}", r.body);
    }
    let viewer = user_with_role(&s, &admin, "vic", "viewer").await;
    let r = call(&s.app, "POST", "/api/v1/users", Some(json!({"name": "vic"})), Some(&admin), true).await;
    assert_eq!(r.status, StatusCode::CONFLICT);

    for (method, path, body) in [
        ("PUT", "/api/v1/users/admin", Some(json!({"admin": false}))),
        ("DELETE", "/api/v1/users/admin", None),
        ("POST", "/api/v1/users/admin/reset-password", None),
    ] {
        let r = call(&s.app, method, path, body, Some(&admin), true).await;
        assert_eq!(r.status, StatusCode::CONFLICT, "{method} {path}: {}", r.body);
    }

    // A second server admin can take the first one's role away, but not the last one's.
    let second = user_with(&s, &admin, json!({"name": "root2", "admin": true})).await;
    let r = call(&s.app, "PUT", "/api/v1/users/admin", Some(json!({"admin": false})), Some(&second), true).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(r.json()["admin"], false);
    assert_eq!(call(&s.app, "GET", "/api/v1/users", None, Some(&admin), false).await.status, StatusCode::FORBIDDEN);
    let r = call(&s.app, "PUT", "/api/v1/users/admin", Some(json!({"admin": true})), Some(&second), true).await;
    assert_eq!(r.status, StatusCode::OK);

    // A reset password signs the user out and must be replaced at the next sign-in.
    let r = call(&s.app, "POST", "/api/v1/users/vic/reset-password", None, Some(&admin), true).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    let generated = r.json()["password"].as_str().unwrap().to_string();
    assert_eq!(call(&s.app, "GET", "/api/v1/runbooks", None, Some(&viewer), false).await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(sign_in_as(&s, "vic", CHOSEN).await.status, StatusCode::UNAUTHORIZED);
    let r = sign_in_as(&s, "vic", &generated).await;
    assert_eq!(r.json()["must_change_password"], true);

    let users = call(&s.app, "GET", "/api/v1/users", None, Some(&admin), false).await.json();
    let names: Vec<_> = users.as_array().unwrap().iter().map(|u| u["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["admin", "root2", "vic"]);
    assert_eq!(users[2]["workspaces"], json!({"default": "viewer"}));
    assert!(!users.to_string().contains("password_hash"));
}

/// A database written by 0.11 (no schema version, the admin in admin.hash) or by 0.12
/// and 0.13 (schema 1: users with one role, secrets keyed by name).
#[test]
fn old_databases_are_backed_up_and_migrated() {
    let dir = std::env::temp_dir().join(format!("opskills-it-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("opskills.redb");
    let t = |name: &'static str| redb::TableDefinition::<&str, &str>::new(name);
    {
        let db = redb::Database::create(&path).unwrap();
        let w = db.begin_write().unwrap();
        w.open_table(t("runbooks")).unwrap().insert("rb", r#"{"id":"rb","name":"Old","description":"","tags":[],"steps":[],"timeout_seconds":600,"created_at":0,"updated_at":0}"#).unwrap();
        w.commit().unwrap();
    }
    std::fs::write(dir.join("admin.hash"), crate::auth::hash_password("old admin password").unwrap()).unwrap();

    let store = crate::store::Store::open(&path, &dir).unwrap();
    assert_eq!(store.schema_version().unwrap(), crate::store::SCHEMA_VERSION);
    let admin = store.user("admin").unwrap().unwrap();
    assert!(admin.user.admin, "the single admin becomes a server admin");
    assert_eq!(admin.user.role_in("default"), Some(Role::Admin));
    assert!(!admin.user.must_change_password, "upgrades keep the admin's password working");
    assert!(crate::auth::verify_password(&admin.password_hash, "old admin password"));
    assert_eq!(store.runbook("rb").unwrap().unwrap().workspace, "default");
    assert_eq!(store.workspaces().unwrap()[0].name, "Default");
    let backups = |dir: &std::path::Path| std::fs::read_dir(dir).unwrap().filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().to_string()).filter(|n| n.contains("-backup-")).count();
    assert_eq!(backups(&dir), 1);
    drop(store);

    // Opening again changes nothing and makes no new backup.
    drop(crate::store::Store::open(&path, &dir).unwrap());
    assert_eq!(backups(&dir), 1);

    // A database from a newer version is refused.
    {
        let db = redb::Database::create(&path).unwrap();
        let w = db.begin_write().unwrap();
        w.open_table(t("meta")).unwrap().insert("schema_version", "99").unwrap();
        w.commit().unwrap();
    }
    let err = crate::store::Store::open(&path, &dir).err().expect("newer schema must be refused").to_string();
    assert!(err.contains("newer OpSkills"), "{err}");
    std::fs::remove_dir_all(&dir).unwrap();

    // Schema 1: an editor and a sealed secret keyed by its bare name.
    std::fs::create_dir_all(&dir).unwrap();
    let vault = crate::vault::Vault::from_key_hex(&hex::encode([7u8; 32])).unwrap();
    let sealed = vault.seal("DB_PASSWORD", b"old-value").unwrap();
    {
        let db = redb::Database::create(&path).unwrap();
        let w = db.begin_write().unwrap();
        w.open_table(t("meta")).unwrap().insert("schema_version", "1").unwrap();
        w.open_table(t("users")).unwrap().insert("erin", r#"{"name":"erin","role":"editor","created_at":0,"password_hash":"x"}"#).unwrap();
        let secret = json!({"name": "DB_PASSWORD", "kind": "password", "description": "", "created_at": 0, "updated_at": 0, "nonce": sealed.nonce, "ciphertext": sealed.ciphertext});
        w.open_table(t("secrets")).unwrap().insert("DB_PASSWORD", secret.to_string().as_str()).unwrap();
        w.commit().unwrap();
    }
    let store = crate::store::Store::open(&path, &dir).unwrap();
    let erin = store.user("erin").unwrap().unwrap().user;
    assert!(!erin.admin);
    assert_eq!(erin.workspaces.get("default"), Some(&Role::Editor));
    let moved = store.secret("default", "DB_PASSWORD").unwrap().expect("the secret moved to the default workspace");
    let opened = vault.open(&crate::model::secret_binding("default", "DB_PASSWORD"), &crate::vault::Sealed { nonce: moved.nonce, ciphertext: moved.ciphertext }).unwrap();
    assert_eq!(opened, b"old-value", "secrets of default still open after the upgrade");
    assert_eq!(backups(&dir), 1);
    drop(store);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn deleting_admin_hash_resets_the_admin_password() {
    let dir = std::env::temp_dir().join(format!("opskills-it-{}", uuid::Uuid::now_v7()));
    let mut cfg = Config::default();
    cfg.data.dir = dir.clone();
    let first = firststart::prepare(&cfg).unwrap();
    {
        let state = web::build_state(cfg.clone(), &first).unwrap();
        let mut admin = state.store.user("admin").unwrap().unwrap();
        assert!(admin.user.must_change_password);
        admin.user.must_change_password = false;
        admin.user.admin = false;
        admin.password_hash = crate::auth::hash_password(CHOSEN).unwrap();
        state.store.put_user(&admin).unwrap();
    }
    std::fs::remove_file(dir.join("admin.hash")).unwrap();
    let second = firststart::prepare(&cfg).unwrap();
    let new_password = second.initial_password.clone().expect("a new password is printed");
    let state = web::build_state(cfg, &second).unwrap();
    let admin = state.store.user("admin").unwrap().unwrap();
    assert!(admin.user.must_change_password);
    assert!(admin.user.admin);
    assert!(crate::auth::verify_password(&admin.password_hash, &new_password));
    assert!(!crate::auth::verify_password(&admin.password_hash, CHOSEN));
    drop(state);
    std::fs::remove_dir_all(dir).unwrap();
}
