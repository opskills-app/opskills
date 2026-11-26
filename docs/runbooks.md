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
| `secret` | Or the name of a vault secret to write. The runbook must list it under `secrets` (the editor does this for you) |
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
  -H 'Content-Type: application/json' -H 'X-OpSkills-Request: 1' -b cookies.txt \
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

Vault secrets the runbook lists are environment variables too, under their own names.

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

## Running on several hosts

A runbook that runs on hosts runs every step on each host, in order. The `rollout`
setting decides how the hosts take turns. In the editor it's the **Rollout** card,
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

## Output limit

A run keeps up to 20,000 lines of output (system lines included). A runbook can set a
lower limit and choose what happens when a run reaches it:

```json
"output_limit": {"lines": 5000, "action": "fail"}
```

`truncate` (the default) keeps the run going and drops further output. `fail` stops
the step that is running, skips the rest and marks the run failed.
