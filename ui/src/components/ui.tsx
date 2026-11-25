import type { ComponentProps, ReactNode } from "react";
import { Check, CircleDashed, Clock3, Hand, Loader2, X, Ban } from "lucide-react";
import type { Status } from "../data";

export function cx(...parts: (string | false | null | undefined)[]) {
  return parts.filter(Boolean).join(" ");
}

const statusMeta: Record<Status, { label: string; tone: string; dot: string; Icon: typeof Check }> = {
  succeeded: { label: "Succeeded", tone: "text-emerald-700 bg-emerald-500/10 dark:text-emerald-300", dot: "bg-emerald-500", Icon: Check },
  failed: { label: "Failed", tone: "text-rose-700 bg-rose-500/10 dark:text-rose-300", dot: "bg-rose-500", Icon: X },
  running: { label: "Running", tone: "text-sky-700 bg-sky-500/10 dark:text-sky-300", dot: "bg-sky-500", Icon: Loader2 },
  queued: { label: "Queued", tone: "text-ink-2 bg-surface-3", dot: "bg-zinc-400", Icon: Clock3 },
  waiting: { label: "Needs approval", tone: "text-amber-700 bg-amber-500/10 dark:text-amber-300", dot: "bg-amber-500", Icon: Hand },
  cancelled: { label: "Cancelled", tone: "text-muted bg-surface-3", dot: "bg-zinc-500", Icon: Ban },
};

export function StatusPill({ status, className }: { status: Status; className?: string }) {
  const m = statusMeta[status];
  return (
    <span className={cx("inline-flex items-center gap-1.5 rounded-full px-2 py-0.5 text-xs font-medium", m.tone, className)}>
      <m.Icon className={cx("size-3", status === "running" && "animate-spin")} strokeWidth={2.5} />
      {m.label}
    </span>
  );
}

export function StatusDot({ status, size = "size-2" }: { status: Status | "online" | "offline" | "degraded"; size?: string }) {
  const map: Record<string, string> = {
    online: "bg-emerald-500 text-emerald-500",
    offline: "bg-rose-500 text-rose-500",
    degraded: "bg-amber-500 text-amber-500",
  };
  const color = map[status] ?? `${statusMeta[status as Status].dot} text-sky-500`;
  const live = status === "running" || status === "online";
  return <span className={cx("inline-block shrink-0 rounded-full", size, color, live && status === "running" && "live-dot")} />;
}

/** Recent-run history as a strip of small bars, newest on the right. */
export function RunStrip({ history }: { history: Status[] }) {
  const slots = [...Array(Math.max(0, 10 - history.length)).fill(null), ...history.slice(-10)];
  return (
    <div className="flex items-end gap-[3px]" aria-label="Recent runs">
      {slots.map((s, i) =>
        s ? (
          <span
            key={i}
            title={statusMeta[s as Status].label}
            className={cx("h-4 w-1.5 rounded-[2px]", statusMeta[s as Status].dot, s === "running" && "animate-pulse")}
          />
        ) : (
          <span key={i} className="h-4 w-1.5 rounded-[2px] bg-surface-3" />
        ),
      )}
    </div>
  );
}

export function Card({ className, children, ...rest }: ComponentProps<"div">) {
  return (
    <div className={cx("rounded-xl border border-line bg-surface shadow-[0_1px_2px_rgb(0_0_0/0.04)]", className)} {...rest}>
      {children}
    </div>
  );
}

export function CardHeader({ title, action, hint }: { title: string; action?: ReactNode; hint?: string }) {
  return (
    <div className="flex items-center justify-between gap-3 px-4 pt-4 pb-3">
      <div>
        <h3 className="text-sm font-semibold tracking-tight">{title}</h3>
        {hint && <p className="mt-0.5 text-xs text-muted">{hint}</p>}
      </div>
      {action}
    </div>
  );
}

