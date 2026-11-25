import { KeyRound, Lock, Plus, ShieldCheck } from "lucide-react";
import { secrets } from "../data";
import { Button, Card, PageHeader } from "../components/ui";

export function VaultPage() {
  return (
    <>
      <PageHeader
        title="Vault"
        subtitle="Keys, passwords and tokens your runbooks use. Values are encrypted and never shown again after saving."
        actions={
          <Button variant="primary">
            <Plus className="size-4" /> Add secret
          </Button>
        }
      />
      <Card className="mb-4 flex items-center gap-3 border-emerald-500/30 bg-emerald-500/5 p-4 text-sm">
        <ShieldCheck className="size-5 shrink-0 text-emerald-600" />
        Encrypted with AES-256-GCM using this server's own key. Secret values are masked in every run log automatically.
      </Card>
      <Card className="overflow-hidden">
        <ul className="divide-y divide-line">
          {secrets.map((s) => (
            <li key={s.path} className="flex items-center gap-4 px-4 py-3">
              <span className="flex size-9 items-center justify-center rounded-lg bg-surface-2 text-ink-2">
                {s.kind.startsWith("SSH") ? <KeyRound className="size-4" /> : <Lock className="size-4" />}
              </span>
              <div className="min-w-0 flex-1">
                <div className="truncate font-mono text-[13px]">{s.path}</div>
                <div className="text-xs text-muted">{s.kind}</div>
              </div>
              <span className="hidden font-mono text-xs tracking-widest text-muted sm:block">••••••••••••</span>
              <span className="hidden w-28 text-right text-xs text-muted md:block">Used by {s.usedBy} runbooks</span>
              <span className="w-28 text-right text-xs text-muted">Updated {s.updated}</span>
            </li>
          ))}
        </ul>
      </Card>
    </>
  );
}
