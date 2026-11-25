# OpSkills

**Runbooks your whole team can run.**

OpSkills turns the operational procedures you repeat (restarts, certificate
rotations, failover drills, cleanups) into safe, self-service runbooks. They run
across your hosts by hand, on a schedule, from a webhook, or through the API,
with live output, approvals and a full audit trail.

Website: [opskills.app](https://opskills.app)

> **Status:** early development. The web app is a clickable prototype with
> sample data; the server is being designed.

## Highlights (planned)

- **Runbooks as code**: YAML files you can keep in Git and review like any other change.
- **Live runs**: streaming output per host, a step timeline, and one-click cancel.
- **Safe rollouts**: batches, percentages or canary first, stopping on failure.
- **Approval gates**: pause a run until the right person signs off.
- **Built-in vault**: SSH keys, passwords and tokens encrypted at rest and masked in every log.
- **One small binary**: written in Rust, no runtime dependencies, secure defaults on first start.
- **Easy to install**: a Debian package and a container image under 30 MB.

## Repository layout

| Path | What it is |
|---|---|
| [`notes/`](notes/) | Design documents: [packaging and configuration](notes/01-packaging-and-config.md), [architecture](notes/02-architecture.md) |
| [`ui/`](ui/) | The web app: React, TypeScript, Vite and Tailwind CSS (prototype with sample data) |
| [`web/`](web/) | The opskills.app website, deployed to Cloudflare Workers |

The Rust server will be added under `server/`.

## Development

Requirements: Node.js 24 (the version in `.nvmrc`; with nvm, run `nvm use`).

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

### Website

```bash
cd web
npm install
npm run dev        # local preview
npm run deploy     # deploy to Cloudflare (requires `npx wrangler login`)
```

`opskills.app` serves the site. `getopskills.com` and `opskills.dev` redirect to it.
