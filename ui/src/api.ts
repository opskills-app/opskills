// Typed client for the OpSkills /api/v1 HTTP API.

import { useCallback, useEffect, useRef, useState } from "react";

export type RunStatus = "pending" | "queued" | "running" | "succeeded" | "failed" | "cancelled" | "skipped";

/** Writes a file on the step's host. */
export interface CopyStep {
  /** Destination; a relative path is inside the run's working folder. */
  to: string;
  /** File content; {{ ... }} values are filled in as plain text. */
  content?: string;
  /** Or a vault secret the runbook lists. */
  secret?: string;
  /** Octal, like "0644". */
  mode?: string;
}

/** Runs another runbook on the step's host and waits for it. */
export interface CallStep {
  runbook: string;
  inputs: Record<string, string>;
}

export type StepKind = "run" | "copy" | "call";

export function stepKind(s: { copy?: CopyStep; call?: CallStep }): StepKind {
  return s.copy ? "copy" : s.call ? "call" : "run";
}

export interface Step {
  name: string;
  /** The shell command; empty for copy and call steps. */
  run: string;
  copy?: CopyStep;
  call?: CallStep;
  /** Overrides the runbook's step timeout. The API also accepts strings like "10m". */
  timeout_seconds?: number | string | null;
  /** Further tries after a failure, 0 to 10. */
  retries?: number;
  /** Wait between tries. The API also accepts strings like "30s". */
  retry_delay_seconds?: number | string;
  /** A failure is recorded but the run goes on. */
  continue_on_error?: boolean;
}

export type InputKind = "text" | "number" | "boolean" | "choice" | "secret";

/** A value filled in when starting a run. */
export interface InputDef {
  name: string;
  label: string;
  description: string;
  type: InputKind;
  required: boolean;
  default: string | null;
  choices: string[];
  pattern: string | null;
}

/** What a run does after a step fails. */
export type FailurePolicy = "stop" | "finish_then_fail";

export interface OutputLimit {
  lines: number;
  action: "truncate" | "fail";
}

export const MAX_OUTPUT_LINES = 20000;

/** How a run spreads over its hosts. */
export interface Rollout {
  mode: "sequential" | "parallel" | "batches";
  /** Parallel: most hosts at once; 0 means all. */
  max_parallel?: number;
  /** Batches: a number of hosts, or a percentage like "25%". */
  batch_size?: number | string;
  /** Hosts that run first, on their own. */
  canary?: number;
  /** Wait after the canary and between batches. The API also accepts "30s". */
  pause_seconds?: number | string;
  /** Unset: follows the runbook's "when a step fails" setting. */
  on_host_failure?: "stop" | "continue";
}

/** When a runbook runs by itself: a cron expression in a time zone, or once. */
export interface Schedule {
  id: string;
  name: string;
  cron?: string;
  /** Once: epoch milliseconds. */
  at?: number;
  timezone: string;
  enabled: boolean;
  inputs: Record<string, string>;
}

export type NotifyEvent = "started" | "succeeded" | "failed" | "slow";

/** Tell people about runs: by email, or with a JSON POST to a webhook URL. */
export interface Notification {
  id: string;
  on: NotifyEvent[];
  slow_after_seconds?: number;
  email?: string[];
  webhook?: string;
  /** Values may use {{ secrets.NAME }}. */
  headers?: { name: string; value: string }[];
  auth?: WebhookAuth;
  /** A JSON body to send instead of the default one, with {{ ... }} placeholders. */
  body?: string;
  enabled: boolean;
}

export type WebhookAuth = { type: "bearer"; secret: string } | { type: "basic"; username: string; secret: string };

/** Settings > Sign-in: one OIDC provider. */
export interface OidcSettings {
  enabled: boolean;
  label: string;
  issuer: string;
  client_id: string;
  /** A shared secret, like shared/oidc/CLIENT_SECRET. */
  client_secret: string;
  link_by_email: boolean;
  create_accounts: boolean;
  allowed_domains: string[];
  trust_unverified_email: boolean;
}

export interface SignInSettings {
  oidc: OidcSettings | null;
  passwords_for_server_admins_only: boolean;
}

/** What the sign-in page offers, from /system. */
export interface SignInOptions {
  provider: string | null;
  passwords: "everyone" | "server_admins";
}

