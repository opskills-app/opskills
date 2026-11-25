import { useMemo, useState } from "react";
import { BookOpen, Plus, Search, ShieldCheck, TrendingUp, Zap } from "lucide-react";
import { api, useData, type Run } from "../api";
import { duration, runDuration, runNumber, timeAgo } from "../format";
import { Button, Card, cx, Delta, EmptyState, IconBubble, StatusPill, Tile, type BubbleTone } from "../components/ui";
import { runbookIcon } from "./Runbooks";
import { go } from "../router";

const DAY = 86_400_000;

function Kpi({ icon, tone, label, value, delta, good, period }: { icon: typeof Zap; tone: BubbleTone; label: string; value: string; delta?: string; good?: boolean; period: string }) {
  return (
    <div className="flex items-center gap-5 px-2 py-2 sm:px-6">
      <IconBubble icon={icon} tone={tone} size="size-[72px] sm:size-20" />
      <div className="min-w-0">
        <div className="text-sm text-muted">{label}</div>
        <div className="mt-0.5 text-[30px] leading-tight font-semibold tracking-tight tabular-nums">{value}</div>
        {delta ? <Delta value={delta} good={!!good} period={period} /> : <span className="text-xs text-ink-2">{period}</span>}
      </div>
    </div>
  );
}

function startOfDay(ms: number) {
  const d = new Date(ms);
  d.setHours(0, 0, 0, 0);
  return d.getTime();
}

function BarChart({ runs }: { runs: Run[] }) {
  const today = startOfDay(Date.now());
  const days = Array.from({ length: 14 }, (_, i) => today - (13 - i) * DAY);
  const counts = days.map((d) => runs.filter((r) => r.created_at >= d && r.created_at < d + DAY).length);
  const max = Math.max(1, ...counts) * 1.25;
  return (
    <div className="flex h-64 items-end gap-1.5 px-6 pb-6 sm:gap-2.5 sm:px-8">
      {days.map((d, i) => {
        const current = i === days.length - 1;
        const label = new Date(d).toLocaleDateString([], { weekday: "short" });
        const h = Math.max(3, (counts[i] / max) * 100);
        return (
          <div key={d} className="flex h-full min-w-0 flex-1 flex-col items-center justify-end gap-3">
            <div className="relative flex w-full flex-1 items-end justify-center">
              <div
                title={`${new Date(d).toLocaleDateString()}: ${counts[i]} runs`}
                className={cx("w-full max-w-[30px] rounded-[9px] transition-colors", current ? "bg-accent" : "bg-bar hover:bg-surface-3")}
                style={{ height: `${h}%` }}
              />
              {current && (
                <span className="absolute left-1/2 -translate-x-1/2 -translate-y-[calc(100%+8px)] rounded-lg bg-ink px-2 py-1 text-[11px] font-semibold whitespace-nowrap text-canvas" style={{ bottom: `${h}%` }}>
                  {counts[i]} today
                  <span className="absolute -bottom-1 left-1/2 size-2 -translate-x-1/2 rotate-45 bg-ink" />
                </span>
              )}
            </div>
            <span className={cx("text-[11px]", current ? "font-semibold text-ink" : "text-muted")}>
              <span className="sm:hidden">{label.slice(0, 1)}</span>
              <span className="hidden sm:inline">{label}</span>
            </span>
          </div>
        );
      })}
    </div>
  );
}

function Donut({ percent, label }: { percent: number; label: string }) {
  const r = 70;
  const c = 2 * Math.PI * r;
  return (
    <div className="relative mx-auto flex size-60 items-center justify-center rounded-full bg-surface shadow-soft">
      <svg viewBox="0 0 180 180" className="size-52 -rotate-90">
        <defs>
          <linearGradient id="donut-grad" x1="0" y1="0" x2="1" y2="1">
            <stop offset="0%" stopColor="#ef3ba5" />
            <stop offset="55%" stopColor="#7c3aed" />
            <stop offset="100%" stopColor="#5932ea" />
          </linearGradient>
        </defs>
        <circle cx="90" cy="90" r={r} className="fill-none stroke-bar" strokeWidth="26" />
        {percent > 0 && <circle cx="90" cy="90" r={r} fill="none" stroke="url(#donut-grad)" strokeWidth="30" strokeDasharray={`${(percent / 100) * c} ${c}`} />}
      </svg>
      <div className="absolute text-center">
        <div className="text-3xl font-bold tabular-nums">{percent}%</div>
        <div className="text-xs leading-tight font-medium">{label}</div>
      </div>
    </div>
  );
}

