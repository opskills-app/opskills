import { useState } from "react";
import { ArrowLeft, Copy, Pencil, Play, Terminal, Trash2 } from "lucide-react";
import { api, LOCAL_HOST_ID, runsOnServer, useData, type Host, type Runbook } from "../api";
import { duration, runDuration, runNumber, timeAgo } from "../format";
import { Button, Card, CardHeader, cx, ErrorNote, PageHeader, StatusPill, Tag } from "../components/ui";
import { useRunNow } from "./Runbooks";
import { go } from "../router";

function toYaml(rb: Runbook) {
  const q = (s: string) => (/^[\w .,/()-]*$/.test(s) && s.trim() === s && s !== "" ? s : JSON.stringify(s));
  const lines = [`name: ${q(rb.name)}`];
  if (rb.description) lines.push(`description: ${q(rb.description)}`);
  if (rb.tags.length) lines.push(`tags: [${rb.tags.join(", ")}]`);
  if (rb.secrets?.length) lines.push(`secrets: [${rb.secrets.join(", ")}]`);
  if (!runsOnServer(rb.targets)) {
    lines.push("run_on:");
    if (rb.targets.hosts.length) lines.push(`  hosts: [${rb.targets.hosts.join(", ")}]`);
    if (rb.targets.tags.length) lines.push(`  tags: [${rb.targets.tags.join(", ")}]`);
  }
  lines.push(`timeout_seconds: ${rb.timeout_seconds}`, "steps:");
  for (const s of rb.steps) {
    lines.push(`  - name: ${q(s.name)}`);
    if (s.run.includes("\n")) {
      lines.push("    run: |");
      for (const l of s.run.split("\n")) lines.push(`      ${l}`);
    } else {
      lines.push(`    run: ${q(s.run)}`);
    }
  }
  return lines.join("\n");
}

/** Where a runbook runs, in words, with links to its hosts. */
function RunOnSummary({ rb, hosts }: { rb: Runbook; hosts: Host[] }) {
  if (runsOnServer(rb.targets)) return <span>OpSkills server</span>;
  const byId = new Map(hosts.map((h) => [h.id, h]));
  const named = rb.targets.hosts.map((id) => (id === LOCAL_HOST_ID ? "OpSkills server" : (byId.get(id)?.name ?? "a removed host")));
  const tagged = rb.targets.tags.map((t) => `tag ${t}`);
  const count = hosts.filter((h) => (h.builtin ? rb.targets.hosts.includes(LOCAL_HOST_ID) : rb.targets.hosts.includes(h.id) || h.tags.some((t) => rb.targets.tags.includes(t)))).length;
  return (
    <span className="text-right">
      {[...named, ...tagged].join(", ")}
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
  const { runNow, busy, error } = useRunNow();

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
                    <Terminal className="size-4" />
                  </span>
                  <Card className="min-w-0 flex-1 p-5 sm:p-6">
                    <div className="flex items-center gap-2">
                      <span className="text-xs font-medium text-muted tabular-nums">{String(i + 1).padStart(2, "0")}</span>
                      <h3 className="font-medium">{s.name}</h3>
                    </div>
                    <pre className="scroll-thin mt-3 overflow-x-auto rounded-lg bg-surface-2 px-3 py-2 font-mono text-[12.5px] text-ink-2">{s.run}</pre>
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
              ? "Steps run one after another on the OpSkills server, as the user the server runs as. The first failing step stops the run."
              : "Steps run one after another on each host in turn, as root through the OpSkills agent. The first failing step stops the run."}
          </Card>
        </aside>
      </div>
    </>
  );
}
