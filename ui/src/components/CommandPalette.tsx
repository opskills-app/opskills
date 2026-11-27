import { useEffect, useMemo, useRef, useState } from "react";
import { ArrowRight, BookOpen, Play, Plus, Search } from "lucide-react";
import { api, type Runbook } from "../api";
import { go } from "../router";
import { cx, Kbd } from "./ui";
import { useCan } from "../session";

interface Item {
  id: string;
  label: string;
  hint: string;
  group: string;
  icon: typeof Search;
  run: () => void;
}

export function CommandPalette({ open, onClose }: { open: boolean; onClose: () => void }) {
  const [q, setQ] = useState("");
  const [active, setActive] = useState(0);
  const [runbooks, setRunbooks] = useState<Runbook[]>([]);
  const input = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (!open) return;
    setQ("");
    setActive(0);
    requestAnimationFrame(() => input.current?.focus());
    api.runbooks().then(setRunbooks, () => setRunbooks([]));
  }, [open]);

  const canEdit = useCan("editor");
  const canRun = useCan("operator");
  const admin = useCan("admin");
  const items = useMemo<Item[]>(() => {
    const nav: Item[] = [
      ["overview", "Overview"],
      ["runbooks", "Runbooks"],
      ["runs", "Runs"],
      ["hosts", "Hosts"],
      ["vault", "Vault"],
      ["audit", "Audit log"],
      ["settings", "Settings"],
    ]
      .filter(([id]) => (id !== "vault" || canEdit) && (id !== "audit" || admin))
      .map(([id, label]) => ({ id: `nav-${id}`, label: `Go to ${label}`, hint: "Navigate", group: "Navigation", icon: ArrowRight, run: () => go(id) }));
    const create: Item = { id: "new", label: "New runbook", hint: "Create", group: "Actions", icon: Plus, run: () => go("runbooks/new") };
    const rb: Item[] = runbooks.flatMap((r): Item[] => [
      { id: `open-${r.id}`, label: r.name, hint: r.tags.join(", ") || `${r.steps.length} steps`, group: "Runbooks", icon: BookOpen, run: () => go(`runbooks/${r.id}`) },
      {
        id: `run-${r.id}`,
        label: `Run "${r.name}"`,
        hint: r.inputs?.length ? "Asks for inputs" : "Start a run",
        group: "Actions",
        icon: Play,
        run: () => {
          // Inputs are asked for on the runbook's page.
          if (r.inputs?.length) go(`runbooks/${r.id}`);
          else api.startRun(r.id).then((run) => go(`runs/${run.id}`));
        },
      },
    ]);
    return [...nav, ...(canEdit ? [create] : []), ...rb.filter((i) => canRun || !i.id.startsWith("run-"))];
  }, [runbooks, canEdit, canRun, admin]);

  const filtered = useMemo(() => {
    const t = q.trim().toLowerCase();
    if (!t) return items.slice(0, 14);
    return items.filter((i) => `${i.label} ${i.hint}`.toLowerCase().includes(t)).slice(0, 14);
  }, [q, items]);

  useEffect(() => setActive(0), [q]);

  if (!open) return null;

  const choose = (i: Item | undefined) => {
    if (!i) return;
    i.run();
    onClose();
  };

  let lastGroup = "";
  return (
    <div className="fixed inset-0 z-50 flex items-start justify-center bg-black/40 px-4 pt-[12vh] backdrop-blur-[2px]" onMouseDown={onClose}>
      <div role="dialog" aria-label="Command palette" className="w-full max-w-xl overflow-hidden rounded-2xl bg-surface shadow-2xl" onMouseDown={(e) => e.stopPropagation()}>
        <div className="flex items-center gap-3 border-b border-line px-4">
          <Search className="size-4 text-muted" />
          <input
            ref={input}
            value={q}
            onChange={(e) => setQ(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "ArrowDown") {
                e.preventDefault();
                setActive((a) => Math.min(a + 1, filtered.length - 1));
              } else if (e.key === "ArrowUp") {
                e.preventDefault();
                setActive((a) => Math.max(a - 1, 0));
              } else if (e.key === "Enter") {
                choose(filtered[active]);
              } else if (e.key === "Escape") {
                onClose();
              }
            }}
            placeholder="Search runbooks or type a command"
            className="h-12 flex-1 bg-transparent text-sm outline-none placeholder:text-muted"
          />
          <Kbd>Esc</Kbd>
        </div>
        <ul className="scroll-thin max-h-[50vh] overflow-y-auto p-2">
          {filtered.length === 0 && <li className="px-3 py-8 text-center text-sm text-muted">No matches for "{q}"</li>}
          {filtered.map((item, idx) => {
            const header = item.group !== lastGroup ? item.group : null;
            lastGroup = item.group;
            return (
              <li key={item.id}>
                {header && <div className="px-3 pt-3 pb-1 text-[11px] font-medium tracking-wide text-muted uppercase">{header}</div>}
                <button
                  onMouseEnter={() => setActive(idx)}
                  onClick={() => choose(item)}
                  className={cx("flex w-full items-center gap-3 rounded-lg px-3 py-2 text-left text-sm", idx === active ? "bg-surface-2 text-ink" : "text-ink-2")}
                >
                  <item.icon className="size-4 shrink-0 text-muted" />
                  <span className="flex-1 truncate">{item.label}</span>
                  <span className="truncate text-xs text-muted">{item.hint}</span>
                </button>
              </li>
            );
          })}
        </ul>
      </div>
    </div>
  );
}
