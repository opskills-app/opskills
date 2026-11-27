//! Sign-in with an OIDC provider, against a fake provider: discovery, keys, and a token
//! endpoint that checks the client secret and PKCE and signs ID tokens.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use axum::Json;
use axum::http::HeaderMap;
use axum::response::IntoResponse as _;
use base64::Engine as _;
use sha2::Digest as _;
use openidconnect::PrivateSigningKey;
use openidconnect::core::{CoreJwsSigningAlgorithm, CoreRsaPrivateSigningKey};

use super::*;

const CLIENT_ID: &str = "opskills-test";
const CLIENT_SECRET: &str = "client-secret-1234";
const PUBLIC_URL: &str = "http://opskills.test";

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Two RSA keys, made once per test run: the provider's, and a stranger's.
fn keys() -> &'static (String, String) {
    static KEYS: OnceLock<(String, String)> = OnceLock::new();
    KEYS.get_or_init(|| {
        let make = || {
            let k = rsa::RsaPrivateKey::new(&mut rsa::rand_core::OsRng, 2048).unwrap();
            rsa::pkcs1::EncodeRsaPrivateKey::to_pkcs1_pem(&k, rsa::pkcs1::LineEnding::LF).unwrap().to_string()
        };
        (make(), make())
    })
}

fn signer(pem: &str) -> CoreRsaPrivateSigningKey {
    CoreRsaPrivateSigningKey::from_pem(pem, Some(openidconnect::JsonWebKeyId::new("k1".into()))).unwrap()
}

/// What the next ID token should get wrong.
#[derive(Clone, Copy, Default)]
struct Tamper {
    wrong_audience: bool,
    expired: bool,
    wrong_key: bool,
    wrong_nonce: bool,
}

struct Approved {
    nonce: String,
    challenge: String,
    claims: Value,
}

#[derive(Clone)]
struct Idp {
    url: String,
    codes: Arc<Mutex<HashMap<String, Approved>>>,
    tamper: Arc<Mutex<Tamper>>,
}

fn query_param(url: &str, name: &str) -> Option<String> {
    let q = url.split_once('?')?.1;
    q.split('&').find_map(|kv| kv.split_once('=').filter(|(k, _)| *k == name).map(|(_, v)| percent_decode(v)))
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 3 <= b.len() && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
            out.push(v);
            i += 3;
        } else {
            out.push(if b[i] == b'+' { b' ' } else { b[i] });
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

async fn fake_idp() -> Idp {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let idp = Idp { url: url.clone(), codes: Arc::new(Mutex::new(HashMap::new())), tamper: Arc::new(Mutex::new(Tamper::default())) };
    let (u1, u2) = (url.clone(), url.clone());
    let jwks = json!({ "keys": [signer(&keys().0).as_verification_key()] });
    let state = idp.clone();
    let app = Router::new()
        .route(
            "/.well-known/openid-configuration",
            axum::routing::get(move || {
                let u = u1.clone();
                async move {
                    Json(json!({
                        "issuer": u, "authorization_endpoint": format!("{u}/authorize"), "token_endpoint": format!("{u}/token"),
                        "jwks_uri": format!("{u}/jwks"), "response_types_supported": ["code"], "subject_types_supported": ["public"],
                        "id_token_signing_alg_values_supported": ["RS256"],
                    }))
                }
            }),
        )
        .route("/jwks", axum::routing::get(move || std::future::ready(Json(jwks.clone()))))
        .route(
            "/token",
            axum::routing::post(move |headers: HeaderMap, body: String| {
                let (idp, issuer) = (state.clone(), u2.clone());
                async move { token(&idp, &issuer, &headers, &body) }
            }),
        );
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    idp
}

/// The provider's token endpoint: the client secret, the code, PKCE, then a signed ID token.
fn token(idp: &Idp, issuer: &str, headers: &HeaderMap, body: &str) -> axum::response::Response {
    let form: HashMap<String, String> = body.split('&').filter_map(|kv| kv.split_once('=')).map(|(k, v)| (k.to_string(), percent_decode(v))).collect();
    let basic = headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).unwrap_or("");
    let expected = format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(format!("{CLIENT_ID}:{CLIENT_SECRET}")));
    if basic != expected {
        return (StatusCode::UNAUTHORIZED, Json(json!({"error": "invalid_client"}))).into_response();
    }
    let Some(approved) = form.get("code").and_then(|c| idp.codes.lock().unwrap().remove(c)) else {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "invalid_grant"}))).into_response();
    };
    let verifier = form.get("code_verifier").cloned().unwrap_or_default();
    if b64(&sha2::Sha256::digest(verifier.as_bytes())) != approved.challenge {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "invalid_grant", "error_description": "PKCE"}))).into_response();
    }
    let t = *idp.tamper.lock().unwrap();
    let now = now_secs();
    let mut claims = json!({
        "iss": issuer, "aud": if t.wrong_audience { "someone-else" } else { CLIENT_ID },
        "iat": now, "exp": if t.expired { now - 60 } else { now + 300 },
        "nonce": if t.wrong_nonce { "not-the-nonce".to_string() } else { approved.nonce },
    });
    for (k, v) in approved.claims.as_object().unwrap() {
        claims[k] = v.clone();
    }
    let header = b64(json!({"alg": "RS256", "kid": "k1", "typ": "JWT"}).to_string().as_bytes());
    let payload = b64(claims.to_string().as_bytes());
    let message = format!("{header}.{payload}");
    let pem = if t.wrong_key { &keys().1 } else { &keys().0 };
    let sig = signer(pem).sign(&CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha256, message.as_bytes()).unwrap();
    Json(json!({ "access_token": "at", "token_type": "Bearer", "expires_in": 300, "id_token": format!("{message}.{}", b64(&sig)) })).into_response()
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
}

