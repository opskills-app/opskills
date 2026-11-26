# Install and run OpSkills

This guide walks through running OpSkills **v0.4.0** with Docker or with the
Debian package, running your first runbook, and everyday tasks: logs,
configuration, upgrades, backups and removal.

> **About the v0.4.0 preview.** You sign in, keep secrets in an encrypted vault,
> write runbooks (with inputs, retries and failure handling) and run them, with
> output streaming live. Each step is a shell
> command that runs on the OpSkills server itself, or on Linux hosts you enroll
> with the OpSkills agent ([docs/hosts.md](hosts.md)). Schedules and approvals come
> later; the Settings page still shows sample data. Because runbooks run commands,
> the steps below keep OpSkills reachable from your own machine only.

**Choose one:**

- [Option A: Docker](#option-a-docker) for Mac, Windows or Linux, in about two minutes.
- [Option B: Debian or Ubuntu package](#option-b-debian-or-ubuntu-package) to run it as a system service on a Linux server.

---

## Option A: Docker

Works on any machine with Docker: Docker Desktop on macOS or Windows, or Docker
Engine on Linux. The image supports Intel/AMD (`amd64`) and ARM (`arm64`) and is
about 7 MB to download.

### Step 1. Check that Docker is running

```bash
docker version
```

You should see both a **Client** and a **Server** section. If the server part
fails, start Docker Desktop (or `sudo systemctl start docker` on Linux).

### Step 2. Download the image

```bash
docker pull opskills/opskills:0.4.0
```

### Step 3. Start OpSkills

```bash
docker run -d --name opskills \
  -p 127.0.0.1:8080:8080 \
  -v opskills-data:/var/lib/opskills \
  --restart unless-stopped \
  opskills/opskills:0.4.0
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

The password is printed **only on the first start**. You sign in with it, so
store it in your password manager.

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

### Step 6. Sign in

Go to **http://localhost:8080** in your browser and sign in as `admin` with the
password from step 4. Then [run your first runbook](#your-first-runbook).

The image is based on Alpine Linux, so runbook steps can use the BusyBox tools it
includes (`df`, `free`, `ps`, `uptime`, `wget` and more).

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
curl -LO https://github.com/opskills-app/opskills/releases/download/v0.4.0/opskills_0.4.0_amd64.deb
curl -LO https://github.com/opskills-app/opskills/releases/download/v0.4.0/SHA256SUMS
```

### Step 3. Verify the download

```bash
sha256sum --check --ignore-missing SHA256SUMS
```

You should see `opskills_0.4.0_amd64.deb: OK`. If it says `FAILED`, delete the
file and download it again.

### Step 4. Install

```bash
sudo apt install ./opskills_0.4.0_amd64.deb
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

It's printed **only on the first start**. You sign in with it, so store it in
your password manager.

### Step 7. Sign in

On the server itself, open **http://localhost:8080** and sign in as `admin` with
the password from step 6.

To open it from your laptop while it stays private to the server, use an SSH
tunnel:

```bash
ssh -L 8080:localhost:8080 you@your-server
```

Leave that session open and go to **http://localhost:8080** on your laptop.
Then [run your first runbook](#your-first-runbook).

Runbook steps run as the `opskills` system user inside the service's sandbox. They
can read most of the system and write only to their own temporary folder, so they
can't change system files or restart services.

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

## Your first runbook

1. Sign in, then select **New runbook** (top right or on the Overview page).
2. Choose an example, such as **Check disk space**, or type your own: a name, then
   one or more steps. Each step has a name and a shell command, for example
   `df -h`.
3. Select **Create runbook**. You land on the runbook's page.
4. Select **Run now**. The run page opens and streams each step's output as it
   runs. Select a step on the left to show only its output, or use **Find in
   output** to search.
5. When it finishes, the run shows **Succeeded** or **Failed** with each step's
   exit code. **Run again** starts a new run; past runs stay under **Runs**.

Good to know:

- Steps run in order. The first step that exits with a non-zero code stops the run
  and the remaining steps are skipped.
- Each step has a timeout, 10 minutes unless you change it in the runbook. A step
  still running at its timeout is stopped and the run fails.
- **Cancel run** stops the current step, including any processes it started.
- Each run gets a fresh temporary folder as its working directory. Steps can read
  `$OPSKILLS_RUN_ID` and `$OPSKILLS_STEP`.
- New runbooks run on the OpSkills server. To run on other machines, add them on
  the **Hosts** page and pick them under **Run on**; see
  [Hosts and the OpSkills agent](hosts.md).

### Using a secret

1. Open **Vault** and select **Add secret**. Give it a name such as `DB_PASSWORD`,
   choose its kind, and paste the value. After saving, the value can't be viewed
   again.
2. Edit the runbook and select `DB_PASSWORD` under **Secrets**.
3. Use it in a step, in double quotes: `pg_dump --password="$DB_PASSWORD" ...`.

The value is available only to runbooks that list it. If it appears in a run's
output, it shows as `••••••`. To rotate it, select **Replace value** in the vault;
runs that start afterwards use the new value. A secret that a runbook still uses
can't be deleted.

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
| `auth.session_hours` | `OPSKILLS_AUTH__SESSION_HOURS` | `8` (how long a sign-in lasts) |
| `runs.max_concurrent` | `OPSKILLS_RUNS__MAX_CONCURRENT` | `4` (runs at the same time; others wait) |
| `hosts.packages_dir` | `OPSKILLS_HOSTS__PACKAGES_DIR` | `/usr/share/opskills/packages` (agent packages offered for download) |

The data directory holds these files, each readable only by the server:
`server-id`, `master.key` (the vault's encryption key), `admin.hash` (the admin
password, stored as an Argon2id hash) and `opskills.redb` (runbooks, runs, their
output, sessions, hosts and the encrypted vault). Back up the whole directory: without
`master.key`, vault secrets can't be decrypted.

## Troubleshooting

**Port 8080 is already in use.** Pick another port: `-p 127.0.0.1:9000:8080` for
Docker, or `listen = "127.0.0.1:9000"` in the config file for the package.

**I lost the admin password.** Delete `admin.hash` and restart; a new password is
printed once, and everyone who was signed in is signed out.

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

**A step fails with `command not found` (exit code 127).** The command isn't
installed where OpSkills runs. In the container, only BusyBox tools are available.
On a server, install the package that provides it (for example `procps` for
`uptime`, `free` and `ps`).

**A step fails with `Permission denied`.** Steps run as the `opskills` user, with
that user's permissions. The Debian service also blocks writes outside its own
folders. Running commands as other users or on other hosts comes in a later
release.

**I can't sign in.** Check that the username is `admin` and paste the password
exactly as printed. After several wrong attempts each try is slowed down by a few
seconds.

**The container shows `unhealthy`.** Read `docker logs opskills`. If you set
`OPSKILLS_SERVER__LISTEN`, keep the address `0.0.0.0` inside the container and
limit access with the `-p 127.0.0.1:...` mapping instead.
