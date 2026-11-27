# Inventory: sources, attributes and tag rules

Hosts join OpSkills when their agent enrolls ([Hosts and the agent](hosts.md)). The
inventory adds what you already know about your machines from elsewhere: a list you
keep in OpSkills, a URL your CMDB or service registry serves, or a program that asks
a cloud API. It also lets you describe hosts with **attributes** like `env: prod` and
tag them by **rules**, so runbooks can pick hosts by what they are rather than by name.

Everything here belongs to one [workspace](workspaces.md) and is managed by its
Admins, on the **Hosts** page.

## Attributes

An attribute is a key and a value on a host: `env: prod`, `region: eu-west-1`,
`team: payments`. They come from two places:

- **Admins** set them on an enrolled host: **Hosts**, the pencil icon, **Attributes**,
  one `key=value` per line.
- **Inventory sources** set them on the hosts they list.

When both set the same key, the Admin's value wins. Among sources, the first source
(oldest) to set it wins. The edit dialog shows which attributes come from sources.

Keys are 1 to 64 lowercase letters, digits, dots, dashes and underscores, starting
with a letter or digit. `tags`, `name`, `hostname`, `os` and `arch` are taken by the
built-in fields. Values are one line of up to 500 characters; a host has up to 50
attributes.

Use them in:

- **Filters:** `env:prod`, `region:eu-*`, `!team:payments`, in a runbook's targets, a
  tag rule, or the hosts search (`GET /api/v1/hosts?filter=env:prod`). A host without
  the attribute doesn't match `env:prod` and does match `!env:prod`.
- **Commands:** `{{ host.attr.env }}` becomes the host's value as one quoted shell
  word, or an empty word (`''`) when the host doesn't have it. See
  [Using values in commands](runbooks.md#using-values-in-commands).

A run keeps each host's attributes as they were when it started, and a retry uses the
same values.

## Tag rules

A tag rule gives a tag to every host a filter matches:

| Filter | Tags |
|---|---|
| `os:debian*` | `debian` |
| `env:prod region:eu-*` | `prod-eu` |

Rules apply to enrolled hosts and to hosts sources list, using their tags and
attributes from agents, Admins and sources. A rule's tags don't feed other rules.
Rule tags show on the Hosts page like other tags, and runbooks can target them, so
**Run on** a tag like `debian` keeps up as hosts come and go.

Edit them under **Hosts > Tag rules**. A workspace has up to 100 rules, each with 1 to
20 tags.

## Inventory sources

A source lists hosts in this format:

```json
{"hosts": [
  {"name": "web-1", "hostname": "web-1.internal", "tags": ["web"], "attributes": {"env": "prod", "region": "eu-west-1"}},
  {"name": "db-1", "hostname": "10.0.0.12", "tags": ["db"], "attributes": {"env": "prod"}}
]}
```

- `name` is required and unique within the source (ignoring case). `hostname`,
  `tags` and `attributes` are optional. Other fields are ignored, so a document can
  carry more than OpSkills needs.
- Up to 5,000 hosts per source, and 50 sources per workspace.

### What happens to the hosts a source lists

- A host whose `hostname` (or, without one, `name`) matches an **enrolled host's**
  hostname or name, ignoring case, **adds to that host**: its tags join the host's
  tags and its attributes fill in what an Admin hasn't set. The Hosts page shows the
  source under the host's system details.
- Any other host is **listed**: it shows on the Hosts page with a **Listed** badge,
  filters and tag rules see it, but nothing can reach it yet. A run that targets it
  skips it and says so on the run page; a run whose targets are all listed hosts is
  refused with the names. Installing the agent on it (with a matching hostname)
  turns it into an enrolled host with the source's tags and attributes.
- Hosts can't be picked by name in **Run on** until they're enrolled; target listed
  hosts by tag or filter.

Removing a source, or turning it off, removes what it added right away.

### Kinds of source

**List.** Hosts you write in OpSkills, in the format above. Good for a handful of
machines or attributes you want to keep next to your runbooks.

**URL.** OpSkills fetches the document with `GET` on the source's interval. Optional:
one header whose value comes from the [vault](vault.md), like `Authorization` with the
secret `inventory/CMDB_TOKEN` (the value is hidden in errors). The answer must be the
JSON format above, within 20 seconds and 4 MB. Redirects aren't followed (so the
header never goes to another server): use the final URL.

**Program.** OpSkills runs a program on the server and reads the document from its
standard output. This is how inventory plugins work: a plugin is a program that
prints this format.