impl Idp {
    /// What the browser does at the provider: approve the sign-in `location` asks for, as
    /// someone with `claims`. Returns where the provider sends the browser back.
    fn approve(&self, location: &str, claims: Value) -> String {
        assert!(location.starts_with(&format!("{}/authorize?", self.url)), "{location}");
        assert_eq!(query_param(location, "redirect_uri").as_deref(), Some("http://opskills.test/api/v1/auth/oidc/callback"));
        assert_eq!(query_param(location, "code_challenge_method").as_deref(), Some("S256"));
        let code = uuid::Uuid::now_v7().to_string();
        let approved = Approved { nonce: query_param(location, "nonce").unwrap(), challenge: query_param(location, "code_challenge").unwrap(), claims };
        self.codes.lock().unwrap().insert(code.clone(), approved);
        format!("/api/v1/auth/oidc/callback?code={code}&state={}", query_param(location, "state").unwrap())
    }

    fn tamper(&self, t: Tamper) {
        *self.tamper.lock().unwrap() = t;
    }
}

/// A GET with its headers kept.
async fn get(app: &Router, path: &str, cookie: Option<&str>) -> (StatusCode, HeaderMap) {
    let mut req = Request::builder().method("GET").uri(path);
    if let Some(c) = cookie {
        req = req.header(header::COOKIE, c);
    }
    let res = app.clone().oneshot(req.body(Body::empty()).unwrap()).await.unwrap();
    (res.status(), res.headers().clone())
}

fn set_cookie(h: &HeaderMap, name: &str) -> Option<String> {
    h.get_all(header::SET_COOKIE).iter().filter_map(|v| v.to_str().ok()).map(|v| v.split(';').next().unwrap().to_string()).find(|c| c.starts_with(&format!("{name}=")) && c.len() > name.len() + 1)
}

/// Sign in at the provider as someone with `claims`: the session cookie, or the message the
/// sign-in page would show.
async fn provider_sign_in(s: &TestServer, idp: &Idp, claims: Value) -> Result<String, String> {
    let (status, h) = get(&s.app, "/api/v1/auth/oidc/start", None).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let location = h.get(header::LOCATION).unwrap().to_str().unwrap().to_string();
    if location.starts_with("/?sign_in_error=") {
        return Err(percent_decode(location.trim_start_matches("/?sign_in_error=")));
    }
    let pending = set_cookie(&h, "opskills_oidc").expect("a cookie for the sign-in in progress");
    let back = idp.approve(&location, claims);
    finish(s, &back, Some(&pending)).await
}

