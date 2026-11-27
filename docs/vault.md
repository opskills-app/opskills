# Vault

The vault keeps the passwords, tokens and keys your runbooks need, so they never have
to be written into a command. Each [workspace](workspaces.md) has its own vault, and
server admins can share a secret with several workspaces.

- Values are encrypted with **AES-256-GCM** using the server's own key (`master.key` in
  the data directory) and **can't be viewed again** after saving, in the web app or the
  API.
- A runbook lists the secrets it needs. Each one is an environment variable in every
  step of its runs, and nowhere else. Steps on a host get the values from the server
  with the step; they're never written to the host's disk.
- If a value (or a line of a multi-line one, like an SSH key) shows up in a run's
  output, it's replaced with `••••••` before anyone sees it or it's stored.

## Names and folders

A secret has a name like `DB_PASSWORD`, and can be in up to five folders:
`prod/db/DB_PASSWORD`. Folders keep many secrets in order, and let the same name exist
twice, for example `prod/DB_PASSWORD` and `staging/DB_PASSWORD`.

- **Folders** are 1 to 40 lowercase letters, digits, dots, dashes or underscores, like
  `prod`, `db-1` or `eu.west`.
- **Names** are capital letters, digits and underscores, like `DB_PASSWORD`, because the
  name becomes the environment variable: a runbook that lists `prod/db/DB_PASSWORD` reads
  it as `$DB_PASSWORD`.
- A top folder can't be called `shared`: that name is for [shared secrets](#shared-secrets).

Folders exist while something is in them. The **Vault** page shows the folders of the
current one, with breadcrumbs to go back up; search looks in every folder.

## Adding and changing secrets

Editors and Admins of a workspace manage its vault. Choose **Add secret**, give a
folder (optional) and a name, a kind (password, token, SSH key, certificate or other) and
the value, typed or read from a file with **Upload a file** (up to 64 KB). The dialog
shows how a runbook names the secret.

On each secret you can:

- **replace the value**: runs that start from then on get the new one;
- **edit** its kind and description;
- **move** it to another folder or name, while no runbook uses it (the dialog lists the
  ones that do);
- **delete** it, while no runbook or notification uses it.

The list shows which runbooks use each secret, and who changed it last.

## Using secrets in runbooks

Pick secrets under **Secrets** in the runbook editor, or list them in the runbook:

```json
{"secrets": ["prod/db/DB_PASSWORD", "shared/smtp/SMTP_PASSWORD"]}
```

Then use them as variables: `psql "postgresql://app:$DB_PASSWORD@db.example/app"`.
A runbook can't list two secrets with the same name in different folders, since both
would be `$DB_PASSWORD`.

The same names work everywhere a runbook refers to a secret: a [copy step](runbooks.md)
writes one to a file with `"secret": "prod/tls/TLS_KEY"`, and a
[notification](runbooks.md#notifications) can use `{{ secrets.prod/ACME_API_KEY }}` in a
header or body.

### Picking the folder with an input

A whole folder can come from a **choice** input, so one runbook serves several
environments:

```json
{
  "inputs": [{"name": "env", "type": "choice", "choices": ["prod", "staging"], "required": true}],
  "secrets": ["{{ inputs.env }}/DB_PASSWORD"]
}
```

A run started with `env` set to `staging` gets `staging/DB_PASSWORD` as `$DB_PASSWORD`,
and the run page shows which secret it used. Only choice inputs can pick a folder, so
whoever starts a run can pick only among the secrets the runbook's editor chose; saving
checks that every choice has its secret. In the editor, use **Pick the folder with a
choice input** under **Secrets**.

## Shared secrets

A shared secret is one value that runbooks of several workspaces use, like a mail relay
password or a monitoring token. **Server admins** manage them on the **Vault** page's
**Shared** tab, and choose who can use each one: chosen workspaces, or all of them, now
and later.

- Runbooks name them `shared/<name>`, like `shared/smtp/SMTP_PASSWORD`.
- In a workspace it's shared with, Editors see its name and description on the
  **Shared** tab and in the runbook editor, but can't change it. Because a runbook can do
  anything with a secret it uses, sharing a secret with a workspace means its Editors can
  read it.
- A workspace whose runbooks use a shared secret can't be taken off it; change those
  runbooks first. A shared secret in use can't be moved or deleted.
- A shared secret doesn't have to be shared with any workspace: the email password, for
  example, is used only by the server.

## The email password

Email notifications sign in to SMTP with a shared secret, picked under **Settings >
Email**. Before 0.15 the password was a secret of the Default workspace; such a setting
keeps working until it's changed.

## Who can do what

| | Editors and Admins of a workspace | Server admins |
|---|---|---|
| See the workspace's secret names, add, replace, move and delete them | yes | yes, in every workspace |
| See the names of shared secrets shared with the workspace, and use them | yes | yes |
| Add, replace, move, share and delete shared secrets | | yes |

Viewers and Operators don't see secrets, but runs they start get the secrets the runbook
lists. Every change is in the [audit log](audit.md); values never are.

## API

| Endpoint | Who | What it does |
|---|---|---|
| `GET /secrets?workspace=` | Editors | The workspace's secrets by full name, then the shared ones it can use (`"shared": true`, named `shared/<name>`), each with `used_by` and `updated_by`; never values |
| `POST /secrets?workspace=` | Editors | Add one: `{"name": "prod/db/DB_PASSWORD", "kind": "password", "description": "", "value": "..."}` |
| `PUT /secrets/{name}?workspace=` | Editors | Change `kind` or `description`, replace the `value`, or move it with a new `name`; `{name}` is the full name, like `/secrets/prod/db/DB_PASSWORD` |
| `DELETE /secrets/{name}?workspace=` | Editors | Delete one that nothing uses |
| `GET /shared-secrets` | Server admins | Every shared secret, who it's shared with, the runbooks that use it, and whether it's the email password |
| `POST /shared-secrets` | Server admins | Add one: `{"name": "smtp/SMTP_PASSWORD", "kind": "password", "value": "...", "workspaces": ["payments"]}` or `"all_workspaces": true` |
| `PUT /shared-secrets/{name}` | Server admins | Change details, replace the value, move it, or change `workspaces` and `all_workspaces` |
| `DELETE /shared-secrets/{name}` | Server admins | Delete one that nothing uses |

## Backups

Keep `master.key` with your backups of the data directory: without it the vault can't be
decrypted.
