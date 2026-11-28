//! Hosts over SSH, with a fake `ssh` and `ssh-keyscan` that check host keys and
//! credentials the way OpenSSH does, then run the remote shell here.

use base64::Engine;

use super::*;

/// A test SSH world: the folder the fake programs read hosts from, and the server.
struct Ssh {
    dir: std::path::PathBuf,
    s: TestServer,
    c: String,
}

const KEY_TEXT: &str = "-----BEGIN OPENSSH PRIVATE KEY-----\nfake-key-material-0123456789\n-----END OPENSSH PRIVATE KEY-----";

/// A well-formed ed25519 public key, different for each `n`.
fn host_key(n: u8) -> String {
    let mut blob = vec![0, 0, 0, 11];
    blob.extend(b"ssh-ed25519");
    blob.extend([0, 0, 0, 32]);
    blob.extend([n; 32]);
    format!("ssh-ed25519 {}", base64::engine::general_purpose::STANDARD.encode(blob))
}

fn fp(key: &str) -> String {
    crate::model::fingerprint(key).unwrap()
}

fn script(path: &std::path::Path, body: &str) {
    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
}

impl Ssh {
    async fn new() -> Ssh {
        let dir = std::env::temp_dir().join(format!("opskills-ssh-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.display().to_string();
        script(
            &dir.join("ssh"),
            &format!(
                r#"#!/bin/sh
F='{f}'
printf '%s\n' "$*" >> "$F/args.log"
known=""; alias=""; key=""; port=""; user=""
while [ $# -gt 0 ]; do
  case "$1" in
    -o) case "$2" in UserKnownHostsFile=*) known="${{2#UserKnownHostsFile=}}";; HostKeyAlias=*) alias="${{2#HostKeyAlias=}}";; esac; shift 2;;
    -i) key="$2"; shift 2;;
    -p) port="$2"; shift 2;;
    -l) user="$2"; shift 2;;
    -F) shift 2;;
    --) shift; break;;
    *) shift;;
  esac
done
host="$1"; shift
remote="$*"
[ -f "$F/$host.down" ] && {{ echo "ssh: connect to host $host port $port: Connection refused" >&2; exit 255; }}
if ! grep -qxF "$alias $(cat "$F/$host.hostkey")" "$known"; then
  echo "@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@" >&2
  echo "@    WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!     @" >&2
  echo "Host key verification failed." >&2
  exit 255
fi
if [ -n "$key" ]; then
  cmp -s "$key" "$F/$host.authorized" || {{ echo "$user@$host: Permission denied (publickey)." >&2; exit 255; }}
else
  [ "$("$SSH_ASKPASS" "$user@$host's password: ")" = "$(cat "$F/$host.password")" ] || {{ echo "$user@$host: Permission denied (password)." >&2; exit 255; }}
fi
echo "$user" > "$F/$host.user"
case "$remote" in
  "exec sudo -n -H /bin/sh -s") remote="exec /bin/sh -s"; echo yes > "$F/$host.sudo";;
