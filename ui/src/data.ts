// Sample data for the prototype. Replaced by the /api/v1 client later.

export type Status = "succeeded" | "failed" | "running" | "queued" | "cancelled" | "waiting" | "pending" | "skipped";

export interface Workspace {
  id: string;
  name: string;
  color: string;
}

export interface Runbook {
  id: string;
  name: string;
  description: string;
  tags: string[];
  owner: string;
  schedule?: string;
  nextRun?: string;
  targets: string;
  rollout: string;
  recent: Status[];
  avgDuration: string;
  steps: Step[];
  inputs: { name: string; required: boolean; default?: string }[];
}

export interface Step {
  name: string;
  kind: "command" | "script" | "http" | "approval" | "copy" | "runbook";
  detail: string;
}

export interface Run {
  id: string;
  runbookId: string;
  status: Status;
  startedBy: string;
  trigger: "manual" | "schedule" | "webhook" | "api";
  started: string;
  duration: string;
  hosts: { total: number; done: number; failed: number };
  progress: number;
}

export interface Host {
  name: string;
  address: string;
  os: string;
  tags: string[];
  status: "online" | "offline" | "degraded";
  lastSeen: string;
  source: string;
}

export const workspaces: Workspace[] = [
  { id: "platform", name: "Platform", color: "#10b981" },
  { id: "payments", name: "Payments", color: "#6366f1" },
  { id: "data", name: "Data", color: "#f59e0b" },
];

export const runbooks: Runbook[] = [
  {
    id: "restart-web",
    name: "Rolling restart: web tier",
    description: "Drain, restart and health-check each web host in 25% batches.",
    tags: ["web", "prod"],
    owner: "Maya Chen",
    schedule: "Mondays 03:00",
    nextRun: "in 4 days",
    targets: "tags:web env:prod",
    rollout: "Batches of 25%, stop on failure",
    recent: ["succeeded", "succeeded", "succeeded", "failed", "succeeded", "succeeded", "succeeded", "succeeded", "succeeded", "running"],
    avgDuration: "6m 12s",
    inputs: [
      { name: "reason", required: true },
      { name: "batch_size", required: false, default: "25%" },
    ],
    steps: [
      { name: "Drain from load balancer", kind: "command", detail: "sudo systemctl reload nginx-drain" },
      { name: "Restart application", kind: "command", detail: "sudo systemctl restart app" },
      { name: "Health check", kind: "http", detail: "GET http://{{ host.address }}:8080/healthz expects 200, 5 retries" },
      { name: "Return to pool", kind: "command", detail: "sudo systemctl reload nginx-undrain" },
    ],
  },
  {
    id: "rotate-certs",
    name: "Rotate TLS certificates",
    description: "Issue new certificates, push them to edge hosts and reload.",
    tags: ["security", "edge"],
    owner: "Jonas Weber",
    schedule: "1st of month 02:00",
    nextRun: "in 6 days",
    targets: "tags:edge",
    rollout: "Canary 1 host, then all",
    recent: ["succeeded", "succeeded", "succeeded", "succeeded", "succeeded", "succeeded"],
    avgDuration: "2m 40s",
    inputs: [{ name: "domain", required: true }],
    steps: [
      { name: "Request certificate", kind: "http", detail: "POST {{ vault.acme_url }}/order" },
      { name: "Copy to hosts", kind: "copy", detail: "cert.pem, key.pem to /etc/ssl/edge/" },
      { name: "Approval", kind: "approval", detail: "Security on-call approves before reload" },
      { name: "Reload proxy", kind: "command", detail: "sudo systemctl reload haproxy" },
    ],
  },
  {
    id: "db-failover-drill",
    name: "Database failover drill",
    description: "Promote the replica in staging and verify application recovery.",
    tags: ["database", "staging"],
    owner: "Priya Natarajan",
    targets: "tags:postgres env:staging",
    rollout: "One host at a time",
    recent: ["succeeded", "failed", "succeeded", "succeeded"],
    avgDuration: "11m 05s",
    inputs: [{ name: "cluster", required: true, default: "pg-staging-1" }],
    steps: [
      { name: "Snapshot", kind: "script", detail: "scripts/snapshot.sh {{ input.cluster }}" },
      { name: "Promote replica", kind: "command", detail: "patronictl failover --force" },
      { name: "Verify writes", kind: "runbook", detail: "Run: Smoke test API" },
    ],
  },
  {
    id: "disk-cleanup",
    name: "Clean up disk space",
    description: "Prune old logs, container images and temp files when disks pass 80%.",
    tags: ["maintenance"],
    owner: "Maya Chen",
    schedule: "Every 6 hours",
    nextRun: "in 2 hours",
    targets: "disk_used > 80%",
    rollout: "All hosts in parallel (max 20)",
    recent: ["succeeded", "succeeded", "succeeded", "succeeded", "succeeded", "succeeded", "succeeded", "succeeded", "succeeded", "succeeded"],
    avgDuration: "48s",
    inputs: [],
    steps: [
      { name: "Prune images", kind: "command", detail: "docker image prune -af --filter until=168h" },
      { name: "Rotate logs", kind: "command", detail: "sudo journalctl --vacuum-time=7d" },
    ],
  },
  {
    id: "smoke-test",
    name: "Smoke test API",
    description: "Hit the critical API paths and report latency.",
    tags: ["api", "checks"],
    owner: "Leo Martins",
    targets: "localhost",
    rollout: "Single host",
    recent: ["succeeded", "succeeded", "failed", "succeeded", "succeeded"],
    avgDuration: "21s",
    inputs: [{ name: "base_url", required: false, default: "https://api.internal" }],
    steps: [{ name: "Run checks", kind: "script", detail: "scripts/smoke.sh {{ input.base_url }}" }],
  },
  {
    id: "onboard-host",
    name: "Bootstrap new host",
    description: "Install the baseline packages, users and monitoring agent.",
    tags: ["provisioning"],
    owner: "Jonas Weber",
    targets: "Selected at run time",
    rollout: "All at once",
    recent: ["succeeded", "succeeded", "cancelled"],
    avgDuration: "3m 30s",
    inputs: [{ name: "hostname", required: true }],
    steps: [
      { name: "Baseline packages", kind: "script", detail: "scripts/baseline.sh" },
      { name: "Monitoring agent", kind: "command", detail: "curl -fsSL {{ vault.agent_url }} | sh" },
    ],
  },
];

