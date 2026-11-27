# Writing runbooks

A runbook is a list of steps. A step runs a shell command, copies a file to its
host, or calls another runbook. This guide covers the parts that make a runbook
reusable and safe to run: step types, inputs, run values, step options, and what
happens when a step fails.

Everything here works from the runbook editor in the web app and from the API
(`POST /api/v1/runbooks`, `PUT /api/v1/runbooks/{id}`).

## Step types

Every step has a `name` and does exactly one of these things. Pick the type in the
editor with the switch under the step's name.

### Run a command

```json
{"name": "Restart", "run": "systemctl restart {{ inputs.service }}"}
```

The command runs with `/bin/sh` in the run's working folder.

### Copy a file

Writes a file on the step's host, from text you give or from a vault secret:

```json
{"name": "Write config", "copy": {"to": "/etc/myapp/app.ini", "content": "host={{ host.name }}\n", "mode": "0640"}}
{"name": "Install key", "copy": {"to": "/etc/ssl/private/app.key", "secret": "TLS_KEY"}}
```

| Field | What it is |
|---|---|
| `to` | The file to write. A relative path is inside the run's working folder. Missing folders are created |
| `content` | The text to write, up to 64 KB. `{{ ... }}` run values are filled in as plain text |
| `secret` | Or the name of a vault secret to write, like `prod/tls/TLS_KEY`. The runbook must list it under `secrets` (the editor does this for you) |
| `mode` | File mode in octal. Default `0644`, or `0600` for a secret |

The file is written next to its destination and then moved into place, so nothing
ever reads it half written. Its content travels to the host in an environment
variable, not on the command line, so other users can't see it in the process list.
A secret's value is masked if it appears in the output.

### Call another runbook

Runs another runbook on the same host as this step, as a run of its own, and waits
for it:

```json
{"name": "Health check", "call": {"runbook": "<runbook id>", "inputs": {"port": "8080", "service": "{{ inputs.service }}"}}}
```

`inputs` gives values for the called runbook's inputs; `{{ ... }}` run values of the
calling run are filled in as plain text, and inputs left out use the called
runbook's defaults. The called run uses its own secrets, steps and settings, and its
own run page links back to the run that called it.

- The step succeeds when the called run succeeds. A failed or cancelled call fails
  the step; retries start a new call each time.
- Cancelling the calling run, or reaching the step's timeout, cancels the called run.
- Saving checks that the called runbook exists, that every input you give exists,
  and that every required input without a default gets a value. A runbook can't call
  itself, directly or through others, and calls nest at most 5 deep.
- A runbook that others call can't be deleted until those calls are removed.
- Called runs don't wait for a free run slot (`runs.max_concurrent`); they count as
  part of the run that called them.

## Inputs

Inputs are values that people fill in when they start a run. A runbook with inputs
opens a form when you press **Run**.

| Type | Value | Notes |
|---|---|---|
| `text` | Any text, up to 4096 characters | Optional `pattern`: a regular expression the whole value must match |
| `number` | A number, like `3` or `0.5` | |
| `boolean` | `true` or `false` | Shown as Yes and No |
| `choice` | One of the listed `choices` | 1 to 100 choices |
| `secret` | Text, like a one-off token | Masked in the output and never stored with the run; can't have a default |

Each input has a `name` (lower case letters, digits and underscores, starting with a
letter, like `service_name`), and optionally a `label`, a `description` (help text
under the field), `required` and a `default`. A required input with a default never
has to be filled in.

```json
"inputs": [
  {"name": "service", "label": "Service", "type": "text", "required": true, "pattern": "[a-z0-9-]+"},
  {"name": "region", "type": "choice", "choices": ["eu-west", "us-east"], "default": "eu-west"},
  {"name": "dry_run", "type": "boolean", "default": "true"}
]
```

Start a run with input values through the API:

```bash
curl -X POST https://ops.example.com/api/v1/runbooks/<id>/runs \
  -H "Authorization: Bearer $OPSKILLS_TOKEN" -H 'Content-Type: application/json' \
  -d '{"inputs": {"service": "nginx", "region": "us-east"}}'
```

Unknown names, missing required values and values that don't fit their type are
refused with a 400 error that says which input is wrong. The run page shows the
values a run started with; secret ones appear as `••••••`.

## Using values in commands

There are two ways to use an input, or another run value, in a step.

**Template references** are replaced before the command runs. Each one becomes a
single, already quoted shell word, so a value can never turn into extra commands:

```sh
systemctl restart {{ inputs.service }}
echo "Deploying to" {{ inputs.region }} "on" {{ host.name }}
```

