// Typed client for the OpSkills /api/v1 HTTP API.

import { useCallback, useEffect, useRef, useState } from "react";

export type RunStatus = "pending" | "queued" | "running" | "succeeded" | "failed" | "cancelled" | "skipped";

export interface Step {
  name: string;
  run: string;
}

export interface Runbook {
  id: string;
  name: string;
  description: string;
  tags: string[];
  steps: Step[];
  secrets: string[];
  timeout_seconds: number;
  created_at: number;
  updated_at: number;
}

export interface RunbookInput {
  name: string;
  description: string;
  tags: string[];
  steps: Step[];
  secrets: string[];
  timeout_seconds?: number;
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
  status: RunStatus;
  exit_code: number | null;
  started_at: number | null;
  finished_at: number | null;
}

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
  steps: StepResult[];
  error: string | null;
  log_lines: number;
}

export interface LogLine {
  seq: number;
  ts: number;
  step: number;
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
  startRun: (runbookId: string) => request<Run>("POST", `/runbooks/${encodeURIComponent(runbookId)}/runs`),
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
