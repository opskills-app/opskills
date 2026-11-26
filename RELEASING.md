# Releasing OpSkills

Releases are built and published by GitHub Actions. A maintainer prepares the
version in a pull request, then pushes a signed tag; nothing is built or uploaded
from a laptop.

## What runs where

| Workflow | When | What it does |
|---|---|---|
| [`docker.yml`](.github/workflows/docker.yml) | Pull requests, merges to `main`, version tags | Builds and tests the image for `linux/amd64` and `linux/arm64`. Merges publish `opskills/opskills:edge`; a tag `v1.2.3` publishes `1.2.3`, `1.2` and `latest` |
| [`release.yml`](.github/workflows/release.yml) | Version tags; pull requests that change packaging | Builds the server and agent packages, checks their versions and architectures, writes `SHA256SUMS`, attests their build provenance, and publishes the GitHub release. Then runs `site.yml`. Pull requests only build and check |
| [`site.yml`](.github/workflows/site.yml) | After a release, merges to `main` that change `web/`, or by hand | Deploys `web/` to Cloudflare and checks the live page. It skips the deploy while the site links to a release that doesn't exist yet |

## Cutting a release

1. **Version bump pull request.** Set the new version in `server/Cargo.toml` and
   `ui/package.json` (then `npm install --package-lock-only` in `ui/`), and update
   the version in `README.md`, `docs/`, `compose.yaml` and
   `web/public/index.html`. Add the release notes as `docs/releases/vX.Y.Z.md`:
   what's new, how to upgrade, and how to install. The workflow adds the downloads
   table and checksums.
2. **Merge it.** The site doesn't deploy yet, because it now links to a release
   that doesn't exist.
3. **Tag the merge commit** and push the tag:

   ```bash
   git switch main && git pull --ff-only
   git tag -s vX.Y.Z -m "OpSkills vX.Y.Z (preview)"
   git push origin vX.Y.Z
   ```

   The tag must match `server/Cargo.toml`, or the release workflow stops before
   building anything.
4. **Watch the runs** under Actions: `Release` (packages, release, then website)
   and `Docker image` (the image). When both are green, the release is out.

To check a release afterwards:

```bash
gh release download vX.Y.Z --repo opskills-app/opskills
sha256sum --check SHA256SUMS
gh attestation verify opskills_X.Y.Z_amd64.deb --repo opskills-app/opskills
docker run --rm --platform linux/arm64 opskills/opskills:X.Y.Z --version
```

If a release run fails after creating the release, fix the cause and re-run the
workflow: it replaces the release's files and notes rather than failing.

## Settings the workflows need

| Name | Kind | Used by | What it is |
|---|---|---|---|
| `DOCKERHUB_USERNAME` | Variable | `docker.yml` | Docker Hub account that owns `opskills/opskills` |
| `DOCKERHUB_TOKEN` | Secret | `docker.yml` | Docker Hub access token with read and write access |
| `CLOUDFLARE_ACCOUNT_ID` | Variable | `site.yml` | The Cloudflare account that hosts the site |
| `CLOUDFLARE_API_TOKEN` | Secret | `site.yml` | A Cloudflare API token that can deploy the `opskills-web` Worker and its custom domains |

Pull requests from forks never get these secrets, and `site.yml` and the publishing
parts of `release.yml` don't run for pull requests.
