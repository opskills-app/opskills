import { useState } from "react";
import { Cloud, FileText, RefreshCw, Search } from "lucide-react";
import { hosts } from "../data";
import { Button, Card, cx, PageHeader, StatusDot, Tag } from "../components/ui";

export function HostsPage() {
  const [q, setQ] = useState("");
  const list = hosts.filter((h) => `${h.name} ${h.address} ${h.tags.join(" ")} ${h.os}`.toLowerCase().includes(q.toLowerCase()));
  return (
    <>
      <PageHeader
        title="Hosts"
        subtitle="Machines your runbooks can target, collected from your inventory sources."
        actions={
          <Button>
            <RefreshCw className="size-3.5" /> Sync inventory
          </Button>
        }
      />
      <div className="mb-4 grid grid-cols-1 gap-3 md:grid-cols-2">
        {[
          { icon: Cloud, name: "AWS us-east-1", detail: "EC2 instances tagged opskills=true", count: 12, synced: "2 min ago" },
          { icon: FileText, name: "Static inventory", detail: "inventory/prod.yaml in Git", count: 12, synced: "1 h ago" },
        ].map((s) => (
          <Card key={s.name} className="flex items-center gap-3 p-4">
            <span className="flex size-10 items-center justify-center rounded-xl bg-surface-2 text-ink-2">
              <s.icon className="size-5" />
            </span>
            <div className="min-w-0 flex-1">
              <div className="font-medium">{s.name}</div>
              <div className="truncate text-xs text-muted">{s.detail}</div>
            </div>
            <div className="text-right text-xs text-muted">
              <div className="text-sm font-semibold text-ink tabular-nums">{s.count} hosts</div>
              synced {s.synced}
            </div>
          </Card>
        ))}
      </div>
      <label className="mb-3 flex h-9 w-full items-center gap-2 rounded-lg border border-line bg-surface px-3 sm:w-96">
        <Search className="size-4 text-muted" />
        <input value={q} onChange={(e) => setQ(e.target.value)} placeholder="Filter by name, tag or address  (e.g. tags:web)" className="flex-1 bg-transparent text-sm outline-none placeholder:text-muted" />
      </label>
      <Card className="overflow-hidden">
        <div className="hidden grid-cols-[minmax(0,1.2fr)_120px_130px_minmax(0,1fr)_140px_110px] gap-4 border-b border-line bg-surface-2 px-4 py-2 text-xs font-medium text-muted lg:grid">
          <span>Host</span>
          <span>Address</span>
          <span>OS</span>
          <span>Tags</span>
          <span>Source</span>
          <span className="text-right">Last seen</span>
        </div>
        <ul className="divide-y divide-line">
          {list.map((h) => (
            <li key={h.name} className="grid grid-cols-[minmax(0,1fr)_auto] items-center gap-4 px-4 py-2.5 text-sm lg:grid-cols-[minmax(0,1.2fr)_120px_130px_minmax(0,1fr)_140px_110px]">
              <span className="flex min-w-0 items-center gap-2.5">
                <StatusDot status={h.status} />
                <span className="truncate font-mono text-[13px]">{h.name}</span>
              </span>
              <span className="font-mono text-xs text-muted lg:text-[13px]">{h.address}</span>
              <span className="hidden text-muted lg:block">{h.os}</span>
              <span className="hidden flex-wrap gap-1 lg:flex">
                {h.tags.map((t) => (
                  <Tag key={t}>{t}</Tag>
                ))}
              </span>
              <span className="hidden truncate text-muted lg:block">{h.source}</span>
              <span className={cx("hidden text-right lg:block", h.status === "offline" ? "text-rose-600" : "text-muted")}>{h.lastSeen}</span>
            </li>
          ))}
        </ul>
      </Card>
    </>
  );
}