/** SMTP settings made on the Settings page; the password is a vault secret. */
export interface EmailSettings {
  smtp_host: string;
  smtp_port: number;
  security: "starttls" | "tls" | "none";
  username?: string;
  password_secret?: string;
  from: string;
}

export type EmailSource = "config" | "settings" | "off";

/** How one notification about a run went. */
export interface Notice {
  at: number;
  event: NotifyEvent;
  target: string;
  ok: boolean;
  error?: string;
}

export interface ScheduleStatus {
  id: string;
  next: number[];
  last_fired_at: number | null;
  last_run_id: string | null;
  last_error: string | null;
}

export const defaultRollout: Rollout = { mode: "sequential", max_parallel: 0, canary: 0, pause_seconds: 0 };

/** Where a runbook runs. All empty: on the OpSkills server. */
export interface Targets {
  hosts: string[];
  tags: string[];
  /** A host filter expression, like "tags:web !name:db-*". */
  filter?: string;
}

export const LOCAL_HOST_ID = "local";

export function runsOnServer(t: Targets | undefined) {
  return !t || (t.hosts.length === 0 && t.tags.length === 0 && !t.filter?.trim());
}

export interface Runbook {
  id: string;
  workspace: string;
  name: string;
  description: string;
  tags: string[];
  inputs: InputDef[];
  steps: Step[];
  on_step_failure: FailurePolicy;
  output_limit: OutputLimit;
  rollout: Rollout;
  schedules: Schedule[];
  notifications?: Notification[];
  secrets: string[];
  targets: Targets;
  timeout_seconds: number;
  created_at: number;
  updated_at: number;
}

export interface RunbookInput {
  name: string;
  description: string;
  tags: string[];
  inputs?: InputDef[];
  steps: Step[];
  on_step_failure?: FailurePolicy;
  output_limit?: OutputLimit;
  rollout?: Rollout;
  schedules?: Schedule[];
  notifications?: Notification[];
  secrets: string[];
  targets: Targets;
  /** Seconds, or a duration string like "10m". */
  timeout_seconds?: number | string;
}

export interface Host {
  id: string;
  name: string;
  /** The workspaces it's in that you can see. */
  workspaces: string[];
  /** It's also in workspaces you don't belong to. */
  shared_elsewhere?: boolean;
  hostname: string;
  os: string;
  arch: string;
  agent_version: string;
  tags: string[];
  enrolled_at: number;
  last_seen: number;
  online: boolean;
  builtin: boolean;
  used_by: { id: string; name: string }[];
  /** Attributes like env or region, from an Admin and inventory sources. */
  attributes?: Record<string, string>;
  /** How runs reach it: the agent, SSH (its source's settings), or "listed" when nothing can yet. */
  reach: "agent" | "ssh" | "listed";
  /** For a host reached over SSH. */
  ssh?: HostSshView;
  /** Inventory sources that list it. */
  sources: { id: string; name: string }[];
  /** Tags it has from the workspace's tag rules (also in tags). */
  rule_tags: string[];
  /** A source that lists it failed its last refresh. */
  stale: boolean;
  /** What an Admin set on an enrolled host, without what sources and rules add. */
  own_tags: string[];
  own_attributes: Record<string, string>;
}

export interface HostSshView {
  address: string;
  port: number;
  user: string;
  auth: "key" | "password";
  /** Steps run as root with sudo. */
  sudo: boolean;
  /** Pinned host key fingerprints; empty until the first connection. */
  fingerprints: string[];
  pinned_at: number | null;
  pinned_by: string | null;
  /** When a connection found a different host key; cleared when an Admin accepts one. */
  key_changed_at: number | null;
  last_ok: number | null;
  last_error: string | null;
}

/** A source's SSH settings: its hosts without the agent are reached over SSH. */
export interface SourceSsh {
  user: string;
  port: number;
  auth: "key" | "password";
  /** Vault secret with the private key or password. */
  secret: string;
  sudo: boolean;
}

/** A host as an inventory source lists it. */
export interface SourceHost {
  name: string;
  hostname?: string;
  tags?: string[];
  attributes?: Record<string, string>;
  ssh?: { user?: string; port?: number; host_keys?: string[] };
}

