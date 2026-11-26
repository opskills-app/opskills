import { useState, type FormEvent } from "react";
import { Eye, EyeOff, LogIn } from "lucide-react";
import { api } from "../api";
import { Button, ErrorNote } from "../components/ui";

function Logo() {
  return (
    <svg viewBox="0 0 36 36" className="size-11" aria-hidden>
      <path d="M18 2.5l13.4 7.75v15.5L18 33.5 4.6 25.75V10.25z" className="fill-none stroke-ink" strokeWidth="2.6" strokeLinejoin="round" />
      <path d="M12.5 18.2l3.8 3.8 7.4-8" className="fill-none stroke-accent" strokeWidth="3" strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  );
}

/** Where to get this program's source (AGPL-3.0, section 13). The server sends the exact version's link. */
export const SOURCE_URL = "https://github.com/opskills-app/opskills";

export function SourceLink({ href, className = "" }: { href: string; className?: string }) {
  return (
    <p className={`text-[11px] text-muted ${className}`}>
      <a className="font-medium hover:text-ink" href={href} target="_blank" rel="noreferrer">
        Source code
      </a>{" "}
      · AGPL-3.0
    </p>
  );
}

export function Login({ onSignedIn, sourceUrl }: { onSignedIn: (user: string) => void; sourceUrl: string }) {
  const [username, setUsername] = useState("admin");
  const [password, setPassword] = useState("");
  const [show, setShow] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function submit(e: FormEvent) {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      const res = await api.login(username, password);
      onSignedIn(res.user);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Sign-in failed.");
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="flex min-h-full items-center justify-center px-4 py-12">
      <div className="w-full max-w-[420px]">
        <div className="mb-8 flex items-center justify-center gap-3">
          <Logo />
          <span className="text-[30px] leading-none font-semibold tracking-tight">OpSkills</span>
        </div>
        <form onSubmit={submit} className="rounded-[28px] bg-surface p-8 shadow-soft">
          <h1 className="text-[22px] font-semibold tracking-tight">Sign in</h1>
          <p className="mt-1 text-sm text-muted">Use the admin account created on first start.</p>

          <label className="mt-6 block text-sm font-medium" htmlFor="username">
            Username
          </label>
          <input
            id="username"
            autoComplete="username"
            value={username}
            onChange={(e) => setUsername(e.target.value)}
            className="mt-2 h-11 w-full rounded-xl bg-surface-2 px-4 text-sm outline-none focus:ring-2 focus:ring-accent"
          />

          <label className="mt-4 block text-sm font-medium" htmlFor="password">
            Password
          </label>
          <div className="mt-2 flex h-11 items-center rounded-xl bg-surface-2 pr-2 focus-within:ring-2 focus-within:ring-accent">
            <input
              id="password"
              type={show ? "text" : "password"}
              autoComplete="current-password"
              autoFocus
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              className="h-full min-w-0 flex-1 bg-transparent px-4 font-mono text-sm outline-none"
            />
            <button type="button" onClick={() => setShow((s) => !s)} className="rounded-lg p-1.5 text-muted hover:text-ink" aria-label={show ? "Hide password" : "Show password"}>
              {show ? <EyeOff className="size-4" /> : <Eye className="size-4" />}
            </button>
          </div>

          {error && (
            <div className="mt-4">
              <ErrorNote>{error}</ErrorNote>
            </div>
          )}

          <Button variant="primary" type="submit" disabled={busy || !password} className="mt-6 w-full">
            <LogIn className="size-4" />
            {busy ? "Signing in..." : "Sign in"}
          </Button>
        </form>

        <div className="mt-6 rounded-2xl bg-surface/70 p-5 text-xs leading-relaxed text-muted">
          <p className="font-medium text-ink-2">Where is the password?</p>
          <p className="mt-1">It was printed once in the server log on first start:</p>
          <pre className="mt-2 overflow-x-auto rounded-lg bg-surface-2 p-3 font-mono text-[11px] text-ink-2">
            {"docker logs opskills 2>&1 | grep 'Admin password'\nsudo journalctl -u opskills | grep 'Admin password'"}
          </pre>
          <p className="mt-2">
            Lost it? See{" "}
            <a className="font-medium text-accent" href="https://github.com/opskills-app/opskills/blob/main/docs/install.md#troubleshooting" target="_blank" rel="noreferrer">
              resetting the admin password
            </a>
            .
          </p>
        </div>
        <SourceLink href={sourceUrl} className="mt-6 text-center" />
      </div>
    </div>
  );
}
