# Users and roles

Everyone who uses OpSkills signs in as their own user, and each user has a **role**
that decides what they can do. Runs, API tokens and the server log name the user who
did something.

## Roles

Each role can do everything the roles before it can.

| | Viewer | Operator | Editor | Admin |
|---|---|---|---|---|
| See runbooks (and their YAML), runs, output and hosts | yes | yes | yes | yes |
| Start, retry and cancel runs | | yes | yes | yes |
| Create, edit and delete runbooks, with their schedules, webhooks and notifications | | | yes | yes |
| See vault secret names; add, replace and delete secrets | | | yes | yes |
| Enroll, rename, tag and remove hosts | | | | yes |
| Manage users, email settings and everyone's API tokens | | | | yes |

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

As an admin, open **Settings > Users** and choose **Add user**. Pick a user name
(lowercase letters, digits, dots, dashes and underscores) and a role. OpSkills shows a
generated password once: give it to the person, who chooses their own when they first
sign in with it. A user with a generated password can't do anything else until they
have.

From the same list you can:

- **change a role**, which applies at once, also to the user's API tokens;
- **reset a password**, which signs the user out and gives them a new generated
  password;
- **remove a user**, which signs them out and revokes their API tokens.

You can't change your own role or remove yourself, so there is always at least one
admin; ask another admin.

## Passwords

Passwords need at least 12 characters. Change yours under **Settings > Your account**;
that signs you out everywhere else. Failed sign-ins are slowed down, and a wrong user
name takes as long to refuse as a wrong password.

If someone forgets their password, an admin resets it under **Settings > Users**. If
the only admin is locked out, reset the `admin` user from the server: delete
`admin.hash` in the data directory and restart (see
[Troubleshooting](install.md#troubleshooting)). The `admin` user gets a new generated
password and the Admin role, and everyone is signed out.

## API tokens

API tokens belong to the user who created them and can do what that user's role
allows, read on every request: lowering a user's role limits their tokens at once,
and removing the user revokes them. Tokens can't manage tokens, users or sign-ins.
Admins see and can revoke everyone's tokens under **Settings > Everyone's API tokens**.

Webhooks and schedules start runs by themselves; editing them needs the Editor role.

## API

| Endpoint | Who | What it does |
|---|---|---|
| `GET /auth/me` | anyone signed in | Your name, role, and whether you must choose a password |
| `POST /auth/password` | anyone signed in, sessions only | `{"current_password", "new_password"}` |
| `GET /users`, `POST /users` | admins, sessions only | List users; add one with `{"name", "role"}` (the generated password is returned once) |
| `PUT /users/{name}` | admins, sessions only | Change a role: `{"role": "operator"}` |
| `POST /users/{name}/reset-password` | admins, sessions only | A new generated password, returned once |
| `DELETE /users/{name}` | admins, sessions only | Remove a user, their sessions and API tokens |
| `GET /tokens`, `DELETE /tokens/{id}` | admins, sessions only | Everyone's API tokens |

Every other endpoint needs the role in the table above; a request without it gets
`403` with "Your role doesn't allow this."

## Upgrading from 0.11 or earlier

The first start of 0.12 records a schema version in the database. Before changing
anything it copies the database to `opskills.redb.schema-0-backup-<time>` next to it.
The single admin becomes the user `admin` with the Admin role and the same password,
and existing sessions and API tokens keep working. If you go back to an older
version, restore that backup.
