# OpSkills

**Runbooks your whole team can run.**

OpSkills turns the operational procedures you repeat (restarts, certificate
rotations, failover drills, cleanups) into safe, self-service runbooks. They run
across your hosts by hand, on a schedule, from a webhook, or through the API,
with live output, approvals and a full audit trail.

Website: [opskills.app](https://opskills.app)

> **Status:** v0.2.0 preview. Sign in, keep secrets in an encrypted vault, write
> runbooks and run them, with output streaming live to your browser. Steps run on
> the OpSkills server itself; running on other hosts, schedules and approvals are
> next (the Hosts and Settings pages still show sample data).

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
  opskills/opskills:0.2.0
docker logs opskills 2>&1 | grep 'Admin password'
```

Open http://localhost:8080 and sign in as `admin` with that password. The image
is about 7 MB to download, runs as a
non-root user, and supports `linux/amd64` and `linux/arm64`. With Docker Compose,
use the included [`compose.yaml`](compose.yaml): `docker compose up -d`.

### Debian and Ubuntu

```bash
curl -LO https://github.com/opskills-app/opskills/releases/download/v0.2.0/opskills_0.2.0_amd64.deb
sudo apt install ./opskills_0.2.0_amd64.deb
sudo systemctl start opskills
sudo journalctl -u opskills | grep 'Admin password'
```

Then open http://localhost:8080 on that machine and sign in as `admin`. Use `opskills_0.2.0_arm64.deb`
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
2. **Create a runbook**: give it a name, then add steps. Each step is a shell
   command. Start from an example (Hello world, Check disk space, System health)
   or from scratch.
3. **Press Run.** The run page streams each step's output live, shows a timeline
   of the steps, and lets you cancel.
4. **Keep credentials in the vault**, not in commands: add a secret such as
   `DB_PASSWORD` under **Vault**, pick it in the runbook, and use `"$DB_PASSWORD"`
   in a step.

How steps run:

- With `/bin/sh` on the OpSkills server, as the user the server runs as (`opskills`
  for the package and the container). They have that user's permissions, nothing more.
- One after another, in a fresh temporary folder per run. The first step that exits
  with a non-zero code stops the run, and the rest are skipped.
- Each step has a timeout (10 minutes by default, set per runbook). A step still
  running at its timeout, or when you cancel, is stopped along with any processes
  it started.
- `$OPSKILLS_RUN_ID` and `$OPSKILLS_STEP` are set. Other environment variables of
  the server are not passed on.
- Up to `runs.max_concurrent` runs execute at once (4 by default); others wait in
  the queue.

Runs, their output and their status survive restarts. A run that was in progress
when the server stopped is marked as failed.

## Vault

- Secrets are encrypted with **AES-256-GCM** using the server's own 256-bit key
  (`master.key` in the data directory). Each value has a fresh random nonce and is
  bound to its name. Plaintext is never written to disk or to the log.
- Values are **write-only**: the API and the web app never show a value again. You
  can replace a value, edit the kind and description, or delete a secret that no
  runbook uses.
- A runbook lists the secrets it needs. Each one is an environment variable in
  every step of that runbook's runs, and nowhere else.
- Any secret value (and each line of a multi-line one, like an SSH key) that shows
  up in a run's output is replaced with `••••••` before it's shown or stored.
- Keep `master.key` with your backups: without it the vault can't be decrypted.

## Security

- Every API call needs a signed-in session (an HttpOnly, SameSite=Strict cookie).
  Every change also needs the `X-OpSkills-Request: 1` header, which blocks
  cross-site request forgery.
- Failed sign-ins are slowed down. Resetting the admin password signs out every
  session.
- Runbooks run commands with the server user's permissions. Anyone who can sign in
  can run any command that user can.
- The package listens on `127.0.0.1` by default. To reach OpSkills from other
  machines, put it behind a reverse proxy with HTTPS and set
  `server.public_url = "https://..."` so the session cookie is marked `Secure`, or
  use an SSH tunnel.

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

`opskills config` prints the effective settings. `opskills healthcheck` exits 0
when the local server is healthy. HTTP probes are at `/healthz` and `/readyz`.

## API

The web app uses the same HTTP API you can call yourself, under `/api/v1`:

| Endpoint | What it does |
|---|---|
| `GET /system` | Version and server ID (no sign-in needed) |
| `POST /auth/login`, `POST /auth/logout`, `GET /auth/me` | Sign in and out |
| `GET /runbooks`, `POST /runbooks` | List and create runbooks |
| `GET`, `PUT`, `DELETE /runbooks/{id}` | Read, replace and delete a runbook |
| `POST /runbooks/{id}/runs` | Start a run |
| `GET /runs?runbook_id=&limit=`, `GET /runs/{id}` | List runs, newest first, and read one |
| `GET /runs/{id}/logs?after=` | Output lines after a sequence number |
| `GET /runs/{id}/events` | Live run updates and output as Server-Sent Events |
| `POST /runs/{id}/cancel` | Cancel a queued or running run |
| `GET /secrets`, `POST /secrets` | List secrets (never values) and add one |
| `PUT`, `DELETE /secrets/{name}` | Replace a value or edit a secret, and delete one |

Errors are JSON with `title`, `status` and a readable `detail`.

## Roadmap

- **Runbooks as code**: YAML files you can keep in Git and review like any other change.
- **More hosts**: run steps on other machines over SSH, with live output per host.
- **Safe rollouts**: batches, percentages or canary first, stopping on failure.
- **Approval gates**: pause a run until the right person signs off.
- **One small binary**: written in Rust, no runtime dependencies, secure defaults on first start.

## Repository layout

| Path | What it is |
|---|---|
| [`server/`](server/) | The `opskills` server in Rust, with [packaging](server/packaging/) for Debian |
| [`ui/`](ui/) | The web app: React, TypeScript, Vite and Tailwind CSS, built into the server |
| [`web/`](web/) | The opskills.app website, deployed to Cloudflare Workers |
| [`docs/`](docs/) | User documentation: [install and run](docs/install.md) |
| [`notes/`](notes/) | Design documents: [packaging and configuration](notes/01-packaging-and-config.md), [architecture](notes/02-architecture.md) |
| [`compose.yaml`](compose.yaml) | Run the published image with Docker Compose |
| [`Dockerfile`](Dockerfile) | Builds the container image and the Debian packages from source |

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
# Debian packages into ./dist
docker buildx build --target debs --output type=local,dest=dist .

# Container image
docker buildx build --platform linux/amd64,linux/arm64 -t opskills .
```

The binaries are static (musl) and cross-compiled with Rust's bundled linker, so
no C cross-toolchain is needed.

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