export const runs: Run[] = [
  { id: "r-2741", runbookId: "restart-web", status: "running", startedBy: "Maya Chen", trigger: "manual", started: "2 min ago", duration: "1m 54s", hosts: { total: 24, done: 13, failed: 0 }, progress: 54 },
  { id: "r-2740", runbookId: "disk-cleanup", status: "succeeded", startedBy: "Schedule", trigger: "schedule", started: "38 min ago", duration: "46s", hosts: { total: 61, done: 61, failed: 0 }, progress: 100 },
  { id: "r-2739", runbookId: "rotate-certs", status: "waiting", startedBy: "Jonas Weber", trigger: "manual", started: "1 h ago", duration: "Waiting for approval", hosts: { total: 8, done: 1, failed: 0 }, progress: 30 },
  { id: "r-2738", runbookId: "smoke-test", status: "failed", startedBy: "Deploy webhook", trigger: "webhook", started: "2 h ago", duration: "19s", hosts: { total: 1, done: 1, failed: 1 }, progress: 100 },
  { id: "r-2737", runbookId: "smoke-test", status: "succeeded", startedBy: "Leo Martins", trigger: "api", started: "3 h ago", duration: "22s", hosts: { total: 1, done: 1, failed: 0 }, progress: 100 },
  { id: "r-2736", runbookId: "db-failover-drill", status: "succeeded", startedBy: "Priya Natarajan", trigger: "manual", started: "Yesterday", duration: "10m 41s", hosts: { total: 2, done: 2, failed: 0 }, progress: 100 },
  { id: "r-2735", runbookId: "onboard-host", status: "cancelled", startedBy: "Jonas Weber", trigger: "manual", started: "Yesterday", duration: "1m 02s", hosts: { total: 1, done: 0, failed: 0 }, progress: 40 },
  { id: "r-2734", runbookId: "disk-cleanup", status: "queued", startedBy: "Schedule", trigger: "schedule", started: "Starts in 2 h", duration: "", hosts: { total: 61, done: 0, failed: 0 }, progress: 0 },
];

