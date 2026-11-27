//! Users and roles: first sign-in, the access rule of every route, tokens, user management,
//! the database migration and the password reset through `admin.hash`.

use super::*;
use crate::api::{Access, access_rules};
use crate::model::Role;

#[tokio::test]
async fn the_first_sign_in_asks_for_a_new_password() {
    let s = fresh_server();
    let system = call(&s.app, "GET", "/api/v1/system", None, None, false).await.json();
    assert_eq!(system["setup_done"], false, "the sign-in page shows where the password is");
    let r = sign_in_as(&s, "admin", &s.password).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json()["must_change_password"], true);
    assert_eq!(r.json()["role"], "admin");
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
    let system = call(&s.app, "GET", "/api/v1/system", None, None, false).await.json();
    assert_eq!(system["setup_done"], true);
    assert_eq!(sign_in_as(&s, "admin", &s.password).await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(sign_in_as(&s, "admin", CHOSEN).await.status, StatusCode::OK);
    assert_eq!(sign_in_as(&s, "nobody", CHOSEN).await.status, StatusCode::UNAUTHORIZED);
}

/// Every route with a role rule refuses the role below it and lets that role through.
#[tokio::test]
async fn every_route_enforces_its_role() {
    let s = server();
    let admin = sign_in(&s).await;
    let mut cookies = std::collections::HashMap::new();
    for role in ["viewer", "operator", "editor"] {
        cookies.insert(role, user_with_role(&s, &admin, role, role).await);
    }
    cookies.insert("admin", admin.clone());
    let below = |r: Role| match r {
        Role::Viewer => None,
        Role::Operator => Some("viewer"),
        Role::Editor => Some("operator"),
        Role::Admin => Some("editor"),
    };
    let param = regex::Regex::new(r"\{[a-z]+\}").unwrap();
    let mut checked = 0;
    for (method, route, access) in access_rules() {
        let (Access::Role(min) | Access::SessionRole(min)) = access else { continue };
        let path = format!("/api/v1{}", param.replace_all(route, "x"));
        let body = (method != "GET").then(|| json!({}));
        if let Some(lower) = below(min) {
            let r = call(&s.app, method, &path, body.clone(), Some(&cookies[lower]), true).await;
            assert_eq!(r.status, StatusCode::FORBIDDEN, "{lower} on {method} {route}: {}", r.body);
            assert!(r.body.contains("role doesn't allow"), "{lower} on {method} {route}: {}", r.body);
        }
        let r = call(&s.app, method, &path, body, Some(&cookies[min.as_str()]), true).await;
        assert!(!r.body.contains("role doesn't allow"), "{} on {method} {route}: {}", min.as_str(), r.body);
        checked += 1;
    }
    assert!(checked > 30, "checked only {checked} routes");
}