export type SourceKind =
  | { type: "static"; hosts: SourceHost[] }
  | { type: "url"; url: string; header?: string | null; secret?: string | null }
  | { type: "program"; command: string; args?: string[]; config?: Record<string, string>; secrets?: string[] };

export type SourceInput = SourceKind & { name: string; interval_seconds?: number; enabled?: boolean; ssh?: SourceSsh | null };

export type InventorySource = SourceKind & {
  id: string;
  workspace: string;
  name: string;
  interval_seconds: number;
  enabled: boolean;
  created_at: number;
  updated_at: number;
  updated_by?: string | null;
  ssh?: SourceSsh | null;
  state: { last_attempt: number | null; last_success: number | null; error: string | null; host_count: number };
  hosts?: (SourceHost & { id: string; first_seen: number })[];
};

/** Hosts matching the filter get the tags. */
export interface TagRule {
  filter: string;
  tags: string[];
}

export interface AgentPackage {
  format: "deb" | "rpm";
  arch: "amd64" | "arm64";
  file: string;
  bundled: boolean;
}

export type SecretKind = "password" | "token" | "ssh_key" | "certificate" | "other";

/** A secret of the workspace, by full name (`prod/db/DB_PASSWORD`), or a shared secret it can use (`shared/<path>`). */
export interface Secret {
  name: string;
  workspace?: string;
  kind: SecretKind;
  description: string;
  created_at: number;
  updated_at: number;
  updated_by?: string;
  /** Managed by server admins; usable here as `shared/<path>`. */
  shared: boolean;
  used_by: { id: string; name: string }[];
}

/** A shared secret, as server admins see it. */
export interface SharedSecret {
  name: string;
  kind: SecretKind;
  description: string;
  all_workspaces: boolean;
  workspaces: string[];
  created_at: number;
  updated_at: number;
  updated_by?: string;
  used_by: { id: string; name: string; workspace: string }[];
  /** It's the email password (Settings > Email). */
  email_password: boolean;
}

/** How runbooks name a shared secret: `shared/<path>`. */
export const SHARED_PREFIX = "shared/";

/** The environment variable a secret becomes: the last part of its name. */
export const secretEnvName = (reference: string) => reference.slice(reference.lastIndexOf("/") + 1);

/** A secret's full name in a URL, one segment per folder. */
const secretPath = (name: string) => name.split("/").map(encodeURIComponent).join("/");

export interface StepResult {
  name: string;
  run: string;
  host_id: string;
  host_name: string;
  status: RunStatus;
  exit_code: number | null;
  started_at: number | null;
  finished_at: number | null;
  timeout_seconds?: number;
  retries?: number;
  retry_delay_seconds?: number;
  continue_on_error?: boolean;
  /** How many times the step was started. */
  attempts?: number;
  copy?: CopyStep;
  call?: CallStep;
  /** The run a call step started. */
  child_run_id?: string;
}

/** Shown in place of a secret value. */
export const MASK = "••••••";

export interface Run {
  id: string;
  workspace: string;
  runbook_id: string;
  runbook_name: string;
  status: RunStatus;
  trigger: string;
  started_by: string;
  created_at: number;
  started_at: number | null;
  finished_at: number | null;
  timeout_seconds: number;
  secrets: string[];
  /** Input values the run started with; secret ones are masked. */
  inputs?: Record<string, string>;
  on_step_failure?: FailurePolicy;
  output_limit?: OutputLimit;
  rollout?: Rollout;
  hosts: { id: string; name: string; attributes?: Record<string, string> }[];
  /** Targeted hosts left out because nothing can reach them yet. */
  skipped_hosts?: string[];
  steps: StepResult[];
  error: string | null;
  log_lines: number;
  /** Set when a call step in another run started this one. */
  parent?: { run_id: string; runbook_name: string; step: number };
  depth?: number;
  /** The run whose unfinished hosts this run retries. */
  retry_of?: string;
}

/** An inbound webhook: a secret URL that starts a run of one runbook. */
export interface Webhook {
  id: string;
  runbook_id: string;
  name: string;
  enabled: boolean;
  /** Runbook input name to a dotted path in the JSON body. */
  inputs: Record<string, string>;
  signed: boolean;
  created_at: number;
  deliveries: { at: number; status: number; run_id: string | null; error: string | null }[];
}

