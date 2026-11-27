import { useState, type FormEvent } from "react";
import { Check, KeyRound, Layers, Mail, Plus, RotateCcw, Send, Trash2, UserPlus, X } from "lucide-react";
import { CopyButton } from "../components/CopyButton";
import { api, ROLES, useData, type ApiToken, type EmailSettings, type Role, type User } from "../api";
import { ROLE_DESCRIPTIONS, ROLE_LABELS, useCan, useIsServerAdmin, useMe, useSession, useWorkspace } from "../session";
import { MIN_PASSWORD } from "./ChangePassword";
import { timeAgo } from "../format";
import { Avatar, Button, Card, CardHeader, ErrorNote, Modal, PageHeader } from "../components/ui";

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
          Send it as a Bearer token, with <span className="font-mono">?workspace=</span> for a workspace other than Default. It can do what your role allows in each workspace, except manage tokens, sign-ins, users and members, and runs it starts show its name. If your roles change, so does what it can do.
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

const blankEmail: EmailSettings = { smtp_host: "", smtp_port: 587, security: "starttls", from: "" };

/** SMTP settings for email notifications. The password is a vault secret. */
function EmailCard() {
  const current = useData(() => api.emailSettings(), []);
  const vault = useData(() => api.secrets(), []);
  const [form, setForm] = useState<EmailSettings | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);
  const [testTo, setTestTo] = useState("");
  const [test, setTest] = useState<{ busy?: boolean; ok?: string; error?: string }>({});

  const source = current.data?.source;
  const value = form ?? current.data?.settings ?? blankEmail;
  const set = (patch: Partial<EmailSettings>) => {
    setSaved(false);
    setForm({ ...value, ...patch });
  };
  const secrets = (vault.data ?? []).map((s) => s.name);

  async function save(e: FormEvent) {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      const signIn = (value.username ?? "").trim() !== "";
      await api.saveEmailSettings({ ...value, username: signIn ? value.username : undefined, password_secret: signIn ? value.password_secret : undefined });
      setForm(null);
      setSaved(true);
      current.reload();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  async function turnOff() {
    if (!window.confirm("Turn off email? Email notifications will fail until it's set up again.")) return;
    setError(null);
    try {
      await api.deleteEmailSettings();
      setForm(null);
      current.reload();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }

  async function sendTest() {
    setTest({ busy: true });
    try {
      const r = await api.testNotification({ email: [testTo.trim()] });
      setTest({ ok: `Sent to ${r.target}.` });
    } catch (err) {
      setTest({ error: err instanceof Error ? err.message : String(err) });
    }
  }

  const testRow = source && source !== "off" && (
    <div className="flex flex-wrap items-center gap-2 border-t border-line pt-4">
      <input type="email" value={testTo} onChange={(e) => setTestTo(e.target.value)} placeholder="you@example.com" aria-label="Send a test email to" className={`${field} h-9 max-w-xs`} />
      <Button size="sm" type="button" onClick={sendTest} disabled={test.busy || !testTo.includes("@") || form !== null}>
        <Send className="size-3.5" /> {test.busy ? "Sending..." : "Send a test"}
      </Button>
      {test.ok && <span className="flex items-center gap-1 text-xs text-emerald-700 dark:text-emerald-300"><Check className="size-3.5" /> {test.ok}</span>}
      {test.error && <span className="flex min-w-0 items-center gap-1 text-xs text-rose-600 dark:text-rose-400"><X className="size-3.5 shrink-0" /> {test.error}</span>}
    </div>
  );

  return (
    <Card className="mb-4">
      <CardHeader title="Email" hint="The SMTP server email notifications are sent through" />
      <div className="space-y-4 px-6 pb-6 sm:px-8">
        {(error || current.error) && <ErrorNote>{error ?? current.error}</ErrorNote>}
        {source === "config" ? (
          <>
            <p className="text-sm text-muted">Set in the server config file, which takes priority. Change it there, or remove it there to manage email here.</p>
            <dl className="grid grid-cols-[8rem_1fr] gap-y-1.5 text-sm">
              {[
                ["SMTP server", `${current.data?.settings?.smtp_host}:${current.data?.settings?.smtp_port}`],
                ["Security", current.data?.settings?.security],
                ["User name", current.data?.settings?.username ?? "None"],
                ["Sender", current.data?.settings?.from],
              ].map(([k, v]) => (
                <div key={k} className="contents">
                  <dt className="text-muted">{k}</dt>
                  <dd className="truncate">{v}</dd>
                </div>
              ))}
            </dl>
            {testRow}
          </>
        ) : (
          <form onSubmit={save} className="space-y-4">
            <div className="grid grid-cols-1 gap-4 sm:grid-cols-[1fr_7rem_10rem]">
              <label className="text-xs font-medium text-muted">
                SMTP server
                <input required value={value.smtp_host} onChange={(e) => set({ smtp_host: e.target.value })} placeholder="smtp.example.com" spellCheck={false} className={`mt-1.5 ${field} text-ink`} />
              </label>
              <label className="text-xs font-medium text-muted">
                Port
                <input type="number" required min={1} max={65535} value={value.smtp_port} onChange={(e) => set({ smtp_port: Number(e.target.value) })} className={`mt-1.5 ${field} text-ink`} />
              </label>
              <label className="text-xs font-medium text-muted">
                Security
                <select
                  value={value.security}
                  onChange={(e) => {
                    const security = e.target.value as EmailSettings["security"];
                    // Move between the usual ports, unless someone picked another.
                    const port = security === "tls" && value.smtp_port === 587 ? 465 : security === "starttls" && value.smtp_port === 465 ? 587 : value.smtp_port;
                    set({ security, smtp_port: port });
                  }}
                  className={`mt-1.5 ${field} text-ink`}
                >
                  <option value="starttls">STARTTLS</option>
                  <option value="tls">TLS</option>
                  <option value="none">None</option>
                </select>
              </label>
            </div>
            <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
              <label className="text-xs font-medium text-muted">
                User name
                <input value={value.username ?? ""} onChange={(e) => set({ username: e.target.value })} placeholder="Leave empty for no sign-in" autoComplete="off" className={`mt-1.5 ${field} text-ink`} />
              </label>
              <label className="text-xs font-medium text-muted">
                Password, from the vault
                <select value={value.password_secret ?? ""} onChange={(e) => set({ password_secret: e.target.value || undefined })} disabled={!(value.username ?? "").trim()} className={`mt-1.5 ${field} text-ink disabled:opacity-50`}>
                  <option value="">{secrets.length ? "Pick a secret" : "The vault is empty"}</option>
                  {secrets.map((s) => (
                    <option key={s} value={s}>
                      {s}
                    </option>
                  ))}
                </select>
              </label>
            </div>
            <p className="-mt-2 text-xs text-muted">
              Add the password to the <a href="#/vault" className="text-accent hover:underline">vault</a> first, for example as SMTP_PASSWORD. It stays encrypted there; this page only stores its name.
            </p>
            <label className="block text-xs font-medium text-muted">
              Sender
              <input required value={value.from} onChange={(e) => set({ from: e.target.value })} placeholder="OpSkills <opskills@example.com>" className={`mt-1.5 ${field} text-ink`} />
            </label>
            <div className="flex flex-wrap items-center gap-2">
              <Button variant="primary" type="submit" disabled={busy || (form === null && source === "settings")}>
                <Mail className="size-3.5" /> {busy ? "Saving..." : "Save"}
              </Button>
              {source === "settings" && (
                <Button type="button" onClick={turnOff}>
                  Turn off
                </Button>
              )}
              {saved && <span className="flex items-center gap-1 text-xs text-emerald-700 dark:text-emerald-300"><Check className="size-3.5" /> Saved</span>}
              {source === "off" && form === null && <span className="text-xs text-muted">Email is off.</span>}
            </div>
            {testRow}
          </form>
        )}
      </div>
    </Card>
  );
}

/** Change your own password. Other sessions are signed out. */
function AccountCard() {
  const me = useMe();
  const workspace = useWorkspace();
  const [current, setCurrent] = useState("");
  const [next, setNext] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [done, setDone] = useState(false);

  async function submit(e: FormEvent) {
    e.preventDefault();
    setBusy(true);
    setError(null);
    setDone(false);
    try {
      await api.changePassword(current, next);
      setCurrent("");
      setNext("");
      setDone(true);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  if (!me) return null;
  return (
    <Card className="mb-4">
      <CardHeader title="Your account" hint={`Signed in as ${me.user}, ${me.admin ? "server admin" : `${ROLE_LABELS[workspace?.role ?? "viewer"]} in ${workspace?.name ?? "this workspace"}`}`} />
      <form onSubmit={submit} className="grid grid-cols-1 gap-4 px-6 pb-6 sm:px-8 md:grid-cols-[1fr_1fr_auto] md:items-end">
        <div>
          <label className="text-sm font-medium" htmlFor="account-current">
            Current password
          </label>
          <input id="account-current" type="password" autoComplete="current-password" value={current} onChange={(e) => setCurrent(e.target.value)} className={`mt-2 ${field}`} />
        </div>
        <div>
          <label className="text-sm font-medium" htmlFor="account-new">
            New password
          </label>
          <input id="account-new" type="password" autoComplete="new-password" minLength={MIN_PASSWORD} value={next} onChange={(e) => setNext(e.target.value)} className={`mt-2 ${field}`} />
        </div>
        <Button variant="primary" type="submit" disabled={busy || !current || next.length < MIN_PASSWORD}>
          {busy ? "Saving..." : "Change password"}
        </Button>
        <div className="md:col-span-3">
          {error && <ErrorNote>{error}</ErrorNote>}
          {done && <p className="text-sm text-emerald-600 dark:text-emerald-400">Password changed. Your other sessions were signed out.</p>}
          {!error && !done && <p className="text-xs text-muted">At least {MIN_PASSWORD} characters. Changing it signs you out everywhere else.</p>}
        </div>
      </form>
    </Card>
  );
}

/** Shows a generated password once, for the admin to pass on. */
function PasswordShown({ title, user, password, onClose }: { title: string; user: string; password: string; onClose: () => void }) {
  return (
    <Modal title={title} onClose={onClose} footer={<Button variant="primary" onClick={onClose}>Done</Button>}>
      <p className="text-sm text-muted">
        Give <span className="font-medium text-ink-2">{user}</span> this password. It is shown only now; they choose their own when they sign in with it.
      </p>
      <div className="mt-4 flex items-center gap-2">
        <code className="min-w-0 flex-1 rounded-xl bg-surface-2 px-4 py-3 font-mono text-sm">{password}</code>
        <CopyButton text={password} />
      </div>
    </Modal>
  );
}

/** "server admin", or "Editor in Ops, Viewer in Default". */
function memberships(u: User, names: Record<string, string>): string {
  const parts = Object.entries(u.workspaces).map(([slug, role]) => `${ROLE_LABELS[role]} in ${names[slug] ?? slug}`);
  if (u.admin) parts.unshift("Server admin");
  return parts.length ? parts.join(", ") : "Not in a workspace yet";
}

function RolePicker({ role, onChange }: { role: Role; onChange: (r: Role) => void }) {
  return (
    <div className="space-y-2">
      {ROLES.map((r) => (
        <label key={r} className={`flex cursor-pointer gap-3 rounded-xl border p-3 text-sm ${role === r ? "border-accent bg-accent/5" : "border-line"}`}>
          <input type="radio" name="role" checked={role === r} onChange={() => onChange(r)} className="mt-0.5 accent-[var(--accent)]" />
          <span>
            <span className="font-medium">{ROLE_LABELS[r]}</span>
            <span className="block text-xs text-muted">{ROLE_DESCRIPTIONS[r]}</span>
          </span>
        </label>
      ))}
    </div>
  );
}

/** A server admin adds an account, optionally to the current workspace. */
function NewUserDialog({ onClose, onCreated }: { onClose: () => void; onCreated: () => void }) {
  const workspace = useWorkspace();
  const [name, setName] = useState("");
  const [serverAdmin, setServerAdmin] = useState(false);
  const [join, setJoin] = useState(true);
  const [role, setRole] = useState<Role>("operator");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [created, setCreated] = useState<{ user: User; password: string } | null>(null);

  async function submit(e: FormEvent) {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      setCreated(await api.createUser(name, serverAdmin, join && workspace ? { [workspace.slug]: role } : {}));
      onCreated();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  if (created) return <PasswordShown title={`User "${created.user.name}" added`} user={created.user.name} password={created.password} onClose={onClose} />;
  return (
    <Modal
      title="Add a user"
      onClose={onClose}
      footer={
        <>
          <Button type="button" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" type="submit" form="new-user" disabled={busy || !name.trim()}>
            <UserPlus className="size-3.5" /> {busy ? "Adding..." : "Add user"}
          </Button>
        </>
      }
    >
      <form id="new-user" onSubmit={submit} className="space-y-5">
        {error && <ErrorNote>{error}</ErrorNote>}
        <div>
          <label className="text-sm font-medium" htmlFor="user-name">
            User name
          </label>
          <input id="user-name" required maxLength={64} value={name} onChange={(e) => setName(e.target.value)} placeholder="jane.doe" className={`mt-2 ${field}`} />
          <p className="mt-1.5 text-xs text-muted">Lowercase letters, digits, dots, dashes and underscores. They sign in with it.</p>
        </div>
        {workspace && (
          <div>
            <label className="flex items-center gap-2 text-sm font-medium">
              <input type="checkbox" checked={join} onChange={(e) => setJoin(e.target.checked)} className="accent-[var(--accent)]" />
              Add to {workspace.name}
            </label>
            {join && (
              <div className="mt-3">
                <RolePicker role={role} onChange={setRole} />
              </div>
            )}
          </div>
        )}
        <label className="flex gap-3 rounded-xl border border-line p-3 text-sm">
          <input type="checkbox" checked={serverAdmin} onChange={(e) => setServerAdmin(e.target.checked)} className="mt-0.5 accent-[var(--accent)]" />
          <span>
            <span className="font-medium">Server admin</span>
            <span className="block text-xs text-muted">Manages accounts, workspaces, email settings and everyone's API tokens, and is Admin in every workspace.</span>
          </span>
        </label>
        <p className="text-xs text-muted">OpSkills generates a password to give them. They replace it when they first sign in. Workspace Admins can add them to other workspaces.</p>
      </form>
    </Modal>
  );
}

/** Server admins add accounts, choose other server admins, reset passwords and remove accounts. */
function UsersCard() {
  const me = useMe();
  const users = useData(() => api.users(), []);
  const spaces = useData(() => api.workspaces(), []);
  const [adding, setAdding] = useState(false);
  const [shown, setShown] = useState<{ user: string; password: string } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const names = Object.fromEntries((spaces.data ?? []).map((w) => [w.slug, w.name]));

  async function act(f: () => Promise<unknown>) {
    setError(null);
    try {
      await f();
      users.reload();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }

  const list = users.data ?? [];
  return (
    <Card className="mb-4">
      <CardHeader
        title="Users"
        hint="Everyone who can sign in to this server"
        action={
          <Button size="sm" variant="primary" onClick={() => setAdding(true)}>
            <UserPlus className="size-3.5" /> Add user
          </Button>
        }
      />
      {(error || users.error) && (
        <div className="px-6 pb-4 sm:px-8">
          <ErrorNote>{error ?? users.error}</ErrorNote>
        </div>
      )}
      <div className="scroll-thin overflow-x-auto">
        <table className="w-full min-w-[720px] text-sm">
          <thead>
            <tr className="border-y border-line bg-surface-2 text-left text-xs text-muted">
              <th className="px-6 py-3 font-medium sm:px-8">User</th>
              <th className="px-3 py-3 font-medium">Workspaces</th>
              <th className="px-3 py-3 font-medium">Server admin</th>
              <th className="px-3 py-3 font-medium">Last sign-in</th>
              <th className="px-6 py-3 sm:px-8" />
            </tr>
          </thead>
          <tbody className="divide-y divide-line">
            {list.map((u) => {
              const self = u.name === me?.user;
              return (
                <tr key={u.name}>
                  <td className="px-6 py-3 sm:px-8">
                    <span className="flex items-center gap-3">
                      <Avatar name={u.name} size="size-7" />
                      <span>
                        <span className="font-medium">{u.name}</span>
                        {self && <span className="ml-1.5 text-xs text-muted">(you)</span>}
                        {u.must_change_password && <span className="block text-xs text-amber-600 dark:text-amber-400">Has a generated password</span>}
                      </span>
                    </span>
                  </td>
                  <td className="px-3 py-3 text-muted">{memberships({ ...u, admin: false }, names)}</td>
                  <td className="px-3 py-3">
                    <input
                      type="checkbox"
                      aria-label={`${u.name} is a server admin`}
                      checked={u.admin}
                      disabled={self}
                      title={self ? "Another server admin can change this" : undefined}
                      onChange={(e) => act(() => api.setServerAdmin(u.name, e.target.checked))}
                      className="size-4 accent-[var(--accent)] disabled:opacity-60"
                    />
                  </td>
                  <td className="px-3 py-3 text-muted">{u.last_sign_in_at ? timeAgo(u.last_sign_in_at) : "Never"}</td>
                  <td className="px-6 py-3 text-right whitespace-nowrap sm:px-8">
                    {!self && (
                      <>
                        <button
                          type="button"
                          onClick={() => {
                            if (!window.confirm(`Reset the password of ${u.name}? They are signed out and get a new generated password.`)) return;
                            act(async () => setShown(await api.resetPassword(u.name).then((r) => ({ user: u.name, password: r.password }))));
                          }}
                          className="inline-flex items-center gap-1 rounded-lg px-2 py-1 text-xs text-muted hover:text-ink"
                        >
                          <RotateCcw className="size-3.5" /> Reset password
                        </button>
                        <button
                          type="button"
                          onClick={() => {
                            if (!window.confirm(`Remove ${u.name}? They are signed out, leave every workspace, and their API tokens stop working.`)) return;
                            act(() => api.deleteUser(u.name));
                          }}
                          className="inline-flex items-center gap-1 rounded-lg px-2 py-1 text-xs text-muted hover:text-rose-600"
                          aria-label={`Remove ${u.name}`}
                        >
                          <Trash2 className="size-3.5" /> Remove
                        </button>
                      </>
                    )}
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
      {adding && <NewUserDialog onClose={() => setAdding(false)} onCreated={() => users.reload()} />}
      {shown && <PasswordShown title={`New password for ${shown.user}`} user={shown.user} password={shown.password} onClose={() => setShown(null)} />}
    </Card>
  );
}

/** Server admins add and delete workspaces. */
function WorkspacesCard() {
  const session = useSession();
  const spaces = useData(() => api.workspaces(), []);
  const [slug, setSlug] = useState("");
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function create(e: FormEvent) {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      await api.createWorkspace(slug, name, "");
      setSlug("");
      setName("");
      spaces.reload();
      session?.reload();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  async function remove(ws: { slug: string; name: string }) {
    if (!window.confirm(`Delete the workspace ${ws.name}? Its run history is deleted too. It must have no runbooks, secrets or hosts of its own.`)) return;
    setError(null);
    try {
      await api.deleteWorkspace(ws.slug);
      spaces.reload();
      if (session?.workspace.slug === ws.slug) session.switchTo("default");
      session?.reload();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }

  return (
    <Card className="mb-4">
      <CardHeader title="Workspaces" hint="Separate runbooks, runs, vault and hosts, each with its own members" />
      {(error || spaces.error) && (
        <div className="px-6 pb-4 sm:px-8">
          <ErrorNote>{error ?? spaces.error}</ErrorNote>
        </div>
      )}
      <ul className="divide-y divide-line border-y border-line">
        {(spaces.data ?? []).map((w) => (
          <li key={w.slug} className="flex items-center gap-3 px-6 py-3 text-sm sm:px-8">
            <Layers className="size-4 text-muted" />
            <span className="min-w-0 flex-1">
              <span className="font-medium">{w.name}</span> <span className="font-mono text-xs text-muted">{w.slug}</span>
            </span>
            {w.slug !== "default" && (
              <button type="button" onClick={() => remove(w)} className="inline-flex items-center gap-1 rounded-lg px-2 py-1 text-xs text-muted hover:text-rose-600" aria-label={`Delete ${w.name}`}>
                <Trash2 className="size-3.5" /> Delete
              </button>
            )}
          </li>
        ))}
      </ul>
      <form onSubmit={create} className="grid grid-cols-1 gap-3 px-6 py-5 sm:px-8 md:grid-cols-[1fr_1fr_auto] md:items-end">
        <div>
          <label className="text-sm font-medium" htmlFor="ws-name">
            Name
          </label>
          <input id="ws-name" value={name} maxLength={60} onChange={(e) => setName(e.target.value)} placeholder="Payments team" className={`mt-2 ${field}`} />
        </div>
        <div>
          <label className="text-sm font-medium" htmlFor="ws-slug">
            Id
          </label>
          <input id="ws-slug" value={slug} maxLength={40} onChange={(e) => setSlug(e.target.value.toLowerCase())} placeholder="payments" className={`mt-2 font-mono ${field}`} />
        </div>
        <Button variant="primary" type="submit" disabled={busy || !slug.trim() || !name.trim()}>
          <Plus className="size-4" /> {busy ? "Adding..." : "Add workspace"}
        </Button>
        <p className="text-xs text-muted md:col-span-3">The id is used in the API (<span className="font-mono">?workspace=payments</span>) and can't change; the name can.</p>
      </form>
    </Card>
  );
}

/** The current workspace's name and description; its Admins can change them. */
function ThisWorkspaceCard() {
  const session = useSession();
  const admin = useCan("admin");
  const ws = useData(() => api.workspace(session?.workspace.slug ?? "default"), [session?.workspace.slug]);
  const [name, setName] = useState<string | null>(null);
  const [description, setDescription] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);
  const current = ws.data?.workspace;

  async function save(e: FormEvent) {
    e.preventDefault();
    if (!current) return;
    setError(null);
    setSaved(false);
    try {
      await api.updateWorkspace(current.slug, name ?? current.name, description ?? current.description);
      setSaved(true);
      ws.reload();
      session?.reload();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }

  if (!current) return null;
  const counts = ws.data ? `${ws.data.runbooks} runbook(s), ${ws.data.hosts} host(s), ${ws.data.members} member(s)` : "";
  return (
    <Card className="mb-4">
      <CardHeader title={`Workspace: ${current.name}`} hint={`Id ${current.slug}. ${counts}`} />
      {admin ? (
        <form onSubmit={save} className="grid grid-cols-1 gap-3 px-6 pb-6 sm:px-8 md:grid-cols-[1fr_2fr_auto] md:items-end">
          <div>
            <label className="text-sm font-medium" htmlFor="this-ws-name">
              Name
            </label>
            <input id="this-ws-name" value={name ?? current.name} maxLength={60} onChange={(e) => setName(e.target.value)} className={`mt-2 ${field}`} />
          </div>
          <div>
            <label className="text-sm font-medium" htmlFor="this-ws-description">
              Description
            </label>
            <input id="this-ws-description" value={description ?? current.description} maxLength={300} onChange={(e) => setDescription(e.target.value)} placeholder="What this team runs" className={`mt-2 ${field}`} />
          </div>
          <Button type="submit">Save</Button>
          <div className="md:col-span-3">
            {error && <ErrorNote>{error}</ErrorNote>}
            {saved && <p className="text-sm text-emerald-600 dark:text-emerald-400">Saved.</p>}
          </div>
        </form>
      ) : (
        current.description && <p className="px-6 pb-6 text-sm text-muted sm:px-8">{current.description}</p>
      )}
    </Card>
  );
}

/** Who's in the current workspace. Its Admins add existing accounts, set roles and remove members. */
function MembersCard() {
  const session = useSession();
  const me = useMe();
  const admin = useCan("admin");
  const slug = session?.workspace.slug ?? "default";
  const members = useData(() => api.members(slug), [slug]);
  const addable = useData(() => (admin ? api.addableUsers(slug) : Promise.resolve([] as string[])), [slug, admin]);
  const [who, setWho] = useState("");
  const [role, setRole] = useState<Role>("operator");
  const [error, setError] = useState<string | null>(null);

  async function act(f: () => Promise<unknown>) {
    setError(null);
    try {
      await f();
      members.reload();
      addable.reload();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }

  const list = members.data?.members ?? [];
  const candidates = addable.data ?? [];
  return (
    <Card className="mb-4">
      <CardHeader title="Members" hint={`Who can see ${session?.workspace.name ?? "this workspace"}, and their role here`} />
      {(error || members.error) && (
        <div className="px-6 pb-4 sm:px-8">
          <ErrorNote>{error ?? members.error}</ErrorNote>
        </div>
      )}
      <ul className="divide-y divide-line border-y border-line">
        {list.map((m) => {
          const self = m.name === me?.user;
          return (
            <li key={m.name} className="flex items-center gap-3 px-6 py-3 text-sm sm:px-8">
              <Avatar name={m.name} size="size-7" />
              <span className="min-w-0 flex-1 font-medium">
                {m.name}
                {self && <span className="ml-1.5 text-xs font-normal text-muted">(you)</span>}
              </span>
              {admin && !self ? (
                <>
                  <select aria-label={`Role of ${m.name}`} value={m.role} onChange={(e) => act(() => api.setMember(slug, m.name, e.target.value as Role))} className="h-8 rounded-lg border border-line bg-surface px-2 text-sm">
                    {ROLES.map((r) => (
                      <option key={r} value={r}>
                        {ROLE_LABELS[r]}
                      </option>
                    ))}
                  </select>
                  <button
                    type="button"
                    onClick={() => window.confirm(`Remove ${m.name} from this workspace?`) && act(() => api.removeMember(slug, m.name))}
                    className="inline-flex items-center gap-1 rounded-lg px-2 py-1 text-xs text-muted hover:text-rose-600"
                    aria-label={`Remove ${m.name} from the workspace`}
                  >
                    <Trash2 className="size-3.5" /> Remove
                  </button>
                </>
              ) : (
                <span className="text-muted">{ROLE_LABELS[m.role]}</span>
              )}
            </li>
          );
        })}
        {list.length === 0 && <li className="px-6 py-3 text-sm text-muted sm:px-8">No members yet.</li>}
      </ul>
      {admin && (
        <form
          onSubmit={(e) => {
            e.preventDefault();
            act(async () => {
              await api.setMember(slug, who, role);
              setWho("");
            });
          }}
          className="flex flex-col gap-3 px-6 py-4 sm:flex-row sm:items-center sm:px-8"
        >
          <select aria-label="User to add" value={who} onChange={(e) => setWho(e.target.value)} className="h-10 min-w-0 flex-1 rounded-xl bg-surface-2 px-3 text-sm">
            <option value="">{candidates.length ? "Add an existing user..." : "Everyone is already a member"}</option>
            {candidates.map((n) => (
              <option key={n} value={n}>
                {n}
              </option>
            ))}
          </select>
          <select aria-label="Role for the new member" value={role} onChange={(e) => setRole(e.target.value as Role)} className="h-10 rounded-xl bg-surface-2 px-3 text-sm">
            {ROLES.map((r) => (
              <option key={r} value={r}>
                {ROLE_LABELS[r]}
              </option>
            ))}
          </select>
          <Button type="submit" disabled={!who}>
            <UserPlus className="size-3.5" /> Add
          </Button>
        </form>
      )}
      {members.data && members.data.server_admins.length > 0 && (
        <p className="px-6 pb-4 text-xs text-muted sm:px-8">Server admins can act as Admin here too: {members.data.server_admins.join(", ")}.</p>
      )}
    </Card>
  );
}

const permissions: [string, Role][] = [
  ["See runbooks, runs, output and hosts", "viewer"],
  ["Start, retry and cancel runs", "operator"],
  ["Edit runbooks, with schedules, webhooks and notifications", "editor"],
  ["Manage vault secrets", "editor"],
  ["Enroll, tag, share and remove hosts", "admin"],
  ["Manage members and the workspace's name", "admin"],
];

function RolesCard() {
  const rank = (r: Role) => ROLES.indexOf(r);
  return (
    <Card className="mb-4">
      <CardHeader title="Roles" hint="What each role can do in a workspace; each includes the roles to its left" />
      <div className="scroll-thin overflow-x-auto">
        <table className="w-full min-w-[560px] text-sm">
          <thead>
            <tr className="border-y border-line bg-surface-2 text-left text-xs text-muted">
              <th className="px-6 py-3 font-medium sm:px-8">Permission</th>
              {ROLES.map((r) => (
                <th key={r} className="px-2 py-3 text-center font-medium">
                  {ROLE_LABELS[r]}
                </th>
              ))}
            </tr>
          </thead>
          <tbody className="divide-y divide-line">
            {permissions.map(([label, min]) => (
              <tr key={label}>
                <td className="px-6 py-3 sm:px-8">{label}</td>
                {ROLES.map((r) => (
                  <td key={r} className="px-2 py-3 text-center">
                    {rank(r) >= rank(min) ? <Check className="mx-auto size-4 text-accent" aria-label="Yes" /> : <span className="text-line-strong" aria-label="No">-</span>}
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <p className="px-6 py-4 text-xs text-muted sm:px-8">
        Editors can make a runbook run any command, as root, on every host it targets. Give that role only to people you'd trust with root on those hosts. Server admins also manage accounts, workspaces, email settings and everyone's API tokens, and are Admin in every workspace.
      </p>
    </Card>
  );
}

/** Every user's API tokens, for admins to revoke. */
function AllTokensCard() {
  const tokens = useData(() => api.allTokens(), []);
  const [error, setError] = useState<string | null>(null);
  const list = tokens.data ?? [];
  async function revoke(t: ApiToken) {
    if (!window.confirm(`Revoke "${t.name}" of ${t.user}? Anything using it stops working right away.`)) return;
    setError(null);
    try {
      await api.revokeAnyToken(t.id);
      tokens.reload();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }
  return (
    <Card className="mb-4">
      <CardHeader title="Everyone's API tokens" hint="Revoke a token of any user" />
      {(error || tokens.error) && (
        <div className="px-6 pb-4 sm:px-8">
          <ErrorNote>{error ?? tokens.error}</ErrorNote>
        </div>
      )}
      {list.length === 0 ? (
        <p className="px-6 pb-6 text-sm text-muted sm:px-8">No one has an API token.</p>
      ) : (
        <ul className="divide-y divide-line border-t border-line">
          {list.map((t) => (
            <li key={t.id} className="flex items-center gap-3 px-6 py-3 text-sm sm:px-8">
              <KeyRound className="size-3.5 text-muted" />
              <span className="min-w-0 flex-1 truncate">
                <span className="font-medium">{t.name}</span> <span className="text-muted">of {t.user}, last used {t.last_used_at ? timeAgo(t.last_used_at) : "never"}</span>
              </span>
              <button type="button" onClick={() => revoke(t)} className="inline-flex items-center gap-1 rounded-lg px-2 py-1 text-xs text-muted hover:text-rose-600" aria-label={`Revoke ${t.name} of ${t.user}`}>
                <Trash2 className="size-3.5" /> Revoke
              </button>
            </li>
          ))}
        </ul>
      )}
    </Card>
  );
}

export function SettingsPage() {
  const serverAdmin = useIsServerAdmin();
  return (
    <>
      <PageHeader title="Settings" subtitle={serverAdmin ? "Your account and tokens, this workspace, and the server's users, workspaces and email." : "Your account and tokens, and this workspace."} />
      <AccountCard />
      <ApiTokensCard />
      <ThisWorkspaceCard />
      <MembersCard />
      <RolesCard />
      {serverAdmin && (
        <>
          <WorkspacesCard />
          <UsersCard />
          <AllTokensCard />
          <EmailCard />
        </>
      )}
    </>
  );
}
