import { useState, type FormEvent } from "react";
import { Plus, RefreshCw, Trash2, Webhook as WebhookIcon } from "lucide-react";
import { api, useData, type Runbook, type Webhook, type WebhookInput } from "../api";
import { runNumber, timeAgo } from "../format";
import { CopyButton } from "./CopyButton";
import { Button, Card, CardHeader, cx, ErrorNote, Modal } from "./ui";

const field = "h-11 w-full rounded-xl bg-surface-2 px-4 text-sm outline-none focus:ring-2 focus:ring-accent";

const hookUrl = (token: string) => `${window.location.origin}/api/v1/hooks/${token}`;

/** A JSON body with an example value at each mapped path, so the example call works. */
function exampleBody(inputs: Record<string, string>): unknown {
  let root: unknown = {};
  for (const [name, path] of Object.entries(inputs)) {
    const segs = path.split(".");
    const make = (seg: string) => (/^\d+$/.test(seg) ? [] : {});
    if (typeof root !== "object" || root === null) continue;
    let node = root as Record<string, unknown>;
    segs.forEach((seg, i) => {
      if (i === segs.length - 1) node[seg] = `example-${name}`;
      else {
        if (typeof node[seg] !== "object" || node[seg] === null) node[seg] = make(segs[i + 1]);
        node = node[seg] as Record<string, unknown>;
      }
    });
  }
  return root;
}

/** Shows a new URL (and signing secret) once, with an example call. */
function Secrets({ token, secret, inputs }: { token?: string; secret?: string | null; inputs: Record<string, string> }) {
  const sample = JSON.stringify(exampleBody(inputs));
  const example = token
    ? secret
      ? `body='${sample}'\nsig=$(printf %s "$body" | openssl dgst -sha256 -hmac '${secret}' | sed 's/.* //')\ncurl -X POST -H 'Content-Type: application/json' \\\n  -H "X-OpSkills-Signature: sha256=$sig" -d "$body" \\\n  ${hookUrl(token)}`
      : `curl -X POST -H 'Content-Type: application/json' -d '${sample}' \\\n  ${hookUrl(token)}`
    : null;
  return (
    <div className="space-y-4">
      <p className="text-sm text-muted">Copy these now. For your safety they are shown only this once.</p>
      {token && (
        <div>
          <p className="text-sm font-medium">URL</p>
          <div className="mt-1.5 flex items-center gap-2">
            <code className="scroll-thin min-w-0 flex-1 overflow-x-auto rounded-xl bg-surface-2 px-4 py-3 font-mono text-xs whitespace-nowrap">{hookUrl(token)}</code>
            <CopyButton text={hookUrl(token)} />
          </div>
        </div>
      )}
      {secret && (
        <div>
          <p className="text-sm font-medium">Signing secret</p>
          <div className="mt-1.5 flex items-center gap-2">
            <code className="scroll-thin min-w-0 flex-1 overflow-x-auto rounded-xl bg-surface-2 px-4 py-3 font-mono text-xs whitespace-nowrap">{secret}</code>
            <CopyButton text={secret} />
          </div>
          <p className="mt-1.5 text-xs text-muted">Calls must send <code className="font-mono">X-OpSkills-Signature: sha256=&lt;HMAC-SHA256 of the body&gt;</code>. GitHub's <code className="font-mono">X-Hub-Signature-256</code> works too: paste this as the webhook secret there.</p>
        </div>
      )}
      {example && (
        <div>
          <p className="text-sm font-medium">Try it</p>
          <div className="mt-1.5 flex items-start gap-2">
            <pre className="scroll-thin min-w-0 flex-1 overflow-x-auto rounded-xl bg-term px-4 py-3 font-mono text-xs text-term-ink">{example}</pre>
            <CopyButton text={example} />
          </div>
        </div>
      )}
    </div>
  );
}

