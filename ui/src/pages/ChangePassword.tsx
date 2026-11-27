import { useState, type FormEvent } from "react";
import { KeyRound, LogOut } from "lucide-react";
import { api, type Me } from "../api";
import { Button, ErrorNote } from "../components/ui";
import { Logo, SourceLink } from "./Login";

const field = "mt-2 h-11 w-full rounded-xl bg-surface-2 px-4 font-mono text-sm outline-none focus:ring-2 focus:ring-accent";
export const MIN_PASSWORD = 12;

/** Shown after signing in with a generated password: nothing else works until it's replaced. */
export function ChangePassword({ me, onChanged, onSignOut, sourceUrl }: { me: Me; onChanged: () => void; onSignOut: () => void; sourceUrl: string }) {
  const [current, setCurrent] = useState("");
  const [next, setNext] = useState("");
  const [again, setAgain] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (next !== again) {
      setError("The two new passwords don't match.");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await api.changePassword(current, next);
      onChanged();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  async function signOut() {
    try {
      await api.logout();
    } finally {
      onSignOut();
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
          <h1 className="text-[22px] font-semibold tracking-tight">Choose your password</h1>
          <p className="mt-1 text-sm text-muted">
            You signed in as <span className="font-medium text-ink-2">{me.user}</span> with a generated password. Replace it with your own to continue.
          </p>
          <label className="mt-6 block text-sm font-medium" htmlFor="current-password">
            Generated password
          </label>
          <input id="current-password" type="password" autoComplete="current-password" autoFocus value={current} onChange={(e) => setCurrent(e.target.value)} className={field} />
          <label className="mt-4 block text-sm font-medium" htmlFor="new-password">
            New password
          </label>
          <input id="new-password" type="password" autoComplete="new-password" minLength={MIN_PASSWORD} value={next} onChange={(e) => setNext(e.target.value)} className={field} />
          <label className="mt-4 block text-sm font-medium" htmlFor="new-password-again">
            New password again
          </label>
          <input id="new-password-again" type="password" autoComplete="new-password" minLength={MIN_PASSWORD} value={again} onChange={(e) => setAgain(e.target.value)} className={field} />
          <p className="mt-2 text-xs text-muted">At least {MIN_PASSWORD} characters. A few unrelated words work well.</p>
          {error && (
            <div className="mt-4">
              <ErrorNote>{error}</ErrorNote>
            </div>
          )}
          <Button variant="primary" type="submit" disabled={busy || !current || next.length < MIN_PASSWORD || !again} className="mt-6 w-full">
            <KeyRound className="size-4" />
            {busy ? "Saving..." : "Save and continue"}
          </Button>
          <button type="button" onClick={signOut} className="mt-4 inline-flex w-full items-center justify-center gap-1.5 text-xs text-muted hover:text-ink">
            <LogOut className="size-3.5" /> Sign out
          </button>
        </form>
        <SourceLink href={sourceUrl} className="mt-6 text-center" />
      </div>
    </div>
  );
}
