import { useEffect, useState } from "react";
import { Activity, BookOpen, ChevronRight, KeyRound, LayoutGrid, LogOut, Moon, Plus, ScrollText, Search, Server, Settings, Sun } from "lucide-react";
import { CommandPalette } from "./components/CommandPalette";
import { Avatar, Button, cx, Kbd } from "./components/ui";
import { api, setApiWorkspace, UNAUTHORIZED_EVENT, type Me, type MyWorkspace, type Role, type SignInOptions } from "./api";
import { ROLE_LABELS, SessionContext, type Session } from "./session";
import { NoWorkspace } from "./pages/NoWorkspace";
import { ChangePassword } from "./pages/ChangePassword";
import { go, useRoute } from "./router";
import { Login, SourceLink, SOURCE_URL } from "./pages/Login";
import { Overview } from "./pages/Overview";
import { RunbooksPage } from "./pages/Runbooks";
import { RunbookDetail } from "./pages/RunbookDetail";
import { RunbookEditor } from "./pages/RunbookEditor";
import { RunsPage } from "./pages/Runs";
import { RunDetail } from "./pages/RunDetail";
import { HostsPage } from "./pages/Hosts";
import { VaultPage } from "./pages/Vault";
import { SettingsPage } from "./pages/Settings";
import { AuditLogPage } from "./pages/AuditLog";

const allNav: { id: string; label: string; icon: typeof LayoutGrid; min?: Role }[] = [
  { id: "overview", label: "Overview", icon: LayoutGrid },
  { id: "runbooks", label: "Runbooks", icon: BookOpen },
  { id: "runs", label: "Runs", icon: Activity },
  { id: "hosts", label: "Hosts", icon: Server },
  { id: "vault", label: "Vault", icon: KeyRound, min: "editor" },
  // Server admins see every workspace's log; a workspace's Admins see its own.
  { id: "audit", label: "Audit log", icon: ScrollText, min: "admin" },
  { id: "settings", label: "Settings", icon: Settings },
];

const rank = (r: Role) => ["viewer", "operator", "editor", "admin"].indexOf(r);

const WORKSPACE_KEY = "opskills-workspace";

function storedWorkspace(): string | null {
  try {
    return localStorage.getItem(WORKSPACE_KEY);
  } catch {
    return null;
  }
}

/** The workspace to show: the last one used in this browser, else Default, else the first. */
function pickWorkspace(me: Me, wanted: string | null): MyWorkspace | null {
  const find = (slug: string | null) => me.workspaces.find((w) => w.slug === slug);
  return find(wanted) ?? find(storedWorkspace()) ?? find("default") ?? me.workspaces[0] ?? null;
}

function Logo({ className = "size-9" }: { className?: string }) {
  return (
    <svg viewBox="0 0 36 36" className={className} aria-hidden>
      <path d="M18 2.5l13.4 7.75v15.5L18 33.5 4.6 25.75V10.25z" className="fill-none stroke-ink" strokeWidth="2.6" strokeLinejoin="round" />
      <path d="M12.5 18.2l3.8 3.8 7.4-8" className="fill-none stroke-accent" strokeWidth="3" strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  );
}

function useTheme() {
  const [dark, setDark] = useState(() => document.documentElement.classList.contains("dark"));
  useEffect(() => {
    document.documentElement.classList.toggle("dark", dark);
    try {
      localStorage.setItem("opskills-theme", dark ? "dark" : "light");
    } catch {
      /* storage unavailable */
    }
  }, [dark]);
  return [dark, setDark] as const;
}