function WebhookDialog({ rb, hook, onClose, onSaved }: { rb: Runbook; hook?: Webhook; onClose: () => void; onSaved: () => void }) {
  const [form, setForm] = useState<WebhookInput>({ name: hook?.name ?? "", enabled: hook?.enabled ?? true, inputs: hook?.inputs ?? {}, signed: hook?.signed ?? false });
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [shown, setShown] = useState<{ token?: string; secret?: string | null } | null>(null);

  async function submit(e: FormEvent) {
    e.preventDefault();
    setBusy(true);
    setError(null);
    const body = { ...form, inputs: Object.fromEntries(Object.entries(form.inputs).filter(([, v]) => v.trim() !== "").map(([k, v]) => [k, v.trim()])) };
    try {
      if (hook) {
        const r = await api.updateWebhook(hook.id, body);
        onSaved();
        if (r.signing_secret) setShown({ secret: r.signing_secret });
        else onClose();
      } else {
        const r = await api.createWebhook(rb.id, body);
        onSaved();
        setShown({ token: r.token, secret: r.signing_secret });
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  if (shown) {
    return (
      <Modal title={`Webhook "${form.name}"`} onClose={onClose} wide footer={<Button variant="primary" onClick={onClose}>Done</Button>}>
        <Secrets token={shown.token} secret={shown.secret} inputs={form.inputs} />
      </Modal>
    );
  }

  return (
    <Modal
      title={hook ? `Edit webhook "${hook.name}"` : "New webhook"}
      onClose={onClose}
      wide
      footer={
        <>
          <Button type="button" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" type="submit" form="webhook-form" disabled={busy}>
            {busy ? "Saving..." : hook ? "Save" : "Create webhook"}
          </Button>
        </>
      }
    >
      <form id="webhook-form" onSubmit={submit} className="space-y-5">
        {error && <ErrorNote>{error}</ErrorNote>}
        <div>
          <label className="text-sm font-medium" htmlFor="wh-name">
            Name
          </label>
          <input id="wh-name" required maxLength={60} value={form.name} onChange={(e) => setForm({ ...form, name: e.target.value })} placeholder="GitHub push" className={`mt-2 ${field}`} />
        </div>
        {rb.inputs.length > 0 && (
          <div>
            <p className="text-sm font-medium">Inputs from the request body</p>
            <p className="mt-1 text-xs text-muted">A dotted path into the JSON body, like repository.name or commits.0.id. Leave empty to use the input's default.</p>
            <div className="mt-3 grid grid-cols-1 gap-3 sm:grid-cols-2">
              {rb.inputs.map((d) => (
                <div key={d.name}>
                  <label className="text-xs font-medium text-muted" htmlFor={`wh-in-${d.name}`}>
                    {d.label || d.name}
                    {d.required && d.default === null && <span className="text-rose-600"> *</span>}
                  </label>
                  <input
                    id={`wh-in-${d.name}`}
                    value={form.inputs[d.name] ?? ""}
                    onChange={(e) => setForm({ ...form, inputs: { ...form.inputs, [d.name]: e.target.value } })}
                    placeholder="field.path"
                    spellCheck={false}
                    className={`mt-1.5 font-mono ${field}`}
                  />
                </div>
              ))}
            </div>
          </div>
        )}
        <label className="flex items-start gap-2 text-sm">
          <input type="checkbox" checked={form.signed} onChange={(e) => setForm({ ...form, signed: e.target.checked })} className="mt-0.5 size-4 accent-accent" />
          <span>
            Require a signature
            <span className="block text-xs text-muted">Calls must sign the body with a shared secret (HMAC-SHA256, as GitHub does). {hook && !hook.signed && "Turning it on shows a new secret once."}</span>
          </span>
        </label>
        {hook && (
          <label className="flex items-center gap-2 text-sm">
            <input type="checkbox" checked={form.enabled} onChange={(e) => setForm({ ...form, enabled: e.target.checked })} className="size-4 accent-accent" />
            On
          </label>
        )}
      </form>
    </Modal>
  );
}

/** A runbook's webhooks, with their recent deliveries. */
export function WebhooksCard({ rb }: { rb: Runbook }) {
  const hooks = useData(() => api.webhooks(rb.id), [rb.id], 15000);
  const [editing, setEditing] = useState<Webhook | "new" | null>(null);
  const [rotated, setRotated] = useState<{ name: string; token: string; inputs: Record<string, string> } | null>(null);
  const [error, setError] = useState<string | null>(null);

  async function act(f: () => Promise<unknown>) {
    setError(null);
    try {
      await f();
      hooks.reload();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }

  const list = hooks.data ?? [];
  return (
    <Card>
      <CardHeader
        title="Webhooks"
        action={
          <Button size="sm" onClick={() => setEditing("new")}>
            <Plus className="size-3.5" /> New
          </Button>
        }
      />
      <div className="space-y-4 px-6 pb-6 text-sm sm:px-8">
        {(error || hooks.error) && <ErrorNote>{error ?? hooks.error}</ErrorNote>}
        {list.length === 0 && <p className="text-xs text-muted">Let another system, like CI or GitHub, start this runbook with an HTTP request.</p>}
        {list.map((h) => {
          const last = h.deliveries[0];
          return (
            <div key={h.id}>
              <div className="flex items-center justify-between gap-2">
                <button type="button" onClick={() => setEditing(h)} className="flex min-w-0 items-center gap-2 font-medium hover:text-accent">
                  <WebhookIcon className="size-3.5 shrink-0 text-muted" />
                  <span className="truncate">{h.name}</span>
                </button>
                <span className="flex shrink-0 items-center gap-1">
                  <span className={cx("rounded-md px-1.5 py-0.5 text-[11px]", h.enabled ? "bg-emerald-500/10 text-emerald-700 dark:text-emerald-300" : "bg-surface-2 text-muted")}>{h.enabled ? "On" : "Off"}</span>
                  {h.signed && <span className="rounded-md bg-surface-2 px-1.5 py-0.5 text-[11px] text-muted">signed</span>}
                  <button
                    type="button"
                    title="New URL"
                    aria-label={`New URL for ${h.name}`}
                    onClick={() => {
                      if (window.confirm(`Give "${h.name}" a new URL? The current one stops working right away.`))
                        act(async () => setRotated({ name: h.name, token: (await api.rotateWebhook(h.id)).token, inputs: h.inputs }));
                    }}
                    className="rounded-lg p-1.5 text-muted hover:text-ink"
                  >
                    <RefreshCw className="size-3.5" />
                  </button>
                  <button
                    type="button"
                    aria-label={`Delete ${h.name}`}
                    onClick={() => window.confirm(`Delete "${h.name}"? Calls to it will fail.`) && act(() => api.deleteWebhook(h.id))}
                    className="rounded-lg p-1.5 text-muted hover:text-rose-600"
                  >
                    <Trash2 className="size-3.5" />
                  </button>
                </span>
              </div>
              <p className="mt-0.5 text-xs text-muted">
                {last ? (
                  <>
                    Last call {timeAgo(last.at)}:{" "}
                    {last.run_id ? (
                      <a href={`#/runs/${last.run_id}`} className="text-accent hover:underline">
                        run #{runNumber(last.run_id)}
                      </a>
                    ) : (
                      <span className="text-rose-600 dark:text-rose-400">
                        {last.status} {last.error}
                      </span>
                    )}
                  </>
                ) : (
                  "Not called yet"
                )}
              </p>
            </div>
          );
        })}
      </div>
      {editing && <WebhookDialog rb={rb} hook={editing === "new" ? undefined : editing} onClose={() => setEditing(null)} onSaved={() => hooks.reload()} />}
      {rotated && (
        <Modal title={`New URL for "${rotated.name}"`} onClose={() => setRotated(null)} wide footer={<Button variant="primary" onClick={() => setRotated(null)}>Done</Button>}>
          <Secrets token={rotated.token} inputs={rotated.inputs} />
        </Modal>
      )}
    </Card>
  );
}
