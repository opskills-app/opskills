import { createContext, useContext } from "react";
import { ROLES, type Me, type Role } from "./api";

/** The signed-in user, for pages that show or hide what their role allows. */
export const SessionContext = createContext<Me | null>(null);

const order = (r: Role) => ROLES.indexOf(r);

/** True if the signed-in user's role is at least `min`. The server checks too. */
export function useCan(min: Role): boolean {
  const me = useContext(SessionContext);
  return me !== null && order(me.role) >= order(min);
}

export function useMe(): Me | null {
  return useContext(SessionContext);
}

export const ROLE_LABELS: Record<Role, string> = { viewer: "Viewer", operator: "Operator", editor: "Editor", admin: "Admin" };

/** What each role adds, for the roles table and the role picker. */
export const ROLE_DESCRIPTIONS: Record<Role, string> = {
  viewer: "Sees runbooks, runs, output and hosts.",
  operator: "Also starts, retries and cancels runs.",
  editor: "Also edits runbooks with their schedules, webhooks and notifications, and the vault. Can run any command as root on the hosts a runbook targets.",
  admin: "Also manages hosts, users, email settings and everyone's API tokens.",
};