async fn finish(s: &TestServer, back: &str, pending: Option<&str>) -> Result<String, String> {
    let (status, h) = get(&s.app, back, pending).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let to = h.get(header::LOCATION).unwrap().to_str().unwrap();
    match to {
        "/" => Ok(set_cookie(&h, "opskills_session").expect("a session")),
        e => Err(percent_decode(e.trim_start_matches("/?sign_in_error="))),
    }
}

fn oidc_server() -> TestServer {
    server_with(|cfg| cfg.server.public_url = Some(PUBLIC_URL.into()))
}

/// The client secret in the vault, and the provider set up with `extra` settings.
async fn set_up(s: &TestServer, admin: &str, idp: &Idp, extra: Value) -> Reply {
    if call(&s.app, "GET", "/api/v1/shared-secrets", None, Some(admin), false).await.json().as_array().is_none_or(|a| a.is_empty()) {
        let r = call(&s.app, "POST", "/api/v1/shared-secrets", Some(json!({"name": "oidc/CLIENT_SECRET", "kind": "password", "value": CLIENT_SECRET})), Some(admin), true).await;
        assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
    }
    let mut oidc = json!({"label": "Acme ID", "issuer": idp.url, "client_id": CLIENT_ID, "client_secret": "shared/oidc/CLIENT_SECRET"});
    let mut top = json!({});
    for (k, v) in extra.as_object().unwrap() {
        if k == "passwords_for_server_admins_only" {
            top[k] = v.clone();
        } else {
            oidc[k] = v.clone();
        }
    }
    top["oidc"] = oidc;
    call(&s.app, "PUT", "/api/v1/settings/sign-in", Some(top), Some(admin), true).await
}

fn verified(sub: &str, email: &str) -> Value {
    json!({"sub": sub, "email": email, "email_verified": true})
}

#[tokio::test]
async fn a_pre_created_account_links_by_verified_email_then_by_provider_id() {
    let s = oidc_server();
    let admin = sign_in(&s).await;
    let idp = fake_idp().await;
    let r = call(&s.app, "POST", "/api/v1/users", Some(json!({"name": "erin", "email": "Erin@Example.com", "workspaces": {"default": "editor"}})), Some(&admin), true).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
    let generated = r.json()["password"].as_str().unwrap().to_string();
    assert_eq!(set_up(&s, &admin, &idp, json!({})).await.status, StatusCode::OK);
    let system = call(&s.app, "GET", "/api/v1/system", None, None, false).await.json();
    assert_eq!(system["sign_in"], json!({"provider": "Acme ID", "passwords": "everyone"}));

    let erin = provider_sign_in(&s, &idp, verified("u-erin", "erin@example.com")).await.unwrap();
    let me = call(&s.app, "GET", "/api/v1/auth/me", None, Some(&erin), false).await.json();
    assert_eq!((me["user"].as_str(), me["must_change_password"].as_bool(), me["password"].as_bool()), (Some("erin"), Some(false), Some(false)));
    assert_eq!(me["signs_in_with"], "Acme ID");
    assert_eq!(me["workspaces"][0]["role"], "editor");
    // The generated password, never chosen by erin, no longer works.
    let r = call(&s.app, "POST", "/api/v1/auth/login", Some(json!({"username": "erin", "password": generated})), None, true).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    // Later sign-ins go by the provider's id, even with another email.
    let again = provider_sign_in(&s, &idp, verified("u-erin", "erin.new@example.com")).await.unwrap();
    assert_eq!(call(&s.app, "GET", "/api/v1/auth/me", None, Some(&again), false).await.json()["user"], "erin");
    let users = call(&s.app, "GET", "/api/v1/users", None, Some(&admin), false).await.json();
    let row = users.as_array().unwrap().iter().find(|u| u["name"] == "erin").unwrap().clone();
    assert_eq!(row["oidc"], json!({"issuer": idp.url, "subject": "u-erin"}));
    assert_eq!(row["email"], "erin@example.com");

    // Unlinked by a server admin, the next sign-in links again by email.
    let r = call(&s.app, "PUT", "/api/v1/users/erin", Some(json!({"unlink_sign_in": true})), Some(&admin), true).await;
    assert!(r.json().get("oidc").is_none(), "{}", r.body);
    assert!(provider_sign_in(&s, &idp, verified("u-erin-2", "erin@example.com")).await.is_ok());
    let audit = call(&s.app, "GET", "/api/v1/audit?q=Acme", None, Some(&admin), false).await.json();
    let actions: Vec<&str> = audit["events"].as_array().unwrap().iter().map(|e| e["action"].as_str().unwrap()).collect();
    assert!(actions.contains(&"user.sign_in_link") && actions.contains(&"auth.sign_in") && actions.contains(&"settings.sign_in_update"), "{actions:?}");
}

