import { useState } from "react";
import { KeyRound, Lock, Plus, ShieldCheck } from "lucide-react";
import { secrets } from "../data";
import { Badge, Button, Card, PageHeader, SearchField, SortButton, TableHeading } from "../components/ui";

export function VaultPage() {
  const [q, setQ] = useState("");
  const list = secrets.filter((s) => `${s.path} ${s.kind}`.toLowerCase().includes(q.toLowerCase()));
  return (
    <div className="space-y-8">
      <PageHeader
        title="Vault"
        subtitle="Keys, passwords and tokens your runbooks use."
        actions={
          <Button variant="primary">
            <Plus className="size-4" /> Add secret
          </Button>
        }
      />
      <Card className="flex items-center gap-4 px-6 py-5 text-sm sm:px-8">
        <span className="flex size-12 shrink-0 items-center justify-center rounded-full bg-emerald-500/15 text-emerald-600 dark:text-emerald-300">
          <ShieldCheck className="size-6" strokeWidth={1.75} />
        </span>
        Values are encrypted with AES-256-GCM using this server's own key, never shown again after saving, and masked in every run log.
      </Card>
      <Card className="overflow-hidden">
        <TableHeading title="All secrets" subtitle={`${secrets.length} secrets in this workspace`}>
          <SearchField value={q} onChange={setQ} />
          <SortButton value="Recently updated" />
        </TableHeading>
        <div className="hidden grid-cols-[minmax(0,1.4fr)_150px_150px_140px_110px] gap-6 border-b border-line px-10 pb-4 text-sm font-medium text-muted lg:grid">
          <span>Path</span>
          <span>Type</span>
          <span>Used by</span>
          <span>Updated</span>
          <span className="text-center">Value</span>
        </div>
        <ul className="px-6 pb-4 sm:px-10">
          {list.map((s) => (
            <li key={s.path} className="grid grid-cols-[minmax(0,1fr)_auto] items-center gap-x-6 border-b border-line py-5 text-sm last:border-0 lg:grid-cols-[minmax(0,1.4fr)_150px_150px_140px_110px]">
              <span className="flex min-w-0 items-center gap-3">
                <span className="flex size-9 shrink-0 items-center justify-center rounded-full bg-accent-soft text-accent">
                  {s.kind.startsWith("SSH") ? <KeyRound className="size-4" /> : <Lock className="size-4" />}
                </span>
                <span className="truncate font-mono text-[13px]">{s.path}</span>
              </span>
              <span className="hidden lg:block">{s.kind}</span>
              <span className="hidden lg:block">{s.usedBy} runbooks</span>
              <span className="hidden text-muted lg:block">{s.updated}</span>
              <span className="text-center">
                <Badge tone="gray">Hidden</Badge>
              </span>
            </li>
          ))}
        </ul>
      </Card>
    </div>
  );
}
