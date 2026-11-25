import { useEffect, useState } from "react";
import {
  BookOpen,
  ChevronsUpDown,
  KeyRound,
  LayoutGrid,
  Moon,
  Play,
  Search,
  Server,
  Settings,
  Sun,
  Bell,
  Activity,
  Check,
} from "lucide-react";
import { CommandPalette } from "./components/CommandPalette";
import { Avatar, Button, cx, Kbd } from "./components/ui";
import { workspaces } from "./data";
import { go, useRoute } from "./router";
import { Overview } from "./pages/Overview";
import { RunbooksPage } from "./pages/Runbooks";
import { RunbookDetail } from "./pages/RunbookDetail";
import { RunsPage } from "./pages/Runs";
import { RunDetail } from "./pages/RunDetail";
import { HostsPage } from "./pages/Hosts";
import { VaultPage } from "./pages/Vault";
import { SettingsPage } from "./pages/Settings";

const nav = [
  { id: "overview", label: "Overview", icon: LayoutGrid },
  { id: "runbooks", label: "Runbooks", icon: BookOpen },
  { id: "runs", label: "Runs", icon: Activity },
  { id: "hosts", label: "Hosts", icon: Server },
  { id: "vault", label: "Vault", icon: KeyRound },
];

function Logo() {
  return (
    <svg viewBox="0 0 32 32" className="size-8" aria-label="OpSkills">
      <rect width="32" height="32" rx="9" className="fill-accent" />
      <path d="M9 16.5l4.5 4.5L23 11" className="stroke-accent-ink" strokeWidth="3.2" fill="none" strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  );
}

