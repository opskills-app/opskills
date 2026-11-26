import { useState } from "react";
import { ArrowLeft, Copy, FileText, Pencil, Play, Terminal, Trash2, Workflow } from "lucide-react";
import { api, LOCAL_HOST_ID, MAX_OUTPUT_LINES, runsOnServer, useData, type FailurePolicy, type Host, type Rollout, type Runbook, type Step } from "../api";
import { inputLabel } from "../components/StartRun";
import { WebhooksCard } from "../components/Webhooks";
import { duration, fromNow, runDuration, runNumber, timeAgo, when } from "../format";
import { Button, Card, CardHeader, cx, ErrorNote, PageHeader, StatusPill, Tag } from "../components/ui";
import { useRunNow } from "./Runbooks";
import { go } from "../router";

function toYaml(rb: Runbook) {
  const q = (s: string) => (/^[\w .,/()-]*$/.test(s) && s.trim() === s && s !== "" ? s : JSON.stringify(s));
  const lines = [`name: ${q(rb.name)}`];
  if (rb.description) lines.push(`description: ${q(rb.description)}`);
  if (rb.tags.length) lines.push(`tags: [${rb.tags.join(", ")}]`);
  if (rb.inputs?.length) {
    lines.push("inputs:");
    for (const d of rb.inputs) {
      lines.push(`  - name: ${d.name}`, `    type: ${d.type}`);
      if (d.label) lines.push(`    label: ${q(d.label)}`);
      if (d.description) lines.push(`    description: ${q(d.description)}`);
      if (d.required) lines.push("    required: true");
      if (d.default !== null) lines.push(`    default: ${q(d.default)}`);
      if (d.choices.length) lines.push(`    choices: [${d.choices.map(q).join(", ")}]`);
      if (d.pattern) lines.push(`    pattern: ${JSON.stringify(d.pattern)}`);
    }
  }
  if (rb.secrets?.length) lines.push(`secrets: [${rb.secrets.join(", ")}]`);
  if (!runsOnServer(rb.targets)) {
    lines.push("run_on:");
    if (rb.targets.hosts.length) lines.push(`  hosts: [${rb.targets.hosts.join(", ")}]`);
    if (rb.targets.tags.length) lines.push(`  tags: [${rb.targets.tags.join(", ")}]`);
    if (rb.targets.filter?.trim()) lines.push(`  filter: ${JSON.stringify(rb.targets.filter)}`);
  }
  lines.push(`timeout_seconds: ${rb.timeout_seconds}`);
  if (rb.on_step_failure === "finish_then_fail") lines.push("on_step_failure: finish_then_fail");
  const ro = rb.rollout;
  if (ro && (ro.mode !== "sequential" || ro.canary || ro.on_host_failure)) {
    lines.push("rollout:", `  mode: ${ro.mode}`);
    if (ro.mode === "parallel" && ro.max_parallel) lines.push(`  max_parallel: ${ro.max_parallel}`);
    if (ro.mode === "batches") lines.push(`  batch_size: ${JSON.stringify(ro.batch_size)}`);
    if (ro.canary) lines.push(`  canary: ${ro.canary}`);
    if (ro.pause_seconds) lines.push(`  pause_seconds: ${ro.pause_seconds}`);
    if (ro.on_host_failure) lines.push(`  on_host_failure: ${ro.on_host_failure}`);
  }
  if (rb.output_limit && (rb.output_limit.lines !== MAX_OUTPUT_LINES || rb.output_limit.action !== "truncate")) {
    lines.push("output_limit:", `  lines: ${rb.output_limit.lines}`, `  action: ${rb.output_limit.action}`);
  }
  if (rb.schedules?.length) {
    lines.push("schedules:");
    for (const s of rb.schedules) {
      lines.push(s.cron ? `  - cron: ${JSON.stringify(s.cron)}` : `  - at: ${new Date(s.at ?? 0).toISOString()}`);
      if (s.name) lines.push(`    name: ${q(s.name)}`);
      if (s.cron) lines.push(`    timezone: ${s.timezone}`);
      if (!s.enabled) lines.push("    enabled: false");
      const ins = Object.entries(s.inputs ?? {});
      if (ins.length) {
        lines.push("    inputs:");
        for (const [k, v] of ins) lines.push(`      ${k}: ${q(v)}`);
      }
    }
  }
  lines.push("steps:");
  for (const s of rb.steps) {
    lines.push(`  - name: ${q(s.name)}`);
    const block = (key: string, text: string, indent: string) => {
      if (text.includes("\n")) {
        lines.push(`${indent}${key}: |`);
        for (const l of text.split("\n")) lines.push(`${indent}  ${l}`);
      } else {
        lines.push(`${indent}${key}: ${q(text)}`);
      }
    };
    if (s.copy) {
      lines.push("    copy:", `      to: ${q(s.copy.to)}`);
      if (s.copy.secret !== undefined) lines.push(`      secret: ${s.copy.secret}`);
      else block("content", s.copy.content ?? "", "      ");
      if (s.copy.mode) lines.push(`      mode: "${s.copy.mode}"`);
    } else if (s.call) {
      lines.push("    call:", `      runbook: ${s.call.runbook}`);
      const entries = Object.entries(s.call.inputs ?? {});
      if (entries.length) {
        lines.push("      inputs:");
        for (const [k, v] of entries) lines.push(`        ${k}: ${q(v)}`);
      }
    } else {
      block("run", s.run, "    ");
    }
    if (s.timeout_seconds) lines.push(`    timeout_seconds: ${s.timeout_seconds}`);
    if (s.retries) lines.push(`    retries: ${s.retries}`);
    if (s.retry_delay_seconds) lines.push(`    retry_delay_seconds: ${s.retry_delay_seconds}`);
    if (s.continue_on_error) lines.push("    continue_on_error: true");
  }
  return lines.join("\n");
}

