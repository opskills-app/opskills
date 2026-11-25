import { useState } from "react";
import { Activity, CalendarClock, CheckCircle2, Code2, Hand, MousePointerClick, Webhook } from "lucide-react";
import { runbookById, runs, type Run } from "../data";
import { Avatar, Card, cx, Delta, IconBubble, Pagination, SearchField, SortButton, StatusPill, TableHeading } from "../components/ui";

const triggerIcon: Record<Run["trigger"], typeof Webhook> = {
  manual: MousePointerClick,
  schedule: CalendarClock,
  webhook: Webhook,
  api: Code2,
};

export function RunsPage() {
  const [q, setQ] = useState("");
  const list = runs.filter((r) => `${runbookById(r.runbookId)!.name} ${r.startedBy} ${r.trigger} ${r.status}`.toLowerCase().includes(q.toLowerCase()));

  return (
    <div className="space-y-8">
      <Card className="grid grid-cols-1 gap-4 px-4 py-6 md:grid-cols-3 md:divide-x md:divide-line md:px-6 md:py-8">
        {[
          { icon: Activity, label: "Runs today", value: "148", delta: "+16%", good: true, period: "vs yesterday" },
          { icon: CheckCircle2, label: "Succeeded", value: "146", delta: "+1%", good: true, period: "this week" },
          { icon: Hand, label: "Needs approval", value: "1", delta: "-50%", good: true, period: "this week" },
        ].map((k) => (
          <div key={k.label} className="flex items-center gap-5 px-2 py-2 sm:px-6">
            <IconBubble icon={k.icon} tone="mint" size="size-[72px] sm:size-20" />
            <div>
              <div className="text-sm text-muted">{k.label}</div>
              <div className="mt-0.5 text-[30px] leading-tight font-semibold tabular-nums">{k.value}</div>
              <Delta value={k.delta} good={k.good} period={k.period} />
            </div>
          </div>
        ))}
      </Card>

      <Card className="overflow-hidden">
        <TableHeading title="All runs" subtitle="Every execution, with who started it and why">
          <SearchField value={q} onChange={setQ} />
          <SortButton value="Newest" />
        </TableHeading>

        <div className="hidden grid-cols-[minmax(0,1.6fr)_minmax(0,1fr)_120px_110px_110px_120px] gap-6 border-b border-line px-10 pb-4 text-sm font-medium text-muted lg:grid">
          <span>Runbook</span>
          <span>Started by</span>
          <span>Trigger</span>
          <span>Hosts</span>
          <span>Duration</span>
          <span className="text-center">Status</span>
        </div>
        <ul className="px-6 sm:px-10">
          {list.map((r) => {
            const rb = runbookById(r.runbookId)!;
            const TIcon = triggerIcon[r.trigger];
            return (
              <li key={r.id} className="border-b border-line last:border-0">
                <a
                  href={`#/runs/${r.id}`}
                  className="grid grid-cols-[minmax(0,1fr)_auto] items-center gap-x-6 gap-y-1 py-5 text-sm lg:grid-cols-[minmax(0,1.6fr)_minmax(0,1fr)_120px_110px_110px_120px]"
                >
                  <span className="min-w-0">
                    <span className="block truncate font-medium hover:text-accent">{rb.name}</span>
                    <span className="font-mono text-xs text-muted">#{r.id.slice(2)}</span>
                  </span>
                  <span className="hidden min-w-0 items-center gap-2 lg:flex">
                    <Avatar name={r.startedBy} size="size-7" />
                    <span className="truncate">{r.startedBy}</span>
                  </span>
                  <span className="hidden items-center gap-1.5 capitalize lg:flex">
                    <TIcon className="size-4 text-muted" /> {r.trigger}
                  </span>
                  <span className="hidden tabular-nums lg:block">
                    <span className={cx(r.hosts.failed > 0 && "text-rose-600")}>{r.hosts.done}</span>/{r.hosts.total}
                  </span>
                  <span className="hidden tabular-nums lg:block">{r.duration || "Not started"}</span>
                  <span className="text-center">
                    <StatusPill status={r.status} />
                  </span>
                </a>
              </li>
            );
          })}
        </ul>
        <Pagination from={1} to={list.length} total="2,741" last={343} />
      </Card>
    </div>
  );
}