Don't put a reference inside quotes of your own: `"{{ inputs.service }}"` would keep
the quote marks in the value. Write it bare, as above.

| Reference | Value |
|---|---|
| `{{ inputs.<name> }}` | The input's value |
| `{{ host.name }}`, `{{ host.id }}` | The host the step runs on |
| `{{ host.attr.<key> }}` | The host's [attribute](inventory.md#attributes), like `{{ host.attr.env }}`; empty when the host doesn't have it |
| `{{ run.id }}`, `{{ run.started_by }}` | This run, and who started it |
| `{{ runbook.id }}`, `{{ runbook.name }}` | This runbook |

Saving a runbook checks every reference, so a misspelled input name is caught
before anything runs. Other uses of double braces, like the Go templates in
`docker ps --format '{{.Names}}'`, are left as they are.

**Environment variables** are set in every step. Use them inside your own quotes:

| Variable | Value |
|---|---|
| `OPSKILLS_INPUT_<NAME>` | Each input, with the name in capitals, like `$OPSKILLS_INPUT_SERVICE` |
| `OPSKILLS_RUN_ID`, `OPSKILLS_STEP` | This run, and the step's number on this host |
| `OPSKILLS_HOST_ID`, `OPSKILLS_HOST_NAME` | The host the step runs on |
| `OPSKILLS_RUNBOOK_ID`, `OPSKILLS_RUNBOOK_NAME`, `OPSKILLS_STARTED_BY` | This runbook, and who started the run |

Vault secrets the runbook lists are environment variables too, named after the last
part of their name: `prod/db/DB_PASSWORD` is `$DB_PASSWORD`. A runbook uses the secrets
of its own [workspace](workspaces.md) and the shared secrets shared with it
(`shared/...`); a choice input can pick a secret's folder, like
`{{ inputs.env }}/DB_PASSWORD`. See [Vault](vault.md).

## Step options

Each step can change how it runs. In the editor, open **Options** under the step.

| Option | Default | What it does |
|---|---|---|
| `timeout_seconds` | The runbook's step timeout | Stops the step after this long. Seconds, or a duration like `90s`, `10m`, `2h` or `1h30m`; at most 1 day |
| `retries` | `0` | Tries a failed or timed out step again, up to 10 more times. A cancelled run is never retried |
| `retry_delay_seconds` | `0` | Waits between tries. Seconds or a duration; at most 1 hour |
| `continue_on_error` | `false` | If the step still fails, it is marked failed but the run goes on as if it had succeeded |

```json
{"name": "Wait for health", "run": "curl -fsS http://localhost:8080/healthz", "retries": 5, "retry_delay_seconds": "10s"}
```

The runbook's own `timeout_seconds` (the default for every step) accepts the same
duration strings.

## When a step fails

A step fails when it exits with a code other than 0, times out, or can't start (for
example, because its host is offline). Retries come first. If the step still fails
and doesn't continue on error, the runbook's `on_step_failure` setting decides
what happens next:

| `on_step_failure` | Effect |
|---|---|
| `stop` (default) | Every later step is skipped, on this host and on hosts after it. The run fails |
| `finish_then_fail` | Every later step still runs, then the run is marked failed. Use it when the last steps clean up |

On an enrolled host, each run has a working folder that later steps can use. Agents
from 0.4.0 keep it after a failed step when a retry or a later step will still run
there. Older agents remove it after any failed step; upgrade them to keep files
between those steps.

## Schedules

A runbook can run by itself: repeating on a cron expression, or once at a set time.
In the editor, use **Add schedule** under the steps.

```json
"schedules": [
  {"name": "nightly", "cron": "0 3 * * *", "timezone": "Europe/Berlin", "inputs": {"env": "prod"}},
  {"name": "after the freeze", "at": 1798761600000}
]
```

| Field | What it is |
|---|---|
| `cron` | 5 fields: minute, hour, day of month, month, day of week, like `0 9 * * 1-5` for weekdays at 09:00. Names such as `@hourly` and `@daily` work too |
| `at` | Or a one-time run, in epoch milliseconds (the editor takes a date and time) |
| `timezone` | The IANA time zone the cron expression is read in, like `UTC`, `Europe/Berlin` or `America/New_York`. Times follow daylight saving: `0 3 * * *` is 03:00 local all year |
| `inputs` | Values for the runbook's inputs. Secret inputs can't be stored in a schedule, so a runbook with a required secret input can't be scheduled |
| `name` | Optional label; runs it starts show `schedule <name>` as who started them |
| `enabled` | Default `true` |