export interface WebhookInput {
  name: string;
  enabled: boolean;
  inputs: Record<string, string>;
  signed: boolean;
}

/** An API token for automation. Its value is shown only once, when it's created. */
/** One change in the audit log. */
export interface AuditEvent {
  id: string;
  at: number;
  actor: string;
  via?: string;
  action: string;
  workspace?: string;
  target?: { kind: string; id: string; name: string };
  detail?: string;
}

export interface AuditPage {
  events: AuditEvent[];
  next: string | null;
  retention_days: number;
}

/** What a user may do in a workspace; each role includes the ones before it. */
export type Role = "viewer" | "operator" | "editor" | "admin";
export const ROLES: Role[] = ["viewer", "operator", "editor", "admin"];

/** A workspace you can see, with your role in it. */
export interface MyWorkspace {
  slug: string;
  name: string;
  role: Role;
}

/** The signed-in user. */
export interface Me {
  user: string;
  /** Server admins manage accounts, workspaces, email and everyone's tokens, and are Admin everywhere. */
  admin: boolean;
  workspaces: MyWorkspace[];
  token: string | null;
  must_change_password: boolean;
  email?: string | null;
  /** False for accounts that sign in only with the provider. */
  password: boolean;
  /** The provider's label, when this account is linked to it. */
  signs_in_with: string | null;
}

export interface Workspace {
  slug: string;
  name: string;
  description: string;
  created_at: number;
  role?: Role;
  tag_rules?: TagRule[];
}

export interface User {
  name: string;
  admin: boolean;
  /** Role per workspace id. */
  workspaces: Record<string, Role>;
  created_at: number;
  must_change_password: boolean;
  last_sign_in_at: number | null;
  password_changed_at: number | null;
  /** Links the account to the sign-in provider on its first sign-in there. */
  email?: string;
  /** The provider account it signs in with, once linked. */
  oidc?: { issuer: string; subject: string };
}

export interface ApiToken {
  id: string;
  name: string;
  user: string;
  created_at: number;
  expires_at: number | null;
  last_used_at: number | null;
}

export interface LogLine {
  seq: number;
  ts: number;
  step: number;
  host: string;
  stream: "stdout" | "stderr" | "system";
  line: string;
}

export class ApiError extends Error {
  constructor(
    public status: number,
    message: string,
  ) {
    super(message);
  }
}

export const UNAUTHORIZED_EVENT = "opskills:unauthorized";

/** The workspace the app is showing. Requests carry it as `?workspace=`, unless they name one. */
let currentWorkspace = "default";

export function setApiWorkspace(slug: string) {
  currentWorkspace = slug;
}

function withWorkspace(path: string): string {
  if (/[?&]workspace=/.test(path)) return path;
  return `${path}${path.includes("?") ? "&" : "?"}workspace=${encodeURIComponent(currentWorkspace)}`;
}

