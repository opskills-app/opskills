import { useEffect, useState } from "react";
import {
  Activity,
  BookOpen,
  Check,
  ChevronDown,
  ChevronRight,
  KeyRound,
  LayoutGrid,
  Moon,
  Play,
  Search,
  Server,
  Settings,
  Sun,
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
  { id: "settings", label: "Settings", icon: Settings },
];

function Logo({ className = "size-9" }: { className?: string }) {
  return (
    <svg viewBox="0 0 36 36" className={className} aria-hidden>
      <path d="M18 2.5l13.4 7.75v15.5L18 33.5 4.6 25.75V10.25z" className="fill-none stroke-ink" strokeWidth="2.6" strokeLinejoin="round" />
      <path d="M12.5 18.2l3.8 3.8 7.4-8" className="fill-none stroke-accent" strokeWidth="3" strokeLinecap="round" strokeLinejoin="round" />
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
      {/* Sidebar */}
      <aside className="sticky top-0 z-30 hidden h-screen w-[288px] shrink-0 flex-col bg-surface px-6 py-8 shadow-soft lg:flex">
        <a href="#/overview" className="flex items-center gap-2.5 px-2">
          <Logo />
          <span className="text-[26px] leading-none font-semibold tracking-tight">OpSkills</span>
          <span className="mt-2 self-end text-[10px] font-medium text-muted">v0.1</span>
        </a>

        <div className="relative mt-8">
          <button
            onClick={() => setWsOpen((o) => !o)}
            className="flex h-11 w-full items-center gap-2.5 rounded-xl bg-surface-2 px-3 text-sm font-medium"
            aria-haspopup="menu"
            aria-expanded={wsOpen}
          >
            <span className="size-2.5 rounded-full" style={{ background: ws.color }} />
            <span className="flex-1 text-left">{ws.name}</span>
            <ChevronDown className="size-4 text-muted" />
          </button>
          {wsOpen && (
            <div role="menu" className="absolute inset-x-0 z-40 mt-2 rounded-2xl bg-surface p-1.5 shadow-soft ring-1 ring-line" onMouseLeave={() => setWsOpen(false)}>
              {workspaces.map((w) => (
                <button
                  key={w.id}
                  role="menuitem"
                  onClick={() => {
                    setWs(w);
                    setWsOpen(false);
                  }}
                  className="flex w-full items-center gap-2.5 rounded-xl px-3 py-2 text-sm hover:bg-surface-2"
                >
                  <span className="size-2.5 rounded-full" style={{ background: w.color }} />
                  <span className="flex-1 text-left">{w.name}</span>
                  {w.id === ws.id && <Check className="size-4 text-accent" />}
                </button>
              ))}
            </div>
          )}
        </div>

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

        <div className="relative overflow-hidden rounded-[22px] bg-gradient-to-br from-fuchsia-300 via-violet-500 to-indigo-600 px-5 py-6 text-center text-white">
          <div className="pointer-events-none absolute -top-10 -right-8 size-28 rounded-full bg-white/15" />
          <div className="pointer-events-none absolute -bottom-12 -left-6 size-24 rounded-full bg-white/10" />
          <p className="relative text-sm leading-snug font-semibold">1 run is waiting for your approval</p>
          <button
            onClick={() => go("runs/r-2739")}
            className="relative mt-4 h-10 w-full rounded-full bg-white text-sm font-semibold text-violet-700 shadow-md hover:bg-violet-50"
          >
            Review now
          </button>
        </div>

        <button className="mt-6 flex items-center gap-3 rounded-xl px-2 py-1 text-left hover:bg-surface-2">
          <Avatar name="Maya Chen" size="size-10" />
          <span className="min-w-0 flex-1">
            <span className="block truncate text-sm font-medium">Maya Chen</span>
            <span className="block text-xs text-muted">Platform lead</span>
          </span>
          <ChevronDown className="size-4 text-muted" />
        </button>
      </aside>

      <div className="flex min-w-0 flex-1 flex-col">
        {/* Header */}
        <header className="sticky top-0 z-20 flex items-center gap-3 bg-canvas/85 px-4 py-4 backdrop-blur-md sm:px-8 lg:px-12 lg:py-6">
          <span className="lg:hidden">
            <Logo className="size-8" />
          </span>
          <h2 className="hidden min-w-0 truncate text-2xl font-medium sm:block">
            Hello, Maya
          </h2>
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
          <span className="hidden shrink-0 md:block">
            <Button variant="primary" onClick={() => go("runs/r-2741")}>
              <Play className="size-3.5" fill="currentColor" />
              New run
            </Button>
          </span>
        </header>

        <main className="w-full max-w-[1320px] flex-1 px-4 pt-2 pb-10 sm:px-8 lg:px-12">{page}</main>

        {/* Bottom nav below the sidebar breakpoint */}
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
