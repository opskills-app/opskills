import { useState } from "react";
import { CalendarClock, Code2, MousePointerClick, Webhook } from "lucide-react";
import { runbookById, runs, type Run, type Status } from "../data";
import { Avatar, Card, cx, PageHeader, StatusPill } from "../components/ui";

const triggerIcon: Record<Run["trigger"], typeof Webhook> = {
  manual: MousePointerClick,
  schedule: CalendarClock,
  webhook: Webhook,
  api: Code2,
};

const filters: (Status | "all")[] = ["all", "running", "waiting", "failed", "succeeded", "queued"];

export function RunsPage() {
  const [f, setF] = useState<Status | "all">("all");
  const list = runs.filter((r) => f === "all" || r.status === f);
  return (
    <>
      <PageHeader title="Runs" subtitle="Every execution, with who started it, why, and what happened on each host." />
      <div className="mb-4 flex flex-wrap gap-1.5">
        {filters.map((s) => (
          <button
            key={s}
            onClick={() => setF(s)}
            className={cx(
              "h-7 rounded-full border px-3 text-xs font-medium capitalize",
              f === s ? "border-transparent bg-ink text-canvas" : "border-line bg-surface text-ink-2 hover:bg-surface-2",
            )}
          >
            {s === "waiting" ? "needs approval" : s} <span className="ml-1 opacity-60">{s === "all" ? runs.length : runs.filter((r) => r.status === s).length}</span>
          </button>
        ))}
      </div>
      <Card className="overflow-hidden">
        <div className="hidden grid-cols-[130px_minmax(0,1fr)_160px_150px_120px_100px] gap-4 border-b border-line bg-surface-2 px-4 py-2 text-xs font-medium text-muted lg:grid">
          <span>Status</span>
          <span>Runbook</span>
          <span>Started by</span>
          <span>Hosts</span>
          <span>Started</span>
          <span className="text-right">Duration</span>
        </div>
        <ul className="divide-y divide-line">
          {list.map((r) => {
            const rb = runbookById(r.runbookId)!;
            const TIcon = triggerIcon[r.trigger];
            return (
              <li key={r.id}>
                <a
                  href={`#/runs/${r.id}`}
                  className="grid grid-cols-[auto_minmax(0,1fr)] items-center gap-x-4 gap-y-1 px-4 py-3 hover:bg-surface-2 lg:grid-cols-[130px_minmax(0,1fr)_160px_150px_120px_100px]"
                >
                  <span>
                    <StatusPill status={r.status} />
                  </span>
                  <span className="min-w-0">
                    <span className="block truncate text-sm font-medium">{rb.name}</span>
                    <span className="flex items-center gap-1 text-xs text-muted">
                      <TIcon className="size-3" /> {r.trigger} · <span className="font-mono">#{r.id.slice(2)}</span>
                    </span>
                  </span>
                  <span className="hidden items-center gap-1.5 text-sm lg:flex">
                    <Avatar name={r.startedBy} size="size-5" />
                    <span className="truncate">{r.startedBy}</span>
                  </span>
                  <span className="hidden items-center gap-2 lg:flex">
                    <span className="h-1.5 w-20 overflow-hidden rounded-full bg-surface-3">
                      <span
                        className={cx(
                          "block h-full rounded-full",
                          r.hosts.failed ? "bg-rose-500" : r.status === "running" ? "progress-live bg-sky-500" : r.status === "waiting" ? "bg-amber-500" : "bg-emerald-500",
                        )}
                        style={{ width: `${(r.hosts.done / r.hosts.total) * 100}%` }}
                      />
                    </span>
                    <span className="text-xs text-muted tabular-nums">
                      {r.hosts.done}/{r.hosts.total}
                    </span>
                  </span>
                  <span className="hidden text-sm text-muted lg:block">{r.started}</span>
                  <span className="hidden text-right text-sm text-muted tabular-nums lg:block">{r.duration}</span>
                </a>
              </li>
            );
          })}
        </ul>
      </Card>
    </>
  );
}
