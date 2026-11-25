import { useState, type FormEvent } from "react";
import { Eye, EyeOff, FileKey, KeyRound, Lock, Pencil, Plus, RefreshCw, ShieldCheck, Trash2 } from "lucide-react";
import { api, useData, type Secret, type SecretKind } from "../api";
import { timeAgo } from "../format";
import { Badge, Button, Card, EmptyState, ErrorNote, Modal, PageHeader, SearchField, TableHeading, type BadgeTone } from "../components/ui";

const kinds: { id: SecretKind; label: string; tone: BadgeTone }[] = [
  { id: "password", label: "Password", tone: "blue" },
  { id: "token", label: "Token", tone: "amber" },
  { id: "ssh_key", label: "SSH key", tone: "green" },
  { id: "certificate", label: "Certificate", tone: "green" },
  { id: "other", label: "Other", tone: "gray" },
];
const kindOf = (k: SecretKind) => kinds.find((x) => x.id === k) ?? kinds[4];
const multiline = (k: SecretKind) => k === "ssh_key" || k === "certificate";

const field = "h-11 w-full rounded-xl bg-surface-2 px-4 text-sm outline-none focus:ring-2 focus:ring-accent";

function ValueInput({ kind, value, onChange, autoFocus }: { kind: SecretKind; value: string; onChange: (v: string) => void; autoFocus?: boolean }) {
  const [show, setShow] = useState(false);
  if (multiline(kind)) {
    return (
      <textarea
        id="secret-value"
        value={value}
        onChange={(e) => onChange(e.target.value)}
        rows={7}
        spellCheck={false}
        autoComplete="off"
        autoFocus={autoFocus}
        placeholder={kind === "ssh_key" ? "-----BEGIN OPENSSH PRIVATE KEY-----" : "-----BEGIN CERTIFICATE-----"}
        className="mt-2 w-full resize-y rounded-xl bg-term px-4 py-3 font-mono text-[12px] text-term-ink outline-none focus:ring-2 focus:ring-accent"
      />
    );
  }
  return (
    <div className="mt-2 flex h-11 items-center rounded-xl bg-surface-2 pr-2 focus-within:ring-2 focus-within:ring-accent">
      <input
        id="secret-value"
        type={show ? "text" : "password"}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        autoComplete="new-password"
        autoFocus={autoFocus}
        spellCheck={false}
        className="h-full min-w-0 flex-1 bg-transparent px-4 font-mono text-sm outline-none"
      />
      <button type="button" onClick={() => setShow((s) => !s)} className="rounded-lg p-1.5 text-muted hover:text-ink" aria-label={show ? "Hide value" : "Show value"}>
        {show ? <EyeOff className="size-4" /> : <Eye className="size-4" />}
      </button>
    </div>
  );
}

function AddSecret({ onClose, onSaved }: { onClose: () => void; onSaved: () => void }) {
  const [name, setName] = useState("");
  const [kind, setKind] = useState<SecretKind>("password");
  const [value, setValue] = useState("");
  const [description, setDescription] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function save(e: FormEvent) {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      await api.createSecret({ name, kind, value, description });
      onSaved();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Modal
      title="Add a secret"
      onClose={onClose}
      footer={
        <>
          <Button type="button" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" type="submit" form="add-secret" disabled={busy || !name || value.length < 4}>
            {busy ? "Saving..." : "Save secret"}
          </Button>
        </>
      }
    >
      <form id="add-secret" onSubmit={save} className="space-y-4">
        {error && <ErrorNote>{error}</ErrorNote>}
        <div>
          <label className="text-sm font-medium" htmlFor="secret-name">
            Name
          </label>
          <input
            id="secret-name"
            autoFocus
            value={name}
            onChange={(e) => setName(e.target.value.toUpperCase().replace(/[^A-Z0-9_]/g, "_"))}
            placeholder="DB_PASSWORD"
            className={`mt-2 font-mono ${field}`}
          />
          <p className="mt-1.5 text-xs text-muted">
            Steps read it as <code className="font-mono">${name || "DB_PASSWORD"}</code>. Capital letters, digits and underscores.
          </p>
        </div>
        <div>
          <label className="text-sm font-medium" htmlFor="secret-kind">
            Kind
          </label>
          <select id="secret-kind" value={kind} onChange={(e) => setKind(e.target.value as SecretKind)} className={`mt-2 ${field}`}>
            {kinds.map((k) => (
              <option key={k.id} value={k.id}>
                {k.label}
              </option>
            ))}
          </select>
        </div>
        <div>
          <label className="text-sm font-medium" htmlFor="secret-value">
            Value
          </label>
          <ValueInput kind={kind} value={value} onChange={setValue} />
          <p className="mt-1.5 text-xs text-muted">At least 4 characters. It can't be viewed again after saving.</p>
        </div>
        <div>
          <label className="text-sm font-medium" htmlFor="secret-desc">
            Description <span className="font-normal text-muted">(optional)</span>
          </label>
          <input id="secret-desc" value={description} onChange={(e) => setDescription(e.target.value)} placeholder="Read-only user for the reporting database" className={`mt-2 ${field}`} />
        </div>
      </form>
    </Modal>
  );
}

