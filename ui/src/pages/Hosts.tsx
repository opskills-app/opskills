import { useEffect, useMemo, useRef, useState, type FormEvent, type ReactNode } from "react";
import { Check, CircleCheck, Copy, Download, HardDrive, Pencil, Plus, Server, ServerCog, ServerOff, Trash2, TriangleAlert } from "lucide-react";
import { api, useData, type AgentPackage, type Host } from "../api";
import { timeAgo } from "../format";
import { Badge, Button, Card, cx, ErrorNote, IconBubble, Modal, PageHeader, SearchField, Tag, TableHeading } from "../components/ui";

type Family = "deb" | "rpm";
type Arch = "amd64" | "arm64";

const families: { id: Family; label: string; detail: string; install: (file: string) => string }[] = [
  { id: "deb", label: "Debian / Ubuntu", detail: ".deb for Debian 11+, Ubuntu 20.04+", install: (f) => `sudo apt install ./${f}` },
  { id: "rpm", label: "RHEL / Rocky / Fedora", detail: ".rpm for RHEL, Rocky, Alma 8+, Fedora, Amazon Linux 2023", install: (f) => `sudo dnf install ./${f}` },
];
const archLabel: Record<Family, Record<Arch, string>> = {
  deb: { amd64: "amd64", arm64: "arm64" },
  rpm: { amd64: "x86_64", arm64: "aarch64" },
};

const field = "h-11 w-full rounded-xl bg-surface-2 px-4 text-sm outline-none focus:ring-2 focus:ring-accent";

/** The address hosts use to reach this server. */
function useServerUrl() {
  const system = useData(() => api.system(), []);
  return (system.data?.public_url || window.location.origin).replace(/\/+$/, "");
}

function isLocalAddress(url: string) {
  try {
    const h = new URL(url).hostname;
    return h === "localhost" || h === "127.0.0.1" || h === "[::1]" || h.endsWith(".localhost");
  } catch {
    return false;
  }
}

async function copyText(text: string) {
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    // Clipboard API needs HTTPS or localhost; fall back to a hidden textarea.
    const t = document.createElement("textarea");
    t.value = text;
    t.style.position = "fixed";
    t.style.opacity = "0";
    document.body.appendChild(t);
    t.select();
    document.execCommand("copy");
    t.remove();
  }
}

function CommandBlock({ lines, label }: { lines: string[]; label: string }) {
  const [copied, setCopied] = useState(false);
  const text = lines.join("\n");
  return (
    <div className="relative rounded-xl bg-term text-term-ink">
      <pre aria-label={label} className="scroll-thin overflow-x-auto px-4 py-3 pr-12 font-mono text-[12px] leading-6 whitespace-pre">
        {lines.map((l, i) => (
          <div key={i}>
            <span className="text-white/35 select-none">$ </span>
            {l}
          </div>
        ))}
      </pre>
      <button
        type="button"
        onClick={async () => {
          await copyText(text);
          setCopied(true);
          setTimeout(() => setCopied(false), 1500);
        }}
        className="absolute top-2 right-2 rounded-lg bg-term p-2 text-white/60 shadow-[-8px_0_8px_var(--term)] hover:text-white"
        aria-label={`Copy ${label}`}
        title="Copy"
      >
        {copied ? <Check className="size-4" /> : <Copy className="size-4" />}
      </button>
    </div>
  );
}

