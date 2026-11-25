import { ArrowUpRight, CalendarClock, Hand, Server, TrendingUp, Zap, Timer } from "lucide-react";
import { durations, hosts, runbookById, runbooks, runs, successRate, throughput } from "../data";
import { Avatar, Button, Card, CardHeader, cx, PageHeader, Sparkline, StatusDot, StatusPill } from "../components/ui";
import { go } from "../router";

function Kpi({ label, value, delta, good, icon: Icon, series }: { label: string; value: string; delta: string; good: boolean; icon: typeof Zap; series: number[] }) {
  return (
    <Card className="p-4">
      <div className="flex items-center justify-between text-xs font-medium text-muted">
        <span className="flex items-center gap-1.5">
          <Icon className="size-3.5" />
          {label}
        </span>
        <span className={cx("rounded-full px-1.5 py-px", good ? "bg-emerald-500/10 text-emerald-600 dark:text-emerald-400" : "bg-rose-500/10 text-rose-600")}>{delta}</span>
      </div>
      <div className="mt-2 text-[28px] leading-none font-semibold tracking-tight tabular-nums">{value}</div>
      <Sparkline values={series} className="mt-3" />
    </Card>
  );
}

export function Overview({ workspace }: { workspace: string }) {
  const active = runs.filter((r) => r.status === "running" || r.status === "waiting");
  const online = hosts.filter((h) => h.status === "online").length;
  const scheduled = runbooks.filter((r) => r.schedule);

  return (
    <>
      <PageHeader
        title={`Good afternoon, Maya`}
        subtitle={`${workspace} workspace · ${active.length} runs in progress · ${online} of ${hosts.length} hosts online`}
        actions={
          <Button onClick={() => go("runbooks")}>
            Browse runbooks
            <ArrowUpRight className="size-3.5" />
          </Button>
        }
      />

      <div className="grid grid-cols-1 gap-4 sm:grid-cols-2 xl:grid-cols-4">
        <Kpi label="Runs this week" value="1,042" delta="+12%" good icon={Zap} series={throughput} />
        <Kpi label="Success rate" value="99.1%" delta="+0.4 pt" good icon={TrendingUp} series={successRate} />
        <Kpi label="Median duration" value="1m 48s" delta="-9%" good icon={Timer} series={durations} />
        <Kpi label="Hosts online" value={`${online}/${hosts.length}`} delta="1 offline" good={false} icon={Server} series={[23, 24, 24, 24, 23, 24, 24, 23, 24, 24, 24, 23, 23, 22]} />
      </div>

      <div className="mt-4 grid grid-cols-1 gap-4 xl:grid-cols-3">
        <Card className="xl:col-span-2">
          <CardHeader title="In progress" hint="Live, updates as hosts report back" action={<Button size="sm" variant="ghost" onClick={() => go("runs")}>All runs</Button>} />
          <ul className="divide-y divide-line">
            {active.map((r) => {
              const rb = runbookById(r.runbookId)!;
              return (
                <li key={r.id}>
                  <a href={`#/runs/${r.id}`} className="grid grid-cols-[1fr_auto] items-center gap-4 px-4 py-3 hover:bg-surface-2 sm:grid-cols-[minmax(0,1.4fr)_minmax(0,1fr)_auto]">
                    <div className="min-w-0">
                      <div className="flex items-center gap-2">
                        <span className="truncate text-sm font-medium">{rb.name}</span>
                        <span className="font-mono text-xs text-muted">#{r.id.slice(2)}</span>
                      </div>
                      <div className="mt-0.5 flex items-center gap-1.5 text-xs text-muted">
                        <Avatar name={r.startedBy} size="size-4" />
                        {r.startedBy} · {r.started}
                      </div>
                    </div>
                    <div className="hidden sm:block">
                      <div className="mb-1 flex justify-between text-xs text-muted tabular-nums">
                        <span>
                          {r.hosts.done}/{r.hosts.total} hosts
                        </span>
                        <span>{r.progress}%</span>
                      </div>
                      <div className="h-1.5 overflow-hidden rounded-full bg-surface-3">
                        <div
                          className={cx("h-full rounded-full", r.status === "waiting" ? "bg-amber-500" : "progress-live bg-sky-500")}
                          style={{ width: `${r.progress}%` }}
                        />
                      </div>
                    </div>
                    <StatusPill status={r.status} />
                  </a>
                </li>
              );
            })}
          </ul>
          {active.some((r) => r.status === "waiting") && (
            <div className="m-4 mt-1 flex items-center gap-3 rounded-lg border border-amber-500/30 bg-amber-500/5 px-3 py-2.5 text-sm">
              <Hand className="size-4 shrink-0 text-amber-600" />
              <span className="flex-1">
                <span className="font-medium">Rotate TLS certificates</span> is waiting for your approval before reloading 7 edge hosts.
              </span>
              <Button size="sm" variant="primary">
                Review
              </Button>
            </div>
          )}
        </Card>

        <Card>
          <CardHeader title="Coming up" hint="Scheduled in the next 7 days" />
          <ul className="space-y-1 px-2 pb-3">
            {scheduled.map((r) => (
              <li key={r.id}>
                <a href={`#/runbooks/${r.id}`} className="flex items-center gap-3 rounded-lg px-2 py-2 hover:bg-surface-2">
                  <span className="flex size-8 shrink-0 items-center justify-center rounded-lg bg-surface-2 text-muted">
                    <CalendarClock className="size-4" />
                  </span>
                  <span className="min-w-0 flex-1">
                    <span className="block truncate text-sm font-medium">{r.name}</span>
                    <span className="block text-xs text-muted">{r.schedule}</span>
                  </span>
                  <span className="text-xs whitespace-nowrap text-muted">{r.nextRun}</span>
                </a>
              </li>
            ))}
          </ul>
        </Card>
      </div>

      <div className="mt-4 grid grid-cols-1 gap-4 xl:grid-cols-3">
        <Card className="xl:col-span-2">
          <CardHeader title="Recent activity" />
          <ul className="px-4 pb-2">
            {runs
              .filter((r) => r.status !== "running" && r.status !== "waiting")
              .map((r) => {
                const rb = runbookById(r.runbookId)!;
                return (
                  <li key={r.id} className="relative flex gap-3 pb-4 pl-1 last:pb-3">
                    <span className="absolute top-4 bottom-0 left-[7px] w-px bg-line last:hidden" />
                    <span className="relative mt-1.5">
                      <StatusDot status={r.status} size="size-2.5" />
                    </span>
                    <a href={`#/runs/${r.id}`} className="min-w-0 flex-1 text-sm hover:underline-offset-2">
                      <span className="font-medium">{rb.name}</span>{" "}
                      <span className="text-muted">
                        {r.status === "queued" ? "is queued" : r.status} · {r.trigger} by {r.startedBy}
                      </span>
                      <div className="text-xs text-muted">
                        {r.started}
                        {r.duration && ` · ${r.duration}`}
                      </div>
                    </a>
                  </li>
                );
              })}
          </ul>
        </Card>

        <Card>
          <CardHeader title="Fleet" hint="By role" action={<Button size="sm" variant="ghost" onClick={() => go("hosts")}>Hosts</Button>} />
          <div className="grid grid-cols-8 gap-1.5 px-4">
            {hosts.map((h) => (
              <span
                key={h.name}
                title={`${h.name}: ${h.status}`}
                className={cx(
                  "aspect-square rounded-md",
                  h.status === "online" && "bg-emerald-500/80",
                  h.status === "degraded" && "bg-amber-500",
                  h.status === "offline" && "bg-rose-500",
                )}
              />
            ))}
          </div>
          <dl className="grid grid-cols-3 gap-2 p-4 text-center">
            {[
              ["Online", hosts.filter((h) => h.status === "online").length, "text-emerald-600 dark:text-emerald-400"],
              ["Degraded", hosts.filter((h) => h.status === "degraded").length, "text-amber-600"],
              ["Offline", hosts.filter((h) => h.status === "offline").length, "text-rose-600"],
            ].map(([label, n, tone]) => (
              <div key={label as string} className="rounded-lg bg-surface-2 py-2">
                <dt className="text-[11px] text-muted">{label}</dt>
                <dd className={cx("text-lg font-semibold tabular-nums", tone as string)}>{n}</dd>
              </div>
            ))}
          </dl>
        </Card>
      </div>
    </>
  );
}
