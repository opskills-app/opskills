// Typed client for the OpSkills /api/v1 HTTP API.

import { useCallback, useEffect, useRef, useState } from "react";

export type RunStatus = "pending" | "queued" | "running" | "succeeded" | "failed" | "cancelled" | "skipped";

export interface Step {
  name: string;
  run: string;
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

/** Where a runbook runs. Both empty: on the OpSkills server. */
export interface Targets {
  hosts: string[];
  tags: string[];
}

export const LOCAL_HOST_ID = "local";

export function runsOnServer(t: Targets | undefined) {
  return !t || (t.hosts.length === 0 && t.tags.length === 0);
}

export interface Runbook {
  id: string;
  name: string;
  description: string;
  tags: string[];
  inputs: InputDef[];
  steps: Step[];
  on_step_failure: FailurePolicy;
  output_limit: OutputLimit;
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
  secrets: string[];
  targets: Targets;
  /** Seconds, or a duration string like "10m". */
  timeout_seconds?: number | string;
}

export interface Host {
  id: string;
  name: string;
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
}

export interface AgentPackage {
  format: "deb" | "rpm";
  arch: "amd64" | "arm64";
  file: string;
  bundled: boolean;
}

export type SecretKind = "password" | "token" | "ssh_key" | "certificate" | "other";

export interface Secret {
  name: string;
  kind: SecretKind;
  description: string;
  created_at: number;
  updated_at: number;
  used_by: { id: string; name: string }[];
}

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
}

/** Shown in place of a secret value. */
export const MASK = "••••••";

export interface Run {
  id: string;
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
  hosts: { id: string; name: string }[];
  steps: StepResult[];
  error: string | null;
  log_lines: number;
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

async function request<T>(method: string, path: string, body?: unknown): Promise<T> {
  const headers: Record<string, string> = {};
  if (body !== undefined) headers["Content-Type"] = "application/json";
  if (method !== "GET") headers["X-OpSkills-Request"] = "1";
  const res = await fetch(`/api/v1${path}`, {
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
  me: () => request<{ user: string }>("GET", "/auth/me"),
  login: (username: string, password: string) => request<{ user: string }>("POST", "/auth/login", { username, password }),
  logout: () => request<void>("POST", "/auth/logout"),
  runbooks: () => request<Runbook[]>("GET", "/runbooks"),
  runbook: (id: string) => request<Runbook>("GET", `/runbooks/${encodeURIComponent(id)}`),
  createRunbook: (input: RunbookInput) => request<Runbook>("POST", "/runbooks", input),
  updateRunbook: (id: string, input: RunbookInput) => request<Runbook>("PUT", `/runbooks/${encodeURIComponent(id)}`, input),
  deleteRunbook: (id: string) => request<void>("DELETE", `/runbooks/${encodeURIComponent(id)}`),
  startRun: (runbookId: string, inputs?: Record<string, string>) =>
    request<Run>("POST", `/runbooks/${encodeURIComponent(runbookId)}/runs`, inputs ? { inputs } : undefined),
  runs: (opts: { runbookId?: string; limit?: number } = {}) => {
    const q = new URLSearchParams();
    if (opts.runbookId) q.set("runbook_id", opts.runbookId);
    q.set("limit", String(opts.limit ?? 200));
    return request<Run[]>("GET", `/runs?${q}`);
  },
  run: (id: string) => request<Run>("GET", `/runs/${encodeURIComponent(id)}`),
  cancelRun: (id: string) => request<void>("POST", `/runs/${encodeURIComponent(id)}/cancel`),
  secrets: () => request<Secret[]>("GET", "/secrets"),
  createSecret: (s: { name: string; kind: SecretKind; description: string; value: string }) => request<Secret>("POST", "/secrets", s),
  updateSecret: (name: string, s: { kind?: SecretKind; description?: string; value?: string }) =>
    request<Secret>("PUT", `/secrets/${encodeURIComponent(name)}`, s),
  deleteSecret: (name: string) => request<void>("DELETE", `/secrets/${encodeURIComponent(name)}`),
  system: () => request<{ version: string; public_url: string | null }>("GET", "/system"),
  hosts: () => request<Host[]>("GET", "/hosts"),
  updateHost: (id: string, h: { name?: string; tags?: string[] }) => request<Host>("PUT", `/hosts/${encodeURIComponent(id)}`, h),
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
