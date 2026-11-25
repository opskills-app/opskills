import { useEffect, useState } from "react";

/** Minimal hash router: "#/runbooks/restart-web" -> ["runbooks", "restart-web"]. */
export function useRoute(): string[] {
  const read = () => window.location.hash.replace(/^#\/?/, "").split("/").filter(Boolean);
  const [parts, setParts] = useState(read);
  useEffect(() => {
    const onChange = () => {
      setParts(read());
      window.scrollTo({ top: 0 });
    };
    window.addEventListener("hashchange", onChange);
    return () => window.removeEventListener("hashchange", onChange);
  }, []);
  return parts.length ? parts : ["overview"];
}

export function go(path: string) {
  window.location.hash = path.startsWith("/") ? path : `/${path}`;
}
