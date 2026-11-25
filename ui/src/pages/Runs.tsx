import { useMemo, useState } from "react";
import { Activity, CheckCircle2, XCircle } from "lucide-react";
import { api, useData, type RunStatus } from "../api";
import { duration, runDuration, runNumber, timeAgo } from "../format";
import { Avatar, Card, cx, IconBubble, Pager, SearchField, StatusPill, TableHeading } from "../components/ui";

const PAGE = 15;
const filters: (RunStatus | "all")[] = ["all", "running", "succeeded", "failed", "cancelled", "queued"];

export function RunsPage() {
  const runs = useData(() => api.runs({ limit: 1000 }), [], 3000);
  const [q, setQ] = useState("");
  const [f, setF] = useState<RunStatus | "all">("all");
  const [page, setPage] = useState(0);

  const all = useMemo(() => runs.data ?? [], [runs.data]);
  const list = all.filter((r) => (f === "all" || r.status === f) && `${r.runbook_name} ${r.started_by} ${runNumber(r.id)}`.toLowerCase().includes(q.toLowerCase()));
  const pages = Math.max(1, Math.ceil(list.length / PAGE));
  const current = Math.min(page, pages - 1);
  const shown = list.slice(current * PAGE, current * PAGE + PAGE);

  const today = useMemo(() => {
    const start = new Date();
    start.setHours(0, 0, 0, 0);
    const t = all.filter((r) => r.created_at >= start.getTime());
    return { total: t.length, ok: t.filter((r) => r.status === "succeeded").length, failed: t.filter((r) => r.status === "failed").length };
  }, [all]);

  return (
    <div className="space-y-8">
      <Card className="grid grid-cols-1 gap-4 px-4 py-6 md:grid-cols-3 md:divide-x md:divide-line md:px-6 md:py-8">
        {[
          { icon: Activity, label: "Runs today", value: today.total, tone: "mint" as const },
          { icon: CheckCircle2, label: "Succeeded today", value: today.ok, tone: "mint" as const },
          { icon: XCircle, label: "Failed today", value: today.failed, tone: today.failed ? ("pink" as const) : ("mint" as const) },
        ].map((k) => (
          <div key={k.label} className="flex items-center gap-5 px-2 py-2 sm:px-6">
            <IconBubble icon={k.icon} tone={k.tone} size="size-[72px] sm:size-20" />
            <div>
              <div className="text-sm text-muted">{k.label}</div>
              <div className="mt-0.5 text-[30px] leading-tight font-semibold tabular-nums">{k.value}</div>
            </div>
          </div>
        ))}
      </Card>

      <Card className="overflow-hidden">
        <TableHeading title="All runs" subtitle="Every execution, newest first">
          <SearchField
            value={q}
            onChange={(v) => {
              setQ(v);
              setPage(0);
            }}
          />
        </TableHeading>
        <div className="-mt-2 flex flex-wrap gap-2 px-6 pb-5 sm:px-10">
          {filters.map((s) => (
            <button
              key={s}
              onClick={() => {
                setF(s);
                setPage(0);
              }}
              className={cx("h-8 rounded-xl px-3 text-xs font-medium capitalize", f === s ? "bg-accent text-accent-ink" : "bg-surface-2 text-ink-2 hover:bg-surface-3")}
            >
              {s} <span className="ml-1 opacity-60">{s === "all" ? all.length : all.filter((r) => r.status === s).length}</span>
            </button>
          ))}
        </div>

        <div className="hidden grid-cols-[minmax(0,1.6fr)_minmax(0,1fr)_100px_120px_110px_120px] gap-6 border-y border-line px-10 py-4 text-sm font-medium text-muted lg:grid">
          <span>Runbook</span>
          <span>Started by</span>
          <span>Steps</span>
          <span>Started</span>
          <span>Duration</span>
          <span className="text-center">Status</span>
        </div>
        <ul className="px-6 sm:px-10">
          {shown.length === 0 && <li className="py-12 text-center text-sm text-muted">{runs.loading ? "Loading..." : all.length ? "No runs match this filter." : "No runs yet."}</li>}
          {shown.map((r) => (
            <li key={r.id} className="border-b border-line last:border-0">
              <a href={`#/runs/${r.id}`} className="grid grid-cols-[minmax(0,1fr)_auto] items-center gap-x-6 gap-y-1 py-4 text-sm lg:grid-cols-[minmax(0,1.6fr)_minmax(0,1fr)_100px_120px_110px_120px]">
                <span className="min-w-0">
                  <span className="block truncate font-medium hover:text-accent">{r.runbook_name}</span>
                  <span className="font-mono text-xs text-muted">#{runNumber(r.id)}</span>
                </span>
                <span className="hidden min-w-0 items-center gap-2 lg:flex">
                  <Avatar name={r.started_by} size="size-7" />
                  <span className="truncate">{r.started_by}</span>
                </span>
                <span className="hidden tabular-nums lg:block">
                  {r.steps.filter((s) => s.status === "succeeded").length}/{r.steps.length}
                </span>
                <span className="hidden text-muted lg:block">{timeAgo(r.created_at)}</span>
                <span className="hidden tabular-nums lg:block">{duration(runDuration(r)) || "Not started"}</span>
                <span className="text-center">
                  <StatusPill status={r.status} />
                </span>
              </a>
            </li>
          ))}
        </ul>
        <Pager page={current} pages={pages} total={list.length} from={list.length ? current * PAGE + 1 : 0} to={Math.min(list.length, current * PAGE + PAGE)} onPage={setPage} />
      </Card>
    </div>
  );
}
