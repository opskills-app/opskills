import { useState } from "react";
import { Bell, Check, Mail, Send, Trash2, Webhook as WebhookIcon, X } from "lucide-react";
import { api, useData, type Notification, type NotifyEvent } from "../api";
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
  return n.webhook !== undefined ? { ...base, webhook: n.webhook.trim() } : { ...base, email: (n.email ?? []).map((e) => e.trim()).filter(Boolean) };
}

const splitEmails = (s: string) => s.split(/[\s,;]+/).filter(Boolean);

function NotificationCard({ n, index, emailReady, onChange, onRemove }: { n: Notification; index: number; emailReady: boolean | undefined; onChange: (n: Notification) => void; onRemove: () => void }) {
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
      const r = await api.testNotification(byEmail ? { email: splitEmails(emails) } : { webhook: (n.webhook ?? "").trim() });
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
            { on: byEmail, label: "Email", icon: Mail, pick: () => onChange({ ...n, webhook: undefined, email: splitEmails(emails) }) },
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
          {emailReady === false && <p className="mt-1.5 text-xs text-amber-700 dark:text-amber-300">Email isn't set up on this server yet. Add an [email] section to the server config; see the install docs.</p>}
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
