import { useState } from "react";
import { Bell, Check, Mail, Plus, Send, Trash2, Webhook as WebhookIcon, X } from "lucide-react";
import { api, useData, type Notification, type NotifyEvent, type WebhookAuth } from "../api";
import { timeAgo } from "../format";
import { Card, CardHeader, cx } from "./ui";

const field = "h-11 w-full rounded-xl bg-surface-2 px-4 text-sm outline-none focus:ring-2 focus:ring-accent";

const events: { value: NotifyEvent; label: string }[] = [
  { value: "started", label: "Started" },
  { value: "succeeded", label: "Succeeded" },
  { value: "failed", label: "Failed" },
  { value: "slow", label: "Running long" },
];

export const eventLabel = (e: NotifyEvent) => events.find((x) => x.value === e)?.label ?? e;

export function newNotification(): Notification {
  return { id: "", on: ["failed"], webhook: "", enabled: true };
}

/** Only what the API needs: one target kind, and slow_after_seconds only for "slow". */
export function cleanNotification(n: Notification): Notification {
  const base = { id: n.id, on: n.on, enabled: n.enabled, slow_after_seconds: n.on.includes("slow") ? n.slow_after_seconds : undefined };
  if (n.webhook === undefined) return { ...base, email: (n.email ?? []).map((e) => e.trim()).filter(Boolean) };
  const headers = (n.headers ?? []).filter((h) => h.name.trim() || h.value.trim()).map((h) => ({ name: h.name.trim(), value: h.value }));
  return { ...base, webhook: n.webhook.trim(), headers: headers.length ? headers : undefined, auth: n.auth, body: n.body?.trim() ? n.body : undefined };
}

/** A starting point for incident and alerting tools: trigger with a dedup key. */
const incidentBody = `{
  "routing_key": "{{ secrets.ALERT_ROUTING_KEY }}",
  "event_action": "trigger",
  "dedup_key": "opskills-{{ runbook.id }}",
  "payload": {
    "summary": "{{ text }}",
    "source": "opskills",
    "severity": "error",
    "custom_details": {
      "failed_step": "{{ run.failed_step.name }}",
      "host": "{{ run.failed_step.host }}",
      "output": "{{ output_tail }}"
    }
  },
  "links": [{ "href": "{{ run.url }}", "text": "Open the run in OpSkills" }]
}`;

const bodyVars = "text, event, runbook.id, runbook.name, run.id, run.number, run.status, run.trigger, run.started_by, run.started_at, run.finished_at, run.duration_seconds, run.url, run.error, run.failed_step.name, run.failed_step.host, run.failed_step.exit_code, run.inputs.NAME, output_tail, secrets.NAME";

function BodyEditor({ n, index, onChange }: { n: Notification; index: number; onChange: (n: Notification) => void }) {
  const custom = n.body !== undefined;
  let problem: string | null = null;
  if (custom && n.body!.trim()) {
    try {
      const v = JSON.parse(n.body!);
      if (typeof v !== "object" || v === null) problem = "The body must be a JSON object or array.";
    } catch (e) {
      problem = `Not valid JSON yet: ${e instanceof Error ? e.message : String(e)}`;
    }
  }
  return (
    <div>
      <div className="flex flex-wrap items-center gap-3">
        <p className="text-xs font-medium text-muted">Body</p>
        <div role="radiogroup" aria-label={`Notification ${index + 1} body`} className="inline-flex gap-1 rounded-lg bg-surface-2 p-0.5">
          {[
            { on: !custom, label: "Default", pick: () => onChange({ ...n, body: undefined }) },
            { on: custom, label: "Custom", pick: () => onChange({ ...n, body: n.body ?? "{\n  \"text\": \"{{ text }}\"\n}" }) },
          ].map((o) => (
            <button key={o.label} type="button" role="radio" aria-checked={o.on} onClick={o.pick} className={cx("h-7 rounded-md px-2.5 text-xs font-medium", o.on ? "bg-surface text-accent shadow-sm" : "text-muted hover:text-ink")}>
              {o.label}
            </button>
          ))}
        </div>
        {custom && (
          <button type="button" onClick={() => onChange({ ...n, body: incidentBody })} className="text-xs font-medium text-accent hover:underline">
            Start from an incident example
          </button>
        )}
      </div>
      {custom ? (
        <>
          <textarea
            value={n.body}
            onChange={(e) => onChange({ ...n, body: e.target.value })}
            rows={Math.min(18, Math.max(5, (n.body ?? "").split("\n").length + 1))}
            spellCheck={false}
            wrap="off"
            aria-label={`Notification ${index + 1} body template`}
            className="scroll-thin mt-2 w-full rounded-xl bg-surface-2 px-4 py-3 font-mono text-xs leading-relaxed outline-none focus:ring-2 focus:ring-accent"
          />
          {problem && <p className="mt-1 text-xs text-rose-600 dark:text-rose-400">{problem}</p>}
          <p className="mt-1.5 text-xs text-muted">
            JSON, with placeholders inside strings: {bodyVars}. A string that is only one placeholder keeps its type, so a number stays a number and output_tail stays a list.
          </p>
        </>
      ) : (
        <p className="mt-1.5 text-xs text-muted">The default body has a text line for chat tools, plus the run's details.</p>
      )}
    </div>
  );
}