#[tokio::test]
async fn people_without_an_account_get_one_only_from_allowed_domains() {
    let s = oidc_server();
    let admin = sign_in(&s).await;
    let idp = fake_idp().await;
    set_up(&s, &admin, &idp, json!({})).await;
    let e = provider_sign_in(&s, &idp, verified("u-new", "new.person@example.com")).await.unwrap_err();
    assert!(e.contains("no OpSkills account for new.person@example.com"), "{e}");

    let r = set_up(&s, &admin, &idp, json!({"create_accounts": true, "allowed_domains": ["Example.com"]})).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(r.json()["settings"]["oidc"]["allowed_domains"], json!(["example.com"]));
    let claims = json!({"sub": "u-new", "email": "new.person@example.com", "email_verified": true, "preferred_username": "new.person@example.com"});
    let cookie = provider_sign_in(&s, &idp, claims).await.unwrap();
    let me = call(&s.app, "GET", "/api/v1/auth/me", None, Some(&cookie), false).await.json();
    assert_eq!((me["user"].as_str(), me["admin"].as_bool()), (Some("new.person"), Some(false)));
    assert_eq!(me["workspaces"], json!([]), "no workspace until a workspace Admin adds them");
    let r = call(&s.app, "GET", "/api/v1/runbooks", None, Some(&cookie), false).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    // The same name from another person gets a number.
    let claims = json!({"sub": "u-other", "email": "new.person2@example.com", "email_verified": true, "preferred_username": "new.person"});
    let other = provider_sign_in(&s, &idp, claims).await.unwrap();
    assert_eq!(call(&s.app, "GET", "/api/v1/auth/me", None, Some(&other), false).await.json()["user"], "new.person-2");

    let e = provider_sign_in(&s, &idp, verified("u-x", "x@elsewhere.example")).await.unwrap_err();
    assert!(e.contains("not from an email domain"), "{e}");
    let e = provider_sign_in(&s, &idp, json!({"sub": "u-y", "email": "y@example.com", "email_verified": false})).await.unwrap_err();
    assert!(e.contains("hasn't verified"), "{e}");
    let e = provider_sign_in(&s, &idp, json!({"sub": "u-z"})).await.unwrap_err();
    assert!(e.contains("no email address"), "{e}");
    // A provider that doesn't say whether emails are verified can be trusted on purpose.
    set_up(&s, &admin, &idp, json!({"create_accounts": true, "allowed_domains": ["example.com"], "trust_unverified_email": true})).await;
    assert!(provider_sign_in(&s, &idp, json!({"sub": "u-y", "email": "y@example.com"})).await.is_ok());
    let audit = call(&s.app, "GET", "/api/v1/audit?action=user.create", None, Some(&admin), false).await.json();
    assert!(audit["events"][0]["detail"].as_str().unwrap().contains("first sign-in with Acme ID"), "{audit}");
}