function EditSecret({ secret, replace, onClose, onSaved }: { secret: Secret; replace: boolean; onClose: () => void; onSaved: () => void }) {
  const [kind, setKind] = useState<SecretKind>(secret.kind);
  const [description, setDescription] = useState(secret.description);
  const [value, setValue] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function save(e: FormEvent) {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      await api.updateSecret(secret.name, replace ? { value } : { kind, description });
      onSaved();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Modal
      title={replace ? `Replace the value of ${secret.name}` : `Edit ${secret.name}`}
      onClose={onClose}
      footer={
        <>
          <Button type="button" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" type="submit" form="edit-secret" disabled={busy || (replace && value.length < 4)}>
            {busy ? "Saving..." : replace ? "Replace value" : "Save"}
          </Button>
        </>
      }
    >
      <form id="edit-secret" onSubmit={save} className="space-y-4">
        {error && <ErrorNote>{error}</ErrorNote>}
        {replace ? (
          <div>
            <label className="text-sm font-medium" htmlFor="secret-value">
              New value
            </label>
            <ValueInput kind={secret.kind} value={value} onChange={setValue} autoFocus />
            <p className="mt-1.5 text-xs text-muted">The old value is overwritten. Runs that start from now on use the new value.</p>
          </div>
        ) : (
          <>
            <div>
              <label className="text-sm font-medium" htmlFor="secret-kind">
                Kind
              </label>
              <select id="secret-kind" value={kind} onChange={(e) => setKind(e.target.value as SecretKind)} className={`mt-2 ${field}`}>
                {kinds.map((k) => (
                  <option key={k.id} value={k.id}>
                    {k.label}
                  </option>
                ))}
              </select>
            </div>
            <div>
              <label className="text-sm font-medium" htmlFor="secret-desc">
                Description
              </label>
              <input id="secret-desc" value={description} onChange={(e) => setDescription(e.target.value)} className={`mt-2 ${field}`} />
            </div>
          </>
        )}
      </form>
    </Modal>
  );
}

