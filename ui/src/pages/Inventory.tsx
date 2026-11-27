import { useState, type FormEvent } from "react";
import { Database, Globe, Pencil, Plus, RefreshCw, Tags, TerminalSquare, Trash2, TriangleAlert, X } from "lucide-react";
import { api, useData, type InventorySource, type SourceHost, type SourceInput, type TagRule } from "../api";
import { timeAgo } from "../format";
import { Badge, Button, Card, CardHeader, cx, ErrorNote, Modal, Tag } from "../components/ui";
import { useIsServerAdmin } from "../session";

const field = "h-11 w-full rounded-xl bg-surface-2 px-4 text-sm outline-none focus:ring-2 focus:ring-accent";
const area = "w-full rounded-xl bg-surface-2 px-4 py-3 font-mono text-xs outline-none focus:ring-2 focus:ring-accent";
const label = "text-sm font-medium";
const hint = "mt-1.5 text-xs text-muted";

type Kind = "static" | "url" | "program";
const kinds: { id: Kind; label: string; icon: typeof Globe; detail: string }[] = [
  { id: "static", label: "List", icon: Database, detail: "Hosts you write here." },
  { id: "url", label: "URL", icon: Globe, detail: "A JSON document OpSkills fetches on an interval." },
  { id: "program", label: "Program", icon: TerminalSquare, detail: "A program on the server that prints hosts. Server admins only." },
];

const errText = (e: unknown) => (e instanceof Error ? e.message : String(e));

/** `KEY=value` lines to an object, and back. */
function parsePairs(text: string): Record<string, string> {
  const out: Record<string, string> = {};
  for (const line of text.split("\n")) {
    const t = line.trim();
    if (!t) continue;
    const i = t.indexOf("=");
    if (i < 1) throw new Error(`"${t}" is not KEY=value.`);
    out[t.slice(0, i).trim()] = t.slice(i + 1).trim();
  }
  return out;
}
const pairsText = (o?: Record<string, string>) =>
  Object.entries(o ?? {})
    .map(([k, v]) => `${k}=${v}`)
    .join("\n");
const list = (text: string) =>
  text
    .split(",")
    .map((t) => t.trim())
    .filter(Boolean);

const exampleHosts = JSON.stringify(
  {
    hosts: [
      { name: "web-1", hostname: "web-1.internal", tags: ["web"], attributes: { env: "prod", region: "eu-west-1" } },
      { name: "db-1", hostname: "10.0.0.12", tags: ["db"], attributes: { env: "prod" } },
    ],
  },
  null,
  2,
);

