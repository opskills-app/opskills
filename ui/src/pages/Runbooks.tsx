import { useEffect, useMemo, useState } from "react";
import { CalendarClock, Database, HardDrive, LayoutGrid, List, Play, Plus, RefreshCcw, Rocket, ShieldCheck, Stethoscope } from "lucide-react";
import { runbooks } from "../data";
import { Avatar, Button, Card, cx, PageHeader, RunStrip, SearchField, SortButton, Tag, Tile } from "../components/ui";
import { go } from "../router";

const icons: Record<string, typeof Play> = {
  "restart-web": RefreshCcw,
  "rotate-certs": ShieldCheck,
  "db-failover-drill": Database,
  "disk-cleanup": HardDrive,
  "smoke-test": Stethoscope,
  "onboard-host": Rocket,
};

export function runbookIcon(id: string) {
  return icons[id] ?? Play;
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
          className={cx(
            "flex h-9 items-center gap-1.5 rounded-lg px-3 text-xs font-medium transition-colors",
            view === o.id ? "bg-surface text-accent shadow-sm" : "text-muted hover:text-ink",
          )}
        >
          <o.icon className="size-4" />
          <span className="hidden sm:inline">{o.label}</span>
        </button>
      ))}
    </div>
  );
}

export function RunbooksPage() {
  const [q, setQ] = useState("");
  const [tag, setTag] = useState<string | null>(null);
  const [view, setView] = useView();
  const tags = useMemo(() => [...new Set(runbooks.flatMap((r) => r.tags))].sort(), []);
  const list = runbooks.filter(
    (r) => (!tag || r.tags.includes(tag)) && `${r.name} ${r.description} ${r.tags.join(" ")}`.toLowerCase().includes(q.toLowerCase()),
  );

  return (
    <div className="space-y-8">
      <PageHeader
        title="Runbooks"
        subtitle="Repeatable procedures your team can run by hand, on a schedule, or from a webhook."
        actions={
          <Button variant="primary">
            <Plus className="size-4" />
            New runbook
          </Button>
        }
      />

      <Card className="flex flex-wrap items-center gap-4 px-6 py-5 sm:px-8">
        <SearchField value={q} onChange={setQ} placeholder="Search runbooks" />
        <div className="flex flex-1 flex-wrap gap-2">
          {[null, ...tags].map((t) => (
            <button
              key={t ?? "all"}
              onClick={() => setTag(t)}
              className={cx(
                "h-9 rounded-xl px-4 text-xs font-medium transition-colors",
                tag === t ? "bg-accent text-accent-ink" : "bg-surface-2 text-ink-2 hover:bg-surface-3",
              )}
            >
              {t ?? "All"}
            </button>
          ))}
        </div>
        <div className="flex items-center gap-3">
          <SortButton value="Last run" />
          <ViewToggle view={view} onChange={setView} />
        </div>
      </Card>

      {view === "compact" ? (
        <Card className="overflow-hidden">
          <div className="hidden grid-cols-[minmax(0,1.6fr)_minmax(0,0.9fr)_150px_160px_96px] gap-6 border-b border-line px-8 py-4 text-sm font-medium text-muted lg:grid">
            <span>Runbook</span>
            <span>Owner</span>
            <span>Schedule</span>
            <span>Recent runs</span>
            <span />
          </div>
          <ul className="px-4 sm:px-8">
            {list.map((r) => (
              <li
                key={r.id}
                className="grid grid-cols-[minmax(0,1fr)_auto] items-center gap-x-6 border-b border-line py-3 text-sm last:border-0 lg:grid-cols-[minmax(0,1.6fr)_minmax(0,0.9fr)_150px_160px_96px]"
              >
                <a href={`#/runbooks/${r.id}`} className="group flex min-w-0 items-center gap-3">
                  <Tile seed={r.id} icon={runbookIcon(r.id)} className="size-9 rounded-xl [&_svg]:size-4" />
                  <span className="min-w-0">
                    <span className="block truncate font-medium group-hover:text-accent">{r.name}</span>
                    <span className="block truncate text-xs text-muted">{r.tags.join(" · ")}</span>
                  </span>
                </a>
                <span className="hidden min-w-0 items-center gap-2 lg:flex">
                  <Avatar name={r.owner} size="size-6" />
                  <span className="truncate">{r.owner}</span>
                </span>
                <span className="hidden truncate text-muted lg:block">{r.schedule ?? "Manual"}</span>
                <span className="hidden items-center gap-2 text-xs text-muted lg:flex">
                  <RunStrip history={r.recent} />
                  <span className="tabular-nums">{r.avgDuration}</span>
                </span>
                <span className="text-right">
                  <Button size="sm" variant="outline" onClick={() => go("runs/r-2741")} aria-label={`Run ${r.name}`}>
                    <Play className="size-3" fill="currentColor" />
                    Run
                  </Button>
                </span>
              </li>
            ))}
            {list.length === 0 && <li className="py-12 text-center text-sm text-muted">No runbooks match this filter.</li>}
          </ul>
        </Card>
      ) : (
      <div className="grid grid-cols-1 gap-6 lg:grid-cols-2 2xl:grid-cols-3">
        {list.map((r) => (
          <Card key={r.id} className="group flex flex-col p-6 transition-transform hover:-translate-y-0.5">
            <div className="flex items-start gap-4">
              <Tile seed={r.id} icon={runbookIcon(r.id)} className="h-16 w-16" />
              <a href={`#/runbooks/${r.id}`} className="min-w-0 flex-1">
                <h3 className="truncate text-[17px] font-semibold group-hover:text-accent">{r.name}</h3>
                <p className="mt-1 line-clamp-2 text-[13px] text-muted">{r.description}</p>
              </a>
            </div>
            <div className="mt-4 flex flex-wrap gap-1.5">
              {r.tags.map((t) => (
                <Tag key={t}>{t}</Tag>
              ))}
            </div>
            <div className="mt-5 flex items-center justify-between gap-3 border-t border-line pt-5 text-xs text-muted">
              <span className="flex min-w-0 items-center gap-2">
                <Avatar name={r.owner} size="size-7" />
                <span className="truncate">{r.owner}</span>
              </span>
              <span className="flex items-center gap-1.5 whitespace-nowrap">
                {r.schedule ? (
                  <>
                    <CalendarClock className="size-4" />
                    {r.schedule}
                  </>
                ) : (
                  "Manual"
                )}
              </span>
            </div>
            <div className="mt-4 flex items-center justify-between gap-3">
              <span className="flex items-center gap-3 text-xs text-muted">
                <RunStrip history={r.recent} />
                <span className="tabular-nums">avg {r.avgDuration}</span>
              </span>
              <Button size="sm" variant="primary" onClick={() => go("runs/r-2741")} aria-label={`Run ${r.name}`}>
                <Play className="size-3" fill="currentColor" />
                Run
              </Button>
            </div>
          </Card>
        ))}
        {list.length === 0 && <p className="col-span-full py-16 text-center text-sm text-muted">No runbooks match this filter.</p>}
      </div>
      )}
    </div>
  );
}