async function request<T>(method: string, path: string, body?: unknown): Promise<T> {
  const headers: Record<string, string> = {};
  if (body !== undefined) headers["Content-Type"] = "application/json";
  if (method !== "GET") headers["X-OpSkills-Request"] = "1";
  const res = await fetch(`/api/v1${withWorkspace(path)}`, {
    method,
    credentials: "same-origin",
    headers,
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  if (res.status === 401 && path !== "/auth/login") {
    window.dispatchEvent(new Event(UNAUTHORIZED_EVENT));
  }
  if (!res.ok) {
    let detail = res.statusText || "Request failed";
    try {
      const problem = await res.json();
      detail = problem.detail ?? detail;
    } catch {
      /* not JSON */
    }
    throw new ApiError(res.status, detail);
  }
  if (res.status === 204 || res.status === 202) return undefined as T;
  return (await res.json()) as T;
}

export const api = {
  me: () => request<Me>("GET", "/auth/me"),
  changePassword: (current_password: string, new_password: string) => request<void>("POST", "/auth/password", { current_password, new_password }),
  users: () => request<User[]>("GET", "/users"),
  createUser: (name: string, admin: boolean, workspaces: Record<string, Role>, email?: string) =>
    request<{ user: User; password: string }>("POST", "/users", { name, admin, workspaces, ...(email ? { email } : {}) }),
  setServerAdmin: (name: string, admin: boolean) => request<User>("PUT", `/users/${encodeURIComponent(name)}`, { admin }),
  setUserEmail: (name: string, email: string) => request<User>("PUT", `/users/${encodeURIComponent(name)}`, { email }),
  unlinkSignIn: (name: string) => request<User>("PUT", `/users/${encodeURIComponent(name)}`, { unlink_sign_in: true }),
  signInSettings: () => request<{ settings: SignInSettings; redirect_url: string | null; public_url: string | null }>("GET", "/settings/sign-in"),
  saveSignInSettings: (s: SignInSettings) => request<{ settings: SignInSettings; redirect_url: string | null; public_url: string | null }>("PUT", "/settings/sign-in", s),
  testSignInProvider: (issuer: string) =>
    request<{ issuer: string; authorization_endpoint: string; token_endpoint: string | null; keys: number; redirect_url: string | null }>("POST", "/settings/sign-in/test", { issuer }),
  resetPassword: (name: string) => request<{ user: User; password: string }>("POST", `/users/${encodeURIComponent(name)}/reset-password`),
  deleteUser: (name: string) => request<void>("DELETE", `/users/${encodeURIComponent(name)}`),
  allTokens: () => request<ApiToken[]>("GET", "/tokens"),
  /** Everything for server admins (`workspace` empty), or one workspace. */
  audit: (p: { before?: string; action?: string; q?: string; limit?: number; workspace?: string }) => {
    const qs = new URLSearchParams();
    for (const [k, v] of Object.entries(p)) if (v !== undefined && v !== "") qs.set(k, String(v));
    qs.set("workspace", p.workspace ?? "");
    return request<AuditPage>("GET", `/audit?${qs}`);
  },
  /** One workspace's events, for its Admins. */
  workspaceAudit: (slug: string, p: { before?: string; action?: string; q?: string; limit?: number }) => {
    const qs = new URLSearchParams();
    for (const [k, v] of Object.entries(p)) if (v !== undefined && v !== "") qs.set(k, String(v));
    return request<AuditPage>("GET", `/workspaces/${encodeURIComponent(slug)}/audit${qs.size ? `?${qs}` : ""}`);
  },
  workspaces: () => request<Workspace[]>("GET", "/workspaces"),
  workspace: (slug: string) => request<{ workspace: Workspace; runbooks: number; hosts: number; members: number }>("GET", `/workspaces/${encodeURIComponent(slug)}`),
  createWorkspace: (slug: string, name: string, description: string) => request<Workspace>("POST", "/workspaces", { slug, name, description }),
  updateWorkspace: (slug: string, name: string, description: string) => request<Workspace>("PUT", `/workspaces/${encodeURIComponent(slug)}`, { name, description }),
  deleteWorkspace: (slug: string) => request<void>("DELETE", `/workspaces/${encodeURIComponent(slug)}`),
  members: (slug: string) => request<{ members: { name: string; role: Role }[]; server_admins: string[] }>("GET", `/workspaces/${encodeURIComponent(slug)}/members`),
  addableUsers: (slug: string) => request<string[]>("GET", `/workspaces/${encodeURIComponent(slug)}/addable-users`),
  setMember: (slug: string, name: string, role: Role) => request<{ name: string; role: Role }>("PUT", `/workspaces/${encodeURIComponent(slug)}/members/${encodeURIComponent(name)}`, { role }),
  removeMember: (slug: string, name: string) => request<void>("DELETE", `/workspaces/${encodeURIComponent(slug)}/members/${encodeURIComponent(name)}`),
  shareHost: (id: string, slug: string) => request<Host>("PUT", `/hosts/${encodeURIComponent(id)}/workspaces/${encodeURIComponent(slug)}`),
  unshareHost: (id: string, slug: string) => request<void>("DELETE", `/hosts/${encodeURIComponent(id)}/workspaces/${encodeURIComponent(slug)}`),
  revokeAnyToken: (id: string) => request<void>("DELETE", `/tokens/${encodeURIComponent(id)}`),
  tokens: () => request<ApiToken[]>("GET", "/me/tokens"),
  createToken: (name: string, expires_in_days: number | null) => request<{ token: ApiToken; value: string }>("POST", "/me/tokens", { name, expires_in_days }),
  revokeToken: (id: string) => request<void>("DELETE", `/me/tokens/${encodeURIComponent(id)}`),
  login: (username: string, password: string) => request<{ user: string; admin: boolean; must_change_password: boolean }>("POST", "/auth/login", { username, password }),
  logout: () => request<void>("POST", "/auth/logout"),
  runbooks: () => request<Runbook[]>("GET", "/runbooks"),
  runbook: (id: string) => request<Runbook>("GET", `/runbooks/${encodeURIComponent(id)}`),
  createRunbook: (input: RunbookInput) => request<Runbook>("POST", "/runbooks", input),
  updateRunbook: (id: string, input: RunbookInput) => request<Runbook>("PUT", `/runbooks/${encodeURIComponent(id)}`, input),
  deleteRunbook: (id: string) => request<void>("DELETE", `/runbooks/${encodeURIComponent(id)}`),
  startRun: (runbookId: string, inputs?: Record<string, string>) =>
    request<Run>("POST", `/runbooks/${encodeURIComponent(runbookId)}/runs`, inputs ? { inputs } : undefined),
  webhooks: (runbookId: string) => request<Webhook[]>("GET", `/runbooks/${encodeURIComponent(runbookId)}/webhooks`),
  createWebhook: (runbookId: string, w: WebhookInput) =>
    request<{ webhook: Webhook; token: string; signing_secret: string | null }>("POST", `/runbooks/${encodeURIComponent(runbookId)}/webhooks`, w),
  updateWebhook: (id: string, w: WebhookInput) => request<{ webhook: Webhook; signing_secret: string | null }>("PUT", `/webhooks/${encodeURIComponent(id)}`, w),
  rotateWebhook: (id: string) => request<{ token: string }>("POST", `/webhooks/${encodeURIComponent(id)}/rotate`),
  deleteWebhook: (id: string) => request<void>("DELETE", `/webhooks/${encodeURIComponent(id)}`),
  notificationSettings: () => request<{ email: boolean; from: string | null; source: EmailSource }>("GET", "/notifications/settings"),
  emailSettings: () => request<{ source: EmailSource; settings: EmailSettings | null }>("GET", "/settings/email"),
  saveEmailSettings: (s: EmailSettings) => request<{ source: EmailSource; settings: EmailSettings }>("PUT", "/settings/email", s),
  deleteEmailSettings: () => request<void>("DELETE", "/settings/email"),
  testNotification: (target: { email?: string[]; webhook?: string; headers?: Notification["headers"]; auth?: WebhookAuth; body?: string }) => request<{ sent: boolean; target: string }>("POST", "/notifications/test", target),
  runNotices: (runId: string) => request<Notice[]>("GET", `/runs/${encodeURIComponent(runId)}/notifications`),
  scheduleStatus: (runbookId: string) => request<ScheduleStatus[]>("GET", `/runbooks/${encodeURIComponent(runbookId)}/schedules`),
  previewSchedule: (cron: string, timezone: string) => request<{ next: number[] }>("POST", "/schedules/preview", { cron, timezone }),
  runs: (opts: { runbookId?: string; limit?: number } = {}) => {
    const q = new URLSearchParams();
    if (opts.runbookId) q.set("runbook_id", opts.runbookId);
    q.set("limit", String(opts.limit ?? 200));
    return request<Run[]>("GET", `/runs?${q}`);
  },
  run: (id: string) => request<Run>("GET", `/runs/${encodeURIComponent(id)}`),
  cancelRun: (id: string) => request<void>("POST", `/runs/${encodeURIComponent(id)}/cancel`),
  /** Rerun the hosts of a finished run that didn't finish well; secret inputs must be given again. */
  retryRun: (id: string, inputs?: Record<string, string>) => request<Run>("POST", `/runs/${encodeURIComponent(id)}/retry`, inputs ? { inputs } : undefined),
  secrets: () => request<Secret[]>("GET", "/secrets"),
  createSecret: (s: { name: string; kind: SecretKind; description: string; value: string }) => request<Secret>("POST", "/secrets", s),
  /** Change details, replace the value, or move it (`name`, only while unused). */
  updateSecret: (name: string, s: { name?: string; kind?: SecretKind; description?: string; value?: string }) => request<Secret>("PUT", `/secrets/${secretPath(name)}`, s),
  deleteSecret: (name: string) => request<void>("DELETE", `/secrets/${secretPath(name)}`),
  sharedSecrets: () => request<SharedSecret[]>("GET", "/shared-secrets"),
  createSharedSecret: (s: { name: string; kind: SecretKind; description: string; value: string; all_workspaces: boolean; workspaces: string[] }) =>
    request<SharedSecret>("POST", "/shared-secrets", s),
  updateSharedSecret: (name: string, s: { name?: string; kind?: SecretKind; description?: string; value?: string; all_workspaces?: boolean; workspaces?: string[] }) =>
    request<SharedSecret>("PUT", `/shared-secrets/${secretPath(name)}`, s),
  deleteSharedSecret: (name: string) => request<void>("DELETE", `/shared-secrets/${secretPath(name)}`),
  system: () => request<{ version: string; public_url: string | null; license: string; source_url: string; setup_done: boolean; sign_in?: SignInOptions }>("GET", "/system"),
  hosts: (filter?: string) => request<Host[]>("GET", filter ? `/hosts?filter=${encodeURIComponent(filter)}` : "/hosts"),
  updateHost: (id: string, h: { name?: string; tags?: string[]; attributes?: Record<string, string> }) => request<Host>("PUT", `/hosts/${encodeURIComponent(id)}`, h),
  inventorySources: () => request<InventorySource[]>("GET", "/inventory/sources"),
  inventorySource: (id: string) => request<InventorySource>("GET", `/inventory/sources/${encodeURIComponent(id)}`),
  createInventorySource: (s: SourceInput) => request<InventorySource>("POST", "/inventory/sources", s),
  updateInventorySource: (id: string, s: SourceInput) => request<InventorySource>("PUT", `/inventory/sources/${encodeURIComponent(id)}`, s),
  deleteInventorySource: (id: string) => request<void>("DELETE", `/inventory/sources/${encodeURIComponent(id)}`),
  refreshInventorySource: (id: string, acceptEmpty = false) => request<InventorySource>("POST", `/inventory/sources/${encodeURIComponent(id)}/refresh`, { accept_empty: acceptEmpty }),
  testSsh: (id: string) =>
    request<{ ok: boolean; output?: string[]; error: string | null; pinned_now: string[] | null; fingerprints?: string[] }>("POST", `/hosts/${encodeURIComponent(id)}/ssh/test`),
  sshKeys: (id: string) => request<{ pinned: string[]; presented: { type: string; fingerprint: string; pinned: boolean }[] }>("GET", `/hosts/${encodeURIComponent(id)}/ssh/keys`),
  acceptSshKeys: (id: string, fingerprints: string[]) => request<{ pinned: string[] }>("POST", `/hosts/${encodeURIComponent(id)}/ssh/accept`, { fingerprints }),
  setTagRules: (slug: string, rules: TagRule[]) => request<{ rules: TagRule[] }>("PUT", `/workspaces/${encodeURIComponent(slug)}/tag-rules`, { rules }),
  deleteHost: (id: string) => request<void>("DELETE", `/hosts/${encodeURIComponent(id)}`),
  createEnrollmentToken: () => request<{ token: string; expires_at: number }>("POST", "/hosts/enrollment-tokens"),
  agentPackages: () => request<{ version: string; packages: AgentPackage[] }>("GET", "/hosts/packages"),
};

/** Load data, optionally refreshing on an interval. Keeps showing the last data while refreshing. */
export function useData<T>(load: () => Promise<T>, deps: unknown[], pollMs?: number) {
  const [data, setData] = useState<T | undefined>(undefined);
  const [error, setError] = useState<string | null>(null);
  const loadRef = useRef(load);
  loadRef.current = load;

  const reload = useCallback(async () => {
    try {
      setData(await loadRef.current());
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, []);

  useEffect(() => {
    let alive = true;
    setData(undefined);
    reload();
    if (!pollMs) return;
    const t = setInterval(() => {
      if (alive && document.visibilityState === "visible") reload();
    }, pollMs);
    return () => {
      alive = false;
      clearInterval(t);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, deps);

  return { data, error, loading: data === undefined && error === null, reload };
}

export function isActive(status: RunStatus) {
  return status === "queued" || status === "running";
}