const roles = ["web", "api", "worker", "edge", "postgres"];
const oses = ["Ubuntu 24.04", "Debian 13", "Ubuntu 24.04", "Rocky 10"];
export const hosts: Host[] = Array.from({ length: 24 }, (_, i) => {
  const role = roles[i % roles.length];
  const n = String(Math.floor(i / roles.length) + 1).padStart(2, "0");
  const status: Host["status"] = i === 7 ? "offline" : i === 15 ? "degraded" : "online";
  return {
    name: `${role}-${n}.prod`,
    address: `10.20.${i % 5}.${10 + i}`,
    os: oses[i % oses.length],
    tags: [role, i % 3 === 0 ? "eu-west" : "us-east", "prod"],
    status,
    lastSeen: status === "offline" ? "14 min ago" : "just now",
    source: i % 2 === 0 ? "AWS us-east-1" : "Static inventory",
  };
});

export const secrets = [
  { path: "ssh/deploy-key", kind: "SSH private key", updated: "12 days ago", usedBy: 5 },
  { path: "ssh/bastion-key", kind: "SSH private key", updated: "3 months ago", usedBy: 2 },
  { path: "tokens/slack-webhook", kind: "Token", updated: "1 month ago", usedBy: 3 },
  { path: "passwords/pg-admin", kind: "Password", updated: "6 days ago", usedBy: 1 },
  { path: "certs/acme-account", kind: "Certificate", updated: "2 months ago", usedBy: 1 },
];

export const throughput = [42, 38, 51, 47, 60, 58, 72, 66, 70, 81, 77, 88, 84, 92];
export const successRate = [97, 98, 96, 99, 98, 97, 99, 99, 98, 99, 100, 99, 98, 99];
export const durations = [6.8, 6.4, 6.9, 6.1, 6.2, 5.9, 6.3, 6.0, 5.8, 6.1, 5.7, 5.9, 6.0, 6.2];

export function runbookById(id: string) {
  return runbooks.find((r) => r.id === id);
}

export const liveLog: { t: string; host: string; step: string; line: string; level?: "ok" | "warn" | "err" }[] = [
  { t: "14:02:01", host: "web-01.prod", step: "Drain", line: "Removing web-01 from upstream pool" },
  { t: "14:02:01", host: "web-02.prod", step: "Drain", line: "Removing web-02 from upstream pool" },
  { t: "14:02:03", host: "web-01.prod", step: "Drain", line: "Active connections: 12 -> 0", level: "ok" },
  { t: "14:02:04", host: "web-01.prod", step: "Restart", line: "Stopping app.service" },
  { t: "14:02:05", host: "web-02.prod", step: "Drain", line: "Active connections: 9 -> 0", level: "ok" },
  { t: "14:02:06", host: "web-01.prod", step: "Restart", line: "Started app.service (pid 48211)", level: "ok" },
  { t: "14:02:06", host: "web-02.prod", step: "Restart", line: "Stopping app.service" },
  { t: "14:02:08", host: "web-01.prod", step: "Health check", line: "GET :8080/healthz -> 503 (attempt 1 of 5)", level: "warn" },
  { t: "14:02:09", host: "web-02.prod", step: "Restart", line: "Started app.service (pid 31877)", level: "ok" },
  { t: "14:02:10", host: "web-01.prod", step: "Health check", line: "GET :8080/healthz -> 200 in 41 ms", level: "ok" },
  { t: "14:02:11", host: "web-02.prod", step: "Health check", line: "GET :8080/healthz -> 200 in 38 ms", level: "ok" },
  { t: "14:02:12", host: "web-01.prod", step: "Return to pool", line: "Added web-01 to upstream pool", level: "ok" },
  { t: "14:02:13", host: "web-03.prod", step: "Drain", line: "Removing web-03 from upstream pool" },
  { t: "14:02:13", host: "web-04.prod", step: "Drain", line: "Removing web-04 from upstream pool" },
  { t: "14:02:14", host: "web-02.prod", step: "Return to pool", line: "Added web-02 to upstream pool", level: "ok" },
  { t: "14:02:15", host: "web-03.prod", step: "Drain", line: "Active connections: 17 -> 0", level: "ok" },
  { t: "14:02:16", host: "web-03.prod", step: "Restart", line: "Stopping app.service" },
  { t: "14:02:17", host: "web-04.prod", step: "Drain", line: "Active connections: 6 -> 0", level: "ok" },
  { t: "14:02:18", host: "web-03.prod", step: "Restart", line: "Started app.service (pid 22904)", level: "ok" },
  { t: "14:02:19", host: "web-04.prod", step: "Restart", line: "Stopping app.service" },
];
