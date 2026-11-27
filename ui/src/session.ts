import { createContext, useContext, useEffect } from "react";
import { ROLES, type Me, type MyWorkspace, type Role } from "./api";

/** The signed-in user, the workspace the app shows, and how to switch it. */
export interface Session {
  me: Me;
  workspace: MyWorkspace;
  /** Show another workspace; `stay` keeps the current page instead of going to the overview. */
  switchTo: (slug: string, stay?: boolean) => void;
  /** Read the signed-in user again, after their memberships may have changed. */
  reload: () => void;
}

export const SessionContext = createContext<Session | null>(null);

const order = (r: Role) => ROLES.indexOf(r);

export function useSession(): Session | null {
  return useContext(SessionContext);
}

/** True if the signed-in user's role in the current workspace is at least `min`. The server checks too. */
export function useCan(min: Role): boolean {
  const s = useContext(SessionContext);
  return s !== null && order(s.workspace.role) >= order(min);
}

export function useIsServerAdmin(): boolean {
  return useContext(SessionContext)?.me.admin ?? false;
}

export function useMe(): Me | null {
  return useContext(SessionContext)?.me ?? null;
}

export function useWorkspace(): MyWorkspace | null {
  return useContext(SessionContext)?.workspace ?? null;
}

/** On a page about something in another of your workspaces (a link to a run, say), switch to it. */
export function useFollowWorkspace(slug: string | undefined) {
  const s = useContext(SessionContext);
  useEffect(() => {
    if (s && slug && slug !== s.workspace.slug && s.me.workspaces.some((w) => w.slug === slug)) s.switchTo(slug, true);
  }, [s, slug]);
}

export const ROLE_LABELS: Record<Role, string> = { viewer: "Viewer", operator: "Operator", editor: "Editor", admin: "Admin" };

/** What each role adds in a workspace, for the roles table and the role picker. */
export const ROLE_DESCRIPTIONS: Record<Role, string> = {
  viewer: "Sees runbooks, runs, output and hosts.",
  operator: "Also starts, retries and cancels runs.",
  editor: "Also edits runbooks with their schedules, webhooks and notifications, and the vault. Can run any command as root on the hosts a runbook targets.",
  admin: "Also manages the workspace's hosts and members.",
};
