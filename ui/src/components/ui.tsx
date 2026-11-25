import type { ComponentProps, ReactNode } from "react";
import { ArrowDown, ArrowUp, Ban, Check, ChevronDown, ChevronLeft, ChevronRight, CircleDashed, Clock3, Hand, Loader2, Search, X } from "lucide-react";
import type { Status } from "../data";

export function cx(...parts: (string | false | null | undefined)[]) {
  return parts.filter(Boolean).join(" ");
}

const badgeTone = {
  green: "border-emerald-500 bg-emerald-500/15 text-emerald-700 dark:border-emerald-400/70 dark:text-emerald-300",
  red: "border-rose-500 bg-rose-500/15 text-rose-700 dark:border-rose-400/70 dark:text-rose-300",
  blue: "border-sky-500 bg-sky-500/15 text-sky-700 dark:border-sky-400/70 dark:text-sky-300",
  amber: "border-amber-500 bg-amber-500/15 text-amber-700 dark:border-amber-400/70 dark:text-amber-300",
  gray: "border-line-strong bg-surface-2 text-muted",
};
export type BadgeTone = keyof typeof badgeTone;

/** Outlined, tinted status badge. */
export function Badge({ tone, children, className }: { tone: BadgeTone; children: ReactNode; className?: string }) {
  return (
    <span className={cx("inline-flex min-w-[92px] items-center justify-center gap-1.5 rounded-[5px] border px-3 py-1 text-[13px] font-medium whitespace-nowrap", badgeTone[tone], className)}>
      {children}
    </span>
  );
}

const statusMeta: Record<Status, { label: string; tone: BadgeTone; dot: string; Icon: typeof Check }> = {
  succeeded: { label: "Succeeded", tone: "green", dot: "bg-emerald-500", Icon: Check },
  failed: { label: "Failed", tone: "red", dot: "bg-rose-500", Icon: X },
  running: { label: "Running", tone: "blue", dot: "bg-sky-500", Icon: Loader2 },
  queued: { label: "Queued", tone: "gray", dot: "bg-zinc-400", Icon: Clock3 },
  waiting: { label: "Approval", tone: "amber", dot: "bg-amber-500", Icon: Hand },
  cancelled: { label: "Cancelled", tone: "gray", dot: "bg-zinc-500", Icon: Ban },
};

export function StatusPill({ status, className }: { status: Status; className?: string }) {
  const m = statusMeta[status];
  return (
    <Badge tone={m.tone} className={className}>
      {status === "running" && <m.Icon className="size-3.5 animate-spin" strokeWidth={2.5} />}
      {m.label}
    </Badge>
  );
}

/** "Sort by: Newest" style menu button. */
export function SortButton({ label = "Sort by", value }: { label?: string; value: string }) {
  return (
    <button className="flex h-11 items-center gap-6 rounded-xl bg-surface-2 px-4 text-[13px] text-muted">
      <span>
        {label}: <span className="font-semibold text-ink">{value}</span>
      </span>
      <ChevronDown className="size-4 text-ink" />
    </button>
  );
}

export function SearchField({ value, onChange, placeholder = "Search", className }: { value: string; onChange: (v: string) => void; placeholder?: string; className?: string }) {
  return (
    <label className={cx("flex h-11 items-center gap-2.5 rounded-xl bg-surface-2 px-3.5", className ?? "w-full sm:w-64")}>
      <Search className="size-5 text-muted" strokeWidth={1.75} />
      <input value={value} onChange={(e) => onChange(e.target.value)} placeholder={placeholder} className="min-w-0 flex-1 bg-transparent text-[13px] outline-none placeholder:text-muted" />
    </label>
  );
}

/** Footer with an entry count and page buttons. */
export function Pagination({ from, to, total, pages = 4, last }: { from: number; to: number; total: string; pages?: number; last?: number }) {
  const btn = "flex size-7 items-center justify-center rounded-[5px] border text-xs font-medium";
  return (
    <div className="flex flex-wrap items-center justify-between gap-4 px-6 py-6 sm:px-8">
      <span className="text-sm text-muted">
        Showing data {from} to {to} of {total} entries
      </span>
      <nav className="flex items-center gap-2" aria-label="Pagination">
        <button className={cx(btn, "border-line bg-surface-2 text-ink-2")} aria-label="Previous page">
          <ChevronLeft className="size-3.5" />
        </button>
        {Array.from({ length: pages }, (_, i) => (
          <button key={i} aria-current={i === 0 ? "page" : undefined} className={cx(btn, i === 0 ? "border-accent bg-accent text-accent-ink" : "border-line bg-surface-2 text-ink-2")}>
            {i + 1}
          </button>
        ))}
        {last && (
          <>
            <span className="text-xs text-muted">...</span>
            <button className={cx(btn, "border-line bg-surface-2 text-ink-2")}>{last}</button>
          </>
        )}
        <button className={cx(btn, "border-line bg-surface-2 text-ink-2")} aria-label="Next page">
          <ChevronRight className="size-3.5" />
        </button>
      </nav>
    </div>
  );
}

/** Table card heading: title, green subtitle, and controls. */
export function TableHeading({ title, subtitle, children }: { title: string; subtitle?: string; children?: ReactNode }) {
  return (
    <div className="flex flex-wrap items-start justify-between gap-4 px-6 pt-7 pb-6 sm:px-10 sm:pt-9">
      <div>
        <h3 className="text-[22px] font-semibold tracking-tight">{title}</h3>
        {subtitle && <p className="mt-1.5 text-sm text-emerald-600 dark:text-emerald-400">{subtitle}</p>}
      </div>
      <div className="flex flex-wrap items-center gap-4">{children}</div>
    </div>
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
    <div className={cx("rounded-[28px] bg-surface shadow-soft", className)} {...rest}>
      {children}
    </div>
  );
}

