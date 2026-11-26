# OpSkills

**Runbooks your whole team can run.**

OpSkills turns the operational procedures you repeat (restarts, certificate
rotations, failover drills, cleanups) into safe, self-service runbooks. They run
across your hosts by hand, on a schedule, from a webhook, or through the API,
with live output, approvals and a full audit trail.

Website: [opskills.app](https://opskills.app)

> **Status:** v0.8.0 preview. Sign in, keep secrets in an encrypted vault, enroll
> your Linux hosts with the OpSkills agent (`.deb` and `.rpm`), write runbooks with
> inputs, retries, file copies and calls to other runbooks, and run them on the
> server or on your hosts, by hand, on a schedule or with an API token, with output
> streaming live to your browser. Webhooks, notifications and approvals are next
> (members and roles on the Settings page are still sample data).

## Install

**Step-by-step guide: [docs/install.md](docs/install.md)** covers both options,
plus logs, configuration, upgrades, backups, removal and troubleshooting.

Runbooks run shell commands on the OpSkills server, so both quick starts below keep
OpSkills reachable from your own machine only. See [Security](#security) before
opening it to a network.

### Docker

```bash
docker run -d --name opskills \
  -p 127.0.0.1:8080:8080 \
  -v opskills-data:/var/lib/opskills \
  --restart unless-stopped \
  opskills/opskills:0.8.0
docker logs opskills 2>&1 | grep 'Admin password'
```

Open http://localhost:8080 and sign in as `admin` with that password. The image
is small (Alpine based), runs as a non-root user, includes the agent packages the
Hosts page offers, and supports `linux/amd64` and `linux/arm64`. With Docker Compose,
use the included [`compose.yaml`](compose.yaml): `docker compose up -d`.

### Debian and Ubuntu

```bash
curl -LO https://github.com/opskills-app/opskills/releases/download/v0.8.0/opskills_0.8.0_amd64.deb
sudo apt install ./opskills_0.8.0_amd64.deb
sudo systemctl start opskills
sudo journalctl -u opskills | grep 'Admin password'
```

Then open http://localhost:8080 on that machine and sign in as `admin`. Use `opskills_0.8.0_arm64.deb`
on ARM servers (`dpkg --print-architecture` tells you which). All downloads and
checksums are on the [releases page](https://github.com/opskills-app/opskills/releases/latest).
The package installs a sandboxed systemd service that runs as the `opskills`
system user.

### First start

On first start OpSkills creates, in its data directory (`/var/lib/opskills`):

- a unique server ID,
- a 256-bit encryption key, and
- an admin password, printed **once** to the log. Sign in with it as `admin`
  and store it somewhere safe.

These files, and the database `opskills.redb`, are readable only by the server.

## Using OpSkills

1. **Sign in** as `admin` with the password from the first start.
2. **Create a runbook**: give it a name, then add steps. A step runs a shell
   command, copies a file (from text or a vault secret), or calls another runbook.
   Start from an example (Hello world, Check disk space, System health) or from
   scratch. Add **inputs** for the values that change between runs, like a
   service name, and use them as `{{ inputs.service }}` in a step.
3. **Press Run.** Fill in the inputs, if any. The run page streams each step's
   output live, shows a timeline of the steps, and lets you cancel.
4. **Keep credentials in the vault**, not in commands: add a secret such as
   `DB_PASSWORD` under **Vault**, pick it in the runbook, and use `"$DB_PASSWORD"`
   in a step.
5. **Add hosts** to run on other machines: on **Hosts**, press **Add host**,
   download the agent package, and paste the commands it shows on the machine.
   Then pick hosts, host tags or a filter like `tags:web !name:db-*` under
   **Run on** in a runbook, and choose a rollout.

How steps run:

- With `/bin/sh`. On the OpSkills server they run as the user the server runs as
  (`opskills` for the package and the container), with that user's permissions and
  nothing more. On an enrolled host they run as root through the OpSkills agent.
- One after another, in a fresh temporary folder per run. On several hosts, each
  host runs all the steps; hosts go one by one (the default), in parallel up to a
  limit, or in batches, optionally after a canary host and with a pause between
  waves.
- A step that exits with a non-zero code, times out or can't start fails. It can be
  retried (with a wait between tries) or set to continue on error. Otherwise the run
  stops and the rest is skipped, or, if the runbook says so, the remaining steps
  still run and then the run fails.
- Each step has a timeout (10 minutes by default, set per runbook and per step, in
  seconds or like `10m`). A step still running at its timeout, or when you cancel,
  is stopped along with any processes it started.
- `{{ inputs.name }}`, `{{ host.name }}` and other run values in a command become
  single, quoted shell words. `$OPSKILLS_RUN_ID`, `$OPSKILLS_STEP`,
  `$OPSKILLS_HOST_NAME` and one `$OPSKILLS_INPUT_<NAME>` per input are set. Other
  environment variables of the server are not passed on.
- A run keeps up to 20,000 output lines; a runbook can set a lower limit that drops
  further output or stops the run.

The full guide to step types, inputs, run values, step options and failure handling is
[docs/runbooks.md](docs/runbooks.md).
- Up to `runs.max_concurrent` runs execute at once (4 by default); others wait in
  the queue.

Runs, their output and their status survive restarts. A run that was in progress
when the server stopped is marked as failed.

## Hosts

A host is a Linux machine with the **OpSkills agent**, enrolled with your server.
The full guide is [docs/hosts.md](docs/hosts.md).

- **Packages**: `.deb` for Debian and Ubuntu, `.rpm` for RHEL, Rocky, Alma,
  Fedora and Amazon Linux, for `amd64`/`x86_64` and `arm64`/`aarch64`. Download them
  from the Hosts page (served by your OpSkills server) or the
  [releases page](https://github.com/opskills-app/opskills/releases/latest). The
  agent is a static binary with no dependencies, installed as a systemd service.
- **Enrollment**: **Add host** creates a one-time token (valid 24 hours) and shows
  the exact commands:

  ```bash
  curl -fLO https://opskills.example.com/downloads/opskills-agent_0.8.0_amd64.deb
  sudo apt install ./opskills-agent_0.8.0_amd64.deb
  sudo opskills-agent enroll --server https://opskills.example.com --token <token> --tags web,prod
  sudo systemctl enable --now opskills-agent
  ```

- **Many hosts**: `/install-agent.sh` on your server installs, enrolls and starts the
  agent in one line, and [`deploy/`](deploy/) has a tested SSH script, an Ansible
  playbook and a cloud-init example. See
  [Deploy agents to many hosts](docs/hosts.md#deploy-agents-to-many-hosts).
- **Connection**: the agent connects **out** to the server over HTTPS and asks for
  work, so hosts need no open ports, no SSH keys and no inbound firewall rules. A
  host is online while its agent is in touch; steps for an offline host fail.
- **Targets**: a runbook runs on the OpSkills server (the default), or on chosen
  hosts, every host with a tag, or both. Output is labelled with the host it came
  from, and cancel and timeouts reach the agent.
- **Removing a host** revokes its credential at once; its agent stops. Uninstall the
  package with `sudo apt purge opskills-agent` or `sudo dnf remove opskills-agent`.

## Vault

- Secrets are encrypted with **AES-256-GCM** using the server's own 256-bit key
  (`master.key` in the data directory). Each value has a fresh random nonce and is
  bound to its name. Plaintext is never written to disk or to the log.
- Values are **write-only**: the API and the web app never show a value again. You
  can replace a value, edit the kind and description, or delete a secret that no
  runbook uses.
- A runbook lists the secrets it needs. Each one is an environment variable in
  every step of that runbook's runs, and nowhere else. For steps on a host, the
  server sends the values to that host's agent with the step and they're never
  written to the host's disk.
- Any secret value (and each line of a multi-line one, like an SSH key) that shows
  up in a run's output is replaced with `••••••` before it's shown or stored.
- Keep `master.key` with your backups: without it the vault can't be decrypted.

## Security

- Every API call needs a signed-in session (an HttpOnly, SameSite=Strict cookie) or
  an API token. With a session, every change also needs the `X-OpSkills-Request: 1`
  header, which blocks cross-site request forgery.
- API tokens (`osk_...`) are shown once and stored only as a SHA-256 hash. They can
  expire, are revoked instantly, and can't create tokens or sign in. Runs record
  which token started them. A token can do everything else the admin can, so keep
  it in your CI's secret store.
- Failed sign-ins are slowed down. Resetting the admin password signs out every
  session.
- Runbooks run commands with the server user's permissions, and as **root** on
  enrolled hosts. Anyone who can sign in can run any command on those machines, so
  treat the admin password like a root password for your fleet.
- Each agent has its own credential, stored on the server only as a SHA-256 hash.
  Enrollment tokens work once and expire after 24 hours. Agents can only take
  steps for their own host.
- Agents and the server talk over the address you give at enrollment. Use HTTPS
  (see `server.public_url` below) whenever hosts are outside a trusted network:
  steps, output and secrets travel over that connection.
- The package listens on `127.0.0.1` by default. To reach OpSkills from other
  machines, put it behind a reverse proxy with HTTPS and set
  `server.public_url = "https://..."` so the session cookie is marked `Secure`, or
  use an SSH tunnel.

### Reporting a vulnerability

Please report security problems privately, as described in [SECURITY.md](SECURITY.md),
not in a public issue.

## Configuration

Settings live in one TOML file, `/etc/opskills/opskills.toml` (pass another path
with `--config`). Any setting can be overridden with an environment variable named
`OPSKILLS_<SECTION>__<KEY>`:

| Setting | Environment variable | Default |
|---|---|---|
| `server.listen` | `OPSKILLS_SERVER__LISTEN` | `0.0.0.0:8080` in the container, `127.0.0.1:8080` in the package |
| `server.public_url` | `OPSKILLS_SERVER__PUBLIC_URL` | not set |
| `data.dir` | `OPSKILLS_DATA__DIR` | `/var/lib/opskills` |
| `logs.format` | `OPSKILLS_LOGS__FORMAT` | `json` (or `pretty`) |
| `logs.level` | `OPSKILLS_LOGS__LEVEL` | `info` |
| `auth.session_hours` | `OPSKILLS_AUTH__SESSION_HOURS` | `8` |
| `runs.max_concurrent` | `OPSKILLS_RUNS__MAX_CONCURRENT` | `4` |
| `hosts.packages_dir` | `OPSKILLS_HOSTS__PACKAGES_DIR` | `/usr/share/opskills/packages` |

The agent packages offered on the Hosts page come from `hosts.packages_dir` when
they're there (the container image includes them), and otherwise from the GitHub
release of the same version. Set `server.public_url` to the address hosts use to
reach the server; the Hosts page puts it in the enrollment commands.

`opskills config` prints the effective settings. `opskills healthcheck` exits 0
when the local server is healthy. HTTP probes are at `/healthz` and `/readyz`.

## API

The web app uses the same HTTP API you can call yourself, under `/api/v1`. Scripts
and CI use an **API token**: create one under **Settings > API tokens** (it's shown
once) and send it as a Bearer token:

```bash
curl -X POST -H "Authorization: Bearer $OPSKILLS_TOKEN" \
  https://ops.example.com/api/v1/runbooks/<id>/runs
```


| Endpoint | What it does |
|---|---|
| `GET /system` | Version and server ID (no sign-in needed) |
| `POST /auth/login`, `POST /auth/logout`, `GET /auth/me` | Sign in and out, and who you are |
| `GET /me/tokens`, `POST /me/tokens`, `DELETE /me/tokens/{id}` | List, create (the value is returned once) and revoke API tokens; signed-in sessions only |
| `GET /runbooks`, `POST /runbooks` | List and create runbooks |
| `GET`, `PUT`, `DELETE /runbooks/{id}` | Read, replace and delete a runbook |
| `GET /runbooks/{id}/schedules` | Each schedule's next times, last run and last error |
| `POST /schedules/preview` | The next times a cron expression fires in a time zone: `{"cron": "0 3 * * *", "timezone": "UTC"}` |
| `POST /runbooks/{id}/runs` | Start a run; body `{"inputs": {...}}` when the runbook has inputs |
| `GET /runs?runbook_id=&limit=`, `GET /runs/{id}` | List runs, newest first, and read one |
| `GET /runs/{id}/logs?after=` | Output lines after a sequence number |
| `GET /runs/{id}/events` | Live run updates and output as Server-Sent Events |
| `POST /runs/{id}/cancel` | Cancel a queued or running run |
| `POST /runs/{id}/retry` | Run again on the hosts that didn't finish; body `{"inputs": {...}}` for secret inputs |
| `GET /secrets`, `POST /secrets` | List secrets (never values) and add one |
| `PUT`, `DELETE /secrets/{name}` | Replace a value or edit a secret, and delete one |
| `GET /hosts?filter=` | List hosts, starting with the OpSkills server itself, or the enrolled hosts a filter matches |
| `PUT`, `DELETE /hosts/{id}` | Rename or tag a host, and remove one |
| `POST /hosts/enrollment-tokens` | Create a one-time enrollment token |
| `GET /hosts/packages` | Agent packages for this version |
| `GET /downloads/{file}` | Download an agent package (no sign-in needed) |
| `GET /install-agent.sh` | One-line agent installer for this version (no sign-in needed) |

Agents use their own endpoints under `/api/v1/agent/` with a per-host credential;
see [notes/02-architecture.md](notes/02-architecture.md).

Errors are JSON with `title`, `status` and a readable `detail`.

## Roadmap

- **Runbooks as code**: YAML files you can keep in Git and review like any other change.
- **Webhooks and notifications**: start runs when another system calls in, and tell people how runs went.
- **Approval gates**: pause a run until the right person signs off.
- **One small binary**: written in Rust, no runtime dependencies, secure defaults on first start.

## Repository layout

| Path | What it is |
|---|---|
| [`server/`](server/) | The `opskills` server and agent in Rust, with [packaging](server/packaging/): the server `.deb` and the agent `.deb` and `.rpm` |
| [`ui/`](ui/) | The web app: React, TypeScript, Vite and Tailwind CSS, built into the server |
| [`web/`](web/) | The opskills.app website, deployed to Cloudflare Workers |
| [`docs/`](docs/) | User documentation: [install and run](docs/install.md), [hosts and the agent](docs/hosts.md), [writing runbooks](docs/runbooks.md); [release notes](docs/releases/) |
| [`deploy/`](deploy/) | Deploying agents to many hosts: SSH script, Ansible playbook, cloud-init |
| [`notes/`](notes/) | Design documents: [packaging and configuration](notes/01-packaging-and-config.md), [architecture](notes/02-architecture.md) |
| [`compose.yaml`](compose.yaml) | Run the published image with Docker Compose |
| [`Dockerfile`](Dockerfile) | Builds the container image and all packages from source |
| [`RELEASING.md`](RELEASING.md) | How releases and the website are built and published by GitHub Actions |

## Development

Requirements: Node.js 24 (the version in `.nvmrc`; with nvm, run `nvm use`) and
stable Rust.

### Server

The server embeds the built web app, so build the UI first:

```bash
(cd ui && npm install && npm run build)
cd server
cargo test
OPSKILLS_DATA__DIR=./data OPSKILLS_LOGS__FORMAT=pretty cargo run
```

### Web app

```bash
cd ui
npm install
npm run dev        # http://localhost:5173
npm run build      # production build in ui/dist
npm run typecheck
```

Press `Cmd+K` (or `Ctrl+K`) to open the command palette. Add `?theme=light` or
`?theme=dark` to the URL to force a theme.

### Release builds

Everything builds from source in Docker, for both `amd64` and `arm64`:

```bash
# Server .deb and agent .deb/.rpm packages into ./dist
docker buildx build --target packages --output type=local,dest=dist .

# Container image
docker buildx build --platform linux/amd64,linux/arm64 -t opskills .
```

The binaries are static (musl), cross-compiled with
[cargo-zigbuild](https://github.com/rust-cross/cargo-zigbuild), which uses Zig as
the C compiler and linker. The agent packages are built with
[nFPM](https://nfpm.goreleaser.com/). Both tools are downloaded and checksum-verified
inside the build.

### Continuous delivery

[`.github/workflows/docker.yml`](.github/workflows/docker.yml) builds the image for
both architectures on every pull request, running all tests inside the build, and
publishes to [Docker Hub](https://hub.docker.com/r/opskills/opskills):

| Event | Image tags |
|---|---|
| Pull request | Built and tested, not published |
| Merge to `main` | `edge`, `sha-<commit>` |
| Version tag `vX.Y.Z` | `X.Y.Z`, `X.Y`, `latest` |

It needs the repository variable `DOCKERHUB_USERNAME` and the secret
`DOCKERHUB_TOKEN` (a Docker Hub access token with read and write access).

### Pull requests and merging

`main` is protected by two repository rulesets:

- **Always, for everyone:** no force pushes or deletion, and the `image` check
  (the build and all tests) must pass before a merge.
- **Maintainers approve, owners merge:** changes land only through squash-merged
  pull requests that a member of the organization's maintainers team approves. A
  new push dismisses earlier approvals, and only organization owners can merge.
  Owners can merge their own pull requests by bypassing the approval rule.

### Website

```bash
cd web
npm install
npm run dev        # local preview
npm run deploy     # deploy to Cloudflare (requires `npx wrangler login`)
```

`opskills.app` serves the site. `getopskills.com` and `opskills.dev` redirect to it.

## License

OpSkills is open source under the [Apache License 2.0](LICENSE). See [NOTICE](NOTICE).