A runbook can have up to 10 schedules. Runs they start have the trigger `schedule`
and use the runbook's targets, rollout and secrets like any other run.

How the scheduler behaves:

- It checks every few seconds and starts each run once, even across restarts.
- Times that pass while the server isn't running are skipped, not replayed. If the
  scheduler falls behind, it starts one run for the latest due time, not one for
  each missed time.
- A one-time schedule runs once, then shows as done.
- If a run can't start (for example, no host matches the targets), the runbook page
  shows the reason under **Schedules**.

The editor shows the next times as you type. Through the API,
`GET /api/v1/runbooks/{id}/schedules` lists each schedule's next times, last run
and last error, and `POST /api/v1/schedules/preview` with `{"cron": ..., "timezone": ...}`
returns the next five times without saving anything.

## Webhooks

A webhook is a secret URL that another system, like CI, GitHub or a monitoring
tool, calls to start the runbook. On the runbook page, use **New** under
**Webhooks**. The URL is shown once; only a hash of its secret part is stored, so
if you lose it, give the webhook a new URL (the arrows icon) and the old one stops
working.

Calls are a `POST` with a JSON body and need no sign-in:

```bash
curl -X POST -H 'Content-Type: application/json' \
  -d '{"repository": {"name": "shop"}, "after": "9f2c1e7"}' \
  https://ops.example.com/api/v1/hooks/owh_...
```

**Inputs** come from the body. For each runbook input, give a dotted path into
the JSON, like `repository.name`, `after` or `commits.0.id` (array positions are
numbers). Strings are used as they are; numbers, booleans and objects as JSON. An
input with no path, or whose path isn't in the body, gets its default, and a
required input without one fails the call. Values are checked like any other run's
inputs, and expand quoted in commands.

**Signatures.** Turn on **Require a signature** and the server makes a signing
secret, shown once and kept encrypted with the vault key. Each call must then send
the HMAC-SHA256 of the exact body, as hex:

```bash
sig=$(printf %s "$body" | openssl dgst -sha256 -hmac "$secret" | sed 's/.* //')
curl -X POST -H "X-OpSkills-Signature: sha256=$sig" -d "$body" https://ops.example.com/api/v1/hooks/owh_...
```

GitHub sends the same signature as `X-Hub-Signature-256`, which also works. To use
OpSkills from a GitHub repository, add a webhook there with the OpSkills URL,
content type `application/json`, and the signing secret.

| Answer | Why |
|---|---|
| `202` | A run started: `{"run_id": "...", "status": "queued"}` |
| `400` | The body isn't JSON, or an input is missing or not valid |
| `401` | Signing is on and the signature is missing or wrong |
| `404` | No webhook has this URL (also for a URL that was replaced) |
| `409` | The webhook is turned off, or no host matches the runbook's targets |
| `413` | The body is over 256 KB |

The runbook page shows each webhook's last call: the run it started, or why it
didn't. The last 20 calls are kept and returned by
`GET /api/v1/runbooks/{id}/webhooks`. Runs from a webhook have the trigger
`webhook` and show `webhook <name>` as who started them. A runbook can have up to
20 webhooks, and deleting the runbook deletes them.

## Notifications

Tell people about a runbook's runs, by email or in a chat channel. In the editor,
use **Add notification** under the steps, pick when (**Started**, **Succeeded**,
**Failed**, **Running long** with a number of minutes) and where, then **Send a
test** to check it before saving.

```json
"notifications": [
  {"on": ["failed", "slow"], "slow_after_seconds": 1800, "webhook": "https://hooks.slack.com/services/..."},
  {"on": ["failed"], "email": ["oncall@example.com"]}
]
```