esac
mkdir -p "$F/home-$host"
cd "$F/home-$host" && HOME="$F/home-$host" exec /bin/sh -c "$remote"
"#
            ),
        );
        script(
            &dir.join("ssh-keyscan"),
            &format!(
                r##"#!/bin/sh
F='{f}'
while [ $# -gt 0 ]; do case "$1" in -T|-p) shift 2;; --) shift; break;; *) break;; esac; done
[ -f "$F/$1.down" ] && {{ echo "$1: Connection refused" >&2; exit 1; }}
echo "# $1:22 SSH-2.0-OpenSSH_9.6"
echo "$1 $(cat "$F/$1.hostkey")"
"##
            ),
        );
        script(&dir.join("askpass"), "#!/bin/sh\nprintf '%s\\n' \"$OPSKILLS_SSH_PASSWORD\"\n");
        let d = dir.clone();
        let s = server_with(move |cfg| {
            cfg.ssh.client = d.join("ssh").display().to_string();
            cfg.ssh.keyscan = d.join("ssh-keyscan").display().to_string();
            cfg.ssh.askpass = d.join("askpass").display().to_string();
        });
        let c = sign_in(&s).await;
        Ssh { dir, s, c }
    }

    /// A host the fake SSH answers for, presenting `key`.
    fn host(&self, name: &str, key: &str) {
        std::fs::write(self.dir.join(format!("{name}.hostkey")), key).unwrap();
        std::fs::write(self.dir.join(format!("{name}.authorized")), format!("{KEY_TEXT}\n")).unwrap();
    }

    async fn source(&self, ssh: Value, hosts: Value) -> Value {
        let r = call(&self.s.app, "POST", "/api/v1/inventory/sources", Some(json!({"name": "Fleet", "type": "static", "ssh": ssh, "hosts": hosts})), Some(&self.c), true).await;
        assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
        r.json()
    }

    async fn host_view(&self, name: &str) -> Value {
        let list = call(&self.s.app, "GET", "/api/v1/hosts", None, Some(&self.c), false).await.json();
        list.as_array().unwrap().iter().find(|h| h["name"] == name).cloned().unwrap_or_else(|| panic!("no host {name}: {list}"))
    }

    async fn run(&self, steps: Value) -> (Value, Vec<String>) {
        let rb = create(&self.s, &self.c, json!({"name": format!("rb-{}", uuid::Uuid::now_v7()), "targets": {"hosts": [], "tags": ["ssh"], "filter": ""}, "rollout": {"mode": "sequential", "on_host_failure": "continue"}, "steps": steps})).await;
        let run = start(&self.s, &self.c, &rb).await;
        let done = wait_finished(&self.s, &self.c, &run).await;
        (done, log_lines(&self.s, &self.c, &run).await)
    }

    fn args_log(&self) -> String {
        std::fs::read_to_string(self.dir.join("args.log")).unwrap_or_default()
    }

    async fn audit_actions(&self) -> Vec<String> {
        let a = call(&self.s.app, "GET", "/api/v1/audit?action=host.ssh", None, Some(&self.c), false).await.json();
        a["events"].as_array().unwrap().iter().map(|e| e["action"].as_str().unwrap().to_string()).collect()
    }
}

