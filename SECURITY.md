# Security policy

OpSkills runs commands on your servers, as root on enrolled hosts, so we take
security reports seriously and thank everyone who reports responsibly.

## Supported versions

OpSkills is in preview (0.x). Security fixes go into the latest release only. Upgrade
to the [latest release](https://github.com/opskills-app/opskills/releases/latest)
before reporting, if you can.

## Reporting a vulnerability

**Please don't open a public issue, pull request or discussion for a security
problem.** Report it privately instead:

1. Go to the [Security tab](https://github.com/opskills-app/opskills/security) of
   this repository.
2. Select **Report a vulnerability** (or open
   [a new advisory](https://github.com/opskills-app/opskills/security/advisories/new)
   directly).
3. Describe the problem, the affected version and component, and the steps to
   reproduce it. A proof of concept helps.

Only the maintainers see the report. We aim to:

- acknowledge it within **5 business days**,
- confirm whether it's a vulnerability and agree on a fix plan within **14 days**,
- release a fix, then publish a GitHub security advisory crediting you (unless you
  prefer otherwise).

Please give us a reasonable time to fix the problem before you disclose it publicly.

## Scope

In scope: the OpSkills server, its web app and API, the `opskills-agent`, the
`/install-agent.sh` installer, the `.deb` and `.rpm` packages, and the
`opskills/opskills` container image.

These are intended behavior, not vulnerabilities:

- An administrator can run any command on the OpSkills server and, as root, on
  enrolled hosts. That is what runbooks are for. Treat the admin password like a
  root password for your fleet.
- Running OpSkills over plain HTTP on an untrusted network exposes steps, their
  output and secrets in transit. The docs require HTTPS outside a trusted network.

Issues in third-party dependencies are welcome too: tell us if OpSkills is affected
and how.