#[tokio::test]
async fn tokens_that_dont_check_out_sign_no_one_in() {
    let s = oidc_server();
    let admin = sign_in(&s).await;
    let idp = fake_idp().await;
    call(&s.app, "POST", "/api/v1/users", Some(json!({"name": "erin", "email": "erin@example.com"})), Some(&admin), true).await;
    set_up(&s, &admin, &idp, json!({})).await;
    for (t, expect) in [
        (Tamper { wrong_audience: true, ..Default::default() }, "didn't check out"),
        (Tamper { expired: true, ..Default::default() }, "didn't check out"),
        (Tamper { wrong_key: true, ..Default::default() }, "didn't check out"),
        (Tamper { wrong_nonce: true, ..Default::default() }, "didn't check out"),
    ] {
        idp.tamper(t);
        let e = provider_sign_in(&s, &idp, verified("u-erin", "erin@example.com")).await.unwrap_err();
        assert!(e.contains(expect), "{e}");
    }
    idp.tamper(Tamper::default());
    // Coming back in another browser (no cookie), or with the wrong state.
    let (_, h) = get(&s.app, "/api/v1/auth/oidc/start", None).await;
    let location = h.get(header::LOCATION).unwrap().to_str().unwrap().to_string();
    let pending = set_cookie(&h, "opskills_oidc").unwrap();
    let back = idp.approve(&location, verified("u-erin", "erin@example.com"));
    assert!(finish(&s, &back, None).await.unwrap_err().contains("another browser"));
    let wrong_state = back.split("&state=").next().unwrap().to_string() + "&state=forged";
    assert!(finish(&s, &wrong_state, Some(&pending)).await.unwrap_err().contains("wrong state"));
    // Each sign-in in progress can be finished once, even after a failure.
    assert!(finish(&s, &back, Some(&pending)).await.unwrap_err().contains("expired or was started in another browser"));
    // A provider error, and a wrong client secret.
    let e = finish(&s, "/api/v1/auth/oidc/callback?error=access_denied&error_description=User%20cancelled", None).await.unwrap_err();
    assert!(e.contains("access_denied (User cancelled)"), "{e}");
    call(&s.app, "PUT", "/api/v1/shared-secrets/oidc/CLIENT_SECRET", Some(json!({"value": "not-the-secret"})), Some(&admin), true).await;
    let e = provider_sign_in(&s, &idp, verified("u-erin", "erin@example.com")).await.unwrap_err();
    assert!(e.contains("refused the sign-in code"), "{e}");
    let users = call(&s.app, "GET", "/api/v1/users", None, Some(&admin), false).await.json();
    assert!(users.as_array().unwrap().iter().all(|u| u.get("oidc").is_none()), "no one was linked: {users}");
}

#[tokio::test]
async fn passwords_can_be_kept_for_server_admins_only() {
    let s = oidc_server();
    let admin = sign_in(&s).await;
    let idp = fake_idp().await;
    let r = call(&s.app, "PUT", "/api/v1/settings/sign-in", Some(json!({"passwords_for_server_admins_only": true})), Some(&admin), true).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "no one else could sign in: {}", r.body);
    user_with_role(&s, &admin, "pat", "editor").await;
    let pat_password = "pat chose this password";
    assert_eq!(set_up(&s, &admin, &idp, json!({"passwords_for_server_admins_only": true})).await.status, StatusCode::OK);
    assert_eq!(call(&s.app, "GET", "/api/v1/system", None, None, false).await.json()["sign_in"]["passwords"], "server_admins");
    let r = call(&s.app, "POST", "/api/v1/auth/login", Some(json!({"username": "admin", "password": s.password})), None, true).await;
    assert_eq!(r.status, StatusCode::OK, "server admins keep their password: {}", r.body);
    let mut stored = s.state.store.user("pat").unwrap().unwrap();
    stored.password_hash = crate::auth::hash_password(pat_password).unwrap();
    s.state.store.put_user(&stored).unwrap();
    let r = call(&s.app, "POST", "/api/v1/auth/login", Some(json!({"username": "pat", "password": pat_password})), None, true).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert!(r.body.contains("Sign in with Acme ID"), "{}", r.body);
}

