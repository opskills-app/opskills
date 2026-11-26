# OpSkills architecture

The domain model, API, extension points, and interface of OpSkills.

## Vocabulary

| Term | Meaning |
|---|---|
| **Workspace** | A team's space: runbooks, hosts, secrets, access rules and history |
| **Runbook** | A named, versioned procedure made of steps, with inputs, targets and triggers |
| **Step** | One action in a runbook: run a command or script, copy a file, call HTTP, wait for approval, call another runbook |
| **Run** | One execution of a runbook, with inputs, per-host results and a live log |
| **Host** | A machine or endpoint a step can run on, with tags and attributes |
| **Inventory source** | Where hosts come from: a static file, a cloud account, a script, a URL |
| **Rollout** | How a step spreads across hosts: all at once, in batches, a percentage, or canary first |
| **Vault** | Encrypted storage for SSH keys, passwords and tokens |
| **Trigger** | What starts a run: manual, schedule, inbound webhook, API |
| **Policy** | Who can do what, per workspace and resource |

## API

- One HTTP API for both the web app and automation: `/api/v1/...`. The web app has no private endpoints.
- JSON only. Cursor-based pagination. Errors as RFC 9457 problem details.
- OpenAPI document served at `/api/v1/openapi.json`, generated from the Rust types.
- Auth:
  - The web app uses a session cookie (HttpOnly, SameSite=Lax) plus a CSRF header on every write.
  - Automation uses `Authorization: Bearer <token>`.
  - Both go through the same policy checks.
- Live updates (run logs, run status) over Server-Sent Events: `GET /api/v1/runs/{id}/events`.

Core resources:

| Resource | Endpoints |
|---|---|
| Workspaces | `GET/POST /workspaces`, `GET/PATCH/DELETE /workspaces/{ws}` |
| Runbooks | `GET/POST /workspaces/{ws}/runbooks`, `GET/PUT/DELETE .../runbooks/{id}`, `GET .../runbooks/{id}/versions` |
| Runs | `POST .../runbooks/{id}/runs`, `GET /workspaces/{ws}/runs`, `GET /runs/{id}`, `POST /runs/{id}/cancel`, `GET /runs/{id}/events` |
| Hosts | `GET .../hosts?filter=`, `GET/POST .../inventory-sources` |
| Vault | `GET .../vault`, `PUT/DELETE .../vault/{path}` (values are write-only) |
| Triggers | `GET/POST .../triggers`, inbound `POST /hooks/{token}` |
| Policies | `GET/PUT .../policies` |
| Tokens | `GET/POST/DELETE /me/tokens` |
| System | `GET /healthz`, `GET /readyz`, `GET /api/v1/system` |

## Runbook format

Runbooks are stored as YAML, so they can live in Git and be reviewed like code:

```yaml
name: restart-web
description: Rolling restart of the web tier
inputs:
  - name: reason
    required: true
targets:
  filter: "tags:web env:prod"
rollout:
  strategy: batches
  batch_size: 25%
  stop_on_failure: true
steps:
  - name: Drain
    run: sudo systemctl reload nginx-drain
  - name: Restart
    run: sudo systemctl restart app
  - name: Health check
    http:
      url: "http://{{ host.address }}:8080/healthz"
      expect_status: 200
      retries: 5
triggers:
  - schedule: "0 3 * * 1"
```

## Extension points

Plugins are separate executables that speak JSON over stdin and stdout, so they can be written in any language and can't crash the server. Built-in providers are compiled into the binary.

| Extension point | Built-in providers (initial) |
|---|---|
| Executor (run on a host) | SSH, local, container exec, WinRM |
| File transfer | SFTP, local copy |
| Inventory source | Static YAML, script, URL, AWS EC2 |
| Inventory enricher | Tag rules |
| Step type | Command, script, file copy, HTTP request, approval gate, call runbook |
| Output filter | Mask secrets (always on), key-value capture, highlight |
| Output view | Plain, JSON, table, Markdown |
| Rollout strategy | All, batches, percentage, canary |
| Notification | Email, webhook, Slack |
| Inbound trigger | Webhook to run, webhook to log |
| Vault backend | Built-in (AES-256-GCM), external KMS |
| Log storage | Database, S3-compatible |
| Audit sink | Database, stdout |
| Git sync | Import and export runbooks |

## Storage