function useTheme() {
  const [dark, setDark] = useState(() => {
    const forced = new URLSearchParams(window.location.search).get("theme");
    if (forced === "dark" || forced === "light") return forced === "dark";
    try {
      const saved = localStorage.getItem("opskills-theme");
      if (saved) return saved === "dark";
    } catch {
      /* storage unavailable */
    }
    return window.matchMedia("(prefers-color-scheme: dark)").matches;
  });
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
  const route = useRoute();
  const [dark, setDark] = useTheme();
  const [palette, setPalette] = useState(false);
  const [wsOpen, setWsOpen] = useState(false);
  const [ws, setWs] = useState(workspaces[0]);

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

  const [section, id] = route;
  let page;
  if (section === "runbooks" && id) page = <RunbookDetail id={id} />;
  else if (section === "runbooks") page = <RunbooksPage />;
  else if (section === "runs" && id) page = <RunDetail id={id} />;
  else if (section === "runs") page = <RunsPage />;
  else if (section === "hosts") page = <HostsPage />;
  else if (section === "vault") page = <VaultPage />;
  else if (section === "settings") page = <SettingsPage />;
  else page = <Overview workspace={ws.name} />;

  return (
    <div className="flex min-h-full">
      {/* Icon rail */}
      <aside className="sticky top-0 z-30 hidden h-screen w-16 shrink-0 flex-col items-center gap-1 border-r border-line bg-surface py-3 md:flex">
        <a href="#/overview" className="mb-3" aria-label="OpSkills home">
          <Logo />
        </a>
        {nav.map((n) => {
          const active = section === n.id;
          return (
            <a
              key={n.id}
              href={`#/${n.id}`}
              className={cx(
                "group relative flex size-10 items-center justify-center rounded-xl transition-colors",
                active ? "bg-accent-soft text-accent" : "text-muted hover:bg-surface-2 hover:text-ink",
              )}
              aria-label={n.label}
              aria-current={active ? "page" : undefined}
            >
              <n.icon className="size-[18px]" strokeWidth={active ? 2.25 : 1.75} />
              <span className="pointer-events-none absolute left-12 z-40 rounded-md bg-ink px-2 py-1 text-xs font-medium whitespace-nowrap text-canvas opacity-0 shadow-lg transition-opacity group-hover:opacity-100">
                {n.label}
              </span>
            </a>
          );
        })}
        <div className="flex-1" />
        <a
          href="#/settings"
          aria-label="Settings"
          className={cx(
            "flex size-10 items-center justify-center rounded-xl",
            section === "settings" ? "bg-accent-soft text-accent" : "text-muted hover:bg-surface-2 hover:text-ink",
          )}
        >
          <Settings className="size-[18px]" strokeWidth={1.75} />
        </a>
        <button className="mt-1" aria-label="Account">
          <Avatar name="Maya Chen" size="size-8" />
        </button>
      </aside>

      <div className="flex min-w-0 flex-1 flex-col">
        {/* Top bar */}
        <header className="sticky top-0 z-20 flex h-14 items-center gap-3 border-b border-line bg-canvas/80 px-4 backdrop-blur-md md:px-6">
          <div className="md:hidden">
            <Logo />
          </div>
          <div className="relative">
            <button
              onClick={() => setWsOpen((o) => !o)}
              className="flex items-center gap-2 rounded-lg px-2 py-1.5 text-sm font-medium hover:bg-surface-2"
              aria-haspopup="menu"
              aria-expanded={wsOpen}
            >
              <span className="size-2.5 rounded-sm" style={{ background: ws.color }} />
              {ws.name}
              <ChevronsUpDown className="size-3.5 text-muted" />
            </button>
            {wsOpen && (
              <div role="menu" className="absolute left-0 mt-1 w-56 rounded-xl border border-line bg-surface p-1 shadow-xl" onMouseLeave={() => setWsOpen(false)}>
                <div className="px-2 py-1.5 text-[11px] font-medium tracking-wide text-muted uppercase">Workspaces</div>
                {workspaces.map((w) => (
                  <button
                    key={w.id}
                    role="menuitem"
                    onClick={() => {
                      setWs(w);
                      setWsOpen(false);
                    }}
                    className="flex w-full items-center gap-2 rounded-lg px-2 py-1.5 text-sm hover:bg-surface-2"
                  >
                    <span className="size-2.5 rounded-sm" style={{ background: w.color }} />
                    <span className="flex-1 text-left">{w.name}</span>
                    {w.id === ws.id && <Check className="size-3.5 text-accent" />}
                  </button>
                ))}
              </div>
            )}
          </div>

          <button
            onClick={() => setPalette(true)}
            className="mx-auto flex h-9 min-w-0 flex-1 items-center gap-2 rounded-xl border border-line bg-surface px-3 text-sm text-muted shadow-[0_1px_2px_rgb(0_0_0/0.04)] hover:border-line-strong md:max-w-md"
          >
            <Search className="size-4 shrink-0" />
            <span className="flex-1 truncate text-left">
              Search<span className="hidden sm:inline"> or jump to...</span>
            </span>
            <span className="hidden items-center gap-1 sm:flex">
              <Kbd>⌘</Kbd>
              <Kbd>K</Kbd>
            </span>
          </button>

          <div className="flex shrink-0 items-center gap-1">
            <span className="hidden sm:block">
              <Button variant="ghost" size="icon" aria-label="Notifications">
                <Bell className="size-4" />
              </Button>
            </span>
            <Button variant="ghost" size="icon" aria-label="Toggle theme" onClick={() => setDark(!dark)}>
              {dark ? <Sun className="size-4" /> : <Moon className="size-4" />}
            </Button>
            <span className="ml-1 hidden sm:block">
              <Button variant="primary" onClick={() => go("runs/r-2741")}>
                <Play className="size-3.5" fill="currentColor" />
                New run
              </Button>
            </span>
          </div>
        </header>

        <main className="mx-auto w-full max-w-[1400px] flex-1 px-4 py-6 md:px-8 md:py-8">{page}</main>

        {/* Bottom nav on phones */}
        <nav className="sticky bottom-0 z-20 flex justify-around border-t border-line bg-surface/95 py-1.5 backdrop-blur md:hidden">
          {nav.map((n) => (
            <a key={n.id} href={`#/${n.id}`} className={cx("flex flex-col items-center gap-0.5 px-2 py-1 text-[10px]", section === n.id ? "text-accent" : "text-muted")}>
              <n.icon className="size-5" />
              {n.label}
            </a>
          ))}
        </nav>
      </div>

      <CommandPalette open={palette} onClose={() => setPalette(false)} />
    </div>
  );
}
