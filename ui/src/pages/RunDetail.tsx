import { useEffect, useMemo, useRef, useState } from "react";
import { ArrowLeft, ArrowDownToLine, Check, CircleStop, Loader2, RotateCcw, Search } from "lucide-react";
import { hosts, liveLog, runbookById, runs } from "../data";
import { Avatar, Button, Card, CardHeader, cx, StatusPill } from "../components/ui";

const hostHues = [160, 220, 30, 290, 340, 190, 100, 260];

export function RunDetail({ id }: { id: string }) {
  const run = runs.find((r) => r.id === id) ?? runs[0];
  const rb = runbookById(run.runbookId)!;
  const live = run.status === "running";

  // Simulated stream: reveal log lines over time while the run is live.
  const [shown, setShown] = useState(live ? 6 : liveLog.length);
  const [elapsed, setElapsed] = useState(114);
  useEffect(() => {
    if (!live) return;
    const t = setInterval(() => {
      setShown((n) => (n >= liveLog.length ? n : n + 1));
      setElapsed((s) => s + 1);
    }, 900);
    return () => clearInterval(t);
  }, [live]);

  const webHosts = hosts.filter((h) => h.tags.includes("web")).slice(0, 5);
  const fleet = useMemo(
    () =>
      Array.from({ length: run.hosts.total }, (_, i) => ({
        name: webHosts[i % webHosts.length]?.name.replace(/\d+/, String(i + 1).padStart(2, "0")) ?? `host-${i}`,
        state: i < run.hosts.done ? "done" : i < run.hosts.done + 4 && live ? "running" : "pending",
      })),
    [run.hosts.done, run.hosts.total, live, webHosts],
  );

  const [hostFilter, setHostFilter] = useState<string | null>(null);
  const [q, setQ] = useState("");
  const [follow, setFollow] = useState(true);
  const logRef = useRef<HTMLDivElement>(null);
  const lines = liveLog.slice(0, shown).filter((l) => (!hostFilter || l.host === hostFilter) && l.line.toLowerCase().includes(q.toLowerCase()));
  const logHosts = [...new Set(liveLog.map((l) => l.host))];

  useEffect(() => {
    if (follow && logRef.current) logRef.current.scrollTop = logRef.current.scrollHeight;
  }, [lines.length, follow]);

  const stepNames = rb.steps.map((s) => s.name);
  const currentStep = Math.min(stepNames.length - 1, Math.floor((run.progress / 100) * stepNames.length));
  const mm = String(Math.floor(elapsed / 60));
  const ss = String(elapsed % 60).padStart(2, "0");

  return (
    <>
      <a href={`#/runbooks/${rb.id}`} className="mb-3 inline-flex items-center gap-1 text-sm text-muted hover:text-ink">
        <ArrowLeft className="size-3.5" /> {rb.name}
      </a>

      <div className="mb-6 flex flex-wrap items-start justify-between gap-4">
        <div>
          <div className="flex items-center gap-3">
            <h1 className="text-2xl font-semibold tracking-tight">Run #{run.id.slice(2)}</h1>
            <StatusPill status={run.status} />
          </div>
          <p className="mt-1 flex flex-wrap items-center gap-1.5 text-sm text-muted">
            <Avatar name={run.startedBy} size="size-4" /> {run.startedBy} started this {run.started} · reason:{" "}
            <span className="text-ink-2">"Deploy 2026.11.25 hotfix"</span>
          </p>
        </div>
        <div className="flex items-center gap-2">
          {live ? (
            <Button className="text-rose-600 dark:text-rose-400">
              <CircleStop className="size-4" /> Cancel run
            </Button>
          ) : (
            <Button>
              <RotateCcw className="size-3.5" /> Run again
            </Button>
          )}
        </div>
      </div>

      {/* Stat strip */}
      <div className="mb-4 grid grid-cols-2 gap-3 md:grid-cols-4">
        {[
          ["Elapsed", live ? `${mm}m ${ss}s` : run.duration],
          ["Hosts", `${run.hosts.done} of ${run.hosts.total} done`],
          ["Failures", String(run.hosts.failed)],
          ["Rollout", rb.rollout],
        ].map(([k, v]) => (
          <Card key={k} className="px-4 py-3">
            <div className="text-xs text-muted">{k}</div>
            <div className="mt-0.5 truncate text-sm font-semibold tabular-nums">{v}</div>
          </Card>
        ))}
      </div>

      {/* Step timeline */}
      <Card className="mb-4 p-4">
        <ol className="grid gap-3" style={{ gridTemplateColumns: `repeat(${stepNames.length}, minmax(0, 1fr))` }}>
          {stepNames.map((s, i) => {
            const state = i < currentStep || !live ? "done" : i === currentStep ? "active" : "todo";
            return (
              <li key={s} className="min-w-0">
                <div className={cx("h-1 rounded-full", state === "done" && "bg-emerald-500", state === "active" && "progress-live bg-sky-500", state === "todo" && "bg-surface-3")} />
                <div className="mt-2 flex items-center gap-1.5 text-xs">
                  {state === "done" && <Check className="size-3.5 shrink-0 text-emerald-500" strokeWidth={3} />}
                  {state === "active" && <Loader2 className="size-3.5 shrink-0 animate-spin text-sky-500" />}
                  {state === "todo" && <span className="size-3.5 shrink-0 rounded-full border border-line-strong" />}
                  <span className={cx("truncate font-medium", state === "todo" && "text-muted")}>{s}</span>
                </div>
              </li>
            );
          })}
        </ol>
      </Card>

      <div className="grid grid-cols-1 items-start gap-4 xl:grid-cols-[300px_minmax(0,1fr)]">
        <Card>
          <CardHeader title="Hosts" hint="Click a host to filter the log" />
          <div className="grid grid-cols-6 gap-1.5 px-4 pb-4">
            {fleet.map((h) => (
              <button
                key={h.name}
                title={`${h.name}: ${h.state}`}
                onClick={() => setHostFilter((f) => (f === h.name ? null : h.name))}
                className={cx(
                  "aspect-square rounded-md ring-offset-2 ring-offset-surface transition",
                  h.state === "done" && "bg-emerald-500/85",
                  h.state === "running" && "animate-pulse bg-sky-500",
                  h.state === "pending" && "bg-surface-3",
                  hostFilter === h.name && "ring-2 ring-accent",
                )}
              />
            ))}
          </div>
          <div className="flex gap-4 border-t border-line px-4 py-3 text-xs text-muted">
            <span className="flex items-center gap-1.5">
              <span className="size-2.5 rounded-sm bg-emerald-500" /> Done
            </span>
            <span className="flex items-center gap-1.5">
              <span className="size-2.5 rounded-sm bg-sky-500" /> Running
            </span>
            <span className="flex items-center gap-1.5">
              <span className="size-2.5 rounded-sm bg-surface-3" /> Pending
            </span>
          </div>
        </Card>

        {/* Log viewer */}
        <div className="flex min-h-[440px] flex-col overflow-hidden rounded-xl border border-line bg-term text-term-ink shadow-sm">
          <div className="flex flex-wrap items-center gap-2 border-b border-white/10 px-3 py-2">
            <span className="flex items-center gap-2 text-xs font-medium text-white/80">
              {live && <span className="live-dot size-2 rounded-full bg-emerald-400 text-emerald-400" />}
              {live ? "Live output" : "Output"}
            </span>
            <div className="flex flex-1 flex-wrap gap-1">
              <button onClick={() => setHostFilter(null)} className={cx("rounded px-2 py-0.5 font-mono text-[11px]", !hostFilter ? "bg-white/15 text-white" : "text-white/50 hover:text-white/80")}>
                all hosts
              </button>
              {logHosts.map((h, i) => (
                <button
                  key={h}
                  onClick={() => setHostFilter((f) => (f === h ? null : h))}
                  className={cx("rounded px-2 py-0.5 font-mono text-[11px]", hostFilter === h ? "bg-white/15 text-white" : "text-white/50 hover:text-white/80")}
                  style={{ color: hostFilter === h ? undefined : `oklch(0.78 0.12 ${hostHues[i % hostHues.length]})` }}
                >
                  {h.replace(".prod", "")}
                </button>
              ))}
            </div>
            <label className="flex h-7 items-center gap-1.5 rounded-md bg-white/5 px-2">
              <Search className="size-3.5 text-white/40" />
              <input value={q} onChange={(e) => setQ(e.target.value)} placeholder="Find in log" className="w-28 bg-transparent text-xs text-white outline-none placeholder:text-white/35" />
            </label>
            <button
              onClick={() => setFollow((f) => !f)}
              className={cx("flex h-7 items-center gap-1 rounded-md px-2 text-xs", follow ? "bg-white/15 text-white" : "text-white/50 hover:text-white/80")}
              aria-pressed={follow}
            >
              <ArrowDownToLine className="size-3.5" /> Follow
            </button>
          </div>
          <div ref={logRef} className="scroll-thin flex-1 overflow-y-auto py-2 font-mono text-[12.5px] leading-6">
            {lines.map((l, i) => {
              const hi = logHosts.indexOf(l.host);
              return (
                <div key={i} className="grid grid-cols-[64px_110px_120px_minmax(0,1fr)] gap-3 px-3 hover:bg-white/[0.04]">
                  <span className="text-white/30 tabular-nums">{l.t}</span>
                  <span className="truncate" style={{ color: `oklch(0.78 0.12 ${hostHues[hi % hostHues.length]})` }}>
                    {l.host.replace(".prod", "")}
                  </span>
                  <span className="truncate text-white/40">{l.step}</span>
                  <span className={cx("break-words", l.level === "ok" && "text-emerald-300", l.level === "warn" && "text-amber-300", l.level === "err" && "text-rose-300")}>{l.line}</span>
                </div>
              );
            })}
            {live && shown < liveLog.length && (
              <div className="flex items-center gap-2 px-3 text-white/40">
                <span className="inline-block h-4 w-2 animate-pulse bg-white/60" />
              </div>
            )}
            {lines.length === 0 && <div className="px-3 py-6 text-white/40">No lines match.</div>}
          </div>
        </div>
      </div>
    </>
  );
}
