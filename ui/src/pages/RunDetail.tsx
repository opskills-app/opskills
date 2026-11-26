import { useEffect, useMemo, useRef, useState } from "react";
import { ArrowDownToLine, ArrowLeft, Ban, Check, CircleStop, Loader2, RotateCcw, Search, Server, X } from "lucide-react";
import { api, isActive, type LogLine, type Run, type StepResult } from "../api";
import { clock, duration, runDuration, runNumber, timeAgo } from "../format";
import { Button, Card, CardHeader, cx, ErrorNote, StatusPill } from "../components/ui";
import { useRunNow } from "./Runbooks";

/** Subscribe to a run's live events; replays everything so far, then streams. */
function useRunStream(id: string) {
  const [run, setRun] = useState<Run | null>(null);
  const [lines, setLines] = useState<LogLine[]>([]);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    setRun(null);
    setLines([]);
    setError(null);
    const seen = new Set<number>();
    let buffer: LogLine[] = [];
    let raf = 0;
    const flush = () => {
      raf = 0;
      const add = buffer;
      buffer = [];
      setLines((prev) => prev.concat(add));
    };
    const es = new EventSource(`/api/v1/runs/${encodeURIComponent(id)}/events`);
    es.addEventListener("run", (e) => setRun(JSON.parse((e as MessageEvent).data)));
    es.addEventListener("log", (e) => {
      const line: LogLine = JSON.parse((e as MessageEvent).data);
      if (seen.has(line.seq)) return;
      seen.add(line.seq);
      buffer.push(line);
      if (!raf) raf = requestAnimationFrame(flush);
    });
    es.addEventListener("end", (e) => {
      const last = JSON.parse((e as MessageEvent).data);
      if (last) setRun(last);
      es.close();
    });
    es.onerror = () => {
      if (es.readyState === EventSource.CLOSED) return;
      // The browser reconnects on its own; check whether the run still exists.
      api.run(id).then(setRun, (err) => {
        setError(err.message);
        es.close();
      });
    };
    return () => {
      es.close();
      if (raf) cancelAnimationFrame(raf);
    };
  }, [id]);

  return { run, lines, error };
}

function useNow(active: boolean) {
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    if (!active) return;
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, [active]);
  return now;
}

function StepIcon({ s }: { s: StepResult["status"] }) {
  if (s === "succeeded") return <Check className="size-3.5 shrink-0 text-emerald-500" strokeWidth={3} />;
  if (s === "running") return <Loader2 className="size-3.5 shrink-0 animate-spin text-sky-500" />;
  if (s === "failed") return <X className="size-3.5 shrink-0 text-rose-500" strokeWidth={3} />;
  if (s === "cancelled" || s === "skipped") return <Ban className="size-3.5 shrink-0 text-muted" />;
  return <span className="size-3.5 shrink-0 rounded-full border border-line-strong" />;
}

const barTone: Record<string, string> = {
  succeeded: "bg-emerald-500",
  running: "progress-live bg-sky-500",
  failed: "bg-rose-500",
  cancelled: "bg-zinc-400",
  skipped: "bg-surface-3",
  pending: "bg-surface-3",
  queued: "bg-surface-3",
};

