import { useMemo, useState } from "react";
import { CalendarClock, Play, Plus, Search } from "lucide-react";
import { runbooks } from "../data";
import { Avatar, Button, Card, cx, PageHeader, RunStrip, Tag } from "../components/ui";
import { go } from "../router";

export function RunbooksPage() {
  const [q, setQ] = useState("");
  const [tag, setTag] = useState<string | null>(null);
  const tags = useMemo(() => [...new Set(runbooks.flatMap((r) => r.tags))].sort(), []);
  const list = runbooks.filter(
    (r) => (!tag || r.tags.includes(tag)) && `${r.name} ${r.description} ${r.tags.join(" ")}`.toLowerCase().includes(q.toLowerCase()),
  );

  return (
    <>
      <PageHeader
        title="Runbooks"
        subtitle="Repeatable procedures your team can run safely, by hand, on a schedule, or from a webhook."
        actions={
          <Button variant="primary">
            <Plus className="size-4" />
            New runbook
          </Button>
        }
      />

      <div className="mb-4 flex flex-wrap items-center gap-2">
        <label className="flex h-9 w-full items-center gap-2 rounded-lg border border-line bg-surface px-3 sm:w-72">
          <Search className="size-4 text-muted" />
          <input value={q} onChange={(e) => setQ(e.target.value)} placeholder="Filter runbooks" className="flex-1 bg-transparent text-sm outline-none placeholder:text-muted" />
        </label>
        <div className="flex flex-wrap gap-1.5">
          {[null, ...tags].map((t) => (
            <button
              key={t ?? "all"}
              onClick={() => setTag(t)}
              className={cx(
                "h-7 rounded-full border px-3 text-xs font-medium transition-colors",
                tag === t ? "border-transparent bg-ink text-canvas" : "border-line bg-surface text-ink-2 hover:bg-surface-2",
              )}
            >
              {t ?? "All"}
            </button>
          ))}
        </div>
      </div>

      <div className="grid grid-cols-1 gap-3 lg:grid-cols-2 2xl:grid-cols-3">
        {list.map((r) => (
          <Card key={r.id} className="group flex flex-col p-4 transition-shadow hover:shadow-md">
            <div className="flex items-start justify-between gap-3">
              <a href={`#/runbooks/${r.id}`} className="min-w-0">
                <h3 className="truncate font-semibold tracking-tight group-hover:text-accent">{r.name}</h3>
                <p className="mt-1 line-clamp-2 text-sm text-muted">{r.description}</p>
              </a>
              <Button size="sm" variant="outline" onClick={() => go("runs/r-2741")} aria-label={`Run ${r.name}`}>
                <Play className="size-3" fill="currentColor" />
                Run
              </Button>
            </div>
            <div className="mt-3 flex flex-wrap gap-1">
              {r.tags.map((t) => (
                <Tag key={t}>{t}</Tag>
              ))}
            </div>
            <div className="mt-auto flex items-center justify-between gap-3 border-t border-line pt-3 text-xs text-muted [&]:mt-4">
              <span className="flex items-center gap-1.5">
                <Avatar name={r.owner} size="size-5" />
                {r.owner}
              </span>
              <span className="flex items-center gap-1.5">
                {r.schedule ? (
                  <>
                    <CalendarClock className="size-3.5" />
                    {r.schedule}
                  </>
                ) : (
                  "Manual"
                )}
              </span>
              <span className="flex items-center gap-2">
                <span className="tabular-nums">{r.avgDuration}</span>
                <RunStrip history={r.recent} />
              </span>
            </div>
          </Card>
        ))}
        {list.length === 0 && <p className="col-span-full py-16 text-center text-sm text-muted">No runbooks match this filter.</p>}
      </div>
    </>
  );
}