export default function App() {
  const [me, setMe] = useState<Me | null | undefined>(undefined);
  const [wanted, setWanted] = useState<string | null>(null);
  const [version, setVersion] = useState("");
  const [sourceUrl, setSourceUrl] = useState(SOURCE_URL);
  // Unknown until /system answers; the sign-in page waits for it.
  const [setupDone, setSetupDone] = useState<boolean | undefined>(undefined);
  const [signIn, setSignIn] = useState<SignInOptions>({ provider: null, passwords: "everyone" });

  const loadMe = () => api.me().then(setMe, () => setMe(null));
  const loadSystem = () =>
    fetch("/api/v1/system")
      .then((r) => r.json())
      .then((s) => {
        setVersion(s.version ?? "");
        if (s.source_url) setSourceUrl(s.source_url);
        setSetupDone(s.setup_done !== false);
        if (s.sign_in) setSignIn(s.sign_in);
      })
      .catch(() => setSetupDone(true));

  useEffect(() => {
    loadMe();
    loadSystem();
    const onUnauthorized = () => {
      setMe(null);
      loadSystem();
    };
    window.addEventListener(UNAUTHORIZED_EVENT, onUnauthorized);
    return () => window.removeEventListener(UNAUTHORIZED_EVENT, onUnauthorized);
  }, []);

  if (me === undefined || setupDone === undefined) return <div className="flex min-h-full items-center justify-center text-sm text-muted">Loading...</div>;
  if (me === null) return <Login key={String(setupDone)} onSignedIn={loadMe} sourceUrl={sourceUrl} setupDone={setupDone} signIn={signIn} />;
  const signedOut = () => {
    setMe(null);
    loadSystem();
  };
  if (me.must_change_password)
    return (
      <ChangePassword
        me={me}
        onChanged={() => {
          loadMe();
          loadSystem();
        }}
        onSignOut={signedOut}
        sourceUrl={sourceUrl}
      />
    );
  const workspace = pickWorkspace(me, wanted);
  if (!workspace) return <NoWorkspace me={me} onReload={loadMe} onSignOut={signedOut} sourceUrl={sourceUrl} />;
  setApiWorkspace(workspace.slug);
  const session: Session = {
    me,
    workspace,
    switchTo: (slug, stay) => {
      try {
        localStorage.setItem(WORKSPACE_KEY, slug);
      } catch {
        /* storage unavailable */
      }
      setApiWorkspace(slug);
      setWanted(slug);
      if (!stay) go("overview");
    },
    reload: loadMe,
  };
  return (
    <SessionContext.Provider value={session}>
      <Shell session={session} version={version} sourceUrl={sourceUrl} onSignOut={signedOut} />
    </SessionContext.Provider>
  );
}

/** Switch workspaces; a plain label when there's only one. */
function WorkspaceSwitcher({ session, className }: { session: Session; className?: string }) {
  const { me, workspace } = session;
  if (me.workspaces.length < 2)
    return (
      <div className={cx("flex h-11 items-center gap-2.5 rounded-xl bg-surface-2 px-3 text-sm font-medium", className)}>
        <span className="size-2.5 shrink-0 rounded-full bg-emerald-500" />
        <span className="min-w-0 flex-1 truncate">{workspace.name}</span>
      </div>
    );
  return (
    <label className={cx("relative flex h-11 items-center gap-2.5 rounded-xl bg-surface-2 px-3 text-sm font-medium", className)}>
      <span className="size-2.5 shrink-0 rounded-full bg-emerald-500" />
      <span className="sr-only">Workspace</span>
      <select
        aria-label="Workspace"
        value={workspace.slug}
        onChange={(e) => session.switchTo(e.target.value)}
        className="min-w-0 flex-1 cursor-pointer appearance-none truncate bg-transparent pr-6 outline-none"
      >
        {me.workspaces.map((w) => (
          <option key={w.slug} value={w.slug}>
            {w.name}
          </option>
        ))}
      </select>
      <ChevronRight className="pointer-events-none absolute right-3 size-4 rotate-90 text-muted" />
    </label>
  );
}