| Field | What it is |
|---|---|
| `on` | Any of `started`, `succeeded`, `failed` and `slow` |
| `slow_after_seconds` | With `slow`: how long a run may take before it counts as slow. Sent once per run, while it's still running |
| `webhook` | A URL that gets a JSON `POST` |
| `headers` | Extra headers for the webhook, up to 10: `[{"name": "X-Api-Key", "value": "{{ secrets.ACME_API_KEY }}"}]` |
| `body` | A custom JSON body for the webhook, as a string (see below) |
| `auth` | Sign the webhook call in: `{"type": "bearer", "secret": "HOOK_TOKEN"}` or `{"type": "basic", "username": "ops", "secret": "HOOK_PASSWORD"}` |
| `email` | Or up to 20 addresses; email must be set up first ([how](install.md#sending-email)) |
| `enabled` | Default `true` |

A runbook can have up to 10. Runs that a call step starts don't send their own
notifications; the run that called them does. Cancelled runs send nothing.

**Webhooks** get a JSON body with a one-line `text`, which Slack, Mattermost and
Google Chat incoming webhooks show as a message (and the same line as `content`,
which Discord shows), plus the details for anything else:

```json
{
  "event": "failed",
  "text": "Nightly backup failed: run #2fb870, step \"upload\" on db-1, after 12 seconds. https://ops.example.com/#/runs/...",
  "runbook": {"id": "...", "name": "Nightly backup"},
  "run": {"id": "...", "number": "2fb870", "status": "failed", "trigger": "schedule", "started_by": "schedule nightly",
          "started_at": 1795000000000, "finished_at": 1795000012000, "url": "https://ops.example.com/#/runs/...",
          "failed_step": {"name": "upload", "host": "db-1", "exit_code": 2}, "error": null},
  "output_tail": ["bucket not found", "Step failed with exit code 2."]
}
```

**Headers and sign-in.** For receivers that need a key or a token, add headers
and pick a sign-in. A header value can include a vault secret as
`{{ secrets.NAME }}`; **Bearer token** sends `Authorization: Bearer <secret>`, and
**User name and password** sends basic sign-in with the secret as the password.
Only the secret's name is saved with the runbook; its value is read from the vault
when the message goes out, and kept out of any recorded error. A secret in use
can't be deleted from the vault. `Host`, `Content-Type`, `Content-Length`,
`Connection`, `Transfer-Encoding` and `X-OpSkills-Event` are set by OpSkills.

**Custom body.** Some receivers want their own format, like an alerting service's events API.
Under **Body**, pick **Custom** and write the JSON to send. Any string in it can
use placeholders, which take the same fields as the default body:

| Placeholder | Value |
|---|---|
| `{{ text }}`, `{{ event }}` | The one-line message, and `started`, `succeeded`, `failed` or `slow` |
| `{{ runbook.id }}`, `{{ runbook.name }}` | The runbook |
| `{{ run.id }}`, `{{ run.number }}`, `{{ run.status }}`, `{{ run.trigger }}`, `{{ run.started_by }}`, `{{ run.url }}`, `{{ run.error }}` | The run |
| `{{ run.started_at }}`, `{{ run.finished_at }}`, `{{ run.duration_seconds }}` | Epoch milliseconds, and seconds |
| `{{ run.failed_step.name }}`, `{{ run.failed_step.host }}`, `{{ run.failed_step.exit_code }}` | The step that failed |
| `{{ run.inputs.NAME }}` | An input value (secret inputs are masked) |
| `{{ output_tail }}` | The last 20 output lines of a failed run |
| `{{ secrets.NAME }}` | A vault secret by its full name, like `{{ secrets.prod/ACME_API_KEY }}` or `{{ secrets.shared/ACME_API_KEY }}`, read when the message goes out |

A string that is exactly one placeholder takes the value as it is, so
`"{{ run.failed_step.exit_code }}"` becomes a number and `"{{ output_tail }}"` a
list. Placeholders inside longer text are written in (a list as lines, a missing
value as nothing). Values are always escaped, so the result is valid JSON. The body
is checked when you save: it must be a JSON object or array, at most 16 KB, using
only these placeholders. **Send a test** fills it with example values.

For example, to open an alert in Acme, a made-up alerting service, when a run
fails, and close it when the runbook next succeeds, use two webhook notifications
to Acme's events URL (say `https://events.acme.example/v2/enqueue`), with its key
in the vault as `ACME_ROUTING_KEY`. The first, on **Failed**, can start from
**Start from the Acme example** in the editor:

```json
{
  "routing_key": "{{ secrets.ACME_ROUTING_KEY }}",
  "event_action": "trigger",
  "dedup_key": "opskills-{{ runbook.id }}",
  "payload": {"summary": "{{ text }}", "source": "opskills", "severity": "error",
              "custom_details": {"failed_step": "{{ run.failed_step.name }}", "output": "{{ output_tail }}"}},
  "links": [{"href": "{{ run.url }}", "text": "Open the run in OpSkills"}]
}
```

The second, on **Succeeded**, sends the same key and dedup key with the action
that resolves, like
`{"routing_key": "{{ secrets.ACME_ROUTING_KEY }}", "event_action": "resolve", "dedup_key": "opskills-{{ runbook.id }}"}`.
For a real service, use the URL and field names its API documents. Set `server.public_url`
so `run.url` has a value.

The header `X-OpSkills-Event` names the event. `output_tail` holds the last 20
output lines of a failed run (secret values are already masked) and is empty
otherwise. Links need `server.public_url` set. A receiver that is down or answers
`5xx` or `429` is tried twice more; any other answer outside `2xx` counts as failed.

**Emails** have the same line as the subject and first line, then the runbook,
who started it, how long it took, the inputs (secret ones masked) and, for
failures, the last output lines.

Sending never holds up a run. The run page lists each notification it sent and
whether it got through, with the reason if it didn't. Webhook URLs are often
secret themselves (as Slack's are), so only their host is shown there and in logs.

## Running on several hosts

A runbook that runs on hosts runs every step on each host, in order. The `targets`
decide which hosts, and the `rollout` decides how they take turns.

### Choosing hosts with a filter

`targets` picks hosts three ways, and a run goes to every host any of them picks:
`hosts` (by id, including `local` for the OpSkills server), `tags` (any of them),
and `filter`, an expression checked against every host of the workspace when the run
starts:

```json
"targets": {"filter": "tags:web,api !tags:staging os:debian*"}
```

| Part | Meaning |
|---|---|
| `key:value` | The host's field matches the value. Keys: `tags`, `name`, `hostname`, `os`, `arch`, or an [attribute](inventory.md#attributes) like `env` |
| `key:a,b` | Any of the values matches |
| `*`, `?` | Any run of characters, any one character |
| `!key:value` | The host must not match |
| `value` alone | Short for `name:value` |
| spaces | Separate terms; a host must match every term |

Matching ignores case, values can't contain spaces, and a filter never picks the
OpSkills server. A host without an attribute doesn't match `env:prod` and does match
`!env:prod`. Tags include those from [inventory sources and tag rules](inventory.md).
Hosts an inventory source only lists (no agent yet) are skipped, and the run page
names them. Saving checks the filter. In the editor, the field under **Run on**
shows which hosts it matches as you type; the API offers the same preview with
`GET /api/v1/hosts?filter=<expression>`.

### Rollout

The `rollout` setting decides how the hosts take turns. In the editor it's the **Rollout** card,
shown when **Run on** is set to hosts.

| Field | Default | What it does |
|---|---|---|
| `mode` | `sequential` | `sequential`: one host at a time. `parallel`: many hosts at once. `batches`: a group of hosts at a time; the next group starts when the whole group is done |
| `max_parallel` | `0` (all) | Parallel mode only: the most hosts running at once |
| `batch_size` | | Batches mode only, required: a number of hosts like `5`, or a share like `"25%"` (rounded up, at least 1) |
| `canary` | `0` | Hosts that run first, on their own, before any other host starts. A failed canary with the default failure setting stops the rollout there |
| `pause_seconds` | `0` | Wait after the canary and between batches. Seconds or a duration like `30s`; at most 1 hour |
| `on_host_failure` | follows `on_step_failure` | `stop`: hosts that haven't started are skipped. `continue`: the other hosts still run. Hosts already running always finish |

```json
"rollout": {"mode": "batches", "batch_size": "25%", "canary": 1, "pause_seconds": "1m"}
```

With 9 hosts, that runs one canary host, waits a minute, then runs batches of 3
hosts (25% of 9, rounded up), waiting a minute between batches. The run log
announces each wave, like `Canary: web-1` and `Batch 2 of 3: web-5, web-6, web-7`.

A host fails when one of its steps fails and doesn't continue on error. The run
then ends as failed whatever the rollout does next. When `on_host_failure` is unset,
a runbook that stops at a failed step also stops starting hosts, and one set to
finish then fail lets the other hosts run: the same as before rollouts existed.
Cancelling a run stops every host that is running.

### Retrying hosts that didn't finish

When a run ends with hosts that didn't finish well (a step failed and doesn't
continue on error, or the host was skipped or cancelled), its page offers
**Retry N hosts**. That starts a new run of the runbook on just those hosts, with
the same input values. Secret inputs are never stored, so the form asks for them
again. The retry uses the runbook as it is now, so you can fix a step first, and
it links back to the run it retries.

Through the API: `POST /api/v1/runs/{id}/retry`, with an optional body
`{"inputs": {...}}` for new or secret values.

## Output limit

A run keeps up to 20,000 lines of output (system lines included). A runbook can set a
lower limit and choose what happens when a run reaches it:

```json
"output_limit": {"lines": 5000, "action": "fail"}
```

`truncate` (the default) keeps the run going and drops further output. `fail` stops
the step that is running, skips the rest and marks the run failed.
