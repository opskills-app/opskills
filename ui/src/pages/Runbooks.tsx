import { useEffect, useMemo, useState } from "react";
import { Activity, BookOpen, Database, HardDrive, LayoutGrid, List, Play, Plus, RefreshCcw, Rocket, ShieldCheck, Stethoscope, Terminal } from "lucide-react";
import { api, useData, type Run, type Runbook } from "../api";
import { duration, runDuration, timeAgo } from "../format";
import { Button, Card, cx, EmptyState, ErrorNote, PageHeader, RunStrip, SearchField, Tag, Tile } from "../components/ui";
import { StartRunDialog } from "../components/StartRun";
import { templates } from "../templates";
import { go } from "../router";
import { useCan } from "../session";

const icons = [RefreshCcw, ShieldCheck, Database, HardDrive, Stethoscope, Rocket, Terminal, Activity];

export function runbookIcon(id: string) {
  const n = [...id].reduce((a, c) => a + c.charCodeAt(0), 0);
  return icons[n % icons.length];
}

type View = "cards" | "compact";
const VIEW_KEY = "opskills-runbooks-view";

function useView(): [View, (v: View) => void] {
  const [view, setView] = useState<View>(() => {
    const forced = new URLSearchParams(window.location.search).get("view");
    if (forced === "compact" || forced === "cards") return forced;
    try {
      return localStorage.getItem(VIEW_KEY) === "compact" ? "compact" : "cards";
    } catch {
      return "cards";
    }
  });
  useEffect(() => {
    try {
      localStorage.setItem(VIEW_KEY, view);
    } catch {
      /* storage unavailable */
    }
  }, [view]);
  return [view, setView];
}

function ViewToggle({ view, onChange }: { view: View; onChange: (v: View) => void }) {
  const options: { id: View; label: string; icon: typeof List }[] = [
    { id: "cards", label: "Cards", icon: LayoutGrid },
    { id: "compact", label: "Compact", icon: List },
  ];
  return (
    <div role="radiogroup" aria-label="View" className="flex h-11 items-center rounded-xl bg-surface-2 p-1">
      {options.map((o) => (
        <button
          key={o.id}
          role="radio"
          aria-checked={view === o.id}
          onClick={() => onChange(o.id)}
          title={`${o.label} view`}
          className={cx("flex h-9 items-center gap-1.5 rounded-lg px-3 text-xs font-medium transition-colors", view === o.id ? "bg-surface text-accent shadow-sm" : "text-muted hover:text-ink")}
        >
          <o.icon className="size-4" />
          <span className="hidden sm:inline">{o.label}</span>
        </button>
      ))}
    </div>
  );
}

/**
 * Start a run and open its page. A runbook with inputs first asks for them in a
 * dialog, which the caller renders as `dialog`. `prefill` offers an earlier run's values.
 */
export function useRunNow() {
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [asking, setAsking] = useState<{ rb: Runbook; prefill?: Record<string, string> } | null>(null);

  async function start(id: string, inputs?: Record<string, string>) {
    const run = await api.startRun(id, inputs);
    go(`runs/${run.id}`);
  }

  async function runNow(id: string, prefill?: Record<string, string>) {
    setBusy(id);
    setError(null);
    try {
      const rb = await api.runbook(id);
      if (rb.inputs?.length) setAsking({ rb, prefill });
      else await start(id);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(null);
    }
  }

  const dialog = asking && <StartRunDialog rb={asking.rb} prefill={asking.prefill} onStart={(inputs) => start(asking.rb.id, inputs)} onClose={() => setAsking(null)} />;
  return { runNow, busy, error, dialog };
}

function stats(runs: Run[], id: string) {
  const mine = runs.filter((r) => r.runbook_id === id);
  const ok = mine.filter((r) => r.status === "succeeded");
  const avg = ok.length ? ok.reduce((a, r) => a + (runDuration(r) ?? 0), 0) / ok.length : null;
  return { recent: mine.slice(0, 10).reverse().map((r) => r.status), last: mine[0], avg };
}