- The command is an absolute path; up to 20 arguments.
- **Settings** become environment variables: `REGION=eu-west-1` is
  `$OPSKILLS_CONFIG_REGION`. Keys are uppercase letters, digits and underscores.
- **Vault secrets** you list are set as variables named after their last part, like
  `$API_TOKEN` for `cloud/API_TOKEN`, and their values are hidden in errors.
- It also gets `$OPSKILLS_WORKSPACE` and `$OPSKILLS_SOURCE`, plus `PATH`, `HOME`,
  `LANG` and `TMPDIR` from the server, and nothing else from the server's
  environment.
- It must exit with 0 within 60 seconds and print at most 4 MB. On failure the last
  lines of its standard error show as the error.
- It runs as the user the OpSkills server runs as. **Only server admins** can add,
  change or remove a program source, because it runs code on the server; workspace
  Admins see it and can refresh it.

A minimal program:

```sh
#!/bin/sh
# Lists two hosts; a real one would ask an API using $OPSKILLS_CONFIG_* and secrets.
cat <<EOF
{"hosts": [
  {"name": "app-1", "hostname": "app-1.internal", "attributes": {"region": "$OPSKILLS_CONFIG_REGION"}},
  {"name": "app-2", "hostname": "app-2.internal", "attributes": {"region": "$OPSKILLS_CONFIG_REGION"}}
]}
EOF
```

### Refreshing, and when it fails

Each source refreshes on its interval (default 5 minutes, from 30 seconds to a day),
right after you add or change it, and when you select the refresh icon. The list
shows how many hosts it has, when it last refreshed, and its last error.

When a refresh fails (the URL is down, answers an error or bad JSON, the program
fails or times out):

- the source **keeps the hosts of its last good refresh**, so runs don't lose hosts
  because a CMDB blinked;
- the source shows **Failing** with the error, and its hosts show a warning icon on
  the Hosts page: they may be out of date;
- the audit log records the failure once (not every retry), and records when it
  recovers.

A source that suddenly lists **no hosts** after listing some is treated as a failure
too: the hosts stay, and the error says so. If the empty list is right, select
**Accept the empty list** (or refresh with `{"accept_empty": true}`).

A host keeps its id across refreshes as long as its name stays the same.

## Audit log

Recorded in the workspace's audit log: sources added, changed and removed; the first
failure of a streak and the recovery; hosts a source added or removed (names, up to
10 shown); tag rule changes; and attribute changes on hosts.

## API

All with the Admin role in the workspace (`?workspace=<slug>`, default `default`):

| Request | What it does |
|---|---|
| `GET /inventory/sources` | The workspace's sources, each with `state`: `last_attempt`, `last_success`, `error`, `host_count` |
| `POST /inventory/sources` | Add a source; it refreshes right away. Answers like `GET /inventory/sources/{id}` |
| `GET /inventory/sources/{id}` | A source with its `state` and the `hosts` it found |
| `PUT /inventory/sources/{id}` | Replace a source (same body as adding); it refreshes right away |
| `DELETE /inventory/sources/{id}` | Remove it and what it added |
| `POST /inventory/sources/{id}/refresh` | Refresh now; body `{"accept_empty": true}` takes an empty list |
| `PUT /workspaces/{slug}/tag-rules` | Replace the rules: `{"rules": [{"filter": "os:debian*", "tags": ["debian"]}]}` |
| `PUT /hosts/{id}` | Also takes `attributes`, an object that replaces the Admin's attributes |

A source's body:

```json
{"name": "CMDB", "type": "static", "hosts": [{"name": "db-1", "hostname": "10.0.0.12"}], "interval_seconds": 300, "enabled": true}
{"name": "Registry", "type": "url", "url": "https://cmdb.example/hosts.json", "header": "Authorization", "secret": "inventory/CMDB_TOKEN"}
{"name": "Cloud", "type": "program", "command": "/opt/opskills/plugins/list-hosts", "args": ["--all"], "config": {"REGION": "eu-west-1"}, "secrets": ["cloud/API_TOKEN"]}
```

Unknown fields are refused, so a typo doesn't go unnoticed. Adding or changing a
program source needs a server admin.

`GET /hosts` lists enrolled and listed hosts. Each has `reach` (`agent` or `listed`),
`sources` (`id` and `name` of the sources that list it), `attributes` and `tags`
(everything, from agents, Admins, sources and rules), `own_tags` and
`own_attributes` (what an Admin set), `rule_tags`, and `stale`. To change a host's
tags with `PUT /hosts/{id}`, start from `own_tags`, not `tags`. Runs have
`hosts[].attributes` and `skipped_hosts`.