/** A rollout in words, like "In batches of 25%, after 1 canary host, 30s apart". */
export function rolloutText(r: Rollout | undefined) {
  if (!r) return "One host at a time";
  const secs = typeof r.pause_seconds === "number" ? r.pause_seconds : 0;
  const parts = [
    r.mode === "parallel" ? (r.max_parallel ? `Up to ${r.max_parallel} hosts at once` : "All hosts at once") : r.mode === "batches" ? `In batches of ${r.batch_size}` : "One host at a time",
  ];
  if (r.canary) parts.push(`after ${r.canary} canary host${r.canary === 1 ? "" : "s"}`);
  if (secs && (r.canary || r.mode === "batches")) parts.push(`${duration(secs * 1000)} apart`);
  return parts.join(", ");
}

/** What a failed host means for the others, in words. */
export function hostFailureText(r: Rollout | undefined, steps: FailurePolicy | undefined) {
  const stop = r?.on_host_failure ? r.on_host_failure === "stop" : (steps ?? "stop") === "stop";
  return stop ? "Hosts not started yet are skipped" : "The other hosts still run";
}

/** Short words for a step's non-default options. */
export function stepOptions(s: { timeout_seconds?: number | string | null; retries?: number; retry_delay_seconds?: number | string; continue_on_error?: boolean }) {
  const out: string[] = [];
  if (s.timeout_seconds) out.push(`timeout ${typeof s.timeout_seconds === "number" ? duration(s.timeout_seconds * 1000) : s.timeout_seconds}`);
  if (s.retries) {
    const delay = s.retry_delay_seconds ? `, ${typeof s.retry_delay_seconds === "number" ? duration(s.retry_delay_seconds * 1000) : s.retry_delay_seconds} apart` : "";
    out.push(`${s.retries} ${s.retries === 1 ? "retry" : "retries"}${delay}`);
  }
  if (s.continue_on_error) out.push("continues on error");
  return out;
}

