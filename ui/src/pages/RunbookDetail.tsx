import { useState } from "react";
import { ArrowLeft, Copy, FileCode2, Globe, GitBranch, Hand, Pencil, Play, Terminal, Webhook, CalendarClock, MousePointerClick, Workflow } from "lucide-react";
import { runbookById, runs, type Step } from "../data";
import { Avatar, Button, Card, CardHeader, cx, PageHeader, StatusPill, Tag } from "../components/ui";
import { go } from "../router";

const stepIcon: Record<Step["kind"], typeof Terminal> = {
  command: Terminal,
  script: FileCode2,
  http: Globe,
  approval: Hand,
  copy: Copy,
  runbook: Workflow,
};

const stepLabel: Record<Step["kind"], string> = {
  command: "Command",
  script: "Script",
  http: "HTTP check",
  approval: "Approval gate",
  copy: "Copy files",
  runbook: "Call runbook",
};

export function RunbookDetail({ id }: { id: string }) {
  const rb = runbookById(id);
  const [tab, setTab] = useState<"steps" | "runs" | "yaml">("steps");
  if (!rb) return <p className="text-muted">Runbook not found.</p>;
  const history = runs.filter((r) => r.runbookId === rb.id);

  return (
    <>
      <a href="#/runbooks" className="mb-3 inline-flex items-center gap-1 text-sm text-muted hover:text-ink">
        <ArrowLeft className="size-3.5" /> Runbooks
      </a>
      <PageHeader
        title={rb.name}
        subtitle={rb.description}
        actions={
          <>
            <Button>
              <Pencil className="size-3.5" /> Edit
            </Button>
            <Button variant="primary" onClick={() => go("runs/r-2741")}>
              <Play className="size-3.5" fill="currentColor" /> Run now
            </Button>
          </>
        }
      />

      <div className="mb-5 flex gap-1 border-b border-line">
        {(
          [
            ["steps", "Steps"],
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
        <div>
          {tab === "steps" && (
            <ol className="relative space-y-3">
              {rb.steps.map((s, i) => {
                const Icon = stepIcon[s.kind];
                return (
                  <li key={i} className="relative flex gap-4">
                    {i < rb.steps.length - 1 && <span className="absolute top-10 bottom-[-12px] left-[19px] w-px bg-line" />}
                    <span
                      className={cx(
                        "relative z-10 flex size-10 shrink-0 items-center justify-center rounded-xl border",
                        s.kind === "approval" ? "border-amber-500/40 bg-amber-500/10 text-amber-600" : "border-line bg-surface text-ink-2",
                      )}
                    >
                      <Icon className="size-4" />
                    </span>
                    <Card className="flex-1 p-4">
                      <div className="flex items-center justify-between gap-3">
                        <div className="flex items-center gap-2">
                          <span className="text-xs font-medium text-muted tabular-nums">{String(i + 1).padStart(2, "0")}</span>
                          <h3 className="font-medium">{s.name}</h3>
                        </div>
                        <span className="text-xs text-muted">{stepLabel[s.kind]}</span>
                      </div>
                      <pre className="scroll-thin mt-3 overflow-x-auto rounded-lg bg-surface-2 px-3 py-2 font-mono text-[12.5px] text-ink-2">{s.detail}</pre>
                    </Card>
                  </li>
                );
              })}
              <li className="flex gap-4 pl-[3px]">
                <button className="flex h-9 items-center gap-2 rounded-lg border border-dashed border-line-strong px-3 text-sm text-muted hover:border-accent hover:text-accent">
                  + Add step
                </button>
              </li>
            </ol>
          )}

          {tab === "runs" && (
            <Card>
              <ul className="divide-y divide-line">
                {history.length === 0 && <li className="p-6 text-sm text-muted">No runs yet.</li>}
                {history.map((r) => (
                  <li key={r.id}>
                    <a href={`#/runs/${r.id}`} className="flex items-center gap-4 px-4 py-3 hover:bg-surface-2">
                      <StatusPill status={r.status} />
                      <span className="font-mono text-xs text-muted">#{r.id.slice(2)}</span>
                      <span className="flex flex-1 items-center gap-1.5 text-sm">
                        <Avatar name={r.startedBy} size="size-5" /> {r.startedBy}
                      </span>
                      <span className="text-xs text-muted">{r.started}</span>
                      <span className="w-20 text-right text-xs text-muted tabular-nums">{r.duration}</span>
                    </a>
                  </li>
                ))}
              </ul>
            </Card>
          )}

          {tab === "yaml" && (
            <Card className="overflow-hidden">
              <div className="flex items-center justify-between border-b border-line px-4 py-2 text-xs text-muted">
                <span className="flex items-center gap-1.5">
                  <GitBranch className="size-3.5" /> runbooks/{rb.id}.yaml · main
                </span>
                <Button size="sm" variant="ghost">
                  <Copy className="size-3.5" /> Copy
                </Button>
              </div>
              <pre className="scroll-thin overflow-x-auto bg-term p-4 font-mono text-[12.5px] leading-relaxed text-term-ink">
                {`name: ${rb.id}
description: ${rb.description}
inputs:
${rb.inputs.map((i) => `  - name: ${i.name}\n    required: ${i.required}${i.default ? `\n    default: "${i.default}"` : ""}`).join("\n") || "  []"}
targets:
  filter: "${rb.targets}"
rollout: "${rb.rollout}"
steps:
${rb.steps.map((s) => `  - name: ${s.name}\n    ${s.kind}: ${JSON.stringify(s.detail)}`).join("\n")}`}
              </pre>
            </Card>
          )}
        </div>

        <aside className="space-y-4">
          <Card>
            <CardHeader title="Inputs" />
            <ul className="space-y-2 px-4 pb-4">
              {rb.inputs.length === 0 && <li className="text-sm text-muted">No inputs</li>}
              {rb.inputs.map((i) => (
                <li key={i.name} className="flex items-center justify-between gap-2 text-sm">
                  <code className="font-mono text-[13px]">{i.name}</code>
                  <span className="text-xs text-muted">{i.required ? "required" : `default ${i.default}`}</span>
                </li>
              ))}
            </ul>
          </Card>
          <Card>
            <CardHeader title="Targets and rollout" />
            <dl className="space-y-3 px-4 pb-4 text-sm">
              <div>
                <dt className="text-xs text-muted">Host filter</dt>
                <dd className="mt-1">
                  <Tag>{rb.targets}</Tag>
                </dd>
              </div>
              <div>
                <dt className="text-xs text-muted">Rollout</dt>
                <dd className="mt-0.5">{rb.rollout}</dd>
              </div>
            </dl>
          </Card>
          <Card>
            <CardHeader title="Triggers" />
            <ul className="space-y-2 px-4 pb-4 text-sm">
              <li className="flex items-center gap-2">
                <MousePointerClick className="size-4 text-muted" /> Manual, by anyone with Run access
              </li>
              {rb.schedule && (
                <li className="flex items-center gap-2">
                  <CalendarClock className="size-4 text-muted" /> {rb.schedule} <span className="text-xs text-muted">({rb.nextRun})</span>
                </li>
              )}
              <li className="flex items-center gap-2">
                <Webhook className="size-4 text-muted" /> Webhook <code className="font-mono text-xs text-muted">/hooks/••••3f2a</code>
              </li>
            </ul>
          </Card>
          <Card className="p-4 text-sm">
            <div className="flex items-center gap-2">
              <Avatar name={rb.owner} size="size-6" />
              <div>
                <div className="font-medium">{rb.owner}</div>
                <div className="text-xs text-muted">Owner · version 14 · edited 3 days ago</div>
              </div>
            </div>
          </Card>
        </aside>
      </div>
    </>
  );
}