export function CardHeader({ title, action, hint }: { title: string; action?: ReactNode; hint?: string }) {
  return (
    <div className="flex flex-wrap items-start justify-between gap-3 px-6 pt-6 pb-4 sm:px-8 sm:pt-7">
      <div>
        <h3 className="text-[19px] font-semibold tracking-tight">{title}</h3>
        {hint && <p className="mt-1 text-sm text-muted">{hint}</p>}
      </div>
      {action}
    </div>
  );
}

const bubbleTones = {
  mint: "bg-gradient-to-br from-emerald-100 to-emerald-50 text-emerald-600 dark:from-emerald-500/25 dark:to-emerald-500/10 dark:text-emerald-300",
  sky: "bg-gradient-to-br from-sky-100 to-sky-50 text-sky-600 dark:from-sky-500/25 dark:to-sky-500/10 dark:text-sky-300",
  pink: "bg-gradient-to-br from-pink-100 to-fuchsia-50 text-pink-600 dark:from-pink-500/25 dark:to-pink-500/10 dark:text-pink-300",
  violet: "bg-gradient-to-br from-violet-100 to-violet-50 text-violet-600 dark:from-violet-500/25 dark:to-violet-500/10 dark:text-violet-300",
  amber: "bg-gradient-to-br from-amber-100 to-amber-50 text-amber-600 dark:from-amber-500/25 dark:to-amber-500/10 dark:text-amber-300",
};
export type BubbleTone = keyof typeof bubbleTones;

/** Large pastel circle holding an icon. */
export function IconBubble({ icon: Icon, tone, size = "size-20" }: { icon: typeof Check; tone: BubbleTone; size?: string }) {
  return (
    <span className={cx("flex shrink-0 items-center justify-center rounded-full", bubbleTones[tone], size)}>
      <Icon className="size-[42%]" strokeWidth={1.6} />
    </span>
  );
}

/** Change indicator: arrow plus bold percentage, then a muted period. */
export function Delta({ value, good, period }: { value: string; good: boolean; period: string }) {
  const up = !value.startsWith("-");
  return (
    <span className="inline-flex items-center gap-1 text-xs text-ink-2">
      <span className={cx("inline-flex items-center gap-0.5 font-bold", good ? "text-emerald-600 dark:text-emerald-400" : "text-rose-600 dark:text-rose-400")}>
        {up ? <ArrowUp className="size-3.5" strokeWidth={2.5} /> : <ArrowDown className="size-3.5" strokeWidth={2.5} />}
        {value.replace(/^[-+]/, "")}
      </span>
      {period}
    </span>
  );
}

type ButtonProps = ComponentProps<"button"> & { variant?: "primary" | "ghost" | "outline"; size?: "sm" | "md" | "icon" };
export function Button({ variant = "outline", size = "md", className, children, ...rest }: ButtonProps) {
  return (
    <button
      className={cx(
        "inline-flex items-center justify-center gap-1.5 rounded-lg font-medium whitespace-nowrap transition-colors focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent disabled:opacity-50 [&_svg]:shrink-0",
        size === "sm" && "h-8 px-3 text-xs",
        size === "md" && "h-10 px-4 text-sm",
        size === "icon" && "size-10",
        variant === "primary" && "bg-accent text-accent-ink shadow-[0_8px_20px_-8px_var(--accent)] hover:brightness-110",
        variant === "outline" && "bg-surface-2 text-ink-2 hover:bg-surface-3 hover:text-ink",
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
    <span className="inline-flex items-center rounded-full bg-accent-soft px-2.5 py-0.5 text-[11px] font-medium text-accent">
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

const tileGradients = [
  "from-violet-500 to-indigo-600",
  "from-sky-400 to-blue-600",
  "from-fuchsia-400 to-pink-600",
  "from-emerald-400 to-teal-600",
  "from-amber-400 to-orange-500",
  "from-indigo-400 to-violet-700",
];

/** Rounded gradient tile with an icon, used as a runbook thumbnail. */
export function Tile({ seed, icon: Icon, className }: { seed: string; icon: typeof Check; className?: string }) {
  const n = [...seed].reduce((a, c) => a + c.charCodeAt(0), 0);
  return (
    <span className={cx("relative flex shrink-0 items-center justify-center overflow-hidden rounded-2xl bg-gradient-to-br text-white", tileGradients[n % tileGradients.length], className ?? "h-14 w-20")}>
      <span className="absolute -top-4 -right-4 size-12 rounded-full bg-white/20" />
      <span className="absolute -bottom-5 -left-3 size-10 rounded-full bg-black/10" />
      <Icon className="relative size-6" strokeWidth={1.75} />
    </span>
  );
}

export function EmptyIcon() {
  return <CircleDashed className="size-4 text-muted" />;
}

export function PageHeader({ title, subtitle, actions }: { title: ReactNode; subtitle?: ReactNode; actions?: ReactNode }) {
  return (
    <div className="mb-8 flex flex-wrap items-end justify-between gap-4">
      <div className="min-w-0">
        <h1 className="text-[26px] font-semibold tracking-tight text-balance">{title}</h1>
        {subtitle && <p className="mt-1 text-sm text-muted">{subtitle}</p>}
      </div>
      {actions && <div className="flex items-center gap-2">{actions}</div>}
    </div>
  );
}