#[tokio::test]
async fn sign_in_settings_are_checked() {
    let s = oidc_server();
    let admin = sign_in(&s).await;
    let idp = fake_idp().await;
    let got = call(&s.app, "GET", "/api/v1/settings/sign-in", None, Some(&admin), false).await.json();
    assert_eq!(got["redirect_url"], "http://opskills.test/api/v1/auth/oidc/callback");
    assert_eq!(got["settings"]["oidc"], Value::Null);
    let r = set_up(&s, &admin, &idp, json!({})).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    let put = |oidc: Value| call(&s.app, "PUT", "/api/v1/settings/sign-in", Some(json!({"oidc": oidc})), Some(&admin), true);
    let base = json!({"label": "Acme ID", "issuer": idp.url, "client_id": CLIENT_ID, "client_secret": "shared/oidc/CLIENT_SECRET"});
    let with = |k: &str, v: Value| {
        let mut b = base.clone();
        b[k] = v;
        b
    };
    for (bad, expect) in [
        (with("client_secret", json!("CLIENT_SECRET")), "must be a shared secret"),
        (with("client_secret", json!("shared/oidc/MISSING")), "no shared secret named oidc/MISSING"),
        (with("issuer", json!("http://login.example.com")), "https URL"),
        (with("label", json!("")), "1 to 40"),
        (with("create_accounts", json!(true)), "list the email domains"),
        (with("allowed_domains", json!(["not a domain"])), "not an email domain"),
    ] {
        let r = put(bad).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{expect}: {}", r.body);
        assert!(r.body.contains(expect), "{expect}: {}", r.body);
    }
    // The provider test reads its settings; an issuer that isn't one fails.
    let r = call(&s.app, "POST", "/api/v1/settings/sign-in/test", Some(json!({"issuer": idp.url})), Some(&admin), true).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(r.json()["authorization_endpoint"], format!("{}/authorize", idp.url));
    let r = call(&s.app, "POST", "/api/v1/settings/sign-in/test", Some(json!({"issuer": format!("{}/nope", idp.url)})), Some(&admin), true).await;
    assert_eq!(r.status, StatusCode::BAD_GATEWAY);
    // The client secret can't be deleted while sign-in uses it.
    let r = call(&s.app, "DELETE", "/api/v1/shared-secrets/oidc/CLIENT_SECRET", None, Some(&admin), true).await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    assert!(r.body.contains("Settings > Sign-in"), "{}", r.body);
    // Turned off, the button goes away and starting a sign-in explains why.
    assert_eq!(put(with("enabled", json!(false))).await.status, StatusCode::OK);
    assert_eq!(call(&s.app, "GET", "/api/v1/system", None, None, false).await.json()["sign_in"]["provider"], Value::Null);
    assert!(provider_sign_in(&s, &idp, verified("u", "u@example.com")).await.unwrap_err().contains("isn't set up"));
    // Without a public URL, the provider can't send anyone back.
    let s2 = server();
    let admin2 = sign_in(&s2).await;
    let r = set_up(&s2, &admin2, &idp, json!({})).await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    assert!(r.body.contains("public URL"), "{}", r.body);
}

#[tokio::test]
async fn emails_are_unique_and_checked() {
    let s = server();
    let admin = sign_in(&s).await;
    let r = call(&s.app, "POST", "/api/v1/users", Some(json!({"name": "erin", "email": "erin@example.com"})), Some(&admin), true).await;
    assert_eq!(r.status, StatusCode::CREATED);
    let r = call(&s.app, "POST", "/api/v1/users", Some(json!({"name": "erin2", "email": "ERIN@example.com"})), Some(&admin), true).await;
    assert_eq!(r.status, StatusCode::CONFLICT, "{}", r.body);
    let r = call(&s.app, "POST", "/api/v1/users", Some(json!({"name": "erin3", "email": "not-an-email"})), Some(&admin), true).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = call(&s.app, "PUT", "/api/v1/users/erin", Some(json!({"email": ""})), Some(&admin), true).await;
    assert!(r.json().get("email").is_none(), "{}", r.body);
    // A refused change changes nothing.
    call(&s.app, "PUT", "/api/v1/users/erin", Some(json!({"email": "erin@example.com"})), Some(&admin), true).await;
    let r = call(&s.app, "PUT", "/api/v1/users/admin", Some(json!({"email": "admin@example.com", "admin": false})), Some(&admin), true).await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    assert!(s.state.store.user("admin").unwrap().unwrap().user.email.is_none());
}