v0.3.0 stores runbooks, runs, run output, sessions, hosts and enrollment tokens in
one embedded database file, `opskills.redb`, using redb (a pure-Rust key-value
store). Values are JSON documents; run IDs are UUID v7, so key order is creation
order. The agent's HTTPS client (rustls) brings in some C and assembly through its
crypto library, so release builds cross-compile with cargo-zigbuild, which uses
Zig as the C compiler and linker for both musl targets.

Vault secrets are stored in the same file, encrypted with AES-256-GCM under the
server's `master.key`, a fresh random nonce per value and the secret name as
associated data. Runs receive only the secrets their runbook lists, as
environment variables, and the executor masks their values in output.

All access goes through one `Store` type, so a SQL backend (SQLite or
PostgreSQL, needed for clustering) can replace it without touching the API.

## Runs

- A scheduler in the server places runs on a queue. Workers (in-process to start, separate processes later) take runs from it.
- Per-host step results are stored as they finish. Log lines are stored with host, step and timestamp, and streamed to viewers over SSE.
- Cancellation sends a signal to the running step on each host, then marks the run cancelled.
- History cleanup runs daily, keeping at least `min_runs_to_keep` per runbook and deleting runs older than `retention_days`.

Since 0.4.0, runbooks have typed inputs and per-step control:

- Inputs (`text`, `number`, `boolean`, `choice`, `secret`) are checked when a run
  starts. Values travel with the run in memory only; the run record keeps them with
  secret ones masked.
- `{{ scope.name }}` references (`inputs`, `host`, `run`, `runbook`) are expanded
  per host on the server, each into one single-quoted shell word, so the agent
  protocol is unchanged and older agents keep working. Inputs are also
  `OPSKILLS_INPUT_<NAME>` environment variables.
- A step has an optional timeout, `retries` with a delay, and `continue_on_error`.
  The runbook's `on_step_failure` is `stop` or `finish_then_fail`.
- `output_limit` lowers the 20,000-line cap and can fail the run instead of
  truncating; it stops the running step through the same path as a cancel.
- The job message has `keep_on_failure`, so an agent keeps the run's working folder
  when a retry or later step will still come. Older agents ignore the field.
- A runbook's `rollout` (since 0.5.0) is planned up front as waves: the canary, then
  either the rest one by one, the rest in parallel up to `max_parallel`, or batches.
  Hosts of a wave run concurrently in the run's own task; the run record, the output
  sink and the halt flags are shared behind short-lived locks. A failed step stops
  only its own host; `on_host_failure` decides whether hosts that haven't started
  still run.
- Notifications (since 0.10.0) are rules on the runbook (`notifications`). The
  executor reads them when a run starts (`Notifier::rules_for`; called runs don't
  notify), emits `started`, keeps one tokio timer per `slow` rule (aborted when the
  run ends, and re-checked against the stored run before sending) and emits
  `succeeded` or `failed` at the end. Delivery runs on blocking threads: webhooks
  with ureq (10 s timeout, no redirects, two retries on network errors, 5xx and
  429), with optional headers (values may hold `{{ secrets.NAME }}`) and bearer or
  basic sign-in from vault secrets, read at send time and scrubbed from errors,
  and an optional custom `body`: a JSON template checked on save
  (`model::body_vars`) and filled by walking its string values
  (`Notifier::render_body`; a lone placeholder keeps its JSON type), so escaping is
  structural rather than textual;
  email with lettre. SMTP settings come from the `[email]` config section if set,
  else from the `settings` table (key `email`, set on the Settings page), whose
  password is a vault secret named in `password_secret`. Deleting a secret that
  email or a notification uses is refused. Each result is appended
  to the run's entry in the `notices` table in one transaction. Webhook URLs are
  shown and logged by host only, and scrubbed from error text.
- Inbound webhooks (since 0.9.0) are stored apart from runbooks, in the `webhooks`
  table: the SHA-256 of the URL token's secret (`owh_<id>_<secret>`, compared in
  constant time) and, if signing is on, the HMAC secret sealed with the vault key
  (associated data `webhook:<id>`). `POST /hooks/{token}` skips the session and CSRF
  checks. Every change, including recording a delivery, goes through
  `Store::update_webhook`, one read and write in a single transaction, so a delivery
  can't undo a rotation made at the same moment.
