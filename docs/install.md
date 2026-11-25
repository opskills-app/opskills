# Install and run OpSkills

This guide walks through running OpSkills **v0.1.0** with Docker or with the
Debian package, then covers everyday tasks: logs, configuration, upgrades,
backups and removal.

> **About the v0.1.0 preview.** The server installs, creates its own identity,
> encryption key and admin password on first start, and serves the web app with
> sample data. There is **no sign-in screen yet** and runbooks don't run yet, so
> the steps below keep OpSkills reachable from your own machine only. Keep the
> admin password: sign-in arrives in a later release.

**Choose one:**

- [Option A: Docker](#option-a-docker) for Mac, Windows or Linux, in about two minutes.
- [Option B: Debian or Ubuntu package](#option-b-debian-or-ubuntu-package) to run it as a system service on a Linux server.

---

## Option A: Docker

Works on any machine with Docker: Docker Desktop on macOS or Windows, or Docker
Engine on Linux. The image supports Intel/AMD (`amd64`) and ARM (`arm64`) and is
about 3 MB to download.

### Step 1. Check that Docker is running

```bash
docker version
```

You should see both a **Client** and a **Server** section. If the server part
fails, start Docker Desktop (or `sudo systemctl start docker` on Linux).

### Step 2. Download the image

```bash
docker pull opskills/opskills:0.1.0
```

### Step 3. Start OpSkills

```bash
docker run -d --name opskills \
  -p 127.0.0.1:8080:8080 \
  -v opskills-data:/var/lib/opskills \
  --restart unless-stopped \
  opskills/opskills:0.1.0
```

What each part does:

| Part | Meaning |
|---|---|
| `-d --name opskills` | Run in the background as a container named `opskills` |
| `-p 127.0.0.1:8080:8080` | Make it reachable at port 8080 **on this machine only** |
| `-v opskills-data:/var/lib/opskills` | Keep its data in a Docker volume, so it survives restarts and upgrades |
| `--restart unless-stopped` | Start it again after a reboot or a crash |

### Step 4. Save the admin password

```bash
docker logs opskills 2>&1 | grep 'Admin password'
```

The password is printed **only on the first start**. Store it in your password
manager.

### Step 5. Check that it's healthy

```bash
docker ps --filter name=opskills
```

After about 15 seconds the **STATUS** column shows `Up ... (healthy)`. You can
also ask it directly:

```bash
curl http://localhost:8080/healthz
```

which prints `ok`.

### Step 6. Open the app

Go to **http://localhost:8080** in your browser.

### Using Docker Compose instead

The repository includes a ready-made [`compose.yaml`](../compose.yaml):

```bash
curl -LO https://raw.githubusercontent.com/opskills-app/opskills/main/compose.yaml
docker compose up -d
docker compose logs opskills | grep 'Admin password'
```

Then open http://localhost:8080. Stop it with `docker compose down` (your data
stays in the `opskills-data` volume; add `-v` to delete it too).

### Everyday Docker tasks

| Task | Command |
|---|---|
| Follow the logs | `docker logs -f opskills` |
| Stop / start | `docker stop opskills` / `docker start opskills` |
| Use a different port, for example 9000 | Recreate it with `-p 127.0.0.1:9000:8080` |
| Readable logs instead of JSON | Recreate it with `-e OPSKILLS_LOGS__FORMAT=pretty` |
| Show version and server ID | `curl http://localhost:8080/api/v1/system` |

**Upgrade to a newer version.** Your data lives in the volume, so you can replace
the container safely:

```bash
docker pull opskills/opskills:<new-version>
docker rm -f opskills
docker run -d --name opskills -p 127.0.0.1:8080:8080 \
  -v opskills-data:/var/lib/opskills --restart unless-stopped \
  opskills/opskills:<new-version>
```

The server keeps its ID and doesn't create a new password.

**Back up the data** (writes `opskills-data.tgz` to the current folder):

```bash
docker run --rm -v opskills-data:/data -v "$PWD":/backup \
  busybox tar czf /backup/opskills-data.tgz -C /data .
```

**Remove OpSkills completely:**

```bash
docker rm -f opskills
docker volume rm opskills-data
```

---

## Option B: Debian or Ubuntu package

For servers running Debian 12 or newer, or Ubuntu 22.04 or newer, with systemd.
The package installs OpSkills as a sandboxed system service that runs as its own
`opskills` user.

### Step 1. Find your architecture

```bash
dpkg --print-architecture
```

This prints `amd64` (Intel/AMD) or `arm64` (ARM). The commands below use
`amd64`; replace it with `arm64` if that's what you got.

### Step 2. Download the package and its checksums

```bash
curl -LO https://github.com/opskills-app/opskills/releases/download/v0.1.0/opskills_0.1.0_amd64.deb
curl -LO https://github.com/opskills-app/opskills/releases/download/v0.1.0/SHA256SUMS
```

### Step 3. Verify the download

```bash
sha256sum --check --ignore-missing SHA256SUMS
```

You should see `opskills_0.1.0_amd64.deb: OK`. If it says `FAILED`, delete the
file and download it again.

### Step 4. Install

```bash
sudo apt install ./opskills_0.1.0_amd64.deb
```

The `./` matters: it tells apt to install this file rather than search online.
The service is enabled to start at boot, but isn't started yet.

### Step 5. Start the service

```bash
sudo systemctl start opskills
systemctl status opskills
```

Look for `Active: active (running)`. Press `q` to leave the status view.

### Step 6. Save the admin password

```bash
sudo journalctl -u opskills | grep 'Admin password'
```

It's printed **only on the first start**. Store it in your password manager.

### Step 7. Open the app

On the server itself, open **http://localhost:8080**.

To open it from your laptop while it stays private to the server, use an SSH
tunnel:

```bash
ssh -L 8080:localhost:8080 you@your-server
```

Leave that session open and go to **http://localhost:8080** on your laptop.

### Everyday service tasks

| Task | Command |
|---|---|
| Follow the logs | `sudo journalctl -u opskills -f` |
| Stop / start / restart | `sudo systemctl stop opskills` (or `start`, `restart`) |
| Show the effective settings | `sudo opskills config --config /etc/opskills/opskills.toml` |
| Show version and server ID | `curl http://localhost:8080/api/v1/system` |

**Change settings.** Edit `/etc/opskills/opskills.toml`, then restart:

```bash
sudo nano /etc/opskills/opskills.toml
sudo systemctl restart opskills
```

For example, to accept connections from other hosts change `listen` to
`"0.0.0.0:8080"`. Until sign-in arrives, only do this on a trusted network or
behind a firewall.

**Upgrade.** Download the new `.deb` and install it the same way. Your config and
data are kept, and a running service restarts on the new version:

```bash
sudo apt install ./opskills_<new-version>_amd64.deb
```

**Back up the data:**

```bash
sudo tar czf opskills-backup.tgz -C /var/lib opskills
```

**Remove OpSkills.**

```bash
sudo apt remove opskills   # removes the program, keeps config and data
sudo apt purge opskills    # removes the program, config, data and the opskills user
```

---

## Configuration reference

Settings live in one TOML file (`/etc/opskills/opskills.toml` for the package).
Any setting can be overridden with an environment variable named
`OPSKILLS_<SECTION>__<KEY>`, which is the usual way to configure the container.

| Setting | Environment variable | Default |
|---|---|---|
| `server.listen` | `OPSKILLS_SERVER__LISTEN` | `0.0.0.0:8080` in the container, `127.0.0.1:8080` in the package |
| `server.public_url` | `OPSKILLS_SERVER__PUBLIC_URL` | not set |
| `data.dir` | `OPSKILLS_DATA__DIR` | `/var/lib/opskills` |
| `logs.format` | `OPSKILLS_LOGS__FORMAT` | `json` (or `pretty`) |
| `logs.level` | `OPSKILLS_LOGS__LEVEL` | `info` |

The data directory holds three files, each readable only by the server:
`server-id`, `master.key` (the encryption key) and `admin.hash` (the admin
password, stored as an Argon2id hash).

## Troubleshooting

**Port 8080 is already in use.** Pick another port: `-p 127.0.0.1:9000:8080` for
Docker, or `listen = "127.0.0.1:9000"` in the config file for the package.

**I lost the admin password.** Delete `admin.hash` and restart; a new password is
printed once.

- Docker:
  ```bash
  docker run --rm -v opskills-data:/data busybox rm /data/admin.hash
  docker restart opskills
  docker logs opskills 2>&1 | grep 'Admin password' | tail -1
  ```
- Package:
  ```bash
  sudo systemctl stop opskills
  sudo rm /var/lib/opskills/admin.hash
  sudo systemctl start opskills
  sudo journalctl -u opskills | grep 'Admin password' | tail -1
  ```

**The service won't start.** Check the reason with
`sudo journalctl -u opskills -n 50`. A typo in the config file is reported with the
exact setting, and `sudo opskills config --config /etc/opskills/opskills.toml`
checks the file without starting the server.

**The container shows `unhealthy`.** Read `docker logs opskills`. If you set
`OPSKILLS_SERVER__LISTEN`, keep the address `0.0.0.0` inside the container and
limit access with the `-p 127.0.0.1:...` mapping instead.
