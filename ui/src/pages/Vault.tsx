import { useEffect, useRef, useState, type FormEvent, type ReactNode } from "react";
import { ChevronRight, Eye, EyeOff, FileKey, Folder, FolderInput, KeyRound, Lock, Pencil, Plus, RefreshCw, Share2, ShieldCheck, Trash2, Upload } from "lucide-react";
import { api, SHARED_PREFIX, secretEnvName, useData, type Secret, type SecretKind, type SharedSecret, type Workspace } from "../api";
import { timeAgo } from "../format";
import { useIsServerAdmin, useWorkspace } from "../session";
import { Badge, Button, Card, cx, EmptyState, ErrorNote, Modal, PageHeader, SearchField, TableHeading, type BadgeTone } from "../components/ui";

const kinds: { id: SecretKind; label: string; tone: BadgeTone }[] = [
  { id: "password", label: "Password", tone: "blue" },
  { id: "token", label: "Token", tone: "amber" },
  { id: "ssh_key", label: "SSH key", tone: "green" },
  { id: "certificate", label: "Certificate", tone: "green" },
  { id: "other", label: "Other", tone: "gray" },
];
const kindOf = (k: SecretKind) => kinds.find((x) => x.id === k) ?? kinds[4];
const multiline = (k: SecretKind) => k === "ssh_key" || k === "certificate";
const MAX_VALUE_BYTES = 65536;

const field = "h-11 w-full rounded-xl bg-surface-2 px-4 text-sm outline-none focus:ring-2 focus:ring-accent";