impl Drop for Ssh {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[tokio::test]
async fn runbooks_run_on_hosts_over_ssh_and_pin_their_keys() {
    let t = Ssh::new().await;
    t.host("web-9.internal", &host_key(1));
    assert_eq!(add_secret(&t.s, &t.c, "ssh/OPS_KEY", KEY_TEXT).await.status, StatusCode::CREATED);
    assert_eq!(add_secret(&t.s, &t.c, "DB_PASSWORD", "hunter2-db-secret").await.status, StatusCode::CREATED);
    t.source(json!({"user": "ops", "secret": "ssh/OPS_KEY"}), json!([{"name": "web-9", "hostname": "web-9.internal", "tags": ["ssh"], "attributes": {"env": "prod"}}])).await;
    let h = t.host_view("web-9").await;
    assert_eq!((h["reach"].as_str(), h["ssh"]["user"].as_str(), h["ssh"]["sudo"].as_bool()), (Some("ssh"), Some("ops"), Some(true)), "{h}");
    assert_eq!(h["ssh"]["fingerprints"], json!([]));

    let out = t.dir.join("out.txt").display().to_string();
    let rb = create(&t.s, &t.c, json!({"name": "Deploy", "secrets": ["DB_PASSWORD"], "targets": {"hosts": [], "tags": ["ssh"], "filter": ""}, "steps": [
        {"name": "env", "run": "echo env={{ host.attr.env }} pw=$DB_PASSWORD step=$OPSKILLS_STEP; pwd"},
        {"name": "copy", "copy": {"to": out, "secret": "DB_PASSWORD", "mode": "0600"}},
        {"name": "fail", "run": "echo bye >&2; exit 7", "continue_on_error": true},
    ]})).await;
    let run = start(&t.s, &t.c, &rb).await;
    let done = wait_finished(&t.s, &t.c, &run).await;
    let lines = log_lines(&t.s, &t.c, &run).await;
    assert_eq!(done["status"], "succeeded", "{lines:?}");
    let all = lines.join("\n");
    assert!(all.contains("env=prod pw=•••••• step=1"), "{all}");
    assert!(all.contains(&format!("Pinned web-9's SSH host key on first contact: {}.", fp(&host_key(1)))), "{all}");
    assert!(all.contains(&format!("/home-web-9.internal/.opskills/work/{run}")), "the step runs in the user's work folder: {all}");
    assert!(all.contains("bye"), "stderr after the step starts is the step's: {all}");
    assert_eq!(done["steps"][2]["exit_code"], 7);
    assert_eq!(std::fs::read_to_string(&out).unwrap(), "hunter2-db-secret");
    assert!(!t.dir.join(format!("home-web-9.internal/.opskills/work/{run}")).exists(), "the work folder goes after the last step");
    assert_eq!(std::fs::read_to_string(t.dir.join("web-9.internal.sudo")).unwrap().trim(), "yes");
    // Nothing secret is on a command line.
    let args = t.args_log();
    assert!(args.contains("-l ops") && args.contains("-- web-9.internal exec sudo -n -H /bin/sh -s"), "{args}");
    assert!(!args.contains("fake-key-material") && !args.contains("hunter2"), "{args}");
    assert_eq!(done["hosts"][0]["ssh"]["address"], "web-9.internal");

    let h = t.host_view("web-9").await;
    assert_eq!(h["ssh"]["fingerprints"], json!([fp(&host_key(1))]));
    assert_eq!((h["online"].as_bool(), h["ssh"]["pinned_by"].as_str()), (Some(true), Some("first connection")));
    assert_eq!(t.audit_actions().await, ["host.ssh_key_pin"]);

    // Runbooks can name it like an enrolled host.
    let id = h["id"].as_str().unwrap();
    let named = create(&t.s, &t.c, json!({"name": "Named", "targets": {"hosts": [id], "tags": [], "filter": ""}, "steps": [{"name": "a", "run": "true"}]})).await;
    let run = start(&t.s, &t.c, &named).await;
    assert_eq!(wait_finished(&t.s, &t.c, &run).await["status"], "succeeded");
}

#[tokio::test]
async fn a_changed_host_key_is_refused_until_an_admin_accepts_it() {
    let t = Ssh::new().await;
    t.host("db-9", &host_key(1));
    add_secret(&t.s, &t.c, "OPS_KEY", KEY_TEXT).await;
    t.source(json!({"user": "root", "secret": "OPS_KEY"}), json!([{"name": "db-9", "tags": ["ssh"]}])).await;
    assert_eq!(t.run(json!([{"name": "a", "run": "true"}])).await.0["status"], "succeeded");
    assert!(!t.dir.join("db-9.sudo").exists(), "root needs no sudo");

    // The host now presents another key: nothing runs, retries don't help, it's audited once.
    t.host("db-9", &host_key(2));
    for _ in 0..2 {
        let (done, lines) = t.run(json!([{"name": "a", "run": "touch should-not-run", "retries": 2}])).await;
        assert_eq!(done["status"], "failed");
        assert_eq!(done["steps"][0]["attempts"], 1);
        assert!(lines.join("\n").contains("no longer the one pinned for it"), "{lines:?}");
    }
    assert!(!t.dir.join("home-db-9/.opskills").exists() || std::fs::read_dir(t.dir.join("home-db-9/.opskills/work")).map(|d| d.count()).unwrap_or(0) == 0);
    let h = t.host_view("db-9").await;
    assert!(h["ssh"]["key_changed_at"].is_u64() && h["online"] == false, "{h}");
    assert_eq!(t.audit_actions().await, ["host.ssh_key_changed", "host.ssh_key_pin"]);

    let id = h["id"].as_str().unwrap().to_string();
    let keys = call(&t.s.app, "GET", &format!("/api/v1/hosts/{id}/ssh/keys"), None, Some(&t.c), false).await.json();
    assert_eq!(keys["pinned"], json!([fp(&host_key(1))]));
    assert_eq!(keys["presented"][0]["fingerprint"], fp(&host_key(2)));
    let path = format!("/api/v1/hosts/{id}/ssh/accept");
    let accept = |f: String| call(&t.s.app, "POST", &path, Some(json!({"fingerprints": [f]})), Some(&t.c), true);
    assert_eq!(accept(fp(&host_key(1))).await.status, StatusCode::CONFLICT, "only a key it presents now");
    let r = accept(fp(&host_key(2))).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(t.run(json!([{"name": "a", "run": "true"}])).await.0["status"], "succeeded");
    let h = t.host_view("db-9").await;
    assert_eq!((h["ssh"]["fingerprints"].clone(), h["ssh"]["pinned_by"].as_str(), h["ssh"]["key_changed_at"].clone()), (json!([fp(&host_key(2))]), Some("admin"), Value::Null));
    assert_eq!(t.audit_actions().await[0], "host.ssh_key_accept");
}

#[tokio::test]
async fn keys_a_source_gives_are_checked_before_anything_connects() {
    let t = Ssh::new().await;
    t.host("app-1", &host_key(2));
    t.host("app-2", &host_key(3));
    add_secret(&t.s, &t.c, "OPS_KEY", KEY_TEXT).await;
    t.source(
        json!({"secret": "OPS_KEY"}),
        json!([
            {"name": "app-1", "tags": ["ssh"], "ssh": {"host_keys": [fp(&host_key(1))]}},
            {"name": "app-2", "tags": ["ssh"], "ssh": {"host_keys": [host_key(3)], "port": 2222, "user": "deploy"}},
        ]),
    )
    .await;
    let (done, lines) = t.run(json!([{"name": "a", "run": "echo ran-on-$(cat /dev/null)host"}])).await;
    assert_eq!(done["status"], "failed");
    let all = lines.join("\n");
    assert!(all.contains("don't match the ones its inventory source gives"), "{all}");
    let args = t.args_log();
    assert!(!args.contains("app-1") && args.contains("-p 2222 -l deploy -- app-2"), "app-1 is never connected to: {args}");
    assert_eq!(t.host_view("app-1").await["ssh"]["fingerprints"], json!([]));
    assert_eq!(t.host_view("app-2").await["ssh"]["pinned_by"], "inventory source");
}

#[tokio::test]
async fn password_hosts_connection_failures_and_the_test_button() {
    let t = Ssh::new().await;
    t.host("pw-1", &host_key(4));
    std::fs::write(t.dir.join("pw-1.password"), "s3cret-pass-word").unwrap();
    add_secret(&t.s, &t.c, "ssh/PW", "s3cret-pass-word").await;
    t.source(json!({"user": "admin", "auth": "password", "secret": "ssh/PW", "sudo": false}), json!([{"name": "pw-1", "tags": ["ssh"]}])).await;
    let id = t.host_view("pw-1").await["id"].as_str().unwrap().to_string();

    let r = call(&t.s.app, "POST", &format!("/api/v1/hosts/{id}/ssh/test"), None, Some(&t.c), true).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(r.json()["ok"], true, "{}", r.body);
    assert!(r.json()["output"][0].as_str().unwrap().starts_with("Connected as"), "{}", r.body);
    assert_eq!(r.json()["pinned_now"], json!([fp(&host_key(4))]));
    assert!(!t.dir.join("pw-1.sudo").exists());
    assert!(!t.args_log().contains("s3cret"));

    let (done, _) = t.run(json!([{"name": "a", "run": "echo pass=s3cret-pass-word"}])).await;
    assert_eq!(done["status"], "succeeded");

    // A wrong password and a host that's down are failures with SSH's reason.
    std::fs::write(t.dir.join("pw-1.password"), "changed").unwrap();
    let (done, lines) = t.run(json!([{"name": "a", "run": "true"}])).await;
    assert_eq!(done["status"], "failed");
    assert!(lines.join("\n").contains("Permission denied (password)"), "{lines:?}");
    assert!(t.host_view("pw-1").await["ssh"]["last_error"].as_str().unwrap().contains("Permission denied"));
    std::fs::write(t.dir.join("pw-1.down"), "").unwrap();
    let r = call(&t.s.app, "POST", &format!("/api/v1/hosts/{id}/ssh/test"), None, Some(&t.c), true).await.json();
    assert_eq!(r["ok"], false);
    assert!(r["error"].as_str().unwrap().contains("Connection refused"), "{r}");
}

#[tokio::test]
async fn output_is_masked_and_a_cancel_stops_the_remote_step() {
    let t = Ssh::new().await;
    t.host("slow-1", &host_key(5));
    add_secret(&t.s, &t.c, "OPS_KEY", KEY_TEXT).await;
    t.source(json!({"secret": "OPS_KEY"}), json!([{"name": "slow-1", "tags": ["ssh"]}])).await;
    let pid = t.dir.join("slow.pid").display().to_string();
    let rb = create(&t.s, &t.c, json!({"name": "Slow", "targets": {"hosts": [], "tags": ["ssh"], "filter": ""}, "steps": [
        {"name": "a", "run": format!("echo $$ > {pid}; echo started; exec sleep 30")},
    ]})).await;
    let run = start(&t.s, &t.c, &rb).await;
    // Wait for the remote step itself (stored output lines are written in batches).
    let begun = Instant::now();
    while std::fs::read_to_string(&pid).map(|p| p.trim().is_empty()).unwrap_or(true) {
        assert!(begun.elapsed() < Duration::from_secs(10), "the step never started");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    call(&t.s.app, "POST", &format!("/api/v1/runs/{run}/cancel"), None, Some(&t.c), true).await;
    let done = wait_finished(&t.s, &t.c, &run).await;
    assert_eq!(done["status"], "cancelled");
    let remote: i32 = std::fs::read_to_string(&pid).unwrap().trim().parse().unwrap();
    let gone = Instant::now();
    // SAFETY: signal 0 only checks that the process exists.
    while unsafe { libc::kill(remote, 0) } == 0 {
        assert!(gone.elapsed() < Duration::from_secs(10), "the remote step kept running");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    // The key never shows in output, even if a step prints it.
    let (_, lines) = t.run(json!([{"name": "a", "run": format!("echo '{}'", KEY_TEXT.replace('\n', " "))}])).await;
    assert!(!lines.join("\n").contains("fake-key-material"), "{lines:?}");
}

#[tokio::test]
async fn ssh_settings_are_checked() {
    let t = Ssh::new().await;
    for (ssh, hosts, expect) in [
        (json!({"secret": "MISSING"}), json!([]), "no secret named MISSING"),
        (json!({"secret": "X", "user": "-oProxyCommand=sh"}), json!([]), "is not an SSH user name"),
        (json!({"secret": "X", "auth": "certificate"}), json!([]), "unknown variant"),
        (json!({"secret": "X", "jump": "bastion"}), json!([]), "unknown field"),
        (json!({"secret": "X"}), json!([{"name": "a", "ssh": {"host_keys": ["not a key"]}}]), "is not a public key"),
        (json!({"secret": "X"}), json!([{"name": "a", "ssh": {"password": "x"}}]), "unknown field"),
    ] {
        add_secret(&t.s, &t.c, "X", "x").await;
        let r = call(&t.s.app, "POST", "/api/v1/inventory/sources", Some(json!({"name": "Bad", "type": "static", "ssh": ssh, "hosts": hosts})), Some(&t.c), true).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{ssh}: {}", r.body);
        assert!(r.body.contains(expect), "{expect}: {}", r.body);
    }
    // A host whose name can't be an SSH address stays listed.
    add_secret(&t.s, &t.c, "OPS_KEY", KEY_TEXT).await;
    t.source(json!({"secret": "OPS_KEY"}), json!([{"name": "-oProxyCommand=sh"}])).await;
    assert_eq!(t.host_view("-oProxyCommand=sh").await["reach"], "listed");
}