/// The role table agreed for 4a (notes in the docs): who may do what.
#[test]
fn the_role_table_is_the_agreed_one() {
    let rule = |m: &str, r: &str| access_rules().find(|(mm, rr, _)| *mm == m && *rr == r).map(|(_, _, a)| a).unwrap();
    use Access::{Role as R, SessionRole as S};
    for (m, r, want) in [
        ("GET", "/runbooks", R(Role::Viewer)),
        ("GET", "/runs/{id}/logs", R(Role::Viewer)),
        ("GET", "/hosts", R(Role::Viewer)),
        ("POST", "/runbooks/{id}/runs", R(Role::Operator)),
        ("POST", "/runs/{id}/cancel", R(Role::Operator)),
        ("POST", "/runs/{id}/retry", R(Role::Operator)),
        ("POST", "/runbooks", R(Role::Editor)),
        ("PUT", "/runbooks/{id}", R(Role::Editor)),
        ("POST", "/runbooks/{id}/webhooks", R(Role::Editor)),
        ("GET", "/secrets", R(Role::Editor)),
        ("PUT", "/secrets/{name}", R(Role::Editor)),
        ("POST", "/hosts/enrollment-tokens", R(Role::Admin)),
        ("PUT", "/hosts/{id}", R(Role::Admin)),
        ("PUT", "/settings/email", R(Role::Admin)),
        ("GET", "/users", S(Role::Admin)),
        ("GET", "/tokens", S(Role::Admin)),
        ("POST", "/me/tokens", S(Role::Viewer)),
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

    // Tokens are personal: others don't see them, admins see everyone's.
    let mine = call(&s.app, "GET", "/api/v1/me/tokens", None, Some(&admin), false).await.json();
    assert_eq!(mine.as_array().unwrap().len(), 0);
    let all = call(&s.app, "GET", "/api/v1/tokens", None, Some(&admin), false).await.json();
    assert_eq!(all[0]["user"], "erin");
    let id = all[0]["id"].as_str().unwrap().to_string();
    assert_eq!(call(&s.app, "DELETE", &format!("/api/v1/me/tokens/{id}"), None, Some(&admin), true).await.status, StatusCode::NOT_FOUND);
    // Tokens can't manage users or tokens.
    assert_eq!(bearer_call(&s.app, "GET", "/api/v1/me/tokens", None, &token).await.status, StatusCode::FORBIDDEN);

    // A lower role applies to the token at once; removing the user ends it.
    let r = call(&s.app, "PUT", "/api/v1/users/erin", Some(json!({"role": "viewer"})), Some(&admin), true).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    let r = bearer_call(&s.app, "POST", "/api/v1/runbooks", Some(rb), &token).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN, "{}", r.body);
    assert_eq!(bearer_call(&s.app, "GET", "/api/v1/runbooks", None, &token).await.status, StatusCode::OK);
    assert_eq!(call(&s.app, "DELETE", "/api/v1/users/erin", None, Some(&admin), true).await.status, StatusCode::NO_CONTENT);
    assert_eq!(bearer_call(&s.app, "GET", "/api/v1/runbooks", None, &token).await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(call(&s.app, "GET", "/api/v1/runbooks", None, Some(&editor), false).await.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn admins_manage_users_but_not_themselves() {
    let s = server();
    let admin = sign_in(&s).await;
    let r = call(&s.app, "POST", "/api/v1/users", Some(json!({"name": "Bad Name", "role": "viewer"})), Some(&admin), true).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = call(&s.app, "POST", "/api/v1/users", Some(json!({"name": "ops", "role": "root"})), Some(&admin), true).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let viewer = user_with_role(&s, &admin, "vic", "viewer").await;
    let r = call(&s.app, "POST", "/api/v1/users", Some(json!({"name": "vic", "role": "viewer"})), Some(&admin), true).await;
    assert_eq!(r.status, StatusCode::CONFLICT);

    for (method, path, body) in [
        ("PUT", "/api/v1/users/admin", Some(json!({"role": "viewer"}))),
        ("DELETE", "/api/v1/users/admin", None),
        ("POST", "/api/v1/users/admin/reset-password", None),
    ] {
        let r = call(&s.app, method, path, body, Some(&admin), true).await;
        assert_eq!(r.status, StatusCode::CONFLICT, "{method} {path}: {}", r.body);
    }

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
    assert_eq!(names, ["admin", "vic"]);
    assert!(!users.to_string().contains("password_hash"));
}

#[test]
fn old_databases_are_backed_up_and_migrated() {
    let dir = std::env::temp_dir().join(format!("opskills-it-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("opskills.redb");
    // A 0.11 database: tables but no schema version, and the admin password in admin.hash.
    {
        let db = redb::Database::create(&path).unwrap();
        let w = db.begin_write().unwrap();
        w.open_table(redb::TableDefinition::<&str, &str>::new("runbooks")).unwrap().insert("rb", "{}").unwrap();
        w.commit().unwrap();
    }
    std::fs::write(dir.join("admin.hash"), crate::auth::hash_password("old admin password").unwrap()).unwrap();

    let store = crate::store::Store::open(&path, &dir).unwrap();
    assert_eq!(store.schema_version().unwrap(), crate::store::SCHEMA_VERSION);
    let admin = store.user("admin").unwrap().unwrap();
    assert_eq!(admin.user.role, Role::Admin);
    assert!(!admin.user.must_change_password, "upgrades keep the admin's password working");
    assert!(crate::auth::verify_password(&admin.password_hash, "old admin password"));
    let backups: Vec<_> = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().to_string()).filter(|n| n.contains("schema-0-backup")).collect();
    assert_eq!(backups.len(), 1, "{backups:?}");
    drop(store);

    // Opening again changes nothing and makes no new backup.
    drop(crate::store::Store::open(&path, &dir).unwrap());
    let count = std::fs::read_dir(&dir).unwrap().filter(|e| e.as_ref().unwrap().file_name().to_string_lossy().contains("backup")).count();
    assert_eq!(count, 1);

    // A database from a newer version is refused.
    {
        let db = redb::Database::create(&path).unwrap();
        let w = db.begin_write().unwrap();
        w.open_table(redb::TableDefinition::<&str, &str>::new("meta")).unwrap().insert("schema_version", "99").unwrap();
        w.commit().unwrap();
    }
    let err = crate::store::Store::open(&path, &dir).err().expect("newer schema must be refused").to_string();
    assert!(err.contains("newer OpSkills"), "{err}");
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
        admin.user.role = Role::Viewer;
        admin.password_hash = crate::auth::hash_password(CHOSEN).unwrap();
        state.store.put_user(&admin).unwrap();
    }
    std::fs::remove_file(dir.join("admin.hash")).unwrap();
    let second = firststart::prepare(&cfg).unwrap();
    let new_password = second.initial_password.clone().expect("a new password is printed");
    let state = web::build_state(cfg, &second).unwrap();
    let admin = state.store.user("admin").unwrap().unwrap();
    assert!(admin.user.must_change_password);
    assert_eq!(admin.user.role, Role::Admin);
    assert!(crate::auth::verify_password(&admin.password_hash, &new_password));
    assert!(!crate::auth::verify_password(&admin.password_hash, CHOSEN));
    drop(state);
    std::fs::remove_dir_all(dir).unwrap();
}