function Shell({ session, version, sourceUrl, onSignOut }: { session: Session; version: string; sourceUrl: string; onSignOut: () => void }) {
  const { me, workspace } = session;
  const user = me.user;
  const role = workspace.role;
  const nav = allNav.filter((n) => !n.min || rank(role) >= rank(n.min));
  const canEdit = rank(role) >= rank("editor");
  const route = useRoute();
  const [dark, setDark] = useTheme();
  const [palette, setPalette] = useState(false);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        setPalette((p) => !p);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  async function signOut() {
    try {
      await api.logout();
    } finally {
      onSignOut();
    }
  }

  const [section, id, sub] = route;
  let page;
  if (section === "runbooks" && id === "new") page = <RunbookEditor template={sub} />;
  else if (section === "runbooks" && id && sub === "edit") page = <RunbookEditor id={id} />;
  else if (section === "runbooks" && id) page = <RunbookDetail id={id} />;
  else if (section === "runbooks") page = <RunbooksPage />;
  else if (section === "runs" && id) page = <RunDetail id={id} />;
  else if (section === "runs") page = <RunsPage />;
  else if (section === "hosts") page = <HostsPage />;
  else if (section === "vault") page = <VaultPage />;
  else if (section === "audit") page = <AuditLogPage />;
  else if (section === "settings") page = <SettingsPage />;
  else page = <Overview />;

  return (
    <div className="flex min-h-full">
      <aside className="sticky top-0 z-30 hidden h-screen w-[288px] shrink-0 flex-col bg-surface px-6 py-8 shadow-soft lg:flex">
        <a href="#/overview" className="flex items-center gap-2.5 px-2">
          <Logo />
          <span className="text-[26px] leading-none font-semibold tracking-tight">OpSkills</span>
          {version && <span className="mt-2 self-end text-[10px] font-medium text-muted">v{version}</span>}
        </a>

        <WorkspaceSwitcher session={session} className="mt-8" />

        <nav className="mt-6 space-y-2">
          {nav.map((n) => {
            const active = section === n.id;
            return (
              <a
                key={n.id}
                href={`#/${n.id}`}
                aria-current={active ? "page" : undefined}
                className={cx(
                  "flex h-12 items-center gap-3.5 rounded-xl px-3.5 text-sm font-medium transition-colors",
                  active ? "bg-accent text-accent-ink shadow-[0_10px_24px_-12px_var(--accent)]" : "text-muted hover:bg-surface-2 hover:text-ink",
                )}
              >
                <n.icon className="size-[22px]" strokeWidth={1.6} />
                <span className="flex-1">{n.label}</span>
                {!active && <ChevronRight className="size-4 opacity-70" />}
              </a>
            );
          })}
        </nav>

        <div className="flex-1" />

        {canEdit && (
          <div className="relative overflow-hidden rounded-[22px] bg-gradient-to-br from-fuchsia-300 via-violet-500 to-indigo-600 px-5 py-6 text-center text-white">
            <div className="pointer-events-none absolute -top-10 -right-8 size-28 rounded-full bg-white/15" />
            <div className="pointer-events-none absolute -bottom-12 -left-6 size-24 rounded-full bg-white/10" />
            <p className="relative text-sm leading-snug font-semibold">Turn a procedure into a runbook</p>
            <button onClick={() => go("runbooks/new")} className="relative mt-4 h-10 w-full rounded-full bg-white text-sm font-semibold text-violet-700 shadow-md hover:bg-violet-50">
              New runbook
            </button>
          </div>
        )}

        <div className="mt-6 flex items-center gap-3 px-2 py-1">
          <Avatar name={user} size="size-10" />
          <span className="min-w-0 flex-1">
            <span className="block truncate text-sm font-medium">{user}</span>
            <span className="block text-xs text-muted">{me.admin ? "Server admin" : ROLE_LABELS[role]}</span>
          </span>
          <button onClick={signOut} className="rounded-lg p-2 text-muted hover:bg-surface-2 hover:text-ink" aria-label="Sign out" title="Sign out">
            <LogOut className="size-4" />
          </button>
        </div>
        <SourceLink href={sourceUrl} className="mt-3 px-2" />
      </aside>

      <div className="flex min-w-0 flex-1 flex-col">
        <header className="sticky top-0 z-20 flex items-center gap-3 bg-canvas/85 px-4 py-4 backdrop-blur-md sm:px-8 lg:px-12 lg:py-6">
          <span className="lg:hidden">
            <Logo className="size-8" />
          </span>
          <h2 className="hidden min-w-0 truncate text-2xl font-medium sm:block">Hello, {user}</h2>
          <div className="flex-1" />
          <button
            onClick={() => setPalette(true)}
            className="flex h-11 min-w-0 flex-1 items-center gap-2.5 rounded-xl bg-surface px-4 text-sm text-muted shadow-soft sm:w-64 sm:flex-none"
          >
            <Search className="size-5 shrink-0" strokeWidth={1.75} />
            <span className="flex-1 truncate text-left">Search</span>
            <span className="hidden items-center gap-1 sm:flex">
              <Kbd>⌘</Kbd>
              <Kbd>K</Kbd>
            </span>
          </button>
          <Button variant="ghost" size="icon" aria-label="Toggle theme" onClick={() => setDark(!dark)} className="shrink-0 bg-surface shadow-soft">
            {dark ? <Sun className="size-[18px]" /> : <Moon className="size-[18px]" />}
          </Button>
          <span className="lg:hidden">
            <Button variant="ghost" size="icon" aria-label="Sign out" onClick={signOut} className="shrink-0 bg-surface shadow-soft">
              <LogOut className="size-[18px]" />
            </Button>
          </span>
          {canEdit && (
            <span className="hidden shrink-0 md:block">
              <Button variant="primary" onClick={() => go("runbooks/new")}>
                <Plus className="size-4" />
                New runbook
              </Button>
            </span>
          )}
        </header>

        <WorkspaceSwitcher session={session} className="mx-4 mb-2 sm:mx-8 lg:hidden" />
        {/* Pages load the current workspace's data; switching starts them fresh. */}
        <main key={workspace.slug} className="w-full max-w-[1320px] flex-1 px-4 pt-2 pb-10 sm:px-8 lg:px-12">
          {page}
        </main>
        <SourceLink href={sourceUrl} className="px-4 pb-4 text-center lg:hidden" />

        <nav className="sticky bottom-0 z-20 flex justify-around bg-surface/95 py-2 shadow-soft backdrop-blur lg:hidden">
          {nav.slice(0, 5).map((n) => (
            <a key={n.id} href={`#/${n.id}`} className={cx("flex flex-col items-center gap-0.5 rounded-xl px-3 py-1 text-[10px] font-medium", section === n.id ? "text-accent" : "text-muted")}>
              <n.icon className="size-5" strokeWidth={1.75} />
              {n.label}
            </a>
          ))}
        </nav>
      </div>

      <CommandPalette open={palette} onClose={() => setPalette(false)} />
    </div>
  );
}
