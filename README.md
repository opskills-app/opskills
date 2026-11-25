# OpSkills

**Runbooks your whole team can run.**

OpSkills turns the operational procedures you repeat (restarts, certificate
rotations, failover drills, cleanups) into safe, self-service runbooks. They run
across your hosts by hand, on a schedule, from a webhook, or through the API,
with live output, approvals and a full audit trail.

Website: [opskills.app](https://opskills.app)

> **Status:** v0.1.0 is an early preview. The server installs and runs, creates
> its identity and admin password on first start, and serves the web app, which
> still shows sample data. Running real runbooks is the next milestone.

## Install

**Step-by-step guide: [docs/install.md](docs/install.md)** covers both options,
plus logs, configuration, upgrades, backups, removal and troubleshooting.

The preview has no sign-in screen yet, so both quick starts below keep OpSkills
reachable from your own machine only.

### Docker

```bash
docker run -d --name opskills \
  -p 127.0.0.1:8080:8080 \
  -v opskills-data:/var/lib/opskills \
  --restart unless-stopped \
  opskills/opskills:0.1.0
docker logs opskills 2>&1 | grep 'Admin password'
```

Open http://localhost:8080. The image is about 3 MB to download, runs as a
non-root user, and supports `linux/amd64` and `linux/arm64`. With Docker Compose,
use the included [`compose.yaml`](compose.yaml): `docker compose up -d`.

### Debian and Ubuntu

```bash
curl -LO https://github.com/opskills-app/opskills/releases/download/v0.1.0/opskills_0.1.0_amd64.deb
sudo apt install ./opskills_0.1.0_amd64.deb
sudo systemctl start opskills
sudo journalctl -u opskills | grep 'Admin password'
```

Then open http://localhost:8080 on that machine. Use `opskills_0.1.0_arm64.deb`
on ARM servers (`dpkg --print-architecture` tells you which). All downloads and
checksums are on the [releases page](https://github.com/opskills-app/opskills/releases/latest).
The package installs a sandboxed systemd service that runs as the `opskills`
system user.

### First start

On first start OpSkills creates, in its data directory (`/var/lib/opskills`):

- a unique server ID,
- a 256-bit encryption key, and
- an admin password, printed **once** to the log. Store it somewhere safe.

All three files are readable only by the server.

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

`opskills config` prints the effective settings. `opskills healthcheck` exits 0
when the local server is healthy. HTTP probes are at `/healthz` and `/readyz`, and
`/api/v1/system` reports the version and server ID.

## Highlights (planned)

- **Runbooks as code**: YAML files you can keep in Git and review like any other change.
- **Live runs**: streaming output per host, a step timeline, and one-click cancel.
- **Safe rollouts**: batches, percentages or canary first, stopping on failure.
- **Approval gates**: pause a run until the right person signs off.
- **Built-in vault**: SSH keys, passwords and tokens encrypted at rest and masked in every log.
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
