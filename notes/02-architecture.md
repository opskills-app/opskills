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

v0.2.0 stores runbooks, runs, run output and sessions in one embedded database
file, `opskills.redb`, using redb (a pure-Rust key-value store). Values are JSON
documents; run IDs are UUID v7, so key order is creation order. Choosing a
pure-Rust store keeps the release build free of C code, so static binaries for
amd64 and arm64 cross-compile with Rust's bundled linker alone.

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
