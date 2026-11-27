//! Notifications by webhook and email.

use super::*;

/// A local HTTP receiver that records each POST. `/flaky` answers 503 once, then 200;
/// `/gone` always answers 404.
async fn receiver() -> (String, std::sync::Arc<std::sync::Mutex<Vec<(String, String, Value)>>>) {
    use std::sync::{Arc, Mutex};
    type Seen = Arc<Mutex<Vec<(String, String, Value)>>>;
    let seen: Seen = Arc::new(Mutex::new(vec![]));
    let flaky = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (s2, f2) = (seen.clone(), flaky.clone());
    let app = Router::new().fallback(move |uri: axum::http::Uri, headers: axum::http::HeaderMap, body: String| {
        let (seen, flaky) = (s2.clone(), f2.clone());
        async move {
            let path = uri.path().to_string();
            if path == "/gone" {
                return StatusCode::NOT_FOUND;
            }
            if path == "/flaky" && !flaky.swap(true, std::sync::atomic::Ordering::SeqCst) {
                return StatusCode::SERVICE_UNAVAILABLE;
            }
            let event = headers.get("x-opskills-event").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
            seen.lock().unwrap().push((path, event, serde_json::from_str(&body).unwrap_or(Value::Null)));
            StatusCode::OK
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), seen)
}

async fn wait_for<T>(mut f: impl FnMut() -> Option<T>, what: &str) -> T {
    for _ in 0..300 {
        if let Some(v) = f() {
            return v;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("timed out waiting for {what}");
}

async fn notices(s: &TestServer, c: &str, run: &str, n: usize) -> Vec<Value> {
    for _ in 0..300 {
        let list = call(&s.app, "GET", &format!("/api/v1/runs/{run}/notifications"), None, Some(c), false).await.json();
        if list.as_array().unwrap().len() >= n {
            return list.as_array().unwrap().clone();
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("timed out waiting for {n} notices on {run}");
}

#[tokio::test]
async fn notification_rules_are_checked() {
    let s = server();
    let c = sign_in(&s).await;
    let rb = |n: Value| json!({"name": "N", "steps": [{"name": "a", "run": "true"}], "notifications": [n]});
    for bad in [
        json!({"on": [], "webhook": "https://hooks.example.com/x"}),
        json!({"on": ["failed"], "email": ["not-an-address"]}),
        json!({"on": ["failed"], "email": ["a@example.com"], "webhook": "https://hooks.example.com/x"}),
        json!({"on": ["failed"]}),
        json!({"on": ["failed"], "webhook": "ftp://example.com/x"}),
        json!({"on": ["slow"], "webhook": "https://hooks.example.com/x"}),
        json!({"on": ["exploded"], "webhook": "https://hooks.example.com/x"}),
    ] {
        let r = call(&s.app, "POST", "/api/v1/runbooks", Some(rb(bad.clone())), Some(&c), true).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{bad} -> {}", r.body);
    }
    let ok = call(&s.app, "POST", "/api/v1/runbooks", Some(rb(json!({"on": ["failed", "slow", "failed"], "slow_after_seconds": 600, "email": [" ops@example.com "]}))), Some(&c), true).await;
    assert_eq!(ok.status, StatusCode::CREATED, "{}", ok.body);
    let n = &ok.json()["notifications"][0];
    assert_eq!((n["on"].clone(), n["email"][0].as_str(), n["id"].as_str().unwrap().len()), (json!(["failed", "slow"]), Some("ops@example.com"), 12));
    // No SMTP server configured: email can't be tested, and the editor is told so.
    let settings = call(&s.app, "GET", "/api/v1/notifications/settings", None, Some(&c), false).await.json();
    assert_eq!(settings["email"], json!(false));
    let t = call(&s.app, "POST", "/api/v1/notifications/test", Some(json!({"email": ["ops@example.com"]})), Some(&c), true).await;
    assert_eq!(t.status, StatusCode::CONFLICT, "{}", t.body);
}

#[tokio::test]
async fn webhook_notifications_report_starts_failures_and_retries() {
    let (base, seen) = receiver().await;
    let s = server();
    let c = sign_in(&s).await;
    let rb = create(&s, &c, json!({
        "name": "Migrate",
        "steps": [{"name": "prepare", "run": "echo preparing"}, {"name": "migrate", "run": "echo 'table locked' >&2; exit 3"}],
        "notifications": [
            {"on": ["started", "failed", "succeeded"], "webhook": format!("{base}/chat/T000/B000/secretpart")},
            {"on": ["failed"], "webhook": format!("{base}/flaky")},
            {"on": ["failed"], "webhook": format!("{base}/gone")},
            {"on": ["failed"], "webhook": format!("{base}/off"), "enabled": false}
        ]
    }))
    .await;
    let run = call(&s.app, "POST", &format!("/api/v1/runbooks/{rb}/runs"), None, Some(&c), true).await.json();
    let id = run["id"].as_str().unwrap().to_string();
    wait_finished(&s, &c, &id).await;
    let list = notices(&s, &c, &id, 4).await;
    let got = wait_for(|| { let v = seen.lock().unwrap().clone(); (v.len() >= 3).then_some(v) }, "3 webhook calls").await;
    let main: Vec<&(String, String, Value)> = got.iter().filter(|(p, _, _)| p.starts_with("/chat")).collect();
    assert_eq!(main.iter().map(|(_, e, _)| e.as_str()).collect::<Vec<_>>(), vec!["started", "failed"]);
    let failed = &main[1].2;
    let text = failed["text"].as_str().unwrap();
    assert!(text.starts_with("Migrate failed: run #") && text.contains("step \"migrate\" on OpSkills server"), "{text}");
    assert_eq!(failed["content"], failed["text"]);
    assert_eq!(failed["run"]["failed_step"]["exit_code"], json!(3));
    assert!(failed["output_tail"].as_array().unwrap().iter().any(|l| l.as_str().unwrap().ends_with("table locked")), "{failed}");
    assert!(main[0].2["text"].as_str().unwrap().contains("started by admin"));
    // The flaky receiver got it on the second try; the 404 one was not retried.
    assert_eq!(got.iter().filter(|(p, _, _)| p == "/flaky").count(), 1);
    assert!(!got.iter().any(|(p, _, _)| p == "/off"));
    let by_target = |t: &str| list.iter().filter(|n| n["target"].as_str() == Some(t)).cloned().collect::<Vec<_>>();
    let host = format!("webhook {}", base.trim_start_matches("http://").split(':').next().unwrap());
    assert!(by_target(&host).iter().filter(|n| n["ok"] == json!(true)).count() >= 3, "{list:?}");
    let gone = list.iter().find(|n| n["ok"] == json!(false)).expect("the 404 is recorded");
    assert!(gone["error"].as_str().unwrap().contains("404"), "{gone}");
    // Webhook URLs are often secret: only the host is ever shown.
    assert!(!serde_json::to_string(&list).unwrap().contains("secretpart"));
}

#[tokio::test]
async fn slow_runs_notify_once_and_fast_ones_do_not() {
    let (base, seen) = receiver().await;
    let s = server();
    let c = sign_in(&s).await;
    let rb = create(&s, &c, json!({
        "name": "Backup",
        "inputs": [{"name": "nap", "type": "text", "default": "0"}],
        "steps": [{"name": "copy", "run": "sleep {{ inputs.nap }}"}],
        "notifications": [{"on": ["slow", "succeeded"], "slow_after_seconds": 1, "webhook": format!("{base}/slow")}]
    }))
    .await;
    let fast = call(&s.app, "POST", &format!("/api/v1/runbooks/{rb}/runs"), None, Some(&c), true).await.json();
    let fast_id = fast["id"].as_str().unwrap().to_string();
    wait_finished(&s, &c, &fast_id).await;
    notices(&s, &c, &fast_id, 1).await;
    let slow = call(&s.app, "POST", &format!("/api/v1/runbooks/{rb}/runs"), Some(json!({"inputs": {"nap": "2"}})), Some(&c), true).await.json();
    let slow_id = slow["id"].as_str().unwrap().to_string();
    wait_finished(&s, &c, &slow_id).await;
    let list = notices(&s, &c, &slow_id, 2).await;
    let events: Vec<&str> = list.iter().map(|n| n["event"].as_str().unwrap()).collect();
    assert_eq!(events, vec!["slow", "succeeded"]);
    // Give a stray timer from the fast run time to fire, then check it didn't.
    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
    let got = seen.lock().unwrap().clone();
    let slow_calls: Vec<&Value> = got.iter().filter(|(_, e, _)| e == "slow").map(|(_, _, b)| b).collect();
    assert_eq!(slow_calls.len(), 1, "{got:?}");
    assert!(slow_calls[0]["text"].as_str().unwrap().starts_with("Backup is still running after 1 second"), "{:?}", slow_calls[0]);
    assert_eq!(slow_calls[0]["run"]["id"].as_str(), Some(slow_id.as_str()));
}

/// A tiny SMTP server that accepts every message and keeps its text.
async fn fake_smtp() -> (u16, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
    let (port, mail, _) = fake_smtp_with_auth().await;
    (port, mail)
}

/// Like `fake_smtp`, and also keeps each AUTH command it was sent.
#[allow(clippy::type_complexity)]
async fn fake_smtp_with_auth() -> (u16, std::sync::Arc<std::sync::Mutex<Vec<String>>>, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let mail = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let auths = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (m, a) = (mail.clone(), auths.clone());
    tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = listener.accept().await else { return };
            let (m, a) = (m.clone(), a.clone());
            tokio::spawn(async move {
                let (r, mut w) = sock.into_split();
                let mut lines = BufReader::new(r).lines();
                w.write_all(b"220 test ESMTP\r\n").await.unwrap();
                let (mut in_data, mut msg) = (false, String::new());
                while let Ok(Some(line)) = lines.next_line().await {
                    if in_data {
                        if line == "." {
                            in_data = false;
                            m.lock().unwrap().push(std::mem::take(&mut msg));
                            w.write_all(b"250 queued\r\n").await.unwrap();
                        } else {
                            msg.push_str(&line);
                            msg.push('\n');
                        }
                        continue;
                    }
                    let cmd = line.to_ascii_uppercase();
                    let reply: &[u8] = if cmd.starts_with("EHLO") {
                        b"250-test\r\n250 AUTH PLAIN LOGIN\r\n"
                    } else if cmd.starts_with("AUTH") {
                        a.lock().unwrap().push(line.clone());
                        b"235 2.7.0 accepted\r\n"
                    } else if cmd.starts_with("DATA") {
                        in_data = true;
                        b"354 go ahead\r\n"
                    } else if cmd.starts_with("QUIT") {
                        let _ = w.write_all(b"221 bye\r\n").await;
                        return;
                    } else {
                        b"250 ok\r\n"
                    };
                    w.write_all(reply).await.unwrap();
                }
            });
        }
    });
    (port, mail, auths)
}

#[tokio::test]
async fn email_notifications_go_through_the_smtp_server() {
    let (port, mail) = fake_smtp().await;
    let s = server_with(|cfg| {
        cfg.email.smtp_host = Some("127.0.0.1".into());
        cfg.email.smtp_port = port;
        cfg.email.security = "none".into();
        cfg.email.from = Some("OpSkills <ops@example.com>".into());
        cfg.server.public_url = Some("https://ops.example.com/".into());
    });
    let c = sign_in(&s).await;
    let settings = call(&s.app, "GET", "/api/v1/notifications/settings", None, Some(&c), false).await.json();
    assert_eq!((settings["email"].clone(), settings["from"].as_str()), (json!(true), Some("OpSkills <ops@example.com>")));
    let t = call(&s.app, "POST", "/api/v1/notifications/test", Some(json!({"email": ["oncall@example.com"]})), Some(&c), true).await;
    assert_eq!(t.status, StatusCode::OK, "{}", t.body);
    let rb = create(&s, &c, json!({
        "name": "Rotate certs",
        "inputs": [{"name": "env", "type": "text", "default": "prod"}],
        "steps": [{"name": "rotate", "run": "echo rotated"}],
        "notifications": [{"on": ["succeeded"], "email": ["oncall@example.com", "lead@example.com"]}]
    }))
    .await;
    let run = call(&s.app, "POST", &format!("/api/v1/runbooks/{rb}/runs"), None, Some(&c), true).await.json();
    let id = run["id"].as_str().unwrap().to_string();
    wait_finished(&s, &c, &id).await;
    let list = notices(&s, &c, &id, 1).await;
    assert_eq!((list[0]["ok"].clone(), list[0]["target"].as_str()), (json!(true), Some("oncall@example.com, lead@example.com")), "{list:?}");
    let got = mail.lock().unwrap().clone();
    assert_eq!(got.len(), 2);
    assert!(got[0].contains("Subject: [OpSkills] Test notification"));
    // The body is quoted-printable: undo soft line breaks and the encoded "=".
    let m = &got[1].replace("=\n", "").replace("=3D", "=");
    assert!(m.contains("Subject: [OpSkills] Rotate certs succeeded (run #"), "{m}");
    assert!(m.contains("To: oncall@example.com, lead@example.com") || m.contains("To: oncall@example.com,"), "{m}");
    assert!(m.contains(&format!("https://ops.example.com/#/runs/{id}")) && m.contains("env = prod"), "{m}");
}

#[tokio::test]
async fn email_can_be_set_up_on_the_settings_page_with_a_vault_password() {
    let (port, mail, auths) = fake_smtp_with_auth().await;
    let s = server();
    let c = sign_in(&s).await;
    let get = || call(&s.app, "GET", "/api/v1/settings/email", None, Some(&c), false);
    assert_eq!(get().await.json(), json!({"source": "off", "settings": null}));
    let settings = json!({"smtp_host": "127.0.0.1", "smtp_port": port, "security": "none", "username": "ops", "password_secret": "SMTP_PASSWORD", "from": "OpSkills <ops@example.com>"});
    let r = call(&s.app, "PUT", "/api/v1/settings/email", Some(settings.clone()), Some(&c), true).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "the secret isn't in the vault yet: {}", r.body);
    for bad in [json!({"smtp_host": "", "from": "a@example.com"}), json!({"smtp_host": "smtp.example.com", "from": "nobody"}), json!({"smtp_host": "smtp.example.com", "from": "a@example.com", "username": "ops"}), json!({"smtp_host": "smtp.example.com", "from": "a@example.com", "security": "ssl"})] {
        assert_eq!(call(&s.app, "PUT", "/api/v1/settings/email", Some(bad.clone()), Some(&c), true).await.status, StatusCode::BAD_REQUEST, "{bad}");
    }
    add_secret(&s, &c, "SMTP_PASSWORD", "mail-pass-123").await;
    let r = call(&s.app, "PUT", "/api/v1/settings/email", Some(settings), Some(&c), true).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    let shown = get().await;
    assert_eq!(shown.json()["source"], json!("settings"));
    assert!(!shown.body.contains("mail-pass-123"), "{}", shown.body);
    let ns = call(&s.app, "GET", "/api/v1/notifications/settings", None, Some(&c), false).await.json();
    assert_eq!((ns["email"].clone(), ns["source"].as_str()), (json!(true), Some("settings")));

    let t = call(&s.app, "POST", "/api/v1/notifications/test", Some(json!({"email": ["oncall@example.com"]})), Some(&c), true).await;
    assert_eq!(t.status, StatusCode::OK, "{}", t.body);
    assert_eq!(mail.lock().unwrap().len(), 1);
    // It signed in with the password from the vault (AUTH PLAIN of "\0ops\0mail-pass-123").
    let auth = auths.lock().unwrap().clone();
    assert!(auth.iter().any(|l| l.contains("AG9wcwBtYWlsLXBhc3MtMTIz")), "{auth:?}");

    // The password secret can't be deleted while email uses it.
    let del = call(&s.app, "DELETE", "/api/v1/secrets/SMTP_PASSWORD", None, Some(&c), true).await;
    assert_eq!(del.status, StatusCode::CONFLICT, "{}", del.body);
    assert_eq!(call(&s.app, "DELETE", "/api/v1/settings/email", None, Some(&c), true).await.status, StatusCode::NO_CONTENT);
    assert_eq!(get().await.json()["source"], json!("off"));
    assert_eq!(call(&s.app, "DELETE", "/api/v1/secrets/SMTP_PASSWORD", None, Some(&c), true).await.status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn email_password_can_be_a_shared_secret() {
    let (port, mail, auths) = fake_smtp_with_auth().await;
    let s = server();
    let c = sign_in(&s).await;
    let settings = |secret: &str| json!({"smtp_host": "127.0.0.1", "smtp_port": port, "security": "none", "username": "ops", "password_secret": secret, "from": "OpSkills <ops@example.com>"});
    let r = call(&s.app, "PUT", "/api/v1/settings/email", Some(settings("shared/mail/SMTP_PASSWORD")), Some(&c), true).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.body);
    assert!(r.body.contains("no shared secret named mail/SMTP_PASSWORD"), "{}", r.body);
    // Shared with no workspace: only the server uses it.
    let r = call(&s.app, "POST", "/api/v1/shared-secrets", Some(json!({"name": "mail/SMTP_PASSWORD", "value": "mail-pass-123"})), Some(&c), true).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
    let r = call(&s.app, "PUT", "/api/v1/settings/email", Some(settings("shared/mail/SMTP_PASSWORD")), Some(&c), true).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    let t = call(&s.app, "POST", "/api/v1/notifications/test", Some(json!({"email": ["oncall@example.com"]})), Some(&c), true).await;
    assert_eq!(t.status, StatusCode::OK, "{}", t.body);
    assert_eq!(mail.lock().unwrap().len(), 1);
    assert!(auths.lock().unwrap().iter().any(|l| l.contains("AG9wcwBtYWlsLXBhc3MtMTIz")), "signed in with the shared password");
    let shared = call(&s.app, "GET", "/api/v1/shared-secrets", None, Some(&c), false).await.json();
    assert_eq!(shared[0]["email_password"], true);
    let del = call(&s.app, "DELETE", "/api/v1/shared-secrets/mail/SMTP_PASSWORD", None, Some(&c), true).await;
    assert_eq!(del.status, StatusCode::CONFLICT);
    assert!(del.body.contains("Settings > Email"), "{}", del.body);
}

#[tokio::test]
async fn email_in_the_config_file_wins_and_is_read_only() {
    let s = server_with(|cfg| {
        cfg.email.smtp_host = Some("smtp.example.com".into());
        cfg.email.from = Some("OpSkills <ops@example.com>".into());
        cfg.email.username = Some("ops".into());
        cfg.email.password = Some("from-config-file".into());
    });
    let c = sign_in(&s).await;
    let got = call(&s.app, "GET", "/api/v1/settings/email", None, Some(&c), false).await;
    assert_eq!((got.json()["source"].as_str(), got.json()["settings"]["smtp_host"].as_str()), (Some("config"), Some("smtp.example.com")));
    assert!(!got.body.contains("from-config-file"));
    let put = call(&s.app, "PUT", "/api/v1/settings/email", Some(json!({"smtp_host": "other.example.com", "from": "a@example.com"})), Some(&c), true).await;
    assert_eq!(put.status, StatusCode::CONFLICT);
}

/// Records the headers of each POST.
async fn header_receiver() -> (String, std::sync::Arc<std::sync::Mutex<Vec<axum::http::HeaderMap>>>) {
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let s2 = seen.clone();
    let app = Router::new().fallback(move |headers: axum::http::HeaderMap| {
        let seen = s2.clone();
        async move {
            seen.lock().unwrap().push(headers);
            StatusCode::OK
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}/events"), seen)
}

#[tokio::test]
async fn webhook_notifications_send_custom_headers_and_sign_in() {
    let (url, seen) = header_receiver().await;
    let s = server();
    let c = sign_in(&s).await;
    add_secret(&s, &c, "API_KEY", "k-123").await;
    add_secret(&s, &c, "HOOK_PASS", "p@ss:word").await;
    let rb = |n: Value| json!({"name": "H", "steps": [{"name": "a", "run": "true"}], "notifications": [n]});
    for bad in [
        json!({"on": ["failed"], "email": ["a@example.com"], "headers": [{"name": "X-A", "value": "b"}]}),
        json!({"on": ["failed"], "webhook": url, "headers": [{"name": "Content-Type", "value": "text/plain"}]}),
        json!({"on": ["failed"], "webhook": url, "headers": [{"name": "X Bad", "value": "b"}]}),
        json!({"on": ["failed"], "webhook": url, "headers": [{"name": "X-A", "value": "{{ inputs.x }}"}]}),
        json!({"on": ["failed"], "webhook": url, "headers": [{"name": "X-A", "value": "{{ secrets.NOT_THERE }}"}]}),
        json!({"on": ["failed"], "webhook": url, "headers": [{"name": "Authorization", "value": "x"}], "auth": {"type": "bearer", "secret": "API_KEY"}}),
        json!({"on": ["failed"], "webhook": url, "auth": {"type": "basic", "username": "a:b", "secret": "HOOK_PASS"}}),
    ] {
        let r = call(&s.app, "POST", "/api/v1/runbooks", Some(rb(bad.clone())), Some(&c), true).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{bad} -> {}", r.body);
    }
    let made = call(&s.app, "POST", "/api/v1/runbooks", Some(rb(json!({
        "on": ["succeeded"], "webhook": url,
        "headers": [{"name": "X-Api-Key", "value": "key-{{ secrets.API_KEY }}"}, {"name": "X-Team", "value": "ops"}],
        "auth": {"type": "basic", "username": "opskills", "secret": "HOOK_PASS"}
    }))), Some(&c), true).await;
    assert_eq!(made.status, StatusCode::CREATED, "{}", made.body);
    assert!(!made.body.contains("k-123"), "only the reference is stored");
    let rb_id = made.json()["id"].as_str().unwrap().to_string();
    let run = call(&s.app, "POST", &format!("/api/v1/runbooks/{rb_id}/runs"), None, Some(&c), true).await.json();
    let id = run["id"].as_str().unwrap().to_string();
    wait_finished(&s, &c, &id).await;
    let list = notices(&s, &c, &id, 1).await;
    assert_eq!(list[0]["ok"], json!(true), "{list:?}");
    let h = wait_for(|| seen.lock().unwrap().first().cloned(), "the webhook call").await;
    let get = |k: &str| h.get(k).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    assert_eq!((get("x-api-key"), get("x-team")), ("key-k-123".to_string(), "ops".to_string()));
    // base64("opskills:p@ss:word")
    assert_eq!(get("authorization"), "Basic b3Bza2lsbHM6cEBzczp3b3Jk");

    // Bearer, through the test endpoint.
    let t = call(&s.app, "POST", "/api/v1/notifications/test", Some(json!({"webhook": url, "auth": {"type": "bearer", "secret": "API_KEY"}})), Some(&c), true).await;
    assert_eq!(t.status, StatusCode::OK, "{}", t.body);
    let h = wait_for(|| seen.lock().unwrap().get(1).cloned(), "the test call").await;
    assert_eq!(h.get("authorization").unwrap(), "Bearer k-123");

    // Secrets a notification uses can't be deleted.
    let del = call(&s.app, "DELETE", "/api/v1/secrets/HOOK_PASS", None, Some(&c), true).await;
    assert_eq!(del.status, StatusCode::CONFLICT, "{}", del.body);
}

#[tokio::test]
async fn webhook_notifications_can_send_a_custom_body() {
    let (base, seen) = receiver().await;
    let s = server();
    let c = sign_in(&s).await;
    add_secret(&s, &c, "ACME_ROUTING_KEY", "R0UT1NG").await;
    let rb = |n: Value| json!({"name": "B", "steps": [{"name": "a", "run": "true"}], "notifications": [n]});
    for bad in [
        json!({"on": ["failed"], "webhook": base, "body": "{not json"}),
        json!({"on": ["failed"], "webhook": base, "body": "\"just a string\""}),
        json!({"on": ["failed"], "webhook": base, "body": "{\"a\": \"{{ run.nope }}\"}"}),
        json!({"on": ["failed"], "webhook": base, "body": "{\"a\": \"{{ secrets.MISSING }}\"}"}),
        json!({"on": ["failed"], "email": ["a@example.com"], "body": "{}"}),
    ] {
        let r = call(&s.app, "POST", "/api/v1/runbooks", Some(rb(bad.clone())), Some(&c), true).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{bad} -> {}", r.body);
    }
    let template = r#"{
      "routing_key": "{{ secrets.ACME_ROUTING_KEY }}",
      "event_action": "trigger",
      "dedup_key": "opskills-{{ runbook.id }}",
      "payload": {
        "summary": "{{ text }}",
        "source": "opskills",
        "severity": "error",
        "custom_details": {
          "exit_code": "{{ run.failed_step.exit_code }}",
          "env": "{{ run.inputs.env }}",
          "output": "{{ output_tail }}",
          "output_text": "last lines: {{ output_tail }}",
          "error": "{{ run.error }}"
        }
      }
    }"#;
    let made = call(&s.app, "POST", "/api/v1/runbooks", Some(json!({
        "name": "Payments",
        "inputs": [{"name": "env", "type": "text", "default": "prod"}],
        "steps": [{"name": "charge", "run": "echo 'card declined'; exit 4"}],
        "notifications": [{"on": ["failed"], "webhook": format!("{base}/v2/enqueue"), "body": template}]
    })), Some(&c), true).await;
    assert_eq!(made.status, StatusCode::CREATED, "{}", made.body);
    let rb_id = made.json()["id"].as_str().unwrap().to_string();
    let run = call(&s.app, "POST", &format!("/api/v1/runbooks/{rb_id}/runs"), None, Some(&c), true).await.json();
    let id = run["id"].as_str().unwrap().to_string();
    wait_finished(&s, &c, &id).await;
    assert_eq!(notices(&s, &c, &id, 1).await[0]["ok"], json!(true));
    let got = wait_for(|| seen.lock().unwrap().iter().find(|(p, _, _)| p == "/v2/enqueue").cloned(), "the custom body").await;
    let b = got.2;
    assert_eq!(b["routing_key"], json!("R0UT1NG"));
    assert_eq!(b["dedup_key"], json!(format!("opskills-{rb_id}")));
    assert!(b["payload"]["summary"].as_str().unwrap().starts_with("Payments failed: run #"), "{b}");
    let d = &b["payload"]["custom_details"];
    assert_eq!(d["exit_code"], json!(4), "a lone placeholder keeps its type");
    assert_eq!(d["env"], json!("prod"));
    assert!(d["output"].as_array().unwrap().iter().any(|l| l == "card declined"), "{d}");
    assert!(d["output_text"].as_str().unwrap().starts_with("last lines: ") && d["output_text"].as_str().unwrap().contains("card declined\n"), "{d}");
    assert_eq!(d["error"], Value::Null);
    assert!(b.get("text").is_none(), "only the custom body is sent");

    // The test button renders the template with example values.
    let t = call(&s.app, "POST", "/api/v1/notifications/test", Some(json!({"webhook": format!("{base}/test"), "body": "{\"summary\": \"{{ text }}\", \"code\": \"{{ run.failed_step.exit_code }}\"}"})), Some(&c), true).await;
    assert_eq!(t.status, StatusCode::OK, "{}", t.body);
    let tb = wait_for(|| seen.lock().unwrap().iter().find(|(p, _, _)| p == "/test").cloned(), "the test body").await.2;
    assert_eq!(tb["code"], json!(1));
    assert!(tb["summary"].as_str().unwrap().contains("test notification"));
    // A secret used in a body can't be deleted.
    assert_eq!(call(&s.app, "DELETE", "/api/v1/secrets/ACME_ROUTING_KEY", None, Some(&c), true).await.status, StatusCode::CONFLICT);
}
