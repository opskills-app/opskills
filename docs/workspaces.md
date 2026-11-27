# Workspaces

A workspace is a separate space for one team or purpose: its own runbooks, runs, vault
secrets and hosts, and its own members with their roles. People see only the
workspaces they belong to.

| In each workspace | Server-wide |
|---|---|
| Runbooks, with their schedules, webhooks and notifications | User accounts and server admins |
| Runs and their output | Email settings |
| Vault secrets (the same name can exist in two workspaces, with different values) | API tokens (each acts with its owner's role in each workspace) |
| | Shared secrets, which server admins share with chosen workspaces or all |
| Hosts (a host can be shared with more than one workspace) | The OpSkills server itself, which every workspace can run on |
| Members and their roles | The audit log, where each event names its workspace |

Everything from before workspaces is in the workspace **Default**, which can't be
deleted.

## Switching

The sidebar shows the workspace you're in (under the header on phones). Pick another
from the list to switch; the app remembers your choice in this browser. Opening a link
to a run or runbook of another workspace you belong to switches to it.

## Creating and deleting

Server admins add workspaces under **Settings > Workspaces**: a name, which can change
later, and an id, like `payments`, which can't. The id is how the API names the
workspace.

A workspace can be deleted when it has no runbooks, no vault secrets and no hosts of
its own. Its run history is deleted with it, its members leave it, and hosts it shared
with other workspaces stay in those.

## Members

A workspace's Admins manage who's in it under **Settings > Members**: add an existing
account with a role, change a member's role, or remove a member. Changes apply at
once, also to the member's API tokens. Admins can't change their own membership; ask
another Admin or a server admin. Only server admins create accounts
([Users and roles](users.md)).

Every member can see the list of members and their roles. Server admins act as Admin
in every workspace without being members.

## Hosts

A host joins the workspace its enrollment token was made in: **Add host** on the Hosts
page makes the token for the workspace you're in.

An Admin can **share** a host with another workspace they're also an Admin of, so a
machine two teams use serves both. Runbooks in a workspace only reach that workspace's
hosts; tags and filters only match them. Because changing a shared host affects every
workspace it's in, renaming, tagging or removing it needs Admin in all of them (or a
server admin). Taking it out of one workspace needs Admin there, and isn't possible
while runbooks there target it, or when it's the host's only workspace.

## Vault

Secrets belong to one workspace; runbooks only use their own workspace's secrets, plus
the shared secrets server admins have shared with it (`shared/...`). The same name can
hold different values in different workspaces, or in different folders of one. The
email password (**Settings > Email**) is a shared secret. See [Vault](vault.md).

## Runbooks

Calls, webhooks and schedules stay inside a workspace: a runbook can only call runbooks
of its own workspace, and its runs use that workspace's hosts and secrets.

## API

Lists and creates work in the workspace named by `?workspace=<id>`, or in Default
without one, so scripts written before workspaces keep finding and creating things
where they did:

```bash
curl -H "Authorization: Bearer $OPSKILLS_TOKEN" "https://ops.example.com/api/v1/runbooks?workspace=payments"
curl -X POST -H "Authorization: Bearer $OPSKILLS_TOKEN" -H 'Content-Type: application/json' \
  -d '{"name": "Rotate logs", "steps": [{"name": "rotate", "run": "logrotate -f /etc/logrotate.conf"}]}' \
  "https://ops.example.com/api/v1/runbooks?workspace=payments"
```

Runbooks, runs, webhooks and hosts have ids unique across the server, so
`/runbooks/{id}`, `/runs/{id}` and the rest need no workspace; it comes from the
thing itself. Inbound webhook URLs don't change. Asking about a workspace you don't
belong to, or about something in one, gets `404`, as if it didn't exist.

| Endpoint | Who | What it does |
|---|---|---|
| `GET /workspaces` | anyone signed in | The workspaces you can see, with your role in each |
| `POST /workspaces` | server admins | Add one: `{"slug": "payments", "name": "Payments", "description": ""}` |
| `GET /workspaces/{slug}` | members | The workspace, with how many runbooks, hosts and members it has |
| `PUT /workspaces/{slug}` | its Admins | Rename it: `{"name", "description"}` |
| `DELETE /workspaces/{slug}` | server admins | Delete an empty workspace and its run history |
| `GET /workspaces/{slug}/members` | members | `{"members": [{"name", "role"}], "server_admins": [...]}` |
| `GET /workspaces/{slug}/addable-users` | its Admins, sessions only | Accounts that aren't members yet |
| `PUT /workspaces/{slug}/members/{name}` | its Admins, sessions only | Add a member or change their role: `{"role": "operator"}` |
| `DELETE /workspaces/{slug}/members/{name}` | its Admins, sessions only | Remove a member |
| `GET /workspaces/{slug}/audit` | its Admins | The workspace's audit events ([Audit log](audit.md)) |
| `PUT /hosts/{id}/workspaces/{slug}` | Admins of the host's workspaces and of `{slug}` | Share a host with another workspace |
| `DELETE /hosts/{id}/workspaces/{slug}` | Admins of `{slug}` | Take a host out of a workspace |

## Upgrading from 0.13 or earlier

The first start of 0.14 copies the database to `opskills.redb.schema-1-backup-<time>`
(or `schema-0` from 0.11 and earlier) before it changes anything. Then everything
moves into the workspace Default: runbooks, runs, secrets and hosts. Admins become
server admins; everyone keeps their role in Default. API tokens, webhook URLs and
scripts keep working, because requests without `?workspace=` use Default. The
exception is account data: sign-in and `/auth/me` return `admin` and `workspaces`
instead of `role`, and the users API changed the same way (see the
[release notes](releases/v0.14.0.md)). If you go back to an older version, restore the
backup.
