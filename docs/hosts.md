# Hosts and the OpSkills agent

Runbooks run on the OpSkills server by default. To run them on other Linux
machines, install the **OpSkills agent** on each machine and enroll it with your
server. This guide covers installing one host, deploying agents to many hosts,
targeting, removing and troubleshooting.

**Agents are the workers.** OpSkills has no other worker process to deploy: the
server runs steps that target itself (up to `runs.max_concurrent` runs at once), and
the agent on each enrolled host runs the steps for that host.

Contents: [How it works](#how-it-works) ·
[Add a host](#add-a-host) · [Deploy agents to many hosts](#deploy-agents-to-many-hosts) ·
[Run a runbook on hosts](#run-a-runbook-on-hosts) · [Remove a host](#remove-a-host) ·
[Troubleshooting](#troubleshooting)

## How it works

- The agent is the same small static binary as the server, installed as
  `/usr/bin/opskills-agent` with a systemd service, `opskills-agent`.
- It connects **out** to your OpSkills server and asks for work. Hosts need no
  open ports, no SSH keys and no inbound firewall rules, so they can sit behind
  NAT. They only need to reach the server's address.
- When a run reaches a host, the server hands its agent one step at a time. The
  agent runs it with `/bin/sh` **as root**, streams the output back about once a
  second, and reports the exit code. Cancel and timeouts reach the agent within a
  second or two.
- Each host has its own credential, created at enrollment. The server keeps only a
  SHA-256 hash of it.

## Requirements

- Linux with systemd: Debian 11+, Ubuntu 20.04+, RHEL, Rocky or Alma 8+, Fedora,
  Amazon Linux 2023, or anything that installs `.deb` or `.rpm` packages.
- `amd64` (`x86_64`) or `arm64` (`aarch64`). Check with `uname -m`.
- The host can reach the OpSkills server's address. Use HTTPS when hosts are
  outside a trusted network: steps, their output and the vault secrets a runbook
  uses travel over this connection.

Set `server.public_url` on the server (for example
`OPSKILLS_SERVER__PUBLIC_URL=https://opskills.example.com`) so the Hosts page puts
the right address in the commands it shows.

## Add a host

1. In OpSkills, open **Hosts** and select **Add host**.
2. Pick the host's system (Debian/Ubuntu or RHEL/Rocky/Fedora) and architecture.
   Optionally give it a name (default: its hostname) and tags such as `web, prod`.
3. Run the commands the dialog shows on the host. For a Debian or Ubuntu host on
   `amd64` they look like this:

   ```bash
   curl -fLO https://opskills.example.com/downloads/opskills-agent_0.3.1_amd64.deb
   sudo apt install ./opskills-agent_0.3.1_amd64.deb
   sudo opskills-agent enroll --server https://opskills.example.com --token <token> --tags web,prod
   sudo systemctl enable --now opskills-agent
   ```

   And for RHEL, Rocky, Alma or Fedora on `x86_64`:

   ```bash
   curl -fLO https://opskills.example.com/downloads/opskills-agent-0.3.1-1.x86_64.rpm
   sudo dnf install ./opskills-agent-0.3.1-1.x86_64.rpm
   sudo opskills-agent enroll --server https://opskills.example.com --token <token> --tags web,prod
   sudo systemctl enable --now opskills-agent
   ```

4. The dialog shows **is enrolled** as soon as the host connects. Select **Add
   another host** for a new token, or **Done**.

Each enrollment token works **once** and expires after **24 hours**. The packages
are also on the [releases page](https://github.com/opskills-app/opskills/releases/latest)
with checksums, and the **Download the agent** section of the Hosts page links to
all four.

To enroll without the token showing in the shell history, set it in the
environment instead: `sudo OPSKILLS_ENROLL_TOKEN=<token> opskills-agent enroll --server ...`.

### In one line

The dialog also offers steps 2 and 3 as one command. Your OpSkills server serves
an installer at `/install-agent.sh` that picks the `.deb` or `.rpm` for the host's
system and architecture, installs it, enrolls the host and starts the service:

```bash
curl -fsSL https://opskills.example.com/install-agent.sh | sudo sh -s -- \
  --server https://opskills.example.com --token <token> --name web-1 --tags web,prod
```

It needs `curl` (or `wget`), `apt-get`, `dnf` or `yum`, and root. Running it again
on an enrolled host upgrades the agent to the server's version and keeps the
enrollment, so no token is needed then. To read it before running it:
`curl -fsSL https://opskills.example.com/install-agent.sh -o install-agent.sh`.

## Deploy agents to many hosts

Every host needs its own enrollment token. Automation creates them with the API:
sign in once as `admin`, then `POST /api/v1/hosts/enrollment-tokens` for each host.
Every request that changes something needs the `X-OpSkills-Request: 1` header.

```bash
OPSKILLS=https://opskills.example.com
curl -fsS -c cookies.txt -H 'X-OpSkills-Request: 1' -H 'Content-Type: application/json' \
  -d '{"username":"admin","password":"..."}' "$OPSKILLS/api/v1/auth/login"
curl -fsS -b cookies.txt -X POST -H 'X-OpSkills-Request: 1' "$OPSKILLS/api/v1/hosts/enrollment-tokens"
# {"token":"3f9c...","expires_at":1764201600000}
```

The [`deploy/`](../deploy/) folder has three ready-made ways to use it. Each one
installs the agent from your OpSkills server and is safe to run again.

### Over SSH

[`deploy/enroll-hosts.sh`](../deploy/enroll-hosts.sh) asks for the admin password,
then for each host creates a token and runs the one-line installer over SSH. Set the
three variables at the top (server address, hosts, tags) and run it from a machine
that can SSH to the hosts and use `sudo` there:

```bash
sh deploy/enroll-hosts.sh
```

### With Ansible

[`deploy/ansible/deploy-opskills-agent.yml`](../deploy/ansible/deploy-opskills-agent.yml)
enrolls every host in the `opskills_hosts` group that isn't enrolled yet, upgrades
the agent on the others, and keeps the service running. Keep the admin password in
Ansible Vault:

```bash
cp deploy/ansible/inventory.example.ini inventory.ini        # list your hosts
ansible-vault create opskills-vault.yml                      # opskills_password: ...
ansible-playbook -i inventory.ini deploy/ansible/deploy-opskills-agent.yml \
  -e @opskills-vault.yml -e opskills_url=https://opskills.example.com --ask-vault-pass
```

Set `opskills_tags` per host or group in the inventory. Managed hosts need Python 3,
as for any Ansible run.

### At first boot (cloud-init)

For new cloud machines, create a token first and pass
[`deploy/cloud-init.yaml`](../deploy/cloud-init.yaml) as user data, with your server
address and the token filled in. The machine enrolls itself on first boot. Create a
separate token for each machine; a token works once.

### Machine images

Don't enroll the machine you build an image from: every copy would share its
credential. Install the package in the image if you like, and enroll each machine at
first boot (cloud-init above, or your own first-boot script).

## Run a runbook on hosts

In the runbook editor, under **Run on**, choose **Hosts**, then pick:

- **hosts** by name (including the OpSkills server itself), and/or
- **tags**: every host with that tag at the time the run starts.

The editor shows how many hosts match. A run goes to each host in turn, sorted by
name: all steps on the first host, then the next. The first failing step stops the
run. On the run page, the steps are grouped by host and every output line is
labelled with its host.

On each host a run gets a fresh folder under `/var/lib/opskills-agent/work`, which
is removed when the run's last step there finishes. Steps see `PATH`, `HOME`,
`LANG`, `$OPSKILLS_RUN_ID`, `$OPSKILLS_STEP` and the runbook's vault secrets, and
nothing else from the agent's environment.

## Online and offline

A host is **online** while its agent is in touch (it asks for work at least every
25 seconds). If a run reaches a host that is offline, the step fails after about 45
seconds with "The host didn't pick up the step". If an agent goes silent during a
step, the step fails after 30 seconds with "Lost contact with the agent".

## Everyday tasks

| Task | How |
|---|---|
| See the agent's status | `sudo opskills-agent status` (also checks that the server accepts this host) |
| Agent logs | `sudo journalctl -u opskills-agent -f` |
| Restart the agent | `sudo systemctl restart opskills-agent` |
| Rename or retag a host | **Hosts**, then the pencil icon |
| Upgrade the agent | Install the new package the same way; the service restarts on the new version |
| Enroll with another server | `sudo opskills-agent enroll --server ... --token ... --force` |

## Remove a host

1. In OpSkills, select the trash icon next to the host. Its credential stops
   working at once, and its agent stops within about 25 seconds and doesn't restart.
   A host that a runbook names directly can't be removed until you take it out of
   that runbook; hosts targeted by tag can always be removed.
2. On the host, uninstall the agent:
   - Debian or Ubuntu: `sudo apt purge opskills-agent` removes the agent, its
     credential (`/etc/opskills-agent`) and its work folder.
   - RHEL, Rocky, Alma or Fedora: `sudo dnf remove opskills-agent` removes the agent
     and its work folder. It keeps `/etc/opskills-agent`; delete it with
     `sudo rm -r /etc/opskills-agent`, or just enroll again later: a credential the
     server no longer accepts is replaced automatically.

## Troubleshooting

**`enroll` says the token is not valid.** It was already used or is more than 24
hours old. Select **New token** in the Add host dialog.

**`enroll` says the host is already enrolled.** The server still accepts this
host's current credential. Add `--force` to replace it; the old host entry then
shows as offline and you can remove it.

**The host stays offline.** Check `sudo opskills-agent status` and
`sudo journalctl -u opskills-agent`. "can't reach the server" means the host can't
reach the address you enrolled with: check DNS, firewalls and the URL (it must be
the server's address as seen from the host, not `localhost`).

**The service exited with status 78.** The server no longer accepts this host: it
was removed on the Hosts page. Enroll it again with a new token.

**Steps fail with "Permission denied" on the OpSkills server but work on hosts.**
Steps on the server run as the server's own user, with a sandbox; steps on hosts
run as root. Target a host for administrative tasks.
