# Hosts over SSH

Runbooks reach hosts through the [OpSkills agent](hosts.md) by default: it connects out
to the server, so hosts need no open ports and no keys to hand out. For machines where
you can't install the agent, OpSkills can connect over SSH instead, with the OpenSSH
client on the server.

SSH hosts come from an [inventory source](inventory.md) with **SSH settings**. Its hosts
that no agent enrolled are reached over SSH; everything else about them (tags,
attributes, tag rules, filters, rollouts, retries, copy steps, calls) works as with the
agent.

## Set it up

1. Put the private key (or password) in the [vault](vault.md), like `ssh/OPS_KEY`. The
   workspace's own secrets and shared secrets (`shared/...`) both work.
2. On the target hosts, allow that key for the user (`~/.ssh/authorized_keys`). To run
   steps as root, give the user passwordless sudo, or connect as root.
3. **Hosts > Inventory sources**, add or edit a source, and turn on **Reach its hosts
   over SSH**:

   | Setting | Default | |
   |---|---|---|
   | User | `root` | The user to sign in as |
   | Port | `22` | |
   | Sign in with | Key | A private key or a password, from the vault |
   | Run steps as root with sudo | on | Runs `sudo -n`, so it needs passwordless sudo; not used when the user is root |

4. Select the key icon next to a host and **Test connection**. The first connection pins
   the host's keys (see below).

The address is the host's `hostname`, or its `name` when it has none. A host whose
address SSH couldn't take (spaces, or starting with a dash) stays listed.

A host in the source can change the user and port, and give its host keys:

```json
{"hosts": [
  {"name": "web-1", "hostname": "10.0.0.5", "tags": ["web"],
   "ssh": {"user": "deploy", "port": 2222, "host_keys": ["ssh-ed25519 AAAAC3Nza..."]}}
]}
```

**Credentials only come from the source's settings, never from the hosts it lists.**
The hosts of a URL or program source are whatever that URL or program returns; if they
could name a vault secret, whoever controls it could have OpenSSH send a password to a
host of their choosing. A host's `ssh` object may only have `user`, `port` and
`host_keys`; anything else is refused.

## Host keys

OpSkills never connects to a host whose key it doesn't trust:

- **First contact:** OpSkills fetches the keys the host presents (`ssh-keyscan`) and
  pins them. If the source gives `host_keys` for the host (public keys, or fingerprints
  like `SHA256:...`), at least one must match first; otherwise nothing is sent to the
  host and the step fails saying so.
- **Every connection** requires a pinned key (`StrictHostKeyChecking=yes`, with the pins
  filed under the host's id, so changing its address or port doesn't lose them).
- **A changed key** stops steps on that host with "its SSH host key is no longer the one
  pinned for it", and nothing is sent to it. The Hosts page shows **Key changed**, and
  the audit log records it once.
- **Accepting a new key:** select the key icon, then **Check host keys**. It lists the
  keys the host presents now. Compare them with the host's own fingerprints (on the
  host: `ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub`), then select **Accept and pin
  these keys**. Only keys the host presents at that moment can be pinned, and the audit
  log records who accepted which.

A host's pins are dropped when its source stops listing it, or the source is removed.

## How a step runs

- OpSkills runs the OpenSSH client on the server, ignoring any SSH config files, with
  agent and port forwarding off, a 15 second connection timeout and keepalives.
- On the host, the user's shell runs `/bin/sh -s` (with sudo: `sudo -n -H /bin/sh -s`) and
  reads the step from standard input: its environment as quoted values, then the
  command. **Secrets, inputs and copied file content are never on a command line**, on
  the server or the host.
- Each run gets a work folder, `$HOME/.opskills/work/<run id>` (`/root/...` with sudo),
  removed after the host's last step.
- Steps see `PATH`, `HOME` (the work folder), `LANG`, the `OPSKILLS_...` variables, inputs
  and the runbook's secrets, on top of what the SSH session sets.
- Output streams live. **Cancel** and **timeouts** stop the step and everything it
  started (its own process group, where `setsid` exists) within a few seconds.
- The key or password is written to a file only OpSkills can read for the length of the
  step, and hidden in output.

## Requirements

- **The OpSkills server** needs the OpenSSH client (`ssh` and `ssh-keyscan`). The Docker
  image includes it; for the `.deb`, install `openssh-client` (recommended by the
  package).
- **The host** needs an SSH server and a POSIX `/bin/sh` (Linux and most Unix systems).
- Options in the server's config file:

  ```toml
  [ssh]
  client = "ssh"                # the SSH client
  keyscan = "ssh-keyscan"       # fetches host keys
  connect_timeout_seconds = 15
  ```

## Good to know

- An inventory source decides where runs go: tag and filter targets include its hosts.
  Only add sources you trust, as runbooks send their secrets to the hosts they run on.
- Prefer the agent where you can: it needs no open ports on hosts and no keys on the
  server. A host that later enrolls with the agent (same hostname) is reached through
  the agent and stops using SSH.
- Not yet: jump hosts.

## Troubleshooting

| Message | What to check |
|---|---|
| `Permission denied (publickey)` | The key in the vault matches an `authorized_keys` entry for that user |
| `sudo: a password is required` | Give the user passwordless sudo, or connect as root |
| `Connection refused`, `timed out` | The address and port, and that the OpSkills server can reach them |
| `could not get its SSH host keys` | The same: the host must answer on its SSH port |
| `no longer the one pinned for it` | The host's keys changed; see [Host keys](#host-keys) |
| `don't match the ones its inventory source gives` | The source's `host_keys` for the host are out of date, or something else answered |
| `Could not start ssh` | Install the OpenSSH client on the OpSkills server |

## API

For a workspace's Admins:

| Request | What it does |
|---|---|
| `POST /hosts/{id}/ssh/test` | Connect and run a harmless command; pins the keys on first contact. Answers `ok`, `output`, `error`, `pinned_now` |
| `GET /hosts/{id}/ssh/keys` | `pinned` fingerprints, and the keys the host `presented` now |
| `POST /hosts/{id}/ssh/accept` | `{"fingerprints": [...]}`: pin those of the keys the host presents now |

A source's `ssh` setting: `{"user": "ops", "port": 22, "auth": "key", "secret": "ssh/OPS_KEY", "sudo": true}`
(`auth` is `key` or `password`).

In `GET /hosts`, SSH hosts have `"reach": "ssh"` and an `ssh` object: `address`, `port`,
`user`, `auth`, `sudo`, `fingerprints` (pinned), `pinned_at`, `pinned_by`,
`key_changed_at`, `last_ok` and `last_error`. `online` means the last connection worked.
Runbooks can target SSH hosts by id, and runs record each host's `ssh` settings (never
the credential). Audit actions: `host.ssh_key_pin`, `host.ssh_key_changed`,
`host.ssh_key_accept`.