/** Extra headers and sign-in for a webhook, with secrets from the vault. */
function WebhookExtras({ n, index, secrets, onChange }: { n: Notification; index: number; secrets: string[]; onChange: (n: Notification) => void }) {
  const headers = n.headers ?? [];
  const setHeader = (i: number, patch: Partial<{ name: string; value: string }>) => onChange({ ...n, headers: headers.map((h, k) => (k === i ? { ...h, ...patch } : h)) });
  const auth = n.auth;
  const pickAuth = (type: string) => {
    const secret = auth?.secret ?? secrets[0] ?? "";
    const next: WebhookAuth | undefined = type === "bearer" ? { type: "bearer", secret } : type === "basic" ? { type: "basic", username: auth?.type === "basic" ? auth.username : "", secret } : undefined;
    onChange({ ...n, auth: next });
  };
  const small = "h-9 rounded-lg bg-surface-2 px-3 text-sm outline-none focus:ring-2 focus:ring-accent";
  return (
    <div className="space-y-4">
      <div>
        <p className="text-xs font-medium text-muted">Headers</p>
        <div className="mt-2 space-y-2">
          {headers.map((h, i) => (
            <div key={i} className="flex gap-2">
              <input value={h.name} onChange={(e) => setHeader(i, { name: e.target.value })} placeholder="X-Api-Key" aria-label={`Notification ${index + 1} header ${i + 1} name`} spellCheck={false} className={`${small} w-40 font-mono`} />
              <input
                value={h.value}
                onChange={(e) => setHeader(i, { value: e.target.value })}
                placeholder="value, or {{ secrets.NAME }}"
                aria-label={`Notification ${index + 1} header ${i + 1} value`}
                spellCheck={false}
                className={`${small} min-w-0 flex-1 font-mono`}
              />
              <button type="button" onClick={() => onChange({ ...n, headers: headers.filter((_, k) => k !== i) })} className="rounded-lg p-2 text-muted hover:text-rose-600" aria-label={`Remove header ${i + 1}`}>
                <X className="size-4" />
              </button>
            </div>
          ))}
          {headers.length < 10 && (
            <button type="button" onClick={() => onChange({ ...n, headers: [...headers, { name: "", value: "" }] })} className="flex items-center gap-1.5 text-xs font-medium text-accent hover:underline">
              <Plus className="size-3.5" /> Add header
            </button>
          )}
        </div>
        <p className="mt-1.5 text-xs text-muted">Put a vault secret in a value with {"{{ secrets.NAME }}"}; only its name is saved.</p>
      </div>
      <div>
        <p className="text-xs font-medium text-muted">Sign-in</p>
        <div className="mt-2 flex flex-wrap items-center gap-2">
          <select value={auth?.type ?? "none"} onChange={(e) => pickAuth(e.target.value)} aria-label={`Notification ${index + 1} sign-in`} className={small}>
            <option value="none">None</option>
            <option value="bearer">Bearer token</option>
            <option value="basic">User name and password</option>
          </select>
          {auth?.type === "basic" && (
            <input value={auth.username} onChange={(e) => onChange({ ...n, auth: { ...auth, username: e.target.value } })} placeholder="User name" aria-label={`Notification ${index + 1} user name`} className={`${small} w-40`} />
          )}
          {auth && (
            <select value={auth.secret} onChange={(e) => onChange({ ...n, auth: { ...auth, secret: e.target.value } })} aria-label={`Notification ${index + 1} sign-in secret`} className={small}>
              {!secrets.includes(auth.secret) && <option value={auth.secret}>{auth.secret || "Pick a vault secret"}</option>}
              {secrets.map((s) => (
                <option key={s} value={s}>
                  {s}
                </option>
              ))}
            </select>
          )}
          {auth && secrets.length === 0 && (
            <a href="#/vault" className="text-xs text-accent hover:underline">
              Add the {auth.type === "bearer" ? "token" : "password"} to the vault first
            </a>
          )}
        </div>
      </div>
    </div>
  );
}