export function VaultPage() {
  const secrets = useData(() => api.secrets(), []);
  const [q, setQ] = useState("");
  const [adding, setAdding] = useState(false);
  const [editing, setEditing] = useState<{ secret: Secret; replace: boolean } | null>(null);
  const [error, setError] = useState<string | null>(null);

  const all = secrets.data ?? [];
  const list = all.filter((s) => `${s.name} ${s.description} ${s.kind}`.toLowerCase().includes(q.toLowerCase()));

  async function remove(s: Secret) {
    if (!window.confirm(`Delete the secret ${s.name}? This can't be undone.`)) return;
    setError(null);
    try {
      await api.deleteSecret(s.name);
      secrets.reload();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }

  const saved = () => {
    setAdding(false);
    setEditing(null);
    secrets.reload();
  };

  return (
    <div className="space-y-8">
      <PageHeader
        title="Vault"
        subtitle="Passwords, tokens and keys your runbooks need, encrypted on this server."
        actions={
          <Button variant="primary" onClick={() => setAdding(true)}>
            <Plus className="size-4" /> Add secret
          </Button>
        }
      />

      <Card className="flex items-start gap-4 px-6 py-5 text-sm sm:px-8">
        <span className="flex size-12 shrink-0 items-center justify-center rounded-full bg-emerald-500/15 text-emerald-600 dark:text-emerald-300">
          <ShieldCheck className="size-6" strokeWidth={1.75} />
        </span>
        <div className="text-ink-2">
          <p>
            Values are encrypted with <b>AES-256-GCM</b> using this server's own key and <b>can't be viewed again</b> after saving.
          </p>
          <p className="mt-1 text-muted">
            A runbook that lists a secret gets it as an environment variable in every step, for example <code className="font-mono text-xs">$DB_PASSWORD</code>. If a value shows up in a run's
            output, it's replaced with •••••• before anyone sees it.
          </p>
        </div>
      </Card>

      {error && <ErrorNote>{error}</ErrorNote>}
      {secrets.error && <ErrorNote>{secrets.error}</ErrorNote>}

      {secrets.data && all.length === 0 ? (
        <Card>
          <EmptyState icon={KeyRound} title="No secrets yet" action={<Button variant="primary" onClick={() => setAdding(true)}><Plus className="size-4" /> Add your first secret</Button>}>
            Store a password, API token or SSH key here, then pick it in a runbook instead of writing it into a command.
          </EmptyState>
        </Card>
      ) : (
        <Card className="overflow-hidden">
          <TableHeading title="All secrets" subtitle={`${all.length} secret${all.length === 1 ? "" : "s"} in the vault`}>
            <SearchField value={q} onChange={setQ} />
          </TableHeading>
          <div className="hidden grid-cols-[minmax(0,1.3fr)_120px_minmax(0,1fr)_120px_140px] gap-6 border-y border-line px-10 py-4 text-sm font-medium text-muted lg:grid">
            <span>Name</span>
            <span>Kind</span>
            <span>Used by</span>
            <span>Updated</span>
            <span />
          </div>
          <ul className="px-6 pb-2 sm:px-10">
            {list.length === 0 && <li className="py-10 text-center text-sm text-muted">{secrets.loading ? "Loading..." : "No secrets match."}</li>}
            {list.map((s) => (
              <li key={s.name} className="grid grid-cols-[minmax(0,1fr)_auto] items-center gap-x-6 gap-y-2 border-b border-line py-4 text-sm last:border-0 lg:grid-cols-[minmax(0,1.3fr)_120px_minmax(0,1fr)_120px_140px]">
                <span className="flex min-w-0 items-center gap-3">
                  <span className="flex size-9 shrink-0 items-center justify-center rounded-full bg-accent-soft text-accent">
                    {s.kind === "ssh_key" || s.kind === "certificate" ? <FileKey className="size-4" /> : <Lock className="size-4" />}
                  </span>
                  <span className="min-w-0">
                    <span className="block truncate font-mono text-[13px] font-medium">{s.name}</span>
                    <span className="block truncate text-xs text-muted">{s.description || "No description"}</span>
                  </span>
                </span>
                <span className="hidden lg:block">
                  <Badge tone={kindOf(s.kind).tone} className="min-w-0">
                    {kindOf(s.kind).label}
                  </Badge>
                </span>
                <span className="hidden min-w-0 truncate lg:block">
                  {s.used_by.length === 0 ? (
                    <span className="text-muted">Not used yet</span>
                  ) : (
                    s.used_by.map((r, i) => (
                      <span key={r.id}>
                        {i > 0 && ", "}
                        <a href={`#/runbooks/${r.id}`} className="text-accent hover:underline">
                          {r.name}
                        </a>
                      </span>
                    ))
                  )}
                </span>
                <span className="hidden text-muted lg:block">{timeAgo(s.updated_at)}</span>
                <span className="flex justify-end gap-1">
                  <Button size="sm" variant="ghost" onClick={() => setEditing({ secret: s, replace: true })} aria-label={`Replace the value of ${s.name}`} title="Replace value">
                    <RefreshCw className="size-3.5" />
                  </Button>
                  <Button size="sm" variant="ghost" onClick={() => setEditing({ secret: s, replace: false })} aria-label={`Edit ${s.name}`} title="Edit kind and description">
                    <Pencil className="size-3.5" />
                  </Button>
                  <Button size="sm" variant="ghost" onClick={() => remove(s)} aria-label={`Delete ${s.name}`} title="Delete" className="hover:text-rose-600">
                    <Trash2 className="size-3.5" />
                  </Button>
                </span>
              </li>
            ))}
          </ul>
        </Card>
      )}

      {adding && <AddSecret onClose={() => setAdding(false)} onSaved={saved} />}
      {editing && <EditSecret secret={editing.secret} replace={editing.replace} onClose={() => setEditing(null)} onSaved={saved} />}
    </div>
  );
}
