# Users and roles

Everyone who uses OpSkills signs in as their own user. What they can do depends on
their **role in each workspace** they belong to ([Workspaces](workspaces.md)). Runs,
API tokens, the audit log and the server log name the user who did something.

## Roles

A role applies inside one workspace. Each role can do everything the roles before it
can.

| In a workspace | Viewer | Operator | Editor | Admin |
|---|---|---|---|---|
| See runbooks (and their YAML), runs, output and hosts | yes | yes | yes | yes |
| Start, retry and cancel runs | | yes | yes | yes |
| Create, edit and delete runbooks, with their schedules, webhooks and notifications | | | yes | yes |
| See vault secret names; add, replace and delete secrets | | | yes | yes |
| Enroll, rename, tag, share and remove hosts | | | | yes |
| Add members, change their roles, rename the workspace, read its audit log | | | | yes |

The same person can be an Editor in one workspace and a Viewer in another. Someone who
isn't a member of a workspace doesn't see it at all.

**Server admins** manage the server itself: user accounts, workspaces, email settings
and everyone's API tokens, and the audit log of every workspace. They act as Admin in
every workspace, member or not. The first user, `admin`, is a server admin.

Everyone can change their own password and create their own API tokens.

**Editor means root on your hosts.** Runbooks run their commands as root on the hosts
they target, so anyone who can edit a runbook can run any command, as root, on those
hosts. Give the Editor and Admin roles only to people you'd trust with that.

Operators can only run runbooks as they are. Inputs reach commands as quoted words, so
an input can't add a command of its own (unless the runbook itself hands it to
something like `eval`).

Host tags are Admin-only because tags decide where runbooks run.

## The first admin

On first start OpSkills creates the user `admin` and prints a generated password once
to the log (see [Install](install.md)). Sign in with it, and OpSkills asks you to
choose your own password before anything else. From then on the sign-in page no
longer explains where the generated password is.

## Adding people

Server admins create accounts. Open **Settings > Users** and choose **Add user**. Pick
a user name (lowercase letters, digits, dots, dashes and underscores), and choose
whether to add them to the workspace you're in, with which role, and whether they're a
server admin. OpSkills shows a generated password once: give it to the person, who
chooses their own when they first sign in with it. A user with a generated password
can't do anything else until they have.

From the same list, server admins can:

- **make someone a server admin, or not**, which applies at once;
- **reset a password**, which signs the user out and gives them a new generated
  password;
- **remove a user**, which signs them out, takes them out of every workspace and
  revokes their API tokens.

You can't change whether you're a server admin, or remove yourself, so there is always
at least one server admin; ask another one.

A workspace's Admins add existing accounts to it under **Settings > Members**, and
change or remove their roles there ([Workspaces](workspaces.md#members)).

## Passwords

Passwords need at least 12 characters. Change yours under **Settings > Your account**;
that signs you out everywhere else. Failed sign-ins are slowed down, and a wrong user
name takes as long to refuse as a wrong password.

If someone forgets their password, a server admin resets it under **Settings > Users**.
If the only server admin is locked out, reset the `admin` user from the server: delete
`admin.hash` in the data directory and restart (see
[Troubleshooting](install.md#troubleshooting)). The `admin` user gets a new generated
password and becomes a server admin again, and everyone is signed out.

## API tokens

API tokens belong to the user who created them and can do what that user's roles
allow, read on every request: lowering a role limits their tokens at once, and
removing the user revokes them. Send `?workspace=<id>` to work in a workspace other
than Default. Tokens can't manage tokens, users, members or sign-ins. Server admins see
and can revoke everyone's tokens under **Settings > Everyone's API tokens**.

Webhooks and schedules start runs by themselves; editing them needs the Editor role.

## API

| Endpoint | Who | What it does |
|---|---|---|
| `GET /auth/me` | anyone signed in | Your name, whether you're a server admin, your workspaces with your role in each, and whether you must choose a password |
| `POST /auth/password` | anyone signed in, sessions only | `{"current_password", "new_password"}` |
| `GET /users`, `POST /users` | server admins, sessions only | List users; add one with `{"name", "admin": false, "workspaces": {"ops": "editor"}}` (the generated password is returned once) |
| `PUT /users/{name}` | server admins, sessions only | Make someone a server admin, or not: `{"admin": true}` |
| `POST /users/{name}/reset-password` | server admins, sessions only | A new generated password, returned once |
| `DELETE /users/{name}` | server admins, sessions only | Remove a user, their sessions and API tokens |
| `GET /tokens`, `DELETE /tokens/{id}` | server admins, sessions only | Everyone's API tokens |

Members and workspaces have their own endpoints ([Workspaces](workspaces.md#api)).
Every other endpoint needs the role in the table above, in the workspace it's about. A
request without it gets `403` with "Your role doesn't allow this."; a request about a
workspace you don't belong to gets `404`.

## Upgrading

- **From 0.11 or earlier:** the first start of 0.12 or later records a schema version
  in the database. Before changing anything it copies the database to
  `opskills.redb.schema-0-backup-<time>` next to it. The single admin becomes the user
  `admin`, a server admin with the same password, and existing sessions and API tokens
  keep working. If you go back to an older version, restore that backup.
- **From 0.12 or 0.13:** see [Workspaces](workspaces.md#upgrading-from-013-or-earlier).