/** Folders: lowercase letters, digits, dots, dashes and underscores, separated by `/`. */
const cleanFolder = (s: string) =>
  s
    .toLowerCase()
    .replace(/[^a-z0-9._/-]/g, "-")
    .replace(/\/{2,}/g, "/")
    .replace(/^\//, "");
const cleanName = (s: string) => s.toUpperCase().replace(/[^A-Z0-9_]/g, "_");
const joinPath = (folder: string, name: string) => [folder.replace(/\/+$/, ""), name].filter(Boolean).join("/");
const splitPath = (path: string) => {
  const i = path.lastIndexOf("/");
  return i < 0 ? { folder: "", name: path } : { folder: path.slice(0, i), name: path.slice(i + 1) };
};
const errorText = (e: unknown) => (e instanceof Error ? e.message : String(e));

function ValueInput({ kind, value, onChange, autoFocus }: { kind: SecretKind; value: string; onChange: (v: string) => void; autoFocus?: boolean }) {
  const [show, setShow] = useState(false);
  const [fileError, setFileError] = useState<string | null>(null);
  const file = useRef<HTMLInputElement>(null);
  async function upload(f: File | undefined) {
    setFileError(null);
    if (!f) return;
    if (f.size > MAX_VALUE_BYTES) {
      setFileError(`${f.name} is larger than 64 KB.`);
      return;
    }
    onChange(await f.text());
  }
  const lines = multiline(kind) || value.includes("\n");
  return (
    <>
      {lines ? (
        <textarea
          id="secret-value"
          value={value}
          onChange={(e) => onChange(e.target.value)}
          rows={7}
          spellCheck={false}
          autoComplete="off"
          autoFocus={autoFocus}
          placeholder={kind === "ssh_key" ? "-----BEGIN OPENSSH PRIVATE KEY-----" : kind === "certificate" ? "-----BEGIN CERTIFICATE-----" : ""}
          className="mt-2 w-full resize-y rounded-xl bg-term px-4 py-3 font-mono text-[12px] text-term-ink outline-none focus:ring-2 focus:ring-accent"
        />
      ) : (
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
      )}
      <input ref={file} type="file" className="hidden" aria-label="Upload a file" onChange={(e) => upload(e.target.files?.[0])} />
      <button type="button" onClick={() => file.current?.click()} className="mt-2 inline-flex items-center gap-1.5 text-xs font-medium text-accent hover:underline">
        <Upload className="size-3.5" /> Upload a file
      </button>
      {fileError && <p className="mt-1 text-xs text-rose-600">{fileError}</p>}
    </>
  );
}

function KindSelect({ kind, setKind }: { kind: SecretKind; setKind: (k: SecretKind) => void }) {
  return (
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
  );
}

/** Folder and name, with how a runbook refers to the result. */
function PathFields({ folder, setFolder, name, setName, shared, autoFocus }: { folder: string; setFolder: (f: string) => void; name: string; setName: (n: string) => void; shared: boolean; autoFocus?: boolean }) {
  const path = joinPath(folder, name || "DB_PASSWORD");
  return (
    <div>
      <div className="grid grid-cols-1 gap-3 sm:grid-cols-[minmax(0,1fr)_minmax(0,1fr)]">
        <div>
          <label className="text-sm font-medium" htmlFor="secret-folder">
            Folder <span className="font-normal text-muted">(optional)</span>
          </label>
          <input id="secret-folder" value={folder} onChange={(e) => setFolder(cleanFolder(e.target.value))} placeholder="prod/db" spellCheck={false} className={`mt-2 font-mono ${field}`} />
        </div>
        <div>
          <label className="text-sm font-medium" htmlFor="secret-name">
            Name
          </label>
          <input id="secret-name" autoFocus={autoFocus} value={name} onChange={(e) => setName(cleanName(e.target.value))} placeholder="DB_PASSWORD" spellCheck={false} className={`mt-2 font-mono ${field}`} />
        </div>
      </div>
      <p className="mt-1.5 text-xs text-muted">
        Use it in a runbook as <code className="font-mono text-ink-2">{(shared ? SHARED_PREFIX : "") + path}</code>; steps read it as <code className="font-mono text-ink-2">${secretEnvName(path)}</code>. Folders
        are lowercase, like <code className="font-mono">prod/db</code>; names are capital letters, digits and underscores.
      </p>
    </div>
  );
}

/** Who a shared secret is shared with. */
function SharingFields({ all, setAll, chosen, setChosen, workspaces }: { all: boolean; setAll: (a: boolean) => void; chosen: string[]; setChosen: (c: string[]) => void; workspaces: Workspace[] }) {
  return (
    <fieldset>
      <legend className="text-sm font-medium">Shared with</legend>
      <label className="mt-2 flex items-center gap-2 text-sm">
        <input type="checkbox" checked={all} onChange={(e) => setAll(e.target.checked)} className="size-4 accent-accent" /> All workspaces, now and later
      </label>
      {!all && (
        <div className="mt-2 flex flex-wrap gap-x-5 gap-y-2">
          {workspaces.map((w) => (
            <label key={w.slug} className="flex items-center gap-2 text-sm">
              <input
                type="checkbox"
                checked={chosen.includes(w.slug)}
                onChange={(e) => setChosen(e.target.checked ? [...chosen, w.slug] : chosen.filter((x) => x !== w.slug))}
                className="size-4 accent-accent"
              />
              {w.name}
            </label>
          ))}
        </div>
      )}
      <p className="mt-1.5 text-xs text-muted">Editors there can use it in their runbooks, so they can also read it. Server-wide settings, like the email password, can use it either way.</p>
    </fieldset>
  );
}

function AddSecret({ startFolder, shared, workspaces, onClose, onSaved }: { startFolder: string; shared: boolean; workspaces: Workspace[]; onClose: () => void; onSaved: () => void }) {
  const [folder, setFolder] = useState(startFolder);
  const [name, setName] = useState("");
  const [kind, setKind] = useState<SecretKind>("password");
  const [value, setValue] = useState("");
  const [description, setDescription] = useState("");
  const [all, setAll] = useState(false);
  const [chosen, setChosen] = useState<string[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function save(e: FormEvent) {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      const path = joinPath(folder, name);
      if (shared) await api.createSharedSecret({ name: path, kind, value, description, all_workspaces: all, workspaces: chosen });
      else await api.createSecret({ name: path, kind, value, description });
      onSaved();
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Modal
      title={shared ? "Add a shared secret" : "Add a secret"}
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
        <PathFields folder={folder} setFolder={setFolder} name={name} setName={setName} shared={shared} autoFocus />
        <KindSelect kind={kind} setKind={setKind} />
        <div>
          <label className="text-sm font-medium" htmlFor="secret-value">
            Value
          </label>
          <ValueInput kind={kind} value={value} onChange={setValue} />
          <p className="mt-1.5 text-xs text-muted">At least 4 characters, up to 64 KB. It can't be viewed again after saving.</p>
        </div>
        <div>
          <label className="text-sm font-medium" htmlFor="secret-desc">
            Description <span className="font-normal text-muted">(optional)</span>
          </label>
          <input id="secret-desc" value={description} onChange={(e) => setDescription(e.target.value)} placeholder="Read-only user for the reporting database" className={`mt-2 ${field}`} />
        </div>
        {shared && <SharingFields all={all} setAll={setAll} chosen={chosen} setChosen={setChosen} workspaces={workspaces} />}
      </form>
    </Modal>
  );
}

/** One secret being changed: a workspace secret, or a shared one (server admins). */
type Target = { shared: false; s: Secret } | { shared: true; s: SharedSecret };
type Mode = "replace" | "details" | "move";

function EditSecret({ target, mode, workspaces, onClose, onSaved }: { target: Target; mode: Mode; workspaces: Workspace[]; onClose: () => void; onSaved: () => void }) {
  const s = target.s;
  const start = splitPath(s.name);
  const [kind, setKind] = useState<SecretKind>(s.kind);
  const [description, setDescription] = useState(s.description);
  const [value, setValue] = useState("");
  const [folder, setFolder] = useState(start.folder);
  const [name, setName] = useState(start.name);
  const [all, setAll] = useState(target.shared ? target.s.all_workspaces : false);
  const [chosen, setChosen] = useState<string[]>(target.shared ? target.s.workspaces : []);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const inUse = target.s.used_by.length > 0 || (target.shared && target.s.email_password);

  async function save(e: FormEvent) {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      const change =
        mode === "replace" ? { value } : mode === "move" ? { name: joinPath(folder, name) } : target.shared ? { kind, description, all_workspaces: all, workspaces: all ? [] : chosen } : { kind, description };
      if (target.shared) await api.updateSharedSecret(s.name, change);
      else await api.updateSecret(s.name, change);
      onSaved();
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(false);
    }
  }

  const title = mode === "replace" ? `Replace the value of ${s.name}` : mode === "move" ? `Move ${s.name}` : `Edit ${s.name}`;
  const blocked = mode === "move" && inUse;
  return (
    <Modal
      title={title}
      onClose={onClose}
      footer={
        <>
          <Button type="button" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" type="submit" form="edit-secret" disabled={busy || blocked || (mode === "replace" && value.length < 4) || (mode === "move" && (!name || joinPath(folder, name) === s.name))}>
            {busy ? "Saving..." : mode === "replace" ? "Replace value" : mode === "move" ? "Move" : "Save"}
          </Button>
        </>
      }
    >
      <form id="edit-secret" onSubmit={save} className="space-y-4">
        {error && <ErrorNote>{error}</ErrorNote>}
        {mode === "replace" && (
          <div>
            <label className="text-sm font-medium" htmlFor="secret-value">
              New value
            </label>
            <ValueInput kind={s.kind} value={value} onChange={setValue} autoFocus />
            <p className="mt-1.5 text-xs text-muted">The old value is overwritten. Runs that start from now on use the new value.</p>
          </div>
        )}
        {mode === "move" &&
          (blocked ? (
            <ErrorNote>
              A secret in use can't be moved, because the runbooks that use it would lose it. Change them first:{" "}
              {[...target.s.used_by.map((r) => r.name), ...(target.shared && target.s.email_password ? ["Settings > Email"] : [])].join(", ")}.
            </ErrorNote>
          ) : (
            <PathFields folder={folder} setFolder={setFolder} name={name} setName={setName} shared={target.shared} autoFocus />
          ))}
        {mode === "details" && (
          <>
            <KindSelect kind={kind} setKind={setKind} />
            <div>
              <label className="text-sm font-medium" htmlFor="secret-desc">
                Description
              </label>
              <input id="secret-desc" value={description} onChange={(e) => setDescription(e.target.value)} className={`mt-2 ${field}`} />
            </div>
            {target.shared && <SharingFields all={all} setAll={setAll} chosen={chosen} setChosen={setChosen} workspaces={workspaces} />}
          </>
        )}
      </form>
    </Modal>
  );
}

function SecretIcon({ kind, shared }: { kind: SecretKind; shared?: boolean }) {
  return (
    <span className="flex size-9 shrink-0 items-center justify-center rounded-full bg-accent-soft text-accent">
      {shared ? <Share2 className="size-4" /> : kind === "ssh_key" || kind === "certificate" ? <FileKey className="size-4" /> : <Lock className="size-4" />}
    </span>
  );
}

function Updated({ at, by }: { at: number; by?: string }) {
  return (
    <span className="hidden text-muted lg:block">
      {timeAgo(at)}
      {by && <span className="block truncate text-xs">by {by}</span>}
    </span>
  );
}

function Actions({ name, onEdit, onDelete }: { name: string; onEdit?: (m: Mode) => void; onDelete?: () => void }) {
  if (!onEdit) return <span />;
  return (
    <span className="flex justify-end gap-1">
      <Button size="sm" variant="ghost" onClick={() => onEdit("replace")} aria-label={`Replace the value of ${name}`} title="Replace value">
        <RefreshCw className="size-3.5" />
      </Button>
      <Button size="sm" variant="ghost" onClick={() => onEdit("details")} aria-label={`Edit ${name}`} title="Edit details">
        <Pencil className="size-3.5" />
      </Button>
      <Button size="sm" variant="ghost" onClick={() => onEdit("move")} aria-label={`Move ${name}`} title="Move or rename">
        <FolderInput className="size-3.5" />
      </Button>
      <Button size="sm" variant="ghost" onClick={onDelete} aria-label={`Delete ${name}`} title="Delete" className="hover:text-rose-600">
        <Trash2 className="size-3.5" />
      </Button>
    </span>
  );
}

const cols = "lg:grid-cols-[minmax(0,2fr)_110px_minmax(0,1fr)_110px_150px]";

function Columns({ middle }: { middle: string }) {
  return (
    <div className={`hidden gap-6 border-y border-line px-10 py-4 text-sm font-medium text-muted lg:grid ${cols}`}>
      <span>Name</span>
      <span>Kind</span>
      <span>{middle}</span>
      <span>Updated</span>
      <span />
    </div>
  );
}

function UsedBy({ used, email }: { used: { id: string; name: string; workspace?: string }[]; email?: boolean }) {
  if (used.length === 0 && !email) return <span className="text-muted">Not used yet</span>;
  return (
    <>
      {used.map((r, i) => (
        <span key={r.id}>
          {i > 0 && ", "}
          <a href={`#/runbooks/${r.id}`} className="text-accent hover:underline">
            {r.name}
          </a>
          {r.workspace && <span className="text-muted"> ({r.workspace})</span>}
        </span>
      ))}
      {email && <span>{used.length > 0 && ", "}Email</span>}
    </>
  );
}

function SecretRow({ title, subtitle, kind, shared, middle, at, by, onEdit, onDelete, name }: {
  title: string;
  subtitle: string;
  kind: SecretKind;
  shared?: boolean;
  middle: ReactNode;
  at: number;
  by?: string;
  name: string;
  onEdit?: (m: Mode) => void;
  onDelete?: () => void;
}) {
  return (
    <li className={`grid grid-cols-[minmax(0,1fr)_auto] items-center gap-x-6 gap-y-2 border-b border-line py-4 text-sm last:border-0 ${cols}`}>
      <span className="flex min-w-0 items-center gap-3">
        <SecretIcon kind={kind} shared={shared} />
        <span className="min-w-0">
          <span className="block truncate font-mono text-[13px] font-medium">{title}</span>
          <span className="block truncate text-xs text-muted">{subtitle}</span>
        </span>
      </span>
      <span className="hidden lg:block">
        <Badge tone={kindOf(kind).tone} className="min-w-0">
          {kindOf(kind).label}
        </Badge>
      </span>
      <span className="hidden min-w-0 truncate lg:block">{middle}</span>
      <Updated at={at} by={by} />
      <Actions name={name} onEdit={onEdit} onDelete={onDelete} />
    </li>
  );
}

/** The workspace's own secrets, browsed by folder. */
function WorkspaceSecrets({ secrets, loading, folder, setFolder, onAdd, onEdit, onDelete }: {
  secrets: Secret[];
  loading: boolean;
  folder: string;
  setFolder: (f: string) => void;
  onAdd: () => void;
  onEdit: (s: Secret, m: Mode) => void;
  onDelete: (s: Secret) => void;
}) {
  const [q, setQ] = useState("");
  const prefix = folder ? `${folder}/` : "";
  const inFolder = secrets.filter((s) => s.name.startsWith(prefix));
  const folders = new Map<string, number>();
  const direct: Secret[] = [];
  for (const s of inFolder) {
    const rest = s.name.slice(prefix.length);
    const i = rest.indexOf("/");
    if (i >= 0) folders.set(rest.slice(0, i), (folders.get(rest.slice(0, i)) ?? 0) + 1);
    else direct.push(s);
  }
  const searching = q.trim() !== "";
  const matches = secrets.filter((s) => `${s.name} ${s.description} ${s.kind}`.toLowerCase().includes(q.toLowerCase()));
  // A folder disappears with its last secret: go up to one that still exists.
  useEffect(() => {
    if (!loading && folder && inFolder.length === 0) setFolder(folder.includes("/") ? folder.slice(0, folder.lastIndexOf("/")) : "");
  }, [loading, folder, inFolder.length, setFolder]);

  if (!loading && secrets.length === 0) {
    return (
      <Card>
        <EmptyState icon={KeyRound} title="No secrets yet" action={<Button variant="primary" onClick={onAdd}><Plus className="size-4" /> Add your first secret</Button>}>
          Store a password, API token or SSH key here, then pick it in a runbook instead of writing it into a command. Folders, like prod and staging, keep many secrets in order.
        </EmptyState>
      </Card>
    );
  }
  const crumbs = folder ? folder.split("/") : [];
  const row = (s: Secret, title: string) => (
    <SecretRow
      key={s.name}
      name={s.name}
      title={title}
      subtitle={s.description || "No description"}
      kind={s.kind}
      middle={<UsedBy used={s.used_by} />}
      at={s.updated_at}
      by={s.updated_by}
      onEdit={(m) => onEdit(s, m)}
      onDelete={() => onDelete(s)}
    />
  );
  return (
    <Card className="overflow-hidden">
      <TableHeading title="This workspace's secrets" subtitle={`${secrets.length} secret${secrets.length === 1 ? "" : "s"}`}>
        <SearchField value={q} onChange={setQ} placeholder="Search all folders" />
      </TableHeading>
      {!searching && (
        <nav aria-label="Folder" className="flex flex-wrap items-center gap-1 px-6 pb-4 text-sm sm:px-10">
          <button type="button" onClick={() => setFolder("")} className={cx("rounded-md px-1.5 py-0.5 font-medium", folder ? "text-accent hover:underline" : "text-ink")}>
            All secrets
          </button>
          {crumbs.map((c, i) => (
            <span key={i} className="flex items-center gap-1">
              <ChevronRight className="size-3.5 text-muted" />
              <button type="button" onClick={() => setFolder(crumbs.slice(0, i + 1).join("/"))} className={cx("rounded-md px-1.5 py-0.5 font-mono text-[13px]", i === crumbs.length - 1 ? "text-ink" : "text-accent hover:underline")}>
                {c}
              </button>
            </span>
          ))}
        </nav>
      )}
      <Columns middle="Used by" />
      <ul className="px-6 pb-2 sm:px-10">
        {searching ? (
          <>
            {matches.length === 0 && <li className="py-10 text-center text-sm text-muted">No secrets match.</li>}
            {matches.map((s) => row(s, s.name))}
          </>
        ) : (
          <>
            {[...folders.entries()].sort().map(([f, n]) => (
              <li key={`folder-${f}`} className="border-b border-line last:border-0">
                <button type="button" onClick={() => setFolder(prefix + f)} className="flex w-full items-center gap-3 py-4 text-left text-sm">
                  <span className="flex size-9 shrink-0 items-center justify-center rounded-full bg-surface-2 text-muted">
                    <Folder className="size-4" />
                  </span>
                  <span className="min-w-0 flex-1">
                    <span className="block truncate font-mono text-[13px] font-medium">{f}/</span>
                    <span className="block text-xs text-muted">
                      {n} secret{n === 1 ? "" : "s"}
                    </span>
                  </span>
                  <ChevronRight className="size-4 text-muted" />
                </button>
              </li>
            ))}
            {direct.map((s) => row(s, splitPath(s.name).name))}
            {loading && secrets.length === 0 && <li className="py-10 text-center text-sm text-muted">Loading...</li>}
          </>
        )}
      </ul>
    </Card>
  );
}

/** A shared secret's subtitle: how runbooks name it, then its description. */
const useAs = (reference: string, description: string) => (description ? `${reference} · ${description}` : reference);

/** Shared secrets: all of them for server admins, the ones this workspace can use for everyone else. */
function SharedSecrets({ admin, all, available, onAdd, onEdit, onDelete }: {
  admin: boolean;
  all: SharedSecret[] | undefined;
  available: Secret[];
  onAdd: () => void;
  onEdit: (s: SharedSecret, m: Mode) => void;
  onDelete: (s: SharedSecret) => void;
}) {
  const workspace = useWorkspace();
  const [q, setQ] = useState("");
  const match = (text: string) => text.toLowerCase().includes(q.toLowerCase());
  if (admin && all && all.length === 0) {
    return (
      <Card>
        <EmptyState icon={Share2} title="No shared secrets yet" action={<Button variant="primary" onClick={onAdd}><Plus className="size-4" /> Add a shared secret</Button>}>
          One value for several workspaces, like a mail relay password or a monitoring token. You choose which workspaces can use it.
        </EmptyState>
      </Card>
    );
  }
  if (!admin && available.length === 0) {
    return (
      <Card>
        <EmptyState icon={Share2} title="Nothing shared with this workspace">
          Server admins can share a secret with {workspace?.name ?? "this workspace"}. Runbooks here then use it as <code className="font-mono text-xs">shared/...</code>, without anyone here managing it.
        </EmptyState>
      </Card>
    );
  }
  return (
    <Card className="overflow-hidden">
      <TableHeading title="Shared secrets" subtitle={admin ? "Managed here by server admins; each workspace sees the ones shared with it" : `Shared with ${workspace?.name ?? "this workspace"} by server admins, read-only`}>
        <SearchField value={q} onChange={setQ} />
      </TableHeading>
      <Columns middle={admin ? "Shared with" : "Used by here"} />
      <ul className="px-6 pb-2 sm:px-10">
        {admin
          ? (all ?? [])
              .filter((s) => match(`${s.name} ${s.description} ${s.kind}`))
              .map((s) => (
                <SecretRow
                  key={s.name}
                  name={s.name}
                  title={s.name}
                  subtitle={useAs(SHARED_PREFIX + s.name, s.description)}
                  kind={s.kind}
                  shared
                  middle={
                    <>
                      <span className="block truncate">{s.all_workspaces ? "All workspaces" : s.workspaces.length ? s.workspaces.join(", ") : <span className="text-muted">No workspace yet</span>}</span>
                      <span className="block truncate text-xs">
                        <UsedBy used={s.used_by} email={s.email_password} />
                      </span>
                    </>
                  }
                  at={s.updated_at}
                  by={s.updated_by}
                  onEdit={(m) => onEdit(s, m)}
                  onDelete={() => onDelete(s)}
                />
              ))
          : available
              .filter((s) => match(`${s.name} ${s.description} ${s.kind}`))
              .map((s) => (
                <SecretRow key={s.name} name={s.name} title={s.name.slice(SHARED_PREFIX.length)} subtitle={useAs(s.name, s.description)} kind={s.kind} shared middle={<UsedBy used={s.used_by} />} at={s.updated_at} by={s.updated_by} />
              ))}
      </ul>
    </Card>
  );
}

export function VaultPage() {
  const admin = useIsServerAdmin();
  const secrets = useData(() => api.secrets(), []);
  const shared = useData(() => (admin ? api.sharedSecrets() : Promise.resolve(undefined)), [admin]);
  const workspaces = useData(() => (admin ? api.workspaces() : Promise.resolve([])), [admin]);
  const [tab, setTab] = useState<"workspace" | "shared">("workspace");
  const [folder, setFolder] = useState("");
  const [adding, setAdding] = useState(false);
  const [editing, setEditing] = useState<{ target: Target; mode: Mode } | null>(null);
  const [error, setError] = useState<string | null>(null);

  const own = (secrets.data ?? []).filter((s) => !s.shared);
  const available = (secrets.data ?? []).filter((s) => s.shared);
  const reload = () => {
    secrets.reload();
    if (admin) shared.reload();
  };

  async function remove(target: Target) {
    if (!window.confirm(`Delete the secret ${target.shared ? SHARED_PREFIX : ""}${target.s.name}? This can't be undone.`)) return;
    setError(null);
    try {
      if (target.shared) await api.deleteSharedSecret(target.s.name);
      else await api.deleteSecret(target.s.name);
      reload();
    } catch (e) {
      setError(errorText(e));
    }
  }

  const saved = () => {
    setAdding(false);
    setEditing(null);
    reload();
  };
  const canAdd = tab === "workspace" || admin;

  return (
    <div className="space-y-8">
      <PageHeader
        title="Vault"
        subtitle="Passwords, tokens and keys your runbooks need, encrypted on this server."
        actions={
          canAdd && (
            <Button variant="primary" onClick={() => setAdding(true)}>
              <Plus className="size-4" /> {tab === "shared" ? "Add shared secret" : "Add secret"}
            </Button>
          )
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
            A runbook that lists a secret, like <code className="font-mono text-xs">prod/db/DB_PASSWORD</code>, gets it as an environment variable named after its last part,{" "}
            <code className="font-mono text-xs">$DB_PASSWORD</code>, in every step. If a value shows up in a run's output, it's replaced with •••••• before anyone sees it.
          </p>
        </div>
      </Card>

      <div role="tablist" aria-label="Vault" className="inline-flex gap-1 rounded-xl bg-surface-2 p-1">
        {[
          { id: "workspace" as const, label: "This workspace" },
          { id: "shared" as const, label: `Shared${admin ? "" : available.length ? ` (${available.length})` : ""}` },
        ].map((t) => (
          <button key={t.id} type="button" role="tab" aria-selected={tab === t.id} onClick={() => setTab(t.id)} className={cx("h-9 rounded-lg px-4 text-sm font-medium", tab === t.id ? "bg-surface text-accent shadow-sm" : "text-muted hover:text-ink")}>
            {t.label}
          </button>
        ))}
      </div>

      {error && <ErrorNote>{error}</ErrorNote>}
      {secrets.error && <ErrorNote>{secrets.error}</ErrorNote>}
      {shared.error && <ErrorNote>{shared.error}</ErrorNote>}

      {tab === "workspace" ? (
        <WorkspaceSecrets
          secrets={own}
          loading={secrets.data === undefined}
          folder={folder}
          setFolder={setFolder}
          onAdd={() => setAdding(true)}
          onEdit={(s, mode) => setEditing({ target: { shared: false, s }, mode })}
          onDelete={(s) => remove({ shared: false, s })}
        />
      ) : (
        <SharedSecrets
          admin={admin}
          all={shared.data}
          available={available}
          onAdd={() => setAdding(true)}
          onEdit={(s, mode) => setEditing({ target: { shared: true, s }, mode })}
          onDelete={(s) => remove({ shared: true, s })}
        />
      )}

      {adding && <AddSecret startFolder={tab === "workspace" ? folder : ""} shared={tab === "shared"} workspaces={workspaces.data ?? []} onClose={() => setAdding(false)} onSaved={saved} />}
      {editing && <EditSecret target={editing.target} mode={editing.mode} workspaces={workspaces.data ?? []} onClose={() => setEditing(null)} onSaved={saved} />}
    </div>
  );
}
