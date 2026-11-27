# Audit log

OpSkills records every change with who made it, how, and when. Admins read it on the
**Audit log** page, or with `GET /api/v1/audit`.

Reading isn't recorded: opening a runbook or watching a run leaves no trace. Secret
values never appear in the log, only secret names.

## What's recorded

| Area | Events |
|---|---|
| Sign-ins | Signed in, failed to sign in (with the name that was tried, and whether the user exists), signed out, changed their password |
| Users | Added a user (with the role), changed a role (from and to), reset a password, removed a user |
| API tokens | Created a token (with when it expires), revoked a token (and whose, when an admin revokes someone else's) |
| Runbooks | Created, edited (with the old name when renamed), deleted |
| Runs | Started (by hand, by a schedule or by a webhook), retried, cancelled |
| Webhooks | Added, changed (on or off, signed or not), new URL, deleted |
| Vault | Added a secret, changed a secret (whether the value was replaced), deleted a secret |
| Hosts | Created an enrollment token, enrolled a host (with its OS and architecture), renamed or tagged a host, removed a host |
| Settings | Changed or turned off the email settings |

## Who did it

Each event names an **actor** and, when it applies, **how**:

- a user, signed in: `alice`;
- a user through an API token: `alice`, via `token ci`;
- the server on behalf of something else: `system`, via `schedule nightly`,
  `webhook deploy` or `agent web-1`.

Runs started by a token, schedule or webhook name them the same way on the run page.

## Finding things

The page lists the newest first. Search matches the actor, how, the action, what it
was about and the note; the area menu narrows it to sign-ins, users, runbooks, runs and
so on. **Load older** goes further back.

The API takes the same filters:

```bash
curl -H "Authorization: Bearer $OPSKILLS_TOKEN" \
  "https://ops.example.com/api/v1/audit?action=auth.sign_in_failed&limit=50"
```

| Parameter | Meaning |
|---|---|
| `q` | Text anywhere in the event, any case |
| `action` | Actions starting with this, like `runbook.` or `user.role_change` |
| `actor` | Only this user, or `system` |
| `limit` | Events per page, 1 to 500 (default 100) |
| `before` | The `next` value of the previous page |

The answer is `{"events": [...], "next": "<id or null>", "retention_days": 90}`. Each
event has `id`, `at` (milliseconds), `actor`, `via`, `action`, `target` (`kind`, `id`,
`name`) and `detail`. API tokens of admins can read the log too.

## How long it's kept

Events older than `audit.retention_days` (90 by default, up to 3650) are removed every
hour. Set it in the config file or with `OPSKILLS_AUDIT__RETENTION_DAYS`:

```toml
[audit]
retention_days = 365
```

The log lives in the OpSkills database, so it's part of your backups of the data
directory.
