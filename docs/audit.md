# Audit log

OpSkills records every change with who made it, how, and when, and in which
[workspace](workspaces.md). Server admins read all of it on the **Audit log** page,
including server-wide changes like sign-ins and accounts; a workspace's Admins read
that workspace's events there. The API has both: `GET /api/v1/audit` and
`GET /api/v1/workspaces/{id}/audit`.

Reading isn't recorded: opening a runbook or watching a run leaves no trace. Secret
values never appear in the log, only secret names.

## What's recorded

| Area | Events |
|---|---|
| Sign-ins | Signed in (with a password or the sign-in provider), failed to sign in (with the name that was tried and why), signed out, changed their password |
| Users | Added a user (with their workspaces and roles, or by a first sign-in with the provider), made someone a server admin or not, changed an email, linked to or unlinked from the sign-in provider, reset a password, removed a user |
| Workspaces | Added, renamed, deleted (with how many runs of history went with it) |
| Members | Added a member (with the role), changed a member's role (from and to), removed a member |
| API tokens | Created a token (with when it expires), revoked a token (and whose, when an admin revokes someone else's) |
| Runbooks | Created, edited (with the old name when renamed), deleted |
| Runs | Started (by hand, by a schedule or by a webhook), retried, cancelled |
| Webhooks | Added, changed (on or off, signed or not), new URL, deleted |
| Vault | Added a secret, changed a secret (whether the value was replaced), moved a secret (from where), deleted a secret |
| Shared secrets | Added, changed, moved or deleted a shared secret; shared it with workspaces or stopped sharing it (which ones) |
| Hosts | Created an enrollment token, enrolled a host (with its OS and architecture), renamed or tagged a host, shared a host into a workspace or took it out, removed a host |
| Settings | Changed or turned off the email settings, changed the sign-in settings |

## Who did it

Each event names an **actor** and, when it applies, **how**:

- a user, signed in: `alice`;
- a user through an API token: `alice`, via `token ci`;
- the server on behalf of something else: `system`, via `schedule nightly`,
  `webhook deploy` or `agent web-1`.

Runs started by a token, schedule or webhook name them the same way on the run page.

## Finding things

The page lists the newest first. Search matches the actor, how, the action, the
workspace, what it was about and the note; the area menu narrows it to sign-ins, users,
runbooks, runs and so on. Server admins also choose a workspace, or all of them with
the server-wide events, where a **Where** column names each event's workspace. **Load
older** goes further back.

The API takes the same filters:

```bash
curl -H "Authorization: Bearer $OPSKILLS_TOKEN" \
  "https://ops.example.com/api/v1/audit?action=auth.sign_in_failed&limit=50"
```

| Parameter | Meaning |
|---|---|
| `q` | Text anywhere in the event, any case |
| `action` | Actions starting with this, like `runbook.` or `member.role_change` |
| `actor` | Only this user, or `system` |
| `limit` | Events per page, 1 to 500 (default 100) |
| `before` | The `next` value of the previous page |
| `workspace` | `GET /audit` only: just this workspace's events; empty or left out for all |

The answer is `{"events": [...], "next": "<id or null>", "retention_days": 90}`. Each
event has `id`, `at` (milliseconds), `actor`, `via`, `action`, `workspace` (left out for
server-wide events), `target` (`kind`, `id`, `name`) and `detail`. `GET /audit` is for
server admins; `GET /workspaces/{id}/audit` takes the same filters and is for that
workspace's Admins. API tokens can read the log too.

## How long it's kept

Events are kept for up to 90 days, and removed every hour once they're older than
`audit.retention_days` (90 by default, 1 to 90). To keep less, set it in the config
file or with `OPSKILLS_AUDIT__RETENTION_DAYS`:

```toml
[audit]
retention_days = 30
```

The log lives in the OpSkills database, so it's part of your backups of the data
directory.
