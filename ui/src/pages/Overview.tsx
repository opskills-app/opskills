import { useState } from "react";
import { ChevronDown, Search, Server, ShieldCheck, TrendingUp, Zap } from "lucide-react";
import { runbookById, runs } from "../data";
import { Card, cx, Delta, IconBubble, StatusPill, Tile, type BubbleTone } from "../components/ui";
import { runbookIcon } from "./Runbooks";

const months = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const monthlyRuns = [2810, 3320, 3990, 3150, 2740, 3170, 3330, 3610, 3880, 3510, 4620, 3820];
const CURRENT = 10; // November

function Kpi({ icon, tone, label, value, delta, good, period }: { icon: typeof Zap; tone: BubbleTone; label: string; value: string; delta: string; good: boolean; period: string }) {
  return (
    <div className="flex items-center gap-5 px-2 py-2 sm:px-6">
      <IconBubble icon={icon} tone={tone} size="size-[72px] sm:size-20" />
      <div className="min-w-0">
        <div className="text-sm text-muted">{label}</div>
        <div className="mt-0.5 text-[30px] leading-tight font-semibold tracking-tight tabular-nums">{value}</div>
        <Delta value={delta} good={good} period={period} />
      </div>
    </div>
  );
}

function BarChart() {
  // Leave headroom above the tallest bar for the value label.
  const max = Math.max(...monthlyRuns) * 1.2;
  const change = Math.round(((monthlyRuns[CURRENT] - monthlyRuns[CURRENT - 1]) / monthlyRuns[CURRENT - 1]) * 100);
  return (
    <div className="flex h-64 items-end gap-2 px-6 pb-6 sm:gap-3 sm:px-8">
      {monthlyRuns.map((v, i) => {
        const current = i === CURRENT;
        return (
          <div key={months[i]} className="flex h-full flex-1 flex-col items-center justify-end gap-3">
            <div className="relative flex w-full flex-1 items-end justify-center">
              <div
                title={`${months[i]}: ${v.toLocaleString()} runs`}
                className={cx("w-full max-w-[34px] rounded-[10px] transition-colors", current ? "bg-accent" : "bg-bar hover:bg-surface-3")}
                style={{ height: `${(v / max) * 100}%` }}
              />
              {current && (
                <span className="absolute left-1/2 -translate-x-1/2 -translate-y-[calc(100%+8px)] rounded-lg bg-ink px-2 py-1 text-[11px] font-semibold whitespace-nowrap text-canvas" style={{ bottom: `${(v / max) * 100}%` }}>
                  <TrendingUp className="mr-1 inline size-3 text-emerald-400" />
                  {change}%
                  <span className="absolute -bottom-1 left-1/2 size-2 -translate-x-1/2 rotate-45 bg-ink" />
                </span>
              )}
            </div>
            <span className={cx("text-[11px]", current ? "font-semibold text-ink" : "text-muted")}>{months[i]}</span>
          </div>
        );
      })}
    </div>
  );
}

function Donut({ percent }: { percent: number }) {
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
        <circle cx="90" cy="90" r={r} fill="none" stroke="url(#donut-grad)" strokeWidth="30" strokeDasharray={`${(percent / 100) * c} ${c}`} />
      </svg>
      <div className="absolute text-center">
        <div className="text-3xl font-bold tabular-nums">{percent}%</div>
        <div className="text-xs leading-tight font-medium">
          Automated
          <br />
          runs
        </div>
      </div>
    </div>
  );
}

export function Overview({ workspace }: { workspace: string }) {
  const [q, setQ] = useState("");
  const list = runs.filter((r) => `${runbookById(r.runbookId)!.name} ${r.startedBy}`.toLowerCase().includes(q.toLowerCase()));

  return (
    <div className="space-y-8">
      <Card className="grid grid-cols-1 gap-4 px-4 py-6 md:grid-cols-3 md:divide-x md:divide-line md:px-6 md:py-8">
        <Kpi icon={Zap} tone="mint" label="Runs this week" value="1,042" delta="+12.4%" good period="this week" />
        <Kpi icon={ShieldCheck} tone="sky" label="Success rate" value="99.1%" delta="+0.4%" good period="this week" />
        <Kpi icon={Server} tone="pink" label="Hosts online" value="22/24" delta="-8%" good={false} period="since yesterday" />
      </Card>

      <div className="grid grid-cols-1 gap-8 xl:grid-cols-[minmax(0,1fr)_320px]">
        <Card>
          <div className="flex items-start justify-between gap-3 px-6 pt-6 pb-6 sm:px-8 sm:pt-8">
            <div>
              <h3 className="text-[22px] font-semibold tracking-tight">Run activity</h3>
              <p className="mt-1 text-sm text-muted">Runs per month in {workspace}</p>
            </div>
            <button className="flex h-10 items-center gap-6 rounded-xl bg-surface-2 px-4 text-xs text-ink-2">
              Monthly <ChevronDown className="size-4" />
            </button>
          </div>
          <BarChart />
        </Card>

        <Card className="pb-8">
          <div className="px-6 pt-6 pb-6 sm:px-8 sm:pt-8">
            <h3 className="text-[22px] font-semibold tracking-tight">Triggers</h3>
            <p className="mt-1 text-sm text-muted">Runs started by schedules and webhooks</p>
          </div>
          <Donut percent={72} />
          <div className="mt-6 flex justify-center gap-5 text-xs text-muted">
            <span className="flex items-center gap-1.5">
              <span className="size-2.5 rounded-full bg-gradient-to-br from-pink-500 to-violet-600" /> Automated
            </span>
            <span className="flex items-center gap-1.5">
              <span className="size-2.5 rounded-full bg-bar ring-1 ring-line-strong" /> Manual
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
            <button className="flex h-10 items-center gap-6 rounded-xl bg-surface-2 px-4 text-xs text-ink-2">
              Last 7 days <ChevronDown className="size-4" />
            </button>
          </div>
        </div>
        <div className="hidden grid-cols-[minmax(0,1fr)_120px_110px_150px] gap-6 border-b border-line px-8 pb-3 text-sm font-medium text-muted md:grid">
          <span>Runbook</span>
          <span>Hosts</span>
          <span>Duration</span>
          <span>Status</span>
        </div>
        <ul>
          {list.map((r) => {
            const rb = runbookById(r.runbookId)!;
            return (
              <li key={r.id}>
                <a href={`#/runs/${r.id}`} className="grid grid-cols-[minmax(0,1fr)_auto] items-center gap-x-6 gap-y-2 px-6 py-4 hover:bg-surface-2/60 sm:px-8 md:grid-cols-[minmax(0,1fr)_120px_110px_150px]">
                  <span className="flex min-w-0 items-center gap-5">
                    <Tile seed={rb.id} icon={runbookIcon(rb.id)} className="hidden h-14 w-[88px] sm:flex" />
                    <span className="min-w-0">
                      <span className="block truncate font-semibold">{rb.name}</span>
                      <span className="block truncate text-xs text-muted">
                        #{r.id.slice(2)} · {r.trigger} by {r.startedBy} · {r.started}
                      </span>
                    </span>
                  </span>
                  <span className="hidden text-sm tabular-nums md:block">
                    {r.hosts.done}/{r.hosts.total} done
                  </span>
                  <span className="hidden text-sm tabular-nums md:block">{r.duration || "Not started"}</span>
                  <span>
                    <StatusPill status={r.status} />
                  </span>
                </a>
              </li>
            );
          })}
        </ul>
      </Card>
    </div>
  );
}