function SourceEditor({ source, onClose, onSaved }: { source: InventorySource | null; onClose: () => void; onSaved: () => void }) {
  const serverAdmin = useIsServerAdmin();
  const [kind, setKind] = useState<Kind>(source?.type ?? "static");
  const [name, setName] = useState(source?.name ?? "");
  const [interval, setInterval] = useState(String(source?.interval_seconds ?? 300));
  const [enabled, setEnabled] = useState(source?.enabled ?? true);
  const [hosts, setHosts] = useState(source?.type === "static" ? JSON.stringify({ hosts: source.hosts?.map(({ name, hostname, tags, attributes }) => ({ name, hostname, tags, attributes })) ?? [] }, null, 2) : exampleHosts);
  const [url, setUrl] = useState(source?.type === "url" ? source.url : "");
  const [header, setHeader] = useState(source?.type === "url" ? (source.header ?? "") : "");
  const [secret, setSecret] = useState(source?.type === "url" ? (source.secret ?? "") : "");
  const [command, setCommand] = useState(source?.type === "program" ? source.command : "");
  const [args, setArgs] = useState(source?.type === "program" ? (source.args ?? []).join("\n") : "");
  const [config, setConfig] = useState(source?.type === "program" ? pairsText(source.config) : "");
  const [secrets, setSecrets] = useState(source?.type === "program" ? (source.secrets ?? []).join(", ") : "");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const locked = source?.type === "program" && !serverAdmin;

  function input(): SourceInput {
    const common = { name, interval_seconds: Number(interval) || 300, enabled };
    if (kind === "static") {
      let doc: unknown;
      try {
        doc = JSON.parse(hosts);
      } catch (e) {
        throw new Error(`The host list isn't valid JSON: ${errText(e)}`);
      }
      const list = Array.isArray(doc) ? doc : (doc as { hosts?: unknown })?.hosts;
      if (!Array.isArray(list)) throw new Error('Write {"hosts": [...]}, a list of hosts.');
      return { ...common, type: "static", hosts: list as SourceHost[] };
    }
    if (kind === "url") return { ...common, type: "url", url: url.trim(), header: header.trim() || null, secret: secret.trim() || null };
    return {
      ...common,
      type: "program",
      command: command.trim(),
      args: args
        .split("\n")
        .map((a) => a.trim())
        .filter(Boolean),
      config: parsePairs(config),
      secrets: list(secrets),
    };
  }

  async function save(e: FormEvent) {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      const body = input();
      const saved = source ? await api.updateInventorySource(source.id, body) : await api.createInventorySource(body);
      if (saved.state.error) {
        setError(`Saved, but the refresh failed: ${saved.state.error}`);
        onSaved();
        return;
      }
      onSaved();
      onClose();
    } catch (err) {
      setError(errText(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Modal
      wide
      title={source ? `Edit ${source.name}` : "Add an inventory source"}
      onClose={onClose}
      footer={
        <>
          <Button type="button" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" type="submit" form="source-editor" disabled={busy || locked || !name.trim()}>
            {busy ? "Saving..." : source ? "Save and refresh" : "Add and refresh"}
          </Button>
        </>
      }
    >
      <form id="source-editor" onSubmit={save} className="space-y-5">
        {error && <ErrorNote>{error}</ErrorNote>}
        {locked && <ErrorNote>A program source runs code on the server, so only server admins can change it.</ErrorNote>}
        <div role="radiogroup" aria-label="Source type" className="grid grid-cols-3 gap-1 rounded-xl bg-surface-2 p-1">
          {kinds.map((k) => {
            const off = k.id === "program" && !serverAdmin;
            return (
              <button
                key={k.id}
                type="button"
                role="radio"
                aria-checked={kind === k.id}
                disabled={off || !!source}
                title={off ? "Only server admins can add a program source." : source ? "The type can't change; add a new source instead." : k.detail}
                onClick={() => setKind(k.id)}
                className={cx("flex h-9 items-center justify-center gap-1.5 rounded-lg text-xs font-medium disabled:opacity-50", kind === k.id ? "bg-surface text-accent shadow-sm" : "text-muted hover:text-ink")}
              >
                <k.icon className="size-3.5" /> {k.label}
              </button>
            );
          })}
        </div>
        <p className="-mt-3 text-xs text-muted">{kinds.find((k) => k.id === kind)?.detail}</p>
        <div className="grid gap-4 sm:grid-cols-[minmax(0,1fr)_160px]">
          <div>
            <label className={label} htmlFor="src-name">
              Name
            </label>
            <input id="src-name" autoFocus value={name} onChange={(e) => setName(e.target.value)} placeholder="CMDB" className={`mt-2 ${field}`} />
          </div>
          <div>
            <label className={label} htmlFor="src-interval">
              Refresh (seconds)
            </label>
            <input id="src-interval" type="number" min={30} max={86400} value={interval} onChange={(e) => setInterval(e.target.value)} className={`mt-2 ${field}`} />
          </div>
        </div>
        {kind === "static" && (
          <div>
            <label className={label} htmlFor="src-hosts">
              Hosts
            </label>
            <textarea id="src-hosts" rows={12} value={hosts} onChange={(e) => setHosts(e.target.value)} spellCheck={false} className={`mt-2 ${area}`} />
            <p className={hint}>Each host has a name, and optionally a hostname, tags and attributes. A host whose hostname matches an enrolled host adds its tags and attributes to that host.</p>
          </div>
        )}
        {kind === "url" && (
          <>
            <div>
              <label className={label} htmlFor="src-url">
                URL
              </label>
              <input id="src-url" value={url} onChange={(e) => setUrl(e.target.value)} placeholder="https://cmdb.example/opskills/hosts.json" className={`mt-2 ${field}`} />
              <p className={hint}>It must return {'{"hosts": [...]}'} in the same format as a list. Fetched with a 20 second limit, up to 4 MB.</p>
            </div>
            <div className="grid gap-4 sm:grid-cols-2">
              <div>
                <label className={label} htmlFor="src-header">
                  Header (optional)
                </label>
                <input id="src-header" value={header} onChange={(e) => setHeader(e.target.value)} placeholder="Authorization" className={`mt-2 ${field}`} />
              </div>
              <div>
                <label className={label} htmlFor="src-secret">
                  Its value from the vault
                </label>
                <input id="src-secret" value={secret} onChange={(e) => setSecret(e.target.value)} placeholder="inventory/CMDB_TOKEN" className={`mt-2 ${field}`} />
              </div>
            </div>
          </>
        )}
        {kind === "program" && (
          <>
            <div>
              <label className={label} htmlFor="src-command">
                Program
              </label>
              <input id="src-command" value={command} onChange={(e) => setCommand(e.target.value)} placeholder="/opt/opskills/plugins/list-hosts" disabled={locked} className={`mt-2 ${field} font-mono`} />
              <p className={hint}>An absolute path on the server. It must print {'{"hosts": [...]}'} and exit within 60 seconds.</p>
            </div>
            <div className="grid gap-4 sm:grid-cols-2">
              <div>
                <label className={label} htmlFor="src-args">
                  Arguments, one per line
                </label>
                <textarea id="src-args" rows={3} value={args} onChange={(e) => setArgs(e.target.value)} disabled={locked} spellCheck={false} className={`mt-2 ${area}`} />
              </div>
              <div>
                <label className={label} htmlFor="src-config">
                  Settings, KEY=value per line
                </label>
                <textarea id="src-config" rows={3} value={config} onChange={(e) => setConfig(e.target.value)} disabled={locked} placeholder="REGION=eu-west-1" spellCheck={false} className={`mt-2 ${area}`} />
                <p className={hint}>Each becomes $OPSKILLS_CONFIG_KEY.</p>
              </div>
            </div>
            <div>
              <label className={label} htmlFor="src-secrets">
                Vault secrets
              </label>
              <input id="src-secrets" value={secrets} onChange={(e) => setSecrets(e.target.value)} disabled={locked} placeholder="cloud/API_TOKEN" className={`mt-2 ${field}`} />
              <p className={hint}>Separate with commas. Each is set as a variable named after its last part, like $API_TOKEN, and hidden in errors.</p>
            </div>
          </>
        )}
        <label className="flex items-center gap-2 text-sm">
          <input type="checkbox" checked={enabled} onChange={(e) => setEnabled(e.target.checked)} className="size-4 accent-[var(--color-accent)]" />
          On: refresh it and list its hosts
        </label>
      </form>
    </Modal>
  );
}

/** The workspace's inventory sources, for its Admins. */
export function InventorySources({ onChange }: { onChange: () => void }) {
  const sources = useData(() => api.inventorySources(), [], 15000);
  const serverAdmin = useIsServerAdmin();
  const [editing, setEditing] = useState<InventorySource | "new" | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const reload = () => {
    sources.reload();
    onChange();
  };

  async function refresh(s: InventorySource, acceptEmpty = false) {
    setBusy(s.id);
    setError(null);
    try {
      await api.refreshInventorySource(s.id, acceptEmpty);
      reload();
    } catch (e) {
      setError(errText(e));
    } finally {
      setBusy(null);
    }
  }

  async function remove(s: InventorySource) {
    if (!window.confirm(`Remove the source ${s.name}? Hosts only it lists leave this workspace, and enrolled hosts lose what it added.`)) return;
    setError(null);
    try {
      await api.deleteInventorySource(s.id);
      reload();
    } catch (e) {
      setError(errText(e));
    }
  }

  const all = sources.data ?? [];
  return (
    <Card className="overflow-hidden">
      <CardHeader
        title="Inventory sources"
        hint="Hosts and attributes from a list, a URL or a program. Hosts that match an enrolled host by hostname add to it; others are listed until the agent is installed."
        action={
          <Button onClick={() => setEditing("new")}>
            <Plus className="size-4" /> Add source
          </Button>
        }
      />
      <div className="px-6 pb-6 sm:px-8">
        {error && (
          <div className="mb-3">
            <ErrorNote>{error}</ErrorNote>
          </div>
        )}
        {sources.error && <ErrorNote>{sources.error}</ErrorNote>}
        {all.length === 0 ? (
          <p className="rounded-2xl border border-dashed border-line-strong px-4 py-6 text-center text-sm text-muted">{sources.data ? "No sources yet. Hosts come only from agents that enroll." : "Loading..."}</p>
        ) : (
          <ul className="divide-y divide-line">
            {all.map((s) => {
              const k = kinds.find((x) => x.id === s.type)!;
              const emptyGuard = s.state.error?.includes("listed no hosts after listing");
              const canChange = s.type !== "program" || serverAdmin;
              return (
                <li key={s.id} className="grid grid-cols-[auto_minmax(0,1fr)] items-start gap-x-4 gap-y-2 py-4 text-sm sm:grid-cols-[auto_minmax(0,1fr)_auto]">
                  <span className="flex size-9 shrink-0 items-center justify-center rounded-full bg-accent-soft text-accent">
                    <k.icon className="size-4" />
                  </span>
                  <span className="min-w-0">
                    <span className="flex flex-wrap items-center gap-2">
                      <span className="font-medium">{s.name}</span>
                      <span className="text-xs text-muted">
                        {k.label}
                        {s.type === "url" && ` · ${s.url.replace(/^https?:\/\//, "").split(/[/?]/)[0]}`}
                        {s.type === "program" && ` · ${s.command}`}
                      </span>
                    </span>
                    <span className="mt-0.5 block text-xs text-muted">
                      {!s.enabled
                        ? "Off"
                        : `${s.state.host_count} host${s.state.host_count === 1 ? "" : "s"} · ${s.state.last_success ? `refreshed ${timeAgo(s.state.last_success)}` : "not refreshed yet"} · every ${s.interval_seconds >= 60 && s.interval_seconds % 60 === 0 ? `${s.interval_seconds / 60} min` : `${s.interval_seconds} s`}`}
                    </span>
                    {s.state.error && (
                      <span className="mt-2 flex items-start gap-1.5 text-xs text-rose-700 dark:text-rose-300">
                        <TriangleAlert className="mt-0.5 size-3.5 shrink-0" />
                        <span>
                          {s.state.last_attempt ? `${timeAgo(s.state.last_attempt)}: ` : ""}
                          {s.state.error}
                          {emptyGuard && (
                            <>
                              {" "}
                              <button type="button" onClick={() => refresh(s, true)} className="font-medium underline">
                                Accept the empty list
                              </button>
                            </>
                          )}
                        </span>
                      </span>
                    )}
                  </span>
                  <span className="col-span-2 flex items-center justify-end gap-1 sm:col-span-1">
                    {s.enabled ? <Badge tone={s.state.error ? "red" : "green"}>{s.state.error ? "Failing" : "OK"}</Badge> : <Badge tone="gray">Off</Badge>}
                    <Button size="sm" variant="ghost" disabled={busy === s.id || !s.enabled} onClick={() => refresh(s)} aria-label={`Refresh ${s.name}`} title="Refresh now">
                      <RefreshCw className={cx("size-3.5", busy === s.id && "animate-spin")} />
                    </Button>
                    {canChange && (
                      <>
                        <Button size="sm" variant="ghost" onClick={() => api.inventorySource(s.id).then(setEditing, (e) => setError(errText(e)))} aria-label={`Edit ${s.name}`} title="Edit">
                          <Pencil className="size-3.5" />
                        </Button>
                        <Button size="sm" variant="ghost" onClick={() => remove(s)} aria-label={`Remove ${s.name}`} title="Remove" className="hover:text-rose-600">
                          <Trash2 className="size-3.5" />
                        </Button>
                      </>
                    )}
                  </span>
                </li>
              );
            })}
          </ul>
        )}
      </div>
      {editing && <SourceEditor source={editing === "new" ? null : editing} onClose={() => setEditing(null)} onSaved={reload} />}
    </Card>
  );
}

/** Tags for hosts that match a filter, for the workspace's Admins. */
export function TagRules({ slug, onChange }: { slug: string; onChange: () => void }) {
  const ws = useData(() => api.workspace(slug), [slug]);
  const [rules, setRules] = useState<{ filter: string; tags: string }[] | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const saved: TagRule[] = ws.data?.workspace.tag_rules ?? [];
  const editing = rules !== null;

  async function save() {
    if (!rules) return;
    setBusy(true);
    setError(null);
    try {
      await api.setTagRules(
        slug,
        rules.filter((r) => r.filter.trim() || r.tags.trim()).map((r) => ({ filter: r.filter.trim(), tags: list(r.tags) })),
      );
      setRules(null);
      ws.reload();
      onChange();
    } catch (e) {
      setError(errText(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Card className="overflow-hidden">
      <CardHeader
        title="Tag rules"
        hint="Hosts that match a filter get its tags, so runbooks can target them by tag. Rules see tags and attributes from agents, Admins and sources."
        action={
          editing ? (
            <span className="flex gap-2">
              <Button onClick={() => setRules(null)}>Cancel</Button>
              <Button variant="primary" disabled={busy} onClick={save}>
                {busy ? "Saving..." : "Save rules"}
              </Button>
            </span>
          ) : (
            <Button onClick={() => setRules(saved.length ? saved.map((r) => ({ filter: r.filter, tags: r.tags.join(", ") })) : [{ filter: "", tags: "" }])}>
              <Tags className="size-4" /> Edit rules
            </Button>
          )
        }
      />
      <div className="px-6 pb-6 sm:px-8">
        {error && (
          <div className="mb-3">
            <ErrorNote>{error}</ErrorNote>
          </div>
        )}
        {editing ? (
          <div className="space-y-3">
            {rules.map((r, i) => (
              <div key={i} className="grid grid-cols-[minmax(0,1fr)_auto] gap-2 sm:grid-cols-[minmax(0,1.4fr)_minmax(0,1fr)_auto]">
                <input
                  aria-label={`Rule ${i + 1} filter`}
                  value={r.filter}
                  onChange={(e) => setRules(rules.map((x, j) => (j === i ? { ...x, filter: e.target.value } : x)))}
                  placeholder="os:debian* env:prod"
                  spellCheck={false}
                  className="col-span-2 h-10 rounded-xl bg-surface-2 px-3 font-mono text-xs outline-none focus:ring-2 focus:ring-accent sm:col-span-1"
                />
                <input
                  aria-label={`Rule ${i + 1} tags`}
                  value={r.tags}
                  onChange={(e) => setRules(rules.map((x, j) => (j === i ? { ...x, tags: e.target.value } : x)))}
                  placeholder="debian, prod"
                  className="h-10 rounded-xl bg-surface-2 px-3 text-sm outline-none focus:ring-2 focus:ring-accent"
                />
                <Button size="sm" variant="ghost" type="button" onClick={() => setRules(rules.filter((_, j) => j !== i))} aria-label={`Remove rule ${i + 1}`}>
                  <X className="size-3.5" />
                </Button>
              </div>
            ))}
            <Button size="sm" type="button" onClick={() => setRules([...rules, { filter: "", tags: "" }])}>
              <Plus className="size-3.5" /> Add rule
            </Button>
            <p className="text-xs text-muted">Filters use the same keys as runbook targets: tags, name, hostname, os, arch, or an attribute like env.</p>
          </div>
        ) : saved.length === 0 ? (
          <p className="rounded-2xl border border-dashed border-line-strong px-4 py-6 text-center text-sm text-muted">{ws.data ? "No rules yet." : "Loading..."}</p>
        ) : (
          <ul className="divide-y divide-line">
            {saved.map((r, i) => (
              <li key={i} className="flex flex-wrap items-center gap-x-3 gap-y-1.5 py-3 text-sm">
                <code className="rounded-lg bg-surface-2 px-2 py-1 font-mono text-xs">{r.filter}</code>
                <span className="text-xs text-muted">gets</span>
                {r.tags.map((t) => (
                  <Tag key={t}>{t}</Tag>
                ))}
              </li>
            ))}
          </ul>
        )}
      </div>
    </Card>
  );
}
