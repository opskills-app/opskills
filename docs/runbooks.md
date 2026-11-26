# Writing runbooks

A runbook is a list of steps. Each step is a shell command. This guide covers the
parts that make a runbook reusable and safe to run: inputs, run values in commands,
step options, and what happens when a step fails.

Everything here works from the runbook editor in the web app and from the API
(`POST /api/v1/runbooks`, `PUT /api/v1/runbooks/{id}`).

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

## Output limit

A run keeps up to 20,000 lines of output (system lines included). A runbook can set a
lower limit and choose what happens when a run reaches it:

```json
"output_limit": {"lines": 5000, "action": "fail"}
```

`truncate` (the default) keeps the run going and drops further output. `fail` stops
the step that is running, skips the rest and marks the run failed.
