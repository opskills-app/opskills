import { useState } from "react";
import { Monitor, RefreshCw, Server, ServerCog } from "lucide-react";
import { hosts } from "../data";
import { Avatar, Badge, Button, Card, Delta, IconBubble, PageHeader, Pagination, SearchField, SortButton, TableHeading, type BadgeTone } from "../components/ui";

const statusTone: Record<string, BadgeTone> = { online: "green", offline: "red", degraded: "amber" };

export function HostsPage() {
  const [q, setQ] = useState("");
  const list = hosts.filter((h) => `${h.name} ${h.address} ${h.tags.join(" ")} ${h.os} ${h.status}`.toLowerCase().includes(q.toLowerCase()));
  const online = hosts.filter((h) => h.status === "online").length;

  return (
    <div className="space-y-8">
      <PageHeader
        title="Hosts"
        subtitle="Machines your runbooks can target, collected from your inventory sources."
        actions={
          <Button>
            <RefreshCw className="size-4" /> Sync inventory
          </Button>
        }
      />

      <Card className="grid grid-cols-1 gap-4 px-4 py-6 md:grid-cols-3 md:divide-x md:divide-line md:px-6 md:py-8">
        <div className="flex items-center gap-5 px-2 py-2 sm:px-6">
          <IconBubble icon={Server} tone="mint" size="size-[72px] sm:size-20" />
          <div>
            <div className="text-sm text-muted">Total hosts</div>
            <div className="mt-0.5 text-[30px] leading-tight font-semibold tabular-nums">{hosts.length}</div>
            <Delta value="+16%" good period="this month" />
          </div>
        </div>
        <div className="flex items-center gap-5 px-2 py-2 sm:px-6">
          <IconBubble icon={ServerCog} tone="mint" size="size-[72px] sm:size-20" />
          <div>
            <div className="text-sm text-muted">Online</div>
            <div className="mt-0.5 text-[30px] leading-tight font-semibold tabular-nums">{online}</div>
            <Delta value="-1%" good={false} period="this month" />
          </div>
        </div>
        <div className="flex items-center gap-5 px-2 py-2 sm:px-6">
          <IconBubble icon={Monitor} tone="mint" size="size-[72px] sm:size-20" />
          <div>
            <div className="text-sm text-muted">Running a step now</div>
            <div className="mt-0.5 text-[30px] leading-tight font-semibold tabular-nums">4</div>
            <div className="mt-1 flex -space-x-2">
              {["web-01", "web-02", "web-03", "web-04"].map((h) => (
                <span key={h} className="rounded-full ring-2 ring-surface">
                  <Avatar name={h.replace("-", " ")} size="size-7" />
                </span>
              ))}
            </div>
          </div>
        </div>
      </Card>

      <Card className="overflow-hidden">
        <TableHeading title="All hosts" subtitle="From 2 inventory sources">
          <SearchField value={q} onChange={setQ} />
          <SortButton value="Name" />
        </TableHeading>
        <div className="hidden grid-cols-[minmax(0,1.3fr)_130px_130px_minmax(0,1fr)_150px_120px] gap-6 border-b border-line px-10 pb-4 text-sm font-medium text-muted lg:grid">
          <span>Host name</span>
          <span>Address</span>
          <span>OS</span>
          <span>Tags</span>
          <span>Source</span>
          <span className="text-center">Status</span>
        </div>
        <ul className="px-6 sm:px-10">
          {list.slice(0, 10).map((h) => (
            <li
              key={h.name}
              className="grid grid-cols-[minmax(0,1fr)_auto] items-center gap-x-6 border-b border-line py-5 text-sm last:border-0 lg:grid-cols-[minmax(0,1.3fr)_130px_130px_minmax(0,1fr)_150px_120px]"
            >
              <span className="min-w-0">
                <span className="block truncate font-medium">{h.name}</span>
                <span className="text-xs text-muted lg:hidden">{h.address}</span>
              </span>
              <span className="hidden font-mono text-[13px] lg:block">{h.address}</span>
              <span className="hidden lg:block">{h.os}</span>
              <span className="hidden truncate lg:block">{h.tags.join(", ")}</span>
              <span className="hidden truncate lg:block">{h.source}</span>
              <span className="text-center">
                <Badge tone={statusTone[h.status]} className="capitalize">
                  {h.status}
                </Badge>
              </span>
            </li>
          ))}
        </ul>
        <Pagination from={1} to={Math.min(10, list.length)} total={String(list.length)} pages={3} />
      </Card>
    </div>
  );
}