export function RunbooksPage() {
  const runbooks = useData(() => api.runbooks(), []);
  const runs = useData(() => api.runs({ limit: 1000 }), [], 5000);
  const [q, setQ] = useState("");
  const [tag, setTag] = useState<string | null>(null);
  const [view, setView] = useView();
  const { runNow, busy, error, dialog } = useRunNow();
  const canEdit = useCan("editor");
  const canRun = useCan("operator");

  const all = useMemo(() => runbooks.data ?? [], [runbooks.data]);
  const tags = useMemo(() => [...new Set(all.flatMap((r) => r.tags))].sort(), [all]);
  const list = all.filter((r) => (!tag || r.tags.includes(tag)) && `${r.name} ${r.description} ${r.tags.join(" ")}`.toLowerCase().includes(q.toLowerCase()));

  const runButton = (rb: Runbook, variant: "primary" | "outline" = "primary") =>
    canRun && (
      <Button size="sm" variant={variant} disabled={busy === rb.id} onClick={() => runNow(rb.id)} aria-label={`Run ${rb.name}`}>
        <Play className="size-3" fill="currentColor" />
        {busy === rb.id ? "Starting" : "Run"}
      </Button>
    );

  return (
    <div className="space-y-8">
      {dialog}
      <PageHeader
        title="Runbooks"
        subtitle="Procedures your team can run with one click. Each step is a shell command, run on the OpSkills server or on your hosts."
        actions={
          canEdit && (
            <Button variant="primary" onClick={() => go("runbooks/new")}>
              <Plus className="size-4" />
              New runbook
            </Button>
          )
        }
      />
      {error && <ErrorNote>{error}</ErrorNote>}
      {runbooks.error && <ErrorNote>{runbooks.error}</ErrorNote>}

      {runbooks.data && all.length === 0 ? (
        <Card>
          <EmptyState
            icon={BookOpen}
            title="No runbooks yet"
            action={
              canEdit && (
                <>
                  <Button variant="primary" onClick={() => go("runbooks/new")}>
                    <Plus className="size-4" /> Start from scratch
                  </Button>
                  {templates.map((t) => (
                    <Button key={t.key} onClick={() => go(`runbooks/new/${t.key}`)}>
                      {t.label}
                    </Button>
                  ))}
                </>
              )
            }
          >
            Create one from scratch or start from an example. You can edit everything before saving.
          </EmptyState>
        </Card>
      ) : (
        <>
          <Card className="flex flex-wrap items-center gap-4 px-6 py-5 sm:px-8">
            <SearchField value={q} onChange={setQ} placeholder="Search runbooks" />
            <div className="flex flex-1 flex-wrap gap-2">
              {[null, ...tags].map((t) => (
                <button
                  key={t ?? "all"}
                  onClick={() => setTag(t)}
                  className={cx("h-9 rounded-xl px-4 text-xs font-medium transition-colors", tag === t ? "bg-accent text-accent-ink" : "bg-surface-2 text-ink-2 hover:bg-surface-3")}
                >
                  {t ?? "All"}
                </button>
              ))}
            </div>
            <ViewToggle view={view} onChange={setView} />
          </Card>

          {view === "compact" ? (
            <Card className="overflow-hidden">
              <div className="hidden grid-cols-[minmax(0,1.6fr)_90px_150px_170px_96px] gap-6 border-b border-line px-8 py-4 text-sm font-medium text-muted lg:grid">
                <span>Runbook</span>
                <span>Steps</span>
                <span>Last run</span>
                <span>Recent runs</span>
                <span />
              </div>
              <ul className="px-4 sm:px-8">
                {list.map((rb) => {
                  const s = stats(runs.data ?? [], rb.id);
                  return (
                    <li key={rb.id} className="grid grid-cols-[minmax(0,1fr)_auto] items-center gap-x-6 border-b border-line py-3 text-sm last:border-0 lg:grid-cols-[minmax(0,1.6fr)_90px_150px_170px_96px]">
                      <a href={`#/runbooks/${rb.id}`} className="group flex min-w-0 items-center gap-3">
                        <Tile seed={rb.id} icon={runbookIcon(rb.id)} className="size-9 rounded-xl [&_svg]:size-4" />
                        <span className="min-w-0">
                          <span className="block truncate font-medium group-hover:text-accent">{rb.name}</span>
                          <span className="block truncate text-xs text-muted">{rb.tags.join(" · ") || "No tags"}</span>
                        </span>
                      </a>
                      <span className="hidden text-muted lg:block">{rb.steps.length}</span>
                      <span className="hidden truncate text-muted lg:block">{s.last ? timeAgo(s.last.created_at) : "Never"}</span>
                      <span className="hidden items-center gap-2 text-xs text-muted lg:flex">
                        <RunStrip history={s.recent} />
                        {s.avg !== null && <span className="tabular-nums">{duration(s.avg)}</span>}
                      </span>
                      <span className="text-right">{runButton(rb, "outline")}</span>
                    </li>
                  );
                })}
                {list.length === 0 && <li className="py-12 text-center text-sm text-muted">{runbooks.loading ? "Loading..." : "No runbooks match this filter."}</li>}
              </ul>
            </Card>
          ) : (
            <div className="grid grid-cols-1 gap-6 lg:grid-cols-2 2xl:grid-cols-3">
              {list.map((rb) => {
                const s = stats(runs.data ?? [], rb.id);
                return (
                  <Card key={rb.id} className="group flex flex-col p-6 transition-transform hover:-translate-y-0.5">
                    <div className="flex items-start gap-4">
                      <Tile seed={rb.id} icon={runbookIcon(rb.id)} className="h-16 w-16" />
                      <a href={`#/runbooks/${rb.id}`} className="min-w-0 flex-1">
                        <h3 className="truncate text-[17px] font-semibold group-hover:text-accent">{rb.name}</h3>
                        <p className="mt-1 line-clamp-2 text-[13px] text-muted">{rb.description || `${rb.steps.length} step${rb.steps.length === 1 ? "" : "s"}`}</p>
                      </a>
                    </div>
                    <div className="mt-4 flex min-h-6 flex-wrap gap-1.5">
                      {rb.tags.map((t) => (
                        <Tag key={t}>{t}</Tag>
                      ))}
                    </div>
                    <div className="mt-5 flex items-center justify-between gap-3 border-t border-line pt-5 text-xs text-muted">
                      <span>
                        {rb.steps.length} step{rb.steps.length === 1 ? "" : "s"}
                      </span>
                      <span>Last run: {s.last ? timeAgo(s.last.created_at) : "never"}</span>
                    </div>
                    <div className="mt-4 flex items-center justify-between gap-3">
                      <span className="flex items-center gap-3 text-xs text-muted">
                        <RunStrip history={s.recent} />
                        {s.avg !== null && <span className="tabular-nums">avg {duration(s.avg)}</span>}
                      </span>
                      {runButton(rb)}
                    </div>
                  </Card>
                );
              })}
              {list.length === 0 && <p className="col-span-full py-16 text-center text-sm text-muted">{runbooks.loading ? "Loading..." : "No runbooks match this filter."}</p>}
            </div>
          )}
        </>
      )}
    </div>
  );
}