const splitEmails = (s: string) => s.split(/[\s,;]+/).filter(Boolean);

function NotificationCard({ n, index, emailReady, secrets, onChange, onRemove }: { n: Notification; index: number; emailReady: boolean | undefined; secrets: string[]; onChange: (n: Notification) => void; onRemove: () => void }) {
  const byEmail = n.webhook === undefined;
  const [emails, setEmails] = useState((n.email ?? []).join(", "));
  const [test, setTest] = useState<{ busy?: boolean; ok?: string; error?: string }>({});
  const minutes = n.slow_after_seconds ? Math.max(1, Math.round(n.slow_after_seconds / 60)) : 30;

  function toggle(e: NotifyEvent, on: boolean) {
    const next = on ? [...n.on, e] : n.on.filter((x) => x !== e);
    onChange({ ...n, on: next, slow_after_seconds: e === "slow" && on && !n.slow_after_seconds ? 1800 : n.slow_after_seconds });
  }

  async function sendTest() {
    setTest({ busy: true });
    try {
      const clean = cleanNotification(n);
      const r = await api.testNotification(byEmail ? { email: splitEmails(emails) } : { webhook: clean.webhook, headers: clean.headers, auth: clean.auth, body: clean.body });
      setTest({ ok: `Sent to ${r.target}.` });
    } catch (e) {
      setTest({ error: e instanceof Error ? e.message : String(e) });
    }
  }

  return (
    <Card className="space-y-4 p-5 sm:p-6">
      <div className="flex flex-wrap items-center gap-3">
        <div role="radiogroup" aria-label={`Notification ${index + 1} kind`} className="inline-flex gap-1 rounded-xl bg-surface-2 p-1">
          {[
            { on: !byEmail, label: "Webhook", icon: WebhookIcon, pick: () => onChange({ ...n, webhook: n.webhook ?? "", email: undefined }) },
            { on: byEmail, label: "Email", icon: Mail, pick: () => onChange({ ...n, webhook: undefined, headers: undefined, auth: undefined, body: undefined, email: splitEmails(emails) }) },
          ].map((o) => (
            <button key={o.label} type="button" role="radio" aria-checked={o.on} onClick={o.pick} className={cx("flex h-8 items-center gap-1.5 rounded-lg px-3 text-xs font-medium", o.on ? "bg-surface text-accent shadow-sm" : "text-muted hover:text-ink")}>
              <o.icon className="size-3.5" /> {o.label}
            </button>
          ))}
        </div>
        <div className="flex-1" />
        <label className="flex items-center gap-2 text-sm">
          <input type="checkbox" checked={n.enabled} onChange={(e) => onChange({ ...n, enabled: e.target.checked })} className="size-4 accent-accent" />
          On
        </label>
        <button type="button" onClick={onRemove} className="rounded-lg p-2 text-muted hover:text-rose-600" aria-label={`Remove notification ${index + 1}`}>
          <Trash2 className="size-4" />
        </button>
      </div>

      <div>
        <p className="text-xs font-medium text-muted">When a run</p>
        <div className="mt-2 flex flex-wrap items-center gap-x-5 gap-y-2">
          {events.map((e) => (
            <label key={e.value} className="flex items-center gap-2 text-sm">
              <input type="checkbox" checked={n.on.includes(e.value)} onChange={(ev) => toggle(e.value, ev.target.checked)} className="size-4 accent-accent" />
              {e.label}
            </label>
          ))}
          {n.on.includes("slow") && (
            <label className="flex items-center gap-2 text-sm text-muted">
              longer than
              <input
                type="number"
                min={1}
                max={10080}
                value={minutes}
                onChange={(e) => onChange({ ...n, slow_after_seconds: Math.max(1, Number(e.target.value) || 1) * 60 })}
                aria-label={`Notification ${index + 1} minutes`}
                className="h-9 w-20 rounded-lg bg-surface-2 px-3 text-sm text-ink outline-none focus:ring-2 focus:ring-accent"
              />
              minutes
            </label>
          )}
        </div>
      </div>

      {byEmail ? (
        <div>
          <label className="text-xs font-medium text-muted" htmlFor={`nt-email-${index}`}>
            Send to
          </label>
          <input
            id={`nt-email-${index}`}
            value={emails}
            onChange={(e) => {
              setEmails(e.target.value);
              onChange({ ...n, email: splitEmails(e.target.value) });
            }}
            placeholder="oncall@example.com, lead@example.com"
            className={`mt-1.5 ${field}`}
          />
          {emailReady === false && (
            <p className="mt-1.5 text-xs text-amber-700 dark:text-amber-300">
              Email isn't set up on this server yet.{" "}
              <a href="#/settings" className="underline">
                Set it up in Settings
              </a>
              .
            </p>
          )}
        </div>
      ) : (
        <div>
          <label className="text-xs font-medium text-muted" htmlFor={`nt-url-${index}`}>
            URL
          </label>
          <input
            id={`nt-url-${index}`}
            type="url"
            value={n.webhook ?? ""}
            onChange={(e) => onChange({ ...n, webhook: e.target.value })}
            placeholder="https://hooks.slack.com/services/..."
            spellCheck={false}
            className={`mt-1.5 font-mono ${field}`}
          />
          <p className="mt-1.5 text-xs text-muted">Gets a JSON POST with a text line, which Slack, Mattermost, Google Chat and Discord incoming webhooks show as a message.</p>
          <div className="mt-4">
            <WebhookExtras n={n} index={index} secrets={secrets} onChange={onChange} />
          </div>
          <div className="mt-4">
            <BodyEditor n={n} index={index} onChange={onChange} />
          </div>
        </div>
      )}

      <div className="flex flex-wrap items-center gap-3">
        <button
          type="button"
          onClick={sendTest}
          disabled={test.busy || (byEmail ? splitEmails(emails).length === 0 : !(n.webhook ?? "").trim())}
          className="flex h-9 items-center gap-1.5 rounded-lg bg-surface-2 px-3 text-xs font-medium hover:text-accent disabled:opacity-50"
        >
          <Send className="size-3.5" /> {test.busy ? "Sending..." : "Send a test"}
        </button>
        {test.ok && <span className="flex items-center gap-1 text-xs text-emerald-700 dark:text-emerald-300"><Check className="size-3.5" /> {test.ok}</span>}
        {test.error && <span className="flex min-w-0 items-center gap-1 text-xs text-rose-600 dark:text-rose-400"><X className="size-3.5 shrink-0" /> {test.error}</span>}
      </div>
    </Card>
  );
}