function Segmented<T extends string>({ value, options, onChange, label }: { value: T; options: { id: T; label: string }[]; onChange: (v: T) => void; label: string }) {
  return (
    <div role="radiogroup" aria-label={label} className="flex rounded-xl bg-surface-2 p-1">
      {options.map((o) => (
        <button
          key={o.id}
          type="button"
          role="radio"
          aria-checked={value === o.id}
          onClick={() => onChange(o.id)}
          className={cx("h-9 flex-1 rounded-lg px-3 text-xs font-medium whitespace-nowrap", value === o.id ? "bg-surface text-accent shadow-sm" : "text-muted hover:text-ink")}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}

const shellQuote = (s: string) => (/^[\w.,@%+=:/-]+$/.test(s) ? s : `'${s.replace(/'/g, `'\\''`)}'`);

function Numbered({ n, title, children }: { n: number; title: string; children: ReactNode }) {
  return (
    <div className="grid grid-cols-[28px_minmax(0,1fr)] gap-3">
      <span className="flex size-7 items-center justify-center rounded-full bg-accent-soft text-xs font-semibold text-accent">{n}</span>
      <div className="min-w-0">
        <p className="pt-1 text-sm font-medium">{title}</p>
        <div className="mt-2 space-y-2">{children}</div>
      </div>
    </div>
  );
}

function AddHost({ packages, serverUrl, onClose, onEnrolled }: { packages: AgentPackage[]; serverUrl: string; onClose: () => void; onEnrolled: () => void }) {
  const [family, setFamily] = useState<Family>("deb");
  const [arch, setArch] = useState<Arch>("amd64");
  const [name, setName] = useState("");
  const [tags, setTags] = useState("");
  const [token, setToken] = useState<{ token: string; expires_at: number } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [joined, setJoined] = useState<Host | null>(null);
  const opened = useRef(Date.now());

  async function newToken() {
    setError(null);
    setJoined(null);
    opened.current = Date.now();
    try {
      setToken(await api.createEnrollmentToken());
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }

  useEffect(() => {
    newToken();
  }, []);

  // Watch for the new host to connect.
  useEffect(() => {
    if (!token || joined) return;
    const t = setInterval(async () => {
      try {
        const hosts = await api.hosts();
        const found = hosts.find((h) => !h.builtin && h.enrolled_at >= opened.current - 5000);
        if (found) {
          setJoined(found);
          onEnrolled();
        }
      } catch {
        /* keep waiting */
      }
    }, 2000);
    return () => clearInterval(t);
  }, [token, joined, onEnrolled]);

  const pkg = packages.find((p) => p.format === family && p.arch === arch);
  const fam = families.find((f) => f.id === family)!;
  const tagList = tags
    .split(",")
    .map((t) => t.trim())
    .filter(Boolean);
  const flags = [name.trim() ? ` --name ${shellQuote(name.trim())}` : "", tagList.length ? ` --tags ${shellQuote(tagList.join(","))}` : ""].join("");
  const oneLine = `curl -fsSL ${serverUrl}/install-agent.sh | sudo sh -s -- --server ${serverUrl} --token ${token?.token ?? "..."}${flags}`;
  const enroll = [
    `sudo opskills-agent enroll --server ${serverUrl} --token ${token?.token ?? "..."}`,
    name.trim() ? ` --name ${shellQuote(name.trim())}` : "",
    tagList.length ? ` --tags ${shellQuote(tagList.join(","))}` : "",
  ].join("");

  return (
    <Modal
      title="Add a host"
      wide
      onClose={onClose}
      footer={
        joined ? (
          <>
            <Button type="button" onClick={newToken}>
              Add another host
            </Button>
            <Button variant="primary" type="button" onClick={onClose}>
              Done
            </Button>
          </>
        ) : (
          <Button type="button" onClick={onClose}>
            Close
          </Button>
        )
      }
    >
      <div className="space-y-6">
        <p className="text-sm text-muted">
          Install the OpSkills agent on the machine and enroll it with a one-time token. The agent connects out to this server, so the host needs no open ports and no SSH keys.
        </p>
        {error && <ErrorNote>{error}</ErrorNote>}
        {isLocalAddress(serverUrl) && (
          <div className="flex items-start gap-3 rounded-xl border border-amber-500/40 bg-amber-500/10 px-4 py-3 text-sm text-ink-2">
            <TriangleAlert className="mt-0.5 size-4 shrink-0 text-amber-600" />
            <span>
              Other machines can't reach <b>{serverUrl}</b>. Replace it in the commands with an address they can reach, or set <code className="font-mono text-xs">server.public_url</code> in the
              OpSkills config.
            </span>
          </div>
        )}
        {!isLocalAddress(serverUrl) && serverUrl.startsWith("http://") && (
          <div className="flex items-start gap-3 rounded-xl border border-amber-500/40 bg-amber-500/10 px-4 py-3 text-sm text-ink-2">
            <TriangleAlert className="mt-0.5 size-4 shrink-0 text-amber-600" />
            <span>This server isn't using HTTPS, so steps, their output and secrets cross the network unencrypted. Put OpSkills behind HTTPS for hosts outside a trusted network.</span>
          </div>
        )}

        <Numbered n={1} title="Pick the host's system">
          <div className="grid gap-2 sm:grid-cols-[minmax(0,1.5fr)_minmax(0,1fr)]">
            <Segmented label="Package type" value={family} onChange={setFamily} options={families.map((f) => ({ id: f.id, label: f.label }))} />
            <Segmented
              label="Architecture"
              value={arch}
              onChange={setArch}
              options={[
                { id: "amd64" as Arch, label: archLabel[family].amd64 },
                { id: "arm64" as Arch, label: archLabel[family].arm64 },
              ]}
            />
          </div>
          <p className="text-xs text-muted">{fam.detail}. Check with <code className="font-mono">uname -m</code>.</p>
        </Numbered>

        <Numbered n={2} title="Download and install the agent">
          {pkg && <CommandBlock label="install commands" lines={[`curl -fLO ${serverUrl}/downloads/${pkg.file}`, fam.install(pkg.file)]} />}
          {pkg && (
            <a href={`/downloads/${pkg.file}`} download className="inline-flex items-center gap-1.5 text-xs font-medium text-accent hover:underline">
              <Download className="size-3.5" /> Or download {pkg.file}
            </a>
          )}
        </Numbered>

        <Numbered n={3} title="Enroll it and start the agent">
          <div className="grid gap-3 sm:grid-cols-2">
            <div>
              <label className="text-xs font-medium text-muted" htmlFor="host-name">
                Name <span className="font-normal">(optional, default: its hostname)</span>
              </label>
              <input id="host-name" value={name} onChange={(e) => setName(e.target.value)} placeholder="web-1" className={`mt-1.5 ${field}`} />
            </div>
            <div>
              <label className="text-xs font-medium text-muted" htmlFor="host-tags">
                Tags <span className="font-normal">(optional, comma separated)</span>
              </label>
              <input id="host-tags" value={tags} onChange={(e) => setTags(e.target.value)} placeholder="web, prod" className={`mt-1.5 ${field}`} />
            </div>
          </div>
          <CommandBlock label="enroll commands" lines={[enroll, "sudo systemctl enable --now opskills-agent"]} />
          {token && (
            <p className="text-xs text-muted">
              The token works once and expires {new Date(token.expires_at).toLocaleString([], { dateStyle: "medium", timeStyle: "short" })}.{" "}
              <button type="button" onClick={newToken} className="font-medium text-accent hover:underline">
                New token
              </button>
            </p>
          )}
        </Numbered>

        <details className="group rounded-xl bg-surface-2 px-4 py-3 text-sm">
          <summary className="cursor-pointer font-medium text-ink-2 marker:text-muted">Or do steps 2 and 3 in one line</summary>
          <div className="mt-3 space-y-2">
            <p className="text-xs text-muted">Picks the package for the host's system and architecture, installs it, enrolls the host and starts the agent.</p>
            <CommandBlock label="one-line install" lines={[oneLine]} />
          </div>
        </details>

        <div className={cx("flex items-center gap-3 rounded-xl px-4 py-3 text-sm", joined ? "bg-emerald-500/10 text-emerald-700 dark:text-emerald-300" : "bg-surface-2 text-muted")} role="status">
          {joined ? (
            <>
              <CircleCheck className="size-5 shrink-0" />
              <span>
                <b>{joined.name}</b> is enrolled{joined.os ? ` (${joined.os}, ${joined.arch})` : ""}. Pick it under <b>Run on</b> in a runbook.
              </span>
            </>
          ) : (
            <>
              <span className="live-dot size-2 shrink-0 rounded-full bg-sky-500 text-sky-500" />
              Waiting for the host to enroll...
            </>
          )}
        </div>
      </div>
    </Modal>
  );
}

function EditHost({ host, onClose, onSaved }: { host: Host; onClose: () => void; onSaved: () => void }) {
  const [name, setName] = useState(host.name);
  const [tags, setTags] = useState(host.tags.join(", "));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function save(e: FormEvent) {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      await api.updateHost(host.id, {
        name,
        tags: tags
          .split(",")
          .map((t) => t.trim())
          .filter(Boolean),
      });
      onSaved();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Modal
      title={`Edit ${host.name}`}
      onClose={onClose}
      footer={
        <>
          <Button type="button" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" type="submit" form="edit-host" disabled={busy || !name.trim()}>
            {busy ? "Saving..." : "Save"}
          </Button>
        </>
      }
    >
      <form id="edit-host" onSubmit={save} className="space-y-4">
        {error && <ErrorNote>{error}</ErrorNote>}
        <div>
          <label className="text-sm font-medium" htmlFor="edit-host-name">
            Name
          </label>
          <input id="edit-host-name" autoFocus value={name} onChange={(e) => setName(e.target.value)} className={`mt-2 ${field}`} />
        </div>
        <div>
          <label className="text-sm font-medium" htmlFor="edit-host-tags">
            Tags
          </label>
          <input id="edit-host-tags" value={tags} onChange={(e) => setTags(e.target.value)} placeholder="web, prod" className={`mt-2 ${field}`} />
          <p className="mt-1.5 text-xs text-muted">Runbooks can target every host with a tag. Separate tags with commas.</p>
        </div>
      </form>
    </Modal>
  );
}

function Downloads({ version, packages }: { version: string; packages: AgentPackage[] }) {
  return (
    <Card className="p-6 sm:p-8">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h3 className="text-[19px] font-semibold">Download the agent</h3>
          <p className="mt-1 text-sm text-muted">Version {version}, the same as this server. Static binary, no dependencies, runs as a systemd service.</p>
        </div>
      </div>
      <div className="mt-5 grid grid-cols-[minmax(0,1fr)] gap-4 md:grid-cols-[repeat(2,minmax(0,1fr))]">
        {families.map((f) => (
          <div key={f.id} className="rounded-2xl bg-surface-2 p-5">
            <div className="flex items-center gap-3">
              <span className="flex size-10 items-center justify-center rounded-xl bg-surface text-accent">
                <HardDrive className="size-5" strokeWidth={1.75} />
              </span>
              <div className="min-w-0">
                <p className="text-sm font-semibold">{f.label}</p>
                <p className="truncate text-xs text-muted">{f.detail}</p>
              </div>
            </div>
            <div className="mt-4 flex flex-wrap gap-2">
              {(["amd64", "arm64"] as Arch[]).map((a) => {
                const p = packages.find((x) => x.format === f.id && x.arch === a);
                return (
                  p && (
                    <a
                      key={a}
                      href={`/downloads/${p.file}`}
                      download
                      title={p.file}
                      className="inline-flex h-9 items-center gap-1.5 rounded-lg bg-surface px-3 text-xs font-medium text-ink-2 shadow-sm hover:text-accent"
                    >
                      <Download className="size-3.5" /> .{f.id} {archLabel[f.id][a]}
                    </a>
                  )
                );
              })}
            </div>
          </div>
        ))}
      </div>
    </Card>
  );
}

export function HostsPage() {
  const hosts = useData(() => api.hosts(), [], 5000);
  const pkgs = useData(() => api.agentPackages(), []);
  const serverUrl = useServerUrl();
  const [q, setQ] = useState("");
  const [adding, setAdding] = useState(false);
  const [editing, setEditing] = useState<Host | null>(null);
  const [error, setError] = useState<string | null>(null);

  const all = useMemo(() => hosts.data ?? [], [hosts.data]);
  const enrolled = all.filter((h) => !h.builtin);
  const online = enrolled.filter((h) => h.online).length;
  const list = all.filter((h) => `${h.name} ${h.hostname} ${h.os} ${h.arch} ${h.tags.join(" ")}`.toLowerCase().includes(q.toLowerCase()));
  const reload = hosts.reload;

  async function remove(h: Host) {
    if (!window.confirm(`Remove ${h.name}? Its agent loses access right away and stops. To add it back, enroll it again.`)) return;
    setError(null);
    try {
      await api.deleteHost(h.id);
      reload();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }

  return (
    <div className="space-y-8">
      <PageHeader
        title="Hosts"
        subtitle="Machines your runbooks run on. Each one runs the OpSkills agent, which connects to this server."
        actions={
          <Button variant="primary" onClick={() => setAdding(true)}>
            <Plus className="size-4" /> Add host
          </Button>
        }
      />

      <Card className="grid grid-cols-1 gap-4 px-4 py-6 md:grid-cols-3 md:divide-x md:divide-line md:px-6 md:py-8">
        {[
          { icon: Server, label: "Enrolled hosts", value: enrolled.length, tone: "mint" as const, note: "plus the OpSkills server" },
          { icon: ServerCog, label: "Online", value: online, tone: "mint" as const, note: "agent in touch in the last minute" },
          { icon: ServerOff, label: "Offline", value: enrolled.length - online, tone: enrolled.length - online ? ("pink" as const) : ("mint" as const), note: "steps for them fail" },
        ].map((k) => (
          <div key={k.label} className="flex items-center gap-5 px-2 py-2 sm:px-6">
            <IconBubble icon={k.icon} tone={k.tone} size="size-[72px] sm:size-20" />
            <div className="min-w-0">
              <div className="text-sm text-muted">{k.label}</div>
              <div className="mt-0.5 text-[30px] leading-tight font-semibold tabular-nums">{hosts.data ? k.value : ""}</div>
              <div className="text-xs text-ink-2">{k.note}</div>
            </div>
          </div>
        ))}
      </Card>

      {error && <ErrorNote>{error}</ErrorNote>}
      {hosts.error && <ErrorNote>{hosts.error}</ErrorNote>}

      <Card className="overflow-hidden">
        <TableHeading title="All hosts" subtitle={enrolled.length ? `${enrolled.length} enrolled, ${online} online` : "Add a host to run runbooks on other machines"}>
          <SearchField value={q} onChange={setQ} />
        </TableHeading>
        <div className="hidden grid-cols-[minmax(0,1.3fr)_minmax(0,1.1fr)_minmax(0,0.8fr)_100px_110px_96px] gap-6 border-y border-line px-10 py-4 text-sm font-medium text-muted lg:grid">
          <span>Host</span>
          <span>System</span>
          <span>Tags</span>
          <span>Last seen</span>
          <span className="text-center">Status</span>
          <span />
        </div>
        <ul className="px-6 pb-2 sm:px-10">
          {list.length === 0 && <li className="py-10 text-center text-sm text-muted">{hosts.loading ? "Loading..." : "No hosts match."}</li>}
          {list.map((h) => (
            <li key={h.id} className="grid grid-cols-[minmax(0,1fr)_auto] items-center gap-x-6 gap-y-2 border-b border-line py-4 text-sm last:border-0 lg:grid-cols-[minmax(0,1.3fr)_minmax(0,1.1fr)_minmax(0,0.8fr)_100px_110px_96px]">
              <span className="flex min-w-0 items-center gap-3">
                <span className={cx("flex size-9 shrink-0 items-center justify-center rounded-full", h.builtin ? "bg-accent text-accent-ink" : "bg-accent-soft text-accent")}>
                  <Server className="size-4" />
                </span>
                <span className="min-w-0">
                  <span className="block truncate font-medium">{h.name}</span>
                  <span className="block truncate text-xs text-muted">
                    {h.builtin ? `Built in · ${h.hostname}` : h.hostname}
                    <span className="lg:hidden"> · {h.os}</span>
                  </span>
                </span>
              </span>
              <span className="hidden min-w-0 lg:block">
                <span className="block truncate">{h.os || "Unknown"}</span>
                <span className="block truncate text-xs text-muted">
                  {h.arch} · agent {h.agent_version}
                </span>
              </span>
              <span className="hidden min-w-0 flex-wrap gap-1.5 lg:flex">
                {h.tags.length ? h.tags.map((t) => <Tag key={t}>{t}</Tag>) : <span className="text-xs text-muted">{h.builtin ? "" : "No tags"}</span>}
              </span>
              <span className="hidden text-muted lg:block">{h.builtin || h.online ? "Now" : timeAgo(h.last_seen)}</span>
              <span className="text-center">
                <Badge tone={h.online ? "green" : "red"}>{h.online ? "Online" : "Offline"}</Badge>
              </span>
              <span className="col-span-2 flex justify-end gap-1 lg:col-span-1">
                {!h.builtin && (
                  <>
                    <Button size="sm" variant="ghost" onClick={() => setEditing(h)} aria-label={`Edit ${h.name}`} title="Rename or tag">
                      <Pencil className="size-3.5" />
                    </Button>
                    <Button size="sm" variant="ghost" onClick={() => remove(h)} aria-label={`Remove ${h.name}`} title="Remove" className="hover:text-rose-600">
                      <Trash2 className="size-3.5" />
                    </Button>
                  </>
                )}
              </span>
            </li>
          ))}
        </ul>
      </Card>

      {pkgs.data && <Downloads version={pkgs.data.version} packages={pkgs.data.packages} />}

      {adding && pkgs.data && <AddHost packages={pkgs.data.packages} serverUrl={serverUrl} onClose={() => setAdding(false)} onEnrolled={reload} />}
      {editing && (
        <EditHost
          host={editing}
          onClose={() => setEditing(null)}
          onSaved={() => {
            setEditing(null);
            reload();
          }}
        />
      )}
    </div>
  );
}