type ButtonProps = ComponentProps<"button"> & { variant?: "primary" | "ghost" | "outline"; size?: "sm" | "md" | "icon" };
export function Button({ variant = "outline", size = "md", className, children, ...rest }: ButtonProps) {
  return (
    <button
      className={cx(
        "inline-flex items-center justify-center gap-1.5 rounded-lg font-medium whitespace-nowrap transition-colors focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent disabled:opacity-50 [&_svg]:shrink-0",
        size === "sm" && "h-7 px-2.5 text-xs",
        size === "md" && "h-8 px-3 text-sm",
        size === "icon" && "size-8",
        variant === "primary" && "bg-accent text-accent-ink hover:brightness-110 shadow-sm",
        variant === "outline" && "border border-line bg-surface text-ink hover:bg-surface-2",
        variant === "ghost" && "text-ink-2 hover:bg-surface-2 hover:text-ink",
        className,
      )}
      {...rest}
    >
      {children}
    </button>
  );
}

export function Kbd({ children }: { children: ReactNode }) {
  return (
    <kbd className="inline-flex h-5 min-w-5 items-center justify-center rounded border border-line bg-surface-2 px-1 font-sans text-[10px] font-medium text-muted">
      {children}
    </kbd>
  );
}

export function Tag({ children }: { children: ReactNode }) {
  return (
    <span className="inline-flex items-center rounded-md border border-line bg-surface-2 px-1.5 py-px font-mono text-[11px] text-ink-2">
      {children}
    </span>
  );
}

export function Avatar({ name, size = "size-6" }: { name: string; size?: string }) {
  const initials = name
    .split(" ")
    .map((p) => p[0])
    .join("")
    .slice(0, 2)
    .toUpperCase();
  const hues = [160, 230, 35, 280, 340, 190];
  const hue = hues[name.length % hues.length];
  const tiny = size === "size-4" || size === "size-5";
  return (
    <span
      className={cx("inline-flex shrink-0 items-center justify-center rounded-full font-semibold text-white", tiny ? "text-[7px] tracking-tighter" : "text-[10px]", size)}
      style={{ background: `oklch(0.62 0.13 ${hue})` }}
      title={name}
    >
      {initials}
    </span>
  );
}

export function Sparkline({ values, className, area = true }: { values: number[]; className?: string; area?: boolean }) {
  const w = 120;
  const h = 36;
  const min = Math.min(...values);
  const max = Math.max(...values);
  const span = max - min || 1;
  const pts = values.map((v, i) => [(i / (values.length - 1)) * w, h - 3 - ((v - min) / span) * (h - 6)]);
  const d = pts.map(([x, y], i) => `${i ? "L" : "M"}${x.toFixed(1)},${y.toFixed(1)}`).join(" ");
  return (
    <svg viewBox={`0 0 ${w} ${h}`} className={cx("h-9 w-full overflow-visible", className)} preserveAspectRatio="none" aria-hidden>
      {area && <path d={`${d} L${w},${h} L0,${h} Z`} className="fill-accent/10" />}
      <path d={d} className="fill-none stroke-accent" strokeWidth={1.75} vectorEffect="non-scaling-stroke" strokeLinejoin="round" />
      <circle cx={pts[pts.length - 1][0]} cy={pts[pts.length - 1][1]} r={2.5} className="fill-accent" />
    </svg>
  );
}

export function EmptyIcon() {
  return <CircleDashed className="size-4 text-muted" />;
}

export function PageHeader({ title, subtitle, actions }: { title: ReactNode; subtitle?: ReactNode; actions?: ReactNode }) {
  return (
    <div className="mb-6 flex flex-wrap items-end justify-between gap-4">
      <div className="min-w-0">
        <h1 className="text-2xl font-semibold tracking-tight text-balance">{title}</h1>
        {subtitle && <p className="mt-1 text-sm text-muted">{subtitle}</p>}
      </div>
      {actions && <div className="flex items-center gap-2">{actions}</div>}
    </div>
  );
}
