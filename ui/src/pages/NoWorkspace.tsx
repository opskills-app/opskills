import { LogOut, RefreshCw } from "lucide-react";
import { api, type Me } from "../api";
import { Button } from "../components/ui";
import { Logo, SourceLink } from "./Login";

/** Signed in, but not a member of any workspace yet. */
export function NoWorkspace({ me, onReload, onSignOut, sourceUrl }: { me: Me; onReload: () => void; onSignOut: () => void; sourceUrl: string }) {
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
        <div className="rounded-[28px] bg-surface p-8 shadow-soft">
          <h1 className="text-[22px] font-semibold tracking-tight">No workspace yet</h1>
          <p className="mt-2 text-sm text-muted">
            You're signed in as <span className="font-medium text-ink-2">{me.user}</span>, but you're not a member of any workspace. Ask the Admin of the workspace you work in to add you.
          </p>
          <div className="mt-6 flex gap-2">
            <Button variant="primary" onClick={onReload}>
              <RefreshCw className="size-4" /> Check again
            </Button>
            <Button onClick={signOut}>
              <LogOut className="size-4" /> Sign out
            </Button>
          </div>
        </div>
        <SourceLink href={sourceUrl} className="mt-6 text-center" />
      </div>
    </div>
  );
}