export function RunDetail({ id }: { id: string }) {
  const { run, lines, error } = useRunStream(id);
  const active = !!run && isActive(run.status);
  const now = useNow(active);
  const [stepFilter, setStepFilter] = useState<number | null>(null);
  const [q, setQ] = useState("");
  const [follow, setFollow] = useState(true);
  const [cancelling, setCancelling] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);
  const logRef = useRef<HTMLDivElement>(null);
  const { runNow, busy } = useRunNow();

  const shown = useMemo(
    () => lines.filter((l) => (stepFilter === null || l.step === stepFilter) && (!q || l.line.toLowerCase().includes(q.toLowerCase()))),
    [lines, stepFilter, q],
  );

  useEffect(() => {
    if (follow && logRef.current) logRef.current.scrollTop = logRef.current.scrollHeight;
  }, [shown.length, follow]);

  if (error) return <ErrorNote>{error}</ErrorNote>;
  if (!run) return <p className="text-sm text-muted">Loading run...</p>;

  async function cancel() {
    setCancelling(true);
    setActionError(null);
    try {
      await api.cancelRun(run!.id);
    } catch (e) {
      setActionError(e instanceof Error ? e.message : String(e));
    } finally {
      setCancelling(false);
    }
  }

  const done = run.steps.filter((s) => s.status === "succeeded").length;
  const hostCount = Math.max(1, run.hosts?.length ?? 1);
  const multi = hostCount > 1;
  const perHost = Math.max(1, run.steps.length / hostCount);
  const stepNo = (i: number) => (i % perHost) + 1;
  const hostNames = (run.hosts ?? []).map((h) => h.name);
  const lastExit = [...run.steps].reverse().find((s) => s.exit_code !== null)?.exit_code;

  return (
    <>
      <a href={`#/runbooks/${run.runbook_id}`} className="mb-3 inline-flex items-center gap-1 text-sm text-muted hover:text-ink">
        <ArrowLeft className="size-3.5" /> {run.runbook_name}
      </a>

      <div className="mb-6 flex flex-wrap items-start justify-between gap-4">
        <div>
          <div className="flex flex-wrap items-center gap-3">
            <h1 className="text-[26px] font-semibold tracking-tight">Run #{runNumber(run.id)}</h1>
            <StatusPill status={run.status} />
          </div>
          <p className="mt-1 text-sm text-muted">
            {run.trigger === "manual" ? "Started" : `Triggered by ${run.trigger},`} by {run.started_by} {timeAgo(run.created_at, now)}
            {hostNames.length > 0 && <> · on {hostNames.length > 3 ? `${hostNames.length} hosts` : hostNames.join(", ")}</>}
            {run.secrets?.length > 0 && <> · secrets: <span className="font-mono text-xs">{run.secrets.map((n) => "$" + n).join(", ")}</span> (masked in output)</>}
          </p>
        </div>
        <div className="flex items-center gap-2">
          {active ? (
            <Button onClick={cancel} disabled={cancelling} className="text-rose-600 dark:text-rose-400">
              <CircleStop className="size-4" /> {cancelling ? "Cancelling..." : "Cancel run"}
            </Button>
          ) : (
            <Button variant="primary" disabled={busy === run.runbook_id} onClick={() => runNow(run.runbook_id)}>
              <RotateCcw className="size-3.5" /> Run again
            </Button>
          )}
        </div>
      </div>

      {(run.error || actionError) && (
        <div className="mb-4">
          <ErrorNote>{actionError ?? run.error}</ErrorNote>
        </div>
      )}

      <div className="mb-4 grid grid-cols-2 gap-3 md:grid-cols-4">
        {[
          ["Elapsed", run.started_at ? duration(runDuration(run, now)) : "Waiting to start"],
          ["Steps", `${done} of ${run.steps.length} succeeded`],
          ["Last exit code", lastExit === undefined || lastExit === null ? "None yet" : String(lastExit)],
          ["Step timeout", duration(run.timeout_seconds * 1000)],
        ].map(([k, v]) => (
          <Card key={k} className="rounded-2xl px-5 py-4">
            <div className="text-xs text-muted">{k}</div>
            <div className="mt-0.5 truncate text-sm font-semibold tabular-nums">{v}</div>
          </Card>
        ))}
      </div>

      <Card className="mb-4 rounded-2xl p-5">
        <ol className="grid gap-3" style={{ gridTemplateColumns: `repeat(${Math.min(run.steps.length, 6)}, minmax(0, 1fr))` }}>
          {run.steps.map((s, i) => (
            <li key={i} className="min-w-0">
              <div className={cx("h-1 rounded-full", barTone[s.status])} />
              <div className="mt-2 flex items-center gap-1.5 text-xs">
                <StepIcon s={s.status} />
                <span className={cx("truncate font-medium", (s.status === "pending" || s.status === "skipped") && "text-muted")}>{multi ? `${s.host_name}: ${s.name}` : s.name}</span>
              </div>
            </li>
          ))}
        </ol>
      </Card>

      <div className="grid grid-cols-1 items-start gap-4 xl:grid-cols-[300px_minmax(0,1fr)]">
        <Card>
          <CardHeader title="Steps" hint="Click a step to filter the output" />
          <ul className="space-y-1 px-3 pb-4 sm:px-5">
            {run.steps.map((s, i) => {
              const took = s.started_at ? (s.finished_at ?? now) - s.started_at : null;
              const firstOfHost = i % perHost === 0;
              return (
                <li key={i}>
                  {firstOfHost && (multi || s.host_id !== "local") && (
                    <div className={cx("flex items-center gap-2 px-3 pb-1 text-xs font-semibold text-muted", i > 0 && "mt-3")}>
                      <Server className="size-3.5" /> {s.host_name}
                    </div>
                  )}
                  <button
                    onClick={() => setStepFilter((f) => (f === i ? null : i))}
                    className={cx("flex w-full items-center gap-3 rounded-xl px-3 py-2.5 text-left text-sm", stepFilter === i ? "bg-accent-soft" : "hover:bg-surface-2")}
                  >
                    <StepIcon s={s.status} />
                    <span className="min-w-0 flex-1">
                      <span className="block truncate font-medium">{s.name}</span>
                      <span className="block truncate font-mono text-[11px] text-muted">{s.run.split("\n")[0]}</span>
                    </span>
                    <span className="text-right text-xs text-muted tabular-nums">
                      {took !== null && <span className="block">{duration(took)}</span>}
                      {s.exit_code !== null && <span className="block">exit {s.exit_code}</span>}
                    </span>
                  </button>
                </li>
              );
            })}
          </ul>
        </Card>

        <div className="flex min-h-[460px] flex-col overflow-hidden rounded-[22px] bg-term text-term-ink shadow-soft">
          <div className="flex flex-wrap items-center gap-2 border-b border-white/10 px-4 py-3">
            <span className="flex items-center gap-2 text-xs font-medium text-white/80">
              {active && <span className="live-dot size-2 rounded-full bg-emerald-400 text-emerald-400" />}
              {active ? "Live output" : "Output"}
              <span className="text-white/40">{lines.length} lines</span>
            </span>
            {stepFilter !== null && (
              <button onClick={() => setStepFilter(null)} className="rounded bg-white/15 px-2 py-0.5 font-mono text-[11px] text-white">
                {multi && `${run.steps[stepFilter]?.host_name} `}step {stepNo(stepFilter)} ×
              </button>
            )}
            <div className="flex-1" />
            <label className="flex h-7 items-center gap-1.5 rounded-md bg-white/5 px-2">
              <Search className="size-3.5 text-white/40" />
              <input value={q} onChange={(e) => setQ(e.target.value)} placeholder="Find in output" className="w-28 bg-transparent text-xs text-white outline-none placeholder:text-white/35" />
            </label>
            <button
              onClick={() => setFollow((f) => !f)}
              className={cx("flex h-7 items-center gap-1 rounded-md px-2 text-xs", follow ? "bg-white/15 text-white" : "text-white/50 hover:text-white/80")}
              aria-pressed={follow}
            >
              <ArrowDownToLine className="size-3.5" /> Follow
            </button>
          </div>
          <div ref={logRef} className="scroll-thin max-h-[640px] flex-1 overflow-y-auto py-2 font-mono text-[12.5px] leading-6">
            {shown.map((l) => (
              <div key={l.seq} className={cx("grid gap-3 px-4 hover:bg-white/[0.04]", multi ? "grid-cols-[64px_minmax(0,88px)_28px_minmax(0,1fr)]" : "grid-cols-[64px_44px_minmax(0,1fr)]")}>
                <span className="text-white/30 tabular-nums">{clock(l.ts)}</span>
                {multi && (
                  <span className="truncate text-sky-300/70" title={l.host}>
                    {l.host}
                  </span>
                )}
                <span className="text-white/35">#{stepNo(l.step)}</span>
                <span className={cx("break-words whitespace-pre-wrap", l.stream === "stderr" && "text-amber-300", l.stream === "system" && "text-violet-300")}>{l.line || " "}</span>
              </div>
            ))}
            {active && (
              <div className="px-4 text-white/40">
                <span className="inline-block h-4 w-2 animate-pulse bg-white/60" />
              </div>
            )}
            {!active && shown.length === 0 && <div className="px-4 py-6 text-white/40">{lines.length ? "No lines match." : "No output."}</div>}
          </div>
        </div>
      </div>
    </>
  );
}