const pre = "scroll-thin mt-3 overflow-x-auto rounded-lg bg-surface-2 px-3 py-2 font-mono text-[12.5px] text-ink-2";

/** What a step does: its command, the file it writes, or the runbook it calls. */
function StepBody({ step }: { step: Step }) {
  const runbooks = useData(() => (step.call ? api.runbooks() : Promise.resolve([] as Runbook[])), [step.call?.runbook]);
  if (step.copy) {
    const c = step.copy;
    return (
      <>
        <p className="mt-3 flex flex-wrap items-center gap-x-2 gap-y-1 text-sm text-ink-2">
          <FileText className="size-4 text-muted" /> Writes <code className="font-mono text-[12.5px]">{c.to}</code>
          <span className="text-xs text-muted">mode {c.mode ?? (c.secret !== undefined ? "0600" : "0644")}</span>
          {c.secret !== undefined && (
            <>
              from the vault secret{" "}
              <a href="#/vault" className="font-mono text-[12.5px] hover:text-accent">
                ${c.secret}
              </a>
            </>
          )}
        </p>
        {c.secret === undefined && <pre className={pre}>{c.content}</pre>}
      </>
    );
  }
  if (step.call) {
    const target = (runbooks.data ?? []).find((r) => r.id === step.call!.runbook);
    const inputs = Object.entries(step.call.inputs ?? {});
    return (
      <div className="mt-3 text-sm text-ink-2">
        <p className="flex items-center gap-2">
          <Workflow className="size-4 text-muted" /> Runs{" "}
          <a href={`#/runbooks/${step.call.runbook}`} className="font-medium text-accent hover:underline">
            {target?.name ?? "another runbook"}
          </a>{" "}
          on the same host and waits for it
        </p>
        {inputs.length > 0 && (
          <dl className="mt-2 space-y-1 rounded-lg bg-surface-2 px-3 py-2 font-mono text-[12px]">
            {inputs.map(([k, v]) => (
              <div key={k} className="flex gap-2">
                <dt className="text-muted">{k}:</dt>
                <dd className="min-w-0 break-all">{v}</dd>
              </div>
            ))}
          </dl>
        )}
      </div>
    );
  }
  return <pre className={pre}>{step.run}</pre>;
}

