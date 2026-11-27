import { useEffect, useState } from "react";
import { ScrollText } from "lucide-react";
import { api, type AuditEvent } from "../api";
import { timeAgo, when } from "../format";
import { Avatar, Button, Card, EmptyState, ErrorNote, PageHeader, SearchField } from "../components/ui";

/** How each action reads in the log. Unknown actions show as they are. */
const LABELS: Record<string, string> = {
  "auth.sign_in": "Signed in",
  "auth.sign_in_failed": "Failed to sign in",
  "auth.sign_out": "Signed out",
  "auth.password_change": "Changed their password",
  "user.create": "Added a user",
  "user.role_change": "Changed a role",
  "user.password_reset": "Reset a password",
  "user.delete": "Removed a user",
  "token.create": "Created an API token",
  "token.revoke": "Revoked an API token",
  "runbook.create": "Created a runbook",
  "runbook.update": "Edited a runbook",
  "runbook.delete": "Deleted a runbook",
  "run.start": "Started a run",
  "run.retry": "Retried a run",
  "run.cancel": "Cancelled a run",
  "webhook.create": "Added a webhook",
  "webhook.update": "Changed a webhook",
  "webhook.rotate": "Gave a webhook a new URL",
  "webhook.delete": "Deleted a webhook",
  "secret.create": "Added a secret",
  "secret.update": "Changed a secret",
  "secret.delete": "Deleted a secret",
  "host.enrollment_token": "Created an enrollment token",
  "host.enroll": "Enrolled a host",
  "host.update": "Renamed or tagged a host",
  "host.delete": "Removed a host",
  "settings.email_update": "Changed email settings",
  "settings.email_delete": "Turned email off",
};

const AREAS: [string, string][] = [
  ["", "Everything"],
  ["auth.", "Sign-ins"],
  ["user.", "Users"],
  ["token.", "API tokens"],
  ["runbook.", "Runbooks"],
  ["run.", "Runs"],
  ["webhook.", "Webhooks"],
  ["secret.", "Vault"],
  ["host.", "Hosts"],
  ["settings.", "Settings"],
];

/** A link to what the event is about, when it still has a page. */
function targetHref(e: AuditEvent): string | null {
  if (!e.target) return null;
  if (e.target.kind === "runbook" && e.action !== "runbook.delete") return `#/runbooks/${e.target.id}`;
  if (e.target.kind === "run") return `#/runs/${e.target.id}`;
  return null;
}

/** Who changed what, and when, for admins. */
export function AuditLogPage() {
  const [area, setArea] = useState("");
  const [q, setQ] = useState("");
  const [query, setQuery] = useState("");
  const [events, setEvents] = useState<AuditEvent[]>([]);
  const [next, setNext] = useState<string | null>(null);
  const [days, setDays] = useState<number | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  // Search after typing stops for a moment.
  useEffect(() => {
    const t = setTimeout(() => setQuery(q), 300);
    return () => clearTimeout(t);
  }, [q]);

  useEffect(() => {
    let alive = true;
    setLoading(true);
    api
      .audit({ action: area, q: query })
      .then((page) => {
        if (!alive) return;
        setEvents(page.events);
        setNext(page.next);
        setDays(page.retention_days);
        setError(null);
      })
      .catch((e) => alive && setError(e instanceof Error ? e.message : String(e)))
      .finally(() => alive && setLoading(false));
    return () => {
      alive = false;
    };
  }, [area, query]);

  async function more() {
    if (!next) return;
    setLoading(true);
    try {
      const page = await api.audit({ action: area, q: query, before: next });
      setEvents((list) => [...list, ...page.events]);
      setNext(page.next);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  }

  return (
    <div className="space-y-6">
      <PageHeader title="Audit log" subtitle={`Who changed what, and when. Reading isn't recorded, and secret values never are.${days ? ` Kept for ${days} days.` : ""}`} />
      <div className="flex flex-col gap-3 sm:flex-row sm:items-center">
        <SearchField value={q} onChange={setQ} placeholder="Search people, runbooks, hosts..." className="sm:w-80" />
        <select aria-label="Area" value={area} onChange={(e) => setArea(e.target.value)} className="h-11 rounded-xl bg-surface px-3 text-sm shadow-soft">
          {AREAS.map(([v, label]) => (
            <option key={v} value={v}>
              {label}
            </option>
          ))}
        </select>
      </div>
      {error && <ErrorNote>{error}</ErrorNote>}
      {!loading && events.length === 0 && !error ? (
        <Card>
          <EmptyState icon={ScrollText} title={q || area ? "Nothing matches" : "Nothing recorded yet"}>
            {q || area ? "Try another search or area." : "Changes show up here as people make them."}
          </EmptyState>
        </Card>
      ) : (
        <Card className="overflow-hidden">
          <div className="scroll-thin overflow-x-auto">
            <table className="w-full min-w-[760px] text-sm">
              <thead>
                <tr className="border-b border-line bg-surface-2 text-left text-xs text-muted">
                  <th className="px-6 py-3 font-medium">When</th>
                  <th className="px-3 py-3 font-medium">Who</th>
                  <th className="px-3 py-3 font-medium">What</th>
                  <th className="px-3 py-3 font-medium">On</th>
                  <th className="px-6 py-3 font-medium">Note</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-line">
                {events.map((e) => {
                  const href = targetHref(e);
                  const failed = e.action === "auth.sign_in_failed";
                  return (
                    <tr key={e.id} className="align-top">
                      <td className="px-6 py-3 whitespace-nowrap text-muted" title={when(e.at)}>
                        {timeAgo(e.at)}
                      </td>
                      <td className="px-3 py-3">
                        <span className="flex items-center gap-2">
                          <Avatar name={e.actor} size="size-6" />
                          <span>
                            <span className="font-medium">{e.actor}</span>
                            {e.via && <span className="block text-xs text-muted">{e.via}</span>}
                          </span>
                        </span>
                      </td>
                      <td className={`px-3 py-3 ${failed ? "text-rose-600 dark:text-rose-400" : ""}`}>{LABELS[e.action] ?? e.action}</td>
                      <td className="px-3 py-3">
                        {e.target &&
                          (href ? (
                            <a href={href} className="font-medium text-accent hover:underline">
                              {e.target.name || e.target.id}
                            </a>
                          ) : (
                            <span className="font-medium">{e.target.name || e.target.id}</span>
                          ))}
                        {e.target && <span className="block text-xs text-muted">{e.target.kind}</span>}
                      </td>
                      <td className="px-6 py-3 text-muted">{e.detail}</td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
          {(next || loading) && (
            <div className="border-t border-line px-6 py-4 text-center">
              <Button size="sm" onClick={more} disabled={loading || !next}>
                {loading ? "Loading..." : "Load older"}
              </Button>
            </div>
          )}
        </Card>
      )}
    </div>
  );
}