function pctChange(now: number, before: number) {
  if (before === 0) return now > 0 ? "+100%" : undefined;
  const p = Math.round(((now - before) / before) * 100);
  return `${p >= 0 ? "+" : ""}${p}%`;
}

export function Overview() {
  const runs = useData(() => api.runs({ limit: 1000 }), [], 5000);
  const runbooks = useData(() => api.runbooks(), [], 15000);
  const [q, setQ] = useState("");

  const stats = useMemo(() => {
    const list = runs.data ?? [];
    const now = Date.now();
    const week = list.filter((r) => r.created_at >= now - 7 * DAY);
    const prev = list.filter((r) => r.created_at >= now - 14 * DAY && r.created_at < now - 7 * DAY);
    const finished = (rs: Run[]) => rs.filter((r) => r.status === "succeeded" || r.status === "failed");
    const rate = (rs: Run[]) => {
      const f = finished(rs);
      return f.length ? Math.round((f.filter((r) => r.status === "succeeded").length / f.length) * 1000) / 10 : null;
    };
    const recent30 = finished(list.filter((r) => r.created_at >= now - 30 * DAY));
    return {
      week,
      prev,
      rateNow: rate(week),
      ratePrev: rate(prev),
      ranThisWeek: new Set(week.map((r) => r.runbook_id)).size,
      okPct: recent30.length ? Math.round((recent30.filter((r) => r.status === "succeeded").length / recent30.length) * 100) : 0,
      recent30: recent30.length,
      last14: list.filter((r) => r.created_at >= startOfDay(now) - 13 * DAY).length,
    };
  }, [runs.data]);

  const list = (runs.data ?? []).filter((r) => `${r.runbook_name} ${r.started_by} ${r.status}`.toLowerCase().includes(q.toLowerCase())).slice(0, 8);

  if (runbooks.data?.length === 0 && runs.data?.length === 0) {
    return (
      <Card>
        <EmptyState
          icon={BookOpen}
          title="Welcome to OpSkills"
          action={
            <>
              <Button variant="primary" onClick={() => go("runbooks/new")}>
                <Plus className="size-4" /> Create your first runbook
              </Button>
              <Button onClick={() => go("runbooks/new/hello")}>Start from an example</Button>
            </>
          }
        >
          Runbooks turn the procedures you repeat into steps anyone on your team can run. Each step is a shell command that runs on this OpSkills server, and you watch its output live.
        </EmptyState>
      </Card>
    );
  }

  const rateDelta =
    stats.rateNow !== null && stats.ratePrev !== null ? `${stats.rateNow - stats.ratePrev >= 0 ? "+" : ""}${(stats.rateNow - stats.ratePrev).toFixed(1)}%` : undefined;

  return (
    <div className="space-y-8">
      <Card className="grid grid-cols-1 gap-4 px-4 py-6 md:grid-cols-3 md:divide-x md:divide-line md:px-6 md:py-8">
        <Kpi icon={Zap} tone="mint" label="Runs this week" value={String(stats.week.length)} delta={pctChange(stats.week.length, stats.prev.length)} good={stats.week.length >= stats.prev.length} period="vs last week" />
        <Kpi
          icon={ShieldCheck}
          tone="sky"
          label="Success rate"
          value={stats.rateNow === null ? "None yet" : `${stats.rateNow}%`}
          delta={rateDelta}
          good={!rateDelta?.startsWith("-")}
          period={rateDelta ? "vs last week" : "this week"}
        />
        <Kpi icon={BookOpen} tone="pink" label="Runbooks" value={String(runbooks.data?.length ?? "")} period={`${stats.ranThisWeek} ran this week`} />
      </Card>

      <div className="grid grid-cols-1 gap-8 xl:grid-cols-[minmax(0,1fr)_320px]">
        <Card>
          <div className="flex items-start justify-between gap-3 px-6 pt-6 pb-6 sm:px-8 sm:pt-8">
            <div>
              <h3 className="text-[22px] font-semibold tracking-tight">Run activity</h3>
              <p className="mt-1 text-sm text-muted">Runs per day, last 14 days</p>
            </div>
            <span className="flex h-10 items-center rounded-xl bg-surface-2 px-4 text-xs text-ink-2">
              <TrendingUp className="mr-2 size-4" /> {stats.last14} runs
            </span>
          </div>
          <BarChart runs={runs.data ?? []} />
        </Card>

        <Card className="pb-8">
          <div className="px-6 pt-6 pb-6 sm:px-8 sm:pt-8">
            <h3 className="text-[22px] font-semibold tracking-tight">Results</h3>
            <p className="mt-1 text-sm text-muted">Finished runs, last 30 days ({stats.recent30})</p>
          </div>
          <Donut percent={stats.okPct} label="Succeeded" />
          <div className="mt-6 flex justify-center gap-5 text-xs text-muted">
            <span className="flex items-center gap-1.5">
              <span className="size-2.5 rounded-full bg-gradient-to-br from-pink-500 to-violet-600" /> Succeeded
            </span>
            <span className="flex items-center gap-1.5">
              <span className="size-2.5 rounded-full bg-bar ring-1 ring-line-strong" /> Failed
            </span>
          </div>
        </Card>
      </div>

      <Card className="overflow-hidden">
        <div className="flex flex-wrap items-center justify-between gap-4 px-6 pt-6 pb-4 sm:px-8 sm:pt-8">
          <h3 className="text-[22px] font-semibold tracking-tight">Recent runs</h3>
          <div className="flex flex-wrap items-center gap-3">
            <label className="flex h-10 w-full items-center gap-2 rounded-xl bg-surface-2 px-3 sm:w-56">
              <Search className="size-4 text-muted" />
              <input value={q} onChange={(e) => setQ(e.target.value)} placeholder="Search" className="flex-1 bg-transparent text-xs outline-none placeholder:text-muted" />
            </label>
            <Button size="sm" onClick={() => go("runs")}>
              All runs
            </Button>
          </div>
        </div>
        <div className="hidden grid-cols-[minmax(0,1fr)_110px_110px_130px] gap-6 border-b border-line px-8 pb-3 text-sm font-medium text-muted md:grid">
          <span>Runbook</span>
          <span>Steps</span>
          <span>Duration</span>
          <span>Status</span>
        </div>
        <ul>
          {list.length === 0 && <li className="px-8 py-10 text-center text-sm text-muted">{runs.loading ? "Loading..." : "No runs yet. Open a runbook and press Run."}</li>}
          {list.map((r) => (
            <li key={r.id}>
              <a href={`#/runs/${r.id}`} className="grid grid-cols-[minmax(0,1fr)_auto] items-center gap-x-6 gap-y-2 px-6 py-4 hover:bg-surface-2/60 sm:px-8 md:grid-cols-[minmax(0,1fr)_110px_110px_130px]">
                <span className="flex min-w-0 items-center gap-5">
                  <Tile seed={r.runbook_id} icon={runbookIcon(r.runbook_id)} className="hidden h-14 w-[88px] sm:flex" />
                  <span className="min-w-0">
                    <span className="block truncate font-semibold">{r.runbook_name}</span>
                    <span className="block truncate text-xs text-muted">
                      #{runNumber(r.id)} · {r.trigger} by {r.started_by} · {timeAgo(r.created_at)}
                    </span>
                  </span>
                </span>
                <span className="hidden text-sm tabular-nums md:block">
                  {r.steps.filter((s) => s.status === "succeeded").length}/{r.steps.length} done
                </span>
                <span className="hidden text-sm tabular-nums md:block">{duration(runDuration(r)) || "Not started"}</span>
                <span>
                  <StatusPill status={r.status} />
                </span>
              </a>
            </li>
          ))}
        </ul>
      </Card>
    </div>
  );
}