/** Each schedule with its next run and how the last one went. */
function SchedulesCard({ rb }: { rb: Runbook }) {
  const status = useData(() => api.scheduleStatus(rb.id), [rb.id, rb.updated_at], 30000);
  if (!rb.schedules?.length) return null;
  const byId = new Map((status.data ?? []).map((s) => [s.id, s]));
  return (
    <Card>
      <CardHeader title="Schedules" />
      <ul className="space-y-4 px-6 pb-6 text-sm sm:px-8">
        {rb.schedules.map((s) => {
          const st = byId.get(s.id);
          const next = st?.next[0];
          return (
            <li key={s.id}>
              <div className="flex items-baseline justify-between gap-3">
                <span className="font-medium">{s.name || (s.cron ? "Repeating" : "Once")}</span>
                <code className="font-mono text-[11px] text-muted">{s.cron ? `${s.cron} ${s.timezone}` : s.at ? when(s.at) : ""}</code>
              </div>
              <p className="mt-0.5 text-xs text-muted">
                {!s.enabled ? "Off" : next ? `Next: ${when(next)} (${fromNow(next)})` : s.cron ? "No upcoming time" : "Done"}
                {st?.last_run_id && (
                  <>
                    {" · last "}
                    <a href={`#/runs/${st.last_run_id}`} className="text-accent hover:underline">
                      run #{runNumber(st.last_run_id)}
                    </a>
                  </>
                )}
              </p>
              {st?.last_error && <p className="mt-1 text-xs text-rose-600 dark:text-rose-400">Last time it couldn't start: {st.last_error}</p>}
            </li>
          );
        })}
      </ul>
    </Card>
  );
}

function failureText(rb: Runbook) {
  return rb.on_step_failure === "finish_then_fail" ? "A failing step fails the run, but the remaining steps still run." : "The first failing step stops the run.";
}

/** Where a runbook runs, in words, with links to its hosts. */
function RunOnSummary({ rb, hosts }: { rb: Runbook; hosts: Host[] }) {
  if (runsOnServer(rb.targets)) return <span>OpSkills server</span>;
  const byId = new Map(hosts.map((h) => [h.id, h]));
  const named = rb.targets.hosts.map((id) => (id === LOCAL_HOST_ID ? "OpSkills server" : (byId.get(id)?.name ?? "a removed host")));
  const tagged = rb.targets.tags.map((t) => `tag ${t}`);
  const filtered = rb.targets.filter?.trim() ? [`hosts matching ${rb.targets.filter}`] : [];
  // The server knows which hosts a runbook targets, filter included: they list it as a user.
  const count = hosts.filter((h) => (h.builtin ? rb.targets.hosts.includes(LOCAL_HOST_ID) : h.used_by.some((u) => u.id === rb.id))).length;
  return (
    <span className="text-right">
      {[...named, ...tagged, ...filtered].join(", ")}
      <a href="#/hosts" className="block text-xs text-muted hover:text-accent">
        {count} host{count === 1 ? "" : "s"} now
      </a>
    </span>
  );
}

export function RunbookDetail({ id }: { id: string }) {
  const rb = useData(() => api.runbook(id), [id]);
  const hosts = useData(() => api.hosts(), []);
  const runs = useData(() => api.runs({ runbookId: id, limit: 100 }), [id], 5000);
  const [tab, setTab] = useState<"steps" | "runs" | "yaml">("steps");
  const [copied, setCopied] = useState(false);
  const [deleteError, setDeleteError] = useState<string | null>(null);
  const { runNow, busy, error, dialog } = useRunNow();

  if (rb.error) return <ErrorNote>{rb.error}</ErrorNote>;
  if (!rb.data) return <p className="text-sm text-muted">Loading...</p>;
  const r = rb.data;
  const history = runs.data ?? [];

  async function remove() {
    if (!window.confirm(`Delete the runbook "${r.name}"? Its past runs stay in the run history.`)) return;
    try {
      await api.deleteRunbook(r.id);
      go("runbooks");
    } catch (e) {
      setDeleteError(e instanceof Error ? e.message : String(e));
    }
  }

  return (
    <>
      {dialog}
      <a href="#/runbooks" className="mb-3 inline-flex items-center gap-1 text-sm text-muted hover:text-ink">
        <ArrowLeft className="size-3.5" /> Runbooks
      </a>
      <PageHeader
        title={r.name}
        subtitle={r.description || undefined}
        actions={
          <>
            <Button onClick={remove} aria-label="Delete runbook">
              <Trash2 className="size-4" />
            </Button>
            <Button onClick={() => go(`runbooks/${r.id}/edit`)}>
              <Pencil className="size-3.5" /> Edit
            </Button>
            <Button variant="primary" disabled={busy === r.id} onClick={() => runNow(r.id)}>
              <Play className="size-3.5" fill="currentColor" /> {busy === r.id ? "Starting..." : "Run now"}
            </Button>
          </>
        }
      />
      {(error || deleteError) && (
        <div className="mb-6">
          <ErrorNote>{error ?? deleteError}</ErrorNote>
        </div>
      )}

      <div className="mb-5 flex gap-1 border-b border-line">
        {(
          [
            ["steps", `Steps (${r.steps.length})`],
            ["runs", `Runs (${history.length})`],
            ["yaml", "YAML"],
          ] as const
        ).map(([k, label]) => (
          <button
            key={k}
            onClick={() => setTab(k)}
            className={cx("-mb-px border-b-2 px-3 py-2 text-sm font-medium", tab === k ? "border-accent text-ink" : "border-transparent text-muted hover:text-ink")}
          >
            {label}
          </button>
        ))}
      </div>

      <div className="grid grid-cols-1 gap-6 xl:grid-cols-[minmax(0,1fr)_340px]">
        <div className="min-w-0">
          {tab === "steps" && (
            <ol className="relative space-y-3">
              {r.steps.map((s, i) => (
                <li key={i} className="relative flex gap-4">
                  {i < r.steps.length - 1 && <span className="absolute top-10 bottom-[-12px] left-[19px] w-px bg-line" />}
                  <span className="relative z-10 flex size-10 shrink-0 items-center justify-center rounded-full bg-accent-soft text-accent">
                    {s.copy ? <FileText className="size-4" /> : s.call ? <Workflow className="size-4" /> : <Terminal className="size-4" />}
                  </span>
                  <Card className="min-w-0 flex-1 p-5 sm:p-6">
                    <div className="flex flex-wrap items-center gap-2">
                      <span className="text-xs font-medium text-muted tabular-nums">{String(i + 1).padStart(2, "0")}</span>
                      <h3 className="font-medium">{s.name}</h3>
                      {stepOptions(s).map((o) => (
                        <span key={o} className="rounded-md bg-surface-2 px-2 py-0.5 text-[11px] text-muted">
                          {o}
                        </span>
                      ))}
                    </div>
                    <StepBody step={s} />
                  </Card>
                </li>
              ))}
            </ol>
          )}

          {tab === "runs" && (
            <Card className="overflow-hidden">
              <ul className="divide-y divide-line">
                {history.length === 0 && <li className="p-8 text-center text-sm text-muted">No runs yet. Press Run now to start the first one.</li>}
                {history.map((run) => (
                  <li key={run.id}>
                    <a href={`#/runs/${run.id}`} className="flex flex-wrap items-center gap-x-4 gap-y-1 px-6 py-4 hover:bg-surface-2">
                      <StatusPill status={run.status} />
                      <span className="font-mono text-xs text-muted">#{runNumber(run.id)}</span>
                      <span className="flex-1 text-sm">{run.started_by}</span>
                      <span className="text-xs text-muted">{timeAgo(run.created_at)}</span>
                      <span className="w-20 text-right text-xs text-muted tabular-nums">{duration(runDuration(run))}</span>
                    </a>
                  </li>
                ))}
              </ul>
            </Card>
          )}

          {tab === "yaml" && (
            <Card className="overflow-hidden">
              <div className="flex items-center justify-between border-b border-line px-6 py-3 text-xs text-muted">
                <span>Read-only view of this runbook</span>
                <Button
                  size="sm"
                  variant="ghost"
                  onClick={() => {
                    navigator.clipboard.writeText(toYaml(r)).then(() => {
                      setCopied(true);
                      setTimeout(() => setCopied(false), 1500);
                    });
                  }}
                >
                  <Copy className="size-3.5" /> {copied ? "Copied" : "Copy"}
                </Button>
              </div>
              <pre className="scroll-thin overflow-x-auto bg-term p-5 font-mono text-[12.5px] leading-relaxed text-term-ink">{toYaml(r)}</pre>
            </Card>
          )}
        </div>

        <aside className="space-y-4">
          <Card>
            <CardHeader title="Details" />
            <dl className="space-y-3 px-6 pb-6 text-sm sm:px-8">
              <div className="flex justify-between gap-4">
                <dt className="text-muted">Runs on</dt>
                <dd className="min-w-0">
                  <RunOnSummary rb={r} hosts={hosts.data ?? []} />
                </dd>
              </div>
              <div className="flex justify-between gap-4">
                <dt className="text-muted">Steps</dt>
                <dd>{r.steps.length}</dd>
              </div>
              <div className="flex justify-between gap-4">
                <dt className="text-muted">Step timeout</dt>
                <dd>{duration(r.timeout_seconds * 1000)}</dd>
              </div>
              {!runsOnServer(r.targets) && (
                <>
                  <div className="flex justify-between gap-4">
                    <dt className="text-muted">Rollout</dt>
                    <dd className="text-right">{rolloutText(r.rollout)}</dd>
                  </div>
                  <div className="flex justify-between gap-4">
                    <dt className="text-muted">If a host fails</dt>
                    <dd className="text-right">{hostFailureText(r.rollout, r.on_step_failure)}</dd>
                  </div>
                </>
              )}
              <div className="flex justify-between gap-4">
                <dt className="text-muted">If a step fails</dt>
                <dd className="text-right">{r.on_step_failure === "finish_then_fail" ? "Finish, then fail" : "Stop the run"}</dd>
              </div>
              {r.output_limit && (r.output_limit.lines !== MAX_OUTPUT_LINES || r.output_limit.action === "fail") && (
                <div className="flex justify-between gap-4">
                  <dt className="text-muted">Output limit</dt>
                  <dd className="text-right">
                    {r.output_limit.lines.toLocaleString()} lines, then {r.output_limit.action === "fail" ? "stop" : "drop output"}
                  </dd>
                </div>
              )}
              {r.inputs?.length > 0 && (
                <div>
                  <dt className="text-muted">Inputs</dt>
                  <dd className="mt-2 space-y-1.5">
                    {r.inputs.map((d) => (
                      <div key={d.name} className="flex items-baseline justify-between gap-3">
                        <span>
                          {inputLabel(d)}
                          {d.required && <span className="text-rose-600"> *</span>}
                        </span>
                        <code className="font-mono text-[11px] text-muted">{d.type}</code>
                      </div>
                    ))}
                  </dd>
                </div>
              )}
              <div className="flex justify-between gap-4">
                <dt className="text-muted">Created</dt>
                <dd>{timeAgo(r.created_at)}</dd>
              </div>
              <div className="flex justify-between gap-4">
                <dt className="text-muted">Last edited</dt>
                <dd>{timeAgo(r.updated_at)}</dd>
              </div>
              {r.secrets?.length > 0 && (
                <div>
                  <dt className="text-muted">Secrets</dt>
                  <dd className="mt-2 flex flex-wrap gap-1.5">
                    {r.secrets.map((n) => (
                      <a key={n} href="#/vault" className="inline-flex items-center rounded-md bg-surface-2 px-2 py-0.5 font-mono text-[11px] text-ink-2 hover:text-accent">
                        ${n}
                      </a>
                    ))}
                  </dd>
                </div>
              )}
              {r.tags.length > 0 && (
                <div>
                  <dt className="text-muted">Tags</dt>
                  <dd className="mt-2 flex flex-wrap gap-1.5">
                    {r.tags.map((t) => (
                      <Tag key={t}>{t}</Tag>
                    ))}
                  </dd>
                </div>
              )}
            </dl>
          </Card>
          <SchedulesCard rb={r} />
          <WebhooksCard rb={r} />
          {history[0] && (
            <Card>
              <CardHeader title="Last run" />
              <a href={`#/runs/${history[0].id}`} className="flex items-center justify-between gap-3 px-6 pb-6 text-sm hover:text-accent sm:px-8">
                <span>
                  #{runNumber(history[0].id)} · {timeAgo(history[0].created_at)}
                </span>
                <StatusPill status={history[0].status} />
              </a>
            </Card>
          )}
          <Card className="p-6 text-sm text-muted sm:px-8">
            {runsOnServer(r.targets)
              ? `Steps run one after another on the OpSkills server, as the user the server runs as. ${failureText(r)}`
              : `On each host, steps run one after another, as root through the OpSkills agent. ${rolloutText(r.rollout)}. ${failureText(r)}`}
          </Card>
        </aside>
      </div>
    </>
  );
}