- Schedules (since 0.8.0) live on the runbook (`schedules`: cron in an IANA time
  zone, or a one-time `at`). `schedule.rs` uses croner and chrono-tz; a loop started
  by `web::serve` calls `tick(floor, now)` every 5 s on a blocking thread. `tick` fires
  only the latest due time after `max(floor, last_slot)`, where `floor` is when the
  scheduler started (so missed times aren't replayed) and `last_slot` is stored per
  schedule (so restarts don't fire twice). Runs start through `api::start_runbook`,
  shared with the API.
- API tokens (since 0.7.0): `osk_<12 hex id>_<64 hex secret>`, stored by id with the
  SHA-256 of the secret, compared in constant time. The guard accepts `Authorization:
  Bearer` (no CSRF header needed, since no cookie is involved) or a session. Tokens
  can't reach `/me/tokens` or `/auth/*` except `GET /auth/me`, so a leaked token can't
  mint another. Runs record `admin (token <name>)`; last use is written at most once
  a minute.
- Targets add a host filter (since 0.6.0): `filter.rs` parses `key:value` terms with
  globs, `!` and comma alternatives, all of which must match. A run's hosts are the
  union of explicit ids, tags and filter matches; the server is never matched by a
  filter. `POST /runs/{id}/retry` starts a new run on the hosts of a finished run that
  didn't finish well, with `retry_of` set and the earlier non-secret inputs.
- A step is one of `run`, `copy` or `call`. A copy becomes a short shell script on
  the server (temp file, `chmod`, `mv`) with the content in `OPSKILLS_COPY_CONTENT`,
  so agents need nothing new. A call starts a child run of the other runbook on the
  step's host (`parent` and `depth` on the run, `child_run_id` on the step) and waits
  for it; child runs skip the concurrency queue so a parent can't deadlock waiting
  for a slot. Call loops are rejected on save, and depth is capped at 5.

## Hosts and the agent

The agent is the server binary installed as `opskills-agent`; `main` switches on
the program name. Agents only make outgoing requests, to `/api/v1/agent/*`, with
`Authorization: Bearer <host id>.<secret>`. The server stores the SHA-256 of the
secret and compares in constant time.

| Endpoint | Purpose |
|---|---|
| `POST /agent/enroll` | Trade a one-time enrollment token (24 h) for a host id and secret |
| `POST /agent/poll` | Report machine facts, then long-poll up to 25 s for the next step |
| `POST /agent/jobs/{id}/output` | Send output lines (or an empty heartbeat); the reply says whether to cancel |
| `POST /agent/jobs/{id}/finish` | Report how the step ended: exited with a code, timed out, cancelled, or not run |
| `GET /agent/whoami` | Check the credential without taking work (used by `enroll` and `status`) |

On the server, a hub keeps a queue of steps per host and a slot per step in
flight. The executor walks a run's hosts in order; for a host other than the
server it queues the step and follows it:

- A step counts as started only when the agent checks in about it (the agent does
  this as soon as it starts the step). A poll from an agent that just died can take
  a step and lose it; such a step fails after 45 s as not picked up.
- While a step runs, the agent posts output at least once a second. Silence for
  30 s fails the step as lost contact.
- Cancel sets a flag the agent sees in its next output reply. The agent stops the
  step's process group (TERM, then KILL after 5 s) and reports it.
- The server still masks secret values in all output it receives.

The agent runs each step with the same code the server uses for local steps
(`executor::run_command`): `/bin/sh -c`, a cleared environment, its own process
group, timeouts. Work folders are `/var/lib/opskills-agent/work/<run id>`,
removed after the run's last step on that host or after a failing step.

Hosts are online while their agent is polling or was in touch in the last minute.
Removing a host deletes its credential; the agent's next request gets 401 and the
agent exits with code 78, which the systemd unit does not restart.

## Clustering (later)

- Several servers share one PostgreSQL database.
- Each server has its own generated server ID.
- Schedules are claimed with database row locks, so each scheduled run starts exactly once.

## Web app

- Single-page app in TypeScript, embedded in the binary and served at `/`.
- Talks only to the public API.
- Layout: a slim icon rail plus a top bar with workspace switcher and command palette (Cmd+K). Pages: Overview, Runbooks, Runs, Hosts, Vault, Settings.
- The run page is the centerpiece: a live, filterable log, per-host progress, and a step timeline.
- Light and dark themes, keyboard-first, works on tablets.