export function NotificationsEditor({ notifications, onChange }: { notifications: Notification[]; onChange: (n: Notification[]) => void }) {
  const settings = useData(() => api.notificationSettings(), []);
  const vault = useData(() => api.secrets(), []);
  const secretNames = (vault.data ?? []).map((s) => s.name);
  return (
    <div className="space-y-4">
      <div>
        <h3 className="pt-4 text-[19px] font-semibold">Notifications</h3>
        <p className="mt-1 text-sm text-muted">Tell people when runs start, succeed, fail or run long, by email or in a chat channel.</p>
      </div>
      {notifications.map((n, i) => (
        <NotificationCard
          key={n.id || `new-${i}`}
          n={n}
          index={i}
          emailReady={settings.data?.email}
          secrets={secretNames}
          onChange={(x) => onChange(notifications.map((y, k) => (k === i ? x : y)))}
          onRemove={() => onChange(notifications.filter((_, k) => k !== i))}
        />
      ))}
      {notifications.length < 10 && (
        <button
          type="button"
          onClick={() => onChange([...notifications, newNotification()])}
          className="flex h-11 items-center gap-2 rounded-xl border border-dashed border-line-strong px-4 text-sm text-muted hover:border-accent hover:text-accent"
        >
          <Bell className="size-4" /> Add notification
        </button>
      )}
    </div>
  );
}

/** What was sent about a run, and whether it got through. */
export function RunNotices({ runId, active }: { runId: string; active: boolean }) {
  const notices = useData(() => api.runNotices(runId), [runId], active ? 4000 : undefined);
  const list = notices.data ?? [];
  if (list.length === 0) return null;
  return (
    <Card>
      <CardHeader title="Notifications" />
      <ul className="space-y-2.5 px-6 pb-5 text-sm sm:px-8">
        {list.map((n, i) => (
          <li key={i} className="flex items-start gap-2">
            {n.ok ? <Check className="mt-0.5 size-3.5 shrink-0 text-emerald-600" /> : <X className="mt-0.5 size-3.5 shrink-0 text-rose-600" />}
            <div className="min-w-0">
              <div className="truncate">
                <span className="font-medium">{eventLabel(n.event)}</span> <span className="text-muted">to {n.target}</span>
              </div>
              <div className={cx("text-xs", n.ok ? "text-muted" : "text-rose-600 dark:text-rose-400")}>{n.ok ? `Sent ${timeAgo(n.at)}` : n.error}</div>
            </div>
          </li>
        ))}
      </ul>
    </Card>
  );
}
