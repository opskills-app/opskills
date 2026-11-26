import { useState, type FormEvent } from "react";
import { KeyRound, Plus, Trash2 } from "lucide-react";
import { CopyButton } from "../components/CopyButton";
import { api, useData, type ApiToken } from "../api";
import { timeAgo } from "../format";
import { Avatar, Button, Card, CardHeader, ErrorNote, Modal, PageHeader, SampleBanner } from "../components/ui";

const field = "h-11 w-full rounded-xl bg-surface-2 px-4 text-sm outline-none focus:ring-2 focus:ring-accent";

/** "in 3 days", "today", "expired". */
function expiresText(ms: number | null) {
  if (ms === null) return "Never";
  const days = Math.ceil((ms - Date.now()) / 86_400_000);
  if (days <= 0) return "Expired";
  return days === 1 ? "Tomorrow" : `In ${days} days`;
}

function NewTokenDialog({ onClose, onCreated }: { onClose: () => void; onCreated: () => void }) {
  const [name, setName] = useState("");
  const [days, setDays] = useState("90");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [created, setCreated] = useState<{ token: ApiToken; value: string } | null>(null);

  async function submit(e: FormEvent) {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      setCreated(await api.createToken(name, days === "never" ? null : Number(days)));
      onCreated();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  if (created) {
    const example = `curl -H "Authorization: Bearer ${created.value}" ${window.location.origin}/api/v1/runbooks`;
    return (
      <Modal title={`Token "${created.token.name}" created`} onClose={onClose} wide footer={<Button variant="primary" onClick={onClose}>Done</Button>}>
        <p className="text-sm text-muted">Copy it now. For your safety it is shown only this once; if you lose it, revoke it and create another.</p>
        <div className="mt-4 flex items-center gap-2">
          <code className="scroll-thin min-w-0 flex-1 overflow-x-auto rounded-xl bg-surface-2 px-4 py-3 font-mono text-xs whitespace-nowrap">{created.value}</code>
          <CopyButton text={created.value} />
        </div>
        <p className="mt-5 text-sm font-medium">Use it</p>
        <p className="mt-1 text-xs text-muted">
          Send it as a Bearer token. It can do anything in the API except manage tokens and sign-ins, and runs it starts show its name.
        </p>
        <div className="mt-2 flex items-start gap-2">
          <pre className="scroll-thin min-w-0 flex-1 overflow-x-auto rounded-xl bg-term px-4 py-3 font-mono text-xs text-term-ink">{example}</pre>
          <CopyButton text={example} />
        </div>
      </Modal>
    );
  }

  return (
    <Modal
      title="New API token"
      onClose={onClose}
      footer={
        <>
          <Button type="button" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" type="submit" form="new-token" disabled={busy}>
            <KeyRound className="size-3.5" /> {busy ? "Creating..." : "Create token"}
          </Button>
        </>
      }
    >
      <form id="new-token" onSubmit={submit} className="space-y-5">
        {error && <ErrorNote>{error}</ErrorNote>}
        <div>
          <label className="text-sm font-medium" htmlFor="token-name">
            Name
          </label>
          <input id="token-name" required maxLength={60} value={name} onChange={(e) => setName(e.target.value)} placeholder="CI deploy" className={`mt-2 ${field}`} />
          <p className="mt-1.5 text-xs text-muted">What uses it, so you know what breaks if you revoke it.</p>
        </div>
        <div>
          <label className="text-sm font-medium" htmlFor="token-expiry">
            Expires
          </label>
          <select id="token-expiry" value={days} onChange={(e) => setDays(e.target.value)} className={`mt-2 ${field}`}>
            <option value="30">In 30 days</option>
            <option value="90">In 90 days</option>
            <option value="365">In a year</option>
            <option value="never">Never</option>
          </select>
        </div>
      </form>
    </Modal>
  );
}

/** Tokens for scripts, CI and other systems to call the API. */
function ApiTokensCard() {
  const tokens = useData(() => api.tokens(), []);
  const [creating, setCreating] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function revoke(t: ApiToken) {
    if (!window.confirm(`Revoke "${t.name}"? Anything using it stops working right away.`)) return;
    setError(null);
    try {
      await api.revokeToken(t.id);
      tokens.reload();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }

  const list = tokens.data ?? [];
  return (
    <Card className="mb-4">
      <CardHeader
        title="API tokens"
        hint="For scripts, CI and other systems that call the OpSkills API"
        action={
          <Button size="sm" variant="primary" onClick={() => setCreating(true)}>
            <Plus className="size-3.5" /> New token
          </Button>
        }
      />
      {(error || tokens.error) && (
        <div className="px-6 pb-4 sm:px-8">
          <ErrorNote>{error ?? tokens.error}</ErrorNote>
        </div>
      )}
      {list.length === 0 ? (
        <p className="px-6 pb-6 text-sm text-muted sm:px-8">No tokens yet. Create one to call the API without signing in.</p>
      ) : (
        <div className="scroll-thin overflow-x-auto">
          <table className="w-full min-w-[560px] text-sm">
            <thead>
              <tr className="border-y border-line bg-surface-2 text-left text-xs text-muted">
                <th className="px-6 py-3 font-medium sm:px-8">Name</th>
                <th className="px-3 py-3 font-medium">Created</th>
                <th className="px-3 py-3 font-medium">Expires</th>
                <th className="px-3 py-3 font-medium">Last used</th>
                <th className="px-6 py-3 sm:px-8" />
              </tr>
            </thead>
            <tbody className="divide-y divide-line">
              {list.map((t) => (
                <tr key={t.id}>
                  <td className="px-6 py-3 font-medium sm:px-8">
                    <span className="flex items-center gap-2">
                      <KeyRound className="size-3.5 text-muted" /> {t.name}
                    </span>
                  </td>
                  <td className="px-3 py-3 text-muted">{timeAgo(t.created_at)}</td>
                  <td className={`px-3 py-3 ${t.expires_at !== null && t.expires_at <= Date.now() ? "text-rose-600 dark:text-rose-400" : "text-muted"}`}>{expiresText(t.expires_at)}</td>
                  <td className="px-3 py-3 text-muted">{t.last_used_at ? timeAgo(t.last_used_at) : "Never"}</td>
                  <td className="px-6 py-3 text-right sm:px-8">
                    <button type="button" onClick={() => revoke(t)} className="inline-flex items-center gap-1 rounded-lg px-2 py-1 text-xs text-muted hover:text-rose-600" aria-label={`Revoke ${t.name}`}>
                      <Trash2 className="size-3.5" /> Revoke
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      {creating && <NewTokenDialog onClose={() => setCreating(false)} onCreated={() => tokens.reload()} />}
    </Card>
  );
}

const members = [
  { name: "Maya Chen", role: "Admin" },
  { name: "Jonas Weber", role: "Operator" },
  { name: "Priya Natarajan", role: "Operator" },
  { name: "Leo Martins", role: "Viewer" },
];

export function SettingsPage() {
  return (
    <>
      <PageHeader title="Settings" subtitle="API tokens, workspace members and access." />
      <ApiTokensCard />
      <SampleBanner>Members and roles below are a preview of a later release. This version has one built-in admin user.</SampleBanner>
      <div className="grid grid-cols-1 gap-4 xl:grid-cols-2">
        <Card>
          <CardHeader title="Members" action={<Button size="sm">Invite</Button>} />
          <ul className="divide-y divide-line">
            {members.map((m) => (
              <li key={m.name} className="flex items-center gap-3 px-6 py-3 sm:px-8">
                <Avatar name={m.name} size="size-7" />
                <span className="flex-1 text-sm font-medium">{m.name}</span>
                <select defaultValue={m.role} className="h-7 rounded-md border border-line bg-surface px-2 text-xs">
                  <option>Admin</option>
                  <option>Operator</option>
                  <option>Viewer</option>
                </select>
              </li>
            ))}
          </ul>
        </Card>
        <Card>
          <CardHeader title="Roles" hint="What each role can do in this workspace" />
          <table className="w-full text-sm">
            <thead>
              <tr className="border-y border-line bg-surface-2 text-left text-xs text-muted">
                <th className="px-6 py-3 font-medium sm:px-8">Permission</th>
                <th className="px-2 py-2 text-center font-medium">Admin</th>
                <th className="px-2 py-2 text-center font-medium">Operator</th>
                <th className="px-2 py-2 text-center font-medium">Viewer</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-line">
              {[
                ["View runbooks and runs", 1, 1, 1],
                ["Start and cancel runs", 1, 1, 0],
                ["Approve gated steps", 1, 1, 0],
                ["Edit runbooks", 1, 0, 0],
                ["Manage vault and members", 1, 0, 0],
              ].map(([label, ...cells]) => (
                <tr key={label as string}>
                  <td className="px-6 py-3 sm:px-8">{label}</td>
                  {cells.map((c, i) => (
                    <td key={i} className="px-2 py-2 text-center">
                      {c ? <span className="text-accent">●</span> : <span className="text-line-strong">○</span>}
                    </td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        </Card>
      </div>
    </>
  );
}
