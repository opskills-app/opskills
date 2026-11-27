import { useEffect, useState, type FormEvent } from "react";
import { ArrowDown, ArrowLeft, ArrowUp, CalendarClock, Check, ChevronDown, FileText, KeyRound, Plus, Save, Server, SlidersHorizontal, Tag as TagIcon, Terminal, Trash2, Workflow } from "lucide-react";
import {
  api,
  defaultRollout,
  LOCAL_HOST_ID,
  MAX_OUTPUT_LINES,
  runsOnServer,
  secretEnvName,
  stepKind,
  useData,
  type CallStep,
  type CopyStep,
  type FailurePolicy,
  type InputDef,
  type InputKind,
  type Rollout,
  type Runbook,
  type RunbookInput,
  type Schedule,
  type Step,
  type StepKind,
  type Targets,
} from "../api";
import { Button, Card, cx, ErrorNote, PageHeader, StatusDot } from "../components/ui";
import { stepOptions } from "./RunbookDetail";
import { when } from "../format";
import { templates } from "../templates";
import { go } from "../router";
import { cleanNotification, NotificationsEditor } from "../components/Notifications";
import { useFollowWorkspace } from "../session";

const blank: RunbookInput = {
  name: "",
  description: "",
  tags: [],
  inputs: [],
  steps: [{ name: "", run: "" }],
  on_step_failure: "stop",
  secrets: [],
  targets: { hosts: [], tags: [] },
  timeout_seconds: 600,
};

const kinds: { id: InputKind; label: string }[] = [
  { id: "text", label: "Text" },
  { id: "number", label: "Number" },
  { id: "boolean", label: "Yes or no" },
  { id: "choice", label: "Choice" },
  { id: "secret", label: "Secret" },
];

const newInput = (): InputDef => ({ name: "", label: "", description: "", type: "text", required: false, default: null, choices: [], pattern: null });

/** A number of seconds stays a number; "10m" and the like go to the server as written. */
function durationValue(text: string): number | string | null {
  const t = text.trim();
  if (t === "") return null;
  return /^\d+$/.test(t) ? Number(t) : t;
}

/** Tidy the editor's working copy into what the API expects. */
function cleanInput(d: InputDef): InputDef {
  return {
    ...d,
    name: d.name.trim(),
    choices: d.type === "choice" ? d.choices.map((c) => c.trim()).filter(Boolean) : [],
    pattern: (d.type === "text" || d.type === "secret") && d.pattern?.trim() ? d.pattern : null,
    default: d.type === "secret" || d.default === null || d.default === "" ? null : d.default,
  };
}

function toggle(list: string[], item: string) {
  return list.includes(item) ? list.filter((x) => x !== item) : [...list, item];
}

/** Where the steps run: the OpSkills server, or chosen hosts and host tags. */
/** The hosts a filter expression matches now, asked of the server as you type. */
function useFilterPreview(filter: string) {
  const [state, setState] = useState<{ ids: string[]; names: string[]; error: string | null }>({ ids: [], names: [], error: null });
  useEffect(() => {
    const f = filter.trim();
    if (!f) {
      setState({ ids: [], names: [], error: null });
      return;
    }
    const timer = setTimeout(() => {
      api.hosts(f).then(
        (hs) => setState({ ids: hs.map((h) => h.id), names: hs.map((h) => h.name), error: null }),
        (e) => setState({ ids: [], names: [], error: e instanceof Error ? e.message : String(e) }),
      );
    }, 300);
    return () => clearTimeout(timer);
  }, [filter]);
  return state;
}

function RunOn({ targets, onChange, onHosts, setOnHosts }: { targets: Targets; onChange: (t: Targets) => void; onHosts: boolean; setOnHosts: (v: boolean) => void }) {
  const hosts = useData(() => api.hosts(), []);
  const enrolled = (hosts.data ?? []).filter((h) => !h.builtin);
  const hostTags = [...new Set(enrolled.flatMap((h) => h.tags))].sort();
  const onServer = !onHosts;
  const preview = useFilterPreview(targets.filter ?? "");
  const matched = enrolled.filter((h) => targets.hosts.includes(h.id) || h.tags.some((t) => targets.tags.includes(t)) || preview.ids.includes(h.id));
  const count = matched.length + (targets.hosts.includes(LOCAL_HOST_ID) ? 1 : 0);
  const chip = (on: boolean) => cx("flex h-8 items-center gap-1.5 rounded-lg px-2.5 text-xs", on ? "bg-accent text-accent-ink" : "bg-surface-2 text-ink-2 hover:bg-surface-3");

  return (
    <Card className="p-6">
      <div className="flex items-center justify-between gap-3">
        <p className="text-sm font-medium">Run on</p>
        <a href="#/hosts" className="text-xs font-medium text-accent hover:underline">
          Manage hosts
        </a>
      </div>
      <div role="radiogroup" aria-label="Run on" className="mt-3 grid grid-cols-2 gap-1 rounded-xl bg-surface-2 p-1">
        {[
          {
            on: onServer,
            label: "OpSkills server",
            pick: () => {
              setOnHosts(false);
              onChange({ hosts: [], tags: [], filter: "" });
            },
          },
          {
            on: !onServer,
            label: "Hosts",
            pick: () => {
              if (onServer && enrolled.length === 1) onChange({ hosts: [enrolled[0].id], tags: [] });
              setOnHosts(true);
            },
          },
        ].map((o) => (
          <button key={o.label} type="button" role="radio" aria-checked={o.on} onClick={o.pick} className={cx("h-9 rounded-lg text-xs font-medium", o.on ? "bg-surface text-accent shadow-sm" : "text-muted hover:text-ink")}>
            {o.label}
          </button>
        ))}
      </div>
      {onServer ? (
        <p className="mt-3 text-xs text-muted">Steps run on the OpSkills server itself, as the user the server runs as.</p>
      ) : enrolled.length === 0 ? (
        <p className="mt-3 text-xs text-muted">
          No hosts yet.{" "}
          <a href="#/hosts" className="font-medium text-accent hover:underline">
            Add a host
          </a>{" "}
          first, then pick it here.
        </p>
      ) : (
        <div className="mt-4 space-y-4">
          <div>
            <p className="text-xs font-medium text-muted">Hosts</p>
            <div className="mt-2 flex flex-wrap gap-2">
              <button type="button" aria-pressed={targets.hosts.includes(LOCAL_HOST_ID)} onClick={() => onChange({ ...targets, hosts: toggle(targets.hosts, LOCAL_HOST_ID) })} className={chip(targets.hosts.includes(LOCAL_HOST_ID))}>
                <Server className="size-3.5" /> OpSkills server
              </button>
              {enrolled.map((h) => {
                const on = targets.hosts.includes(h.id);
                return (
                  <button key={h.id} type="button" aria-pressed={on} onClick={() => onChange({ ...targets, hosts: toggle(targets.hosts, h.id) })} className={chip(on)}>
                    {on ? <Check className="size-3.5" /> : <StatusDot status={h.online ? "online" : "offline"} />}
                    {h.name}
                  </button>
                );
              })}
            </div>
          </div>
          {hostTags.length > 0 && (
            <div>
              <p className="text-xs font-medium text-muted">Or every host tagged</p>
              <div className="mt-2 flex flex-wrap gap-2">
                {hostTags.map((t) => {
                  const on = targets.tags.includes(t);
                  return (
                    <button key={t} type="button" aria-pressed={on} onClick={() => onChange({ ...targets, tags: toggle(targets.tags, t) })} className={chip(on)}>
                      {on ? <Check className="size-3.5" /> : <TagIcon className="size-3.5" />}
                      {t}
                    </button>
                  );
                })}
              </div>
            </div>
          )}
          <div>
            <label className="text-xs font-medium text-muted" htmlFor="rb-filter">
              Or hosts matching
            </label>
            <input
              id="rb-filter"
              value={targets.filter ?? ""}
              onChange={(e) => onChange({ ...targets, filter: e.target.value })}
              placeholder="tags:web !name:db-*"
              spellCheck={false}
              className="mt-2 h-10 w-full rounded-xl bg-surface-2 px-3 font-mono text-xs outline-none focus:ring-2 focus:ring-accent"
            />
            <p className="mt-1.5 text-xs text-muted">
              {preview.error ? (
                <span className="text-rose-600 dark:text-rose-400">{preview.error}</span>
              ) : targets.filter?.trim() ? (
                preview.names.length ? (
                  `Matches ${preview.names.join(", ")} now, and any host enrolled later that fits.`
                ) : (
                  "Matches no host now."
                )
              ) : (
                <>
                  Keys: tags, name, hostname, os, arch. <code className="font-mono">!</code> excludes, <code className="font-mono">*</code> is a wildcard, commas mean any of.
                </>
              )}
            </p>
          </div>
          <p className={cx("text-xs", count ? "text-muted" : "text-amber-700 dark:text-amber-300")}>
            {count ? `Runs on ${count} host${count === 1 ? "" : "s"} now. Choose how below, under Rollout.` : "Pick at least one host or tag, or enter a filter that matches a host."}
          </p>
        </div>
      )}
    </Card>
  );
}

const field = "h-11 w-full rounded-xl bg-surface-2 px-4 text-sm outline-none focus:ring-2 focus:ring-accent";
const smallLabel = "text-xs font-medium text-muted";

/** How a run spreads over several hosts. */
function RolloutCard({ rollout, stepPolicy, onChange }: { rollout: Rollout; stepPolicy: FailurePolicy; onChange: (r: Rollout) => void }) {
  const set = (patch: Partial<Rollout>) => onChange({ ...rollout, ...patch });
  const modes: { id: Rollout["mode"]; label: string; hint: string }[] = [
    { id: "sequential", label: "One by one", hint: "Each host runs all steps, then the next host starts." },
    { id: "parallel", label: "In parallel", hint: "Many hosts at once, up to the limit you set." },
    { id: "batches", label: "In batches", hint: "A group of hosts at a time; the next group starts when it's done." },
  ];
  return (
    <Card className="space-y-4 p-6">
      <div>
        <p className="text-sm font-medium">Rollout</p>
        <div role="radiogroup" aria-label="Rollout" className="mt-3 grid grid-cols-3 gap-1 rounded-xl bg-surface-2 p-1">
          {modes.map((m) => (
            <button
              key={m.id}
              type="button"
              role="radio"
              aria-checked={rollout.mode === m.id}
              onClick={() => set({ mode: m.id, batch_size: m.id === "batches" ? (rollout.batch_size ?? "25%") : undefined, max_parallel: m.id === "parallel" ? rollout.max_parallel : 0 })}
              className={cx("h-9 rounded-lg text-xs font-medium", rollout.mode === m.id ? "bg-surface text-accent shadow-sm" : "text-muted hover:text-ink")}
            >
              {m.label}
            </button>
          ))}
        </div>
        <p className="mt-2 text-xs text-muted">{modes.find((m) => m.id === rollout.mode)?.hint}</p>
      </div>
      {rollout.mode === "parallel" && (
        <div>
          <label className={smallLabel} htmlFor="ro-max">
            Most hosts at once
          </label>
          <input id="ro-max" type="number" min={0} max={1000} value={rollout.max_parallel || ""} placeholder="All of them" onChange={(e) => set({ max_parallel: Number(e.target.value) || 0 })} className={`mt-1.5 ${field}`} />
        </div>
      )}
      {rollout.mode === "batches" && (
        <div>
          <label className={smallLabel} htmlFor="ro-batch">
            Batch size
          </label>
          <input id="ro-batch" required value={rollout.batch_size ?? ""} placeholder="5 or 25%" onChange={(e) => set({ batch_size: e.target.value })} className={`mt-1.5 ${field}`} />
          <p className="mt-1.5 text-xs text-muted">A number of hosts, or a percentage of the run's hosts (rounded up).</p>
        </div>
      )}
      <div className="grid grid-cols-2 gap-3">
        <div>
          <label className={smallLabel} htmlFor="ro-canary">
            Canary hosts
          </label>
          <input id="ro-canary" type="number" min={0} max={1000} value={rollout.canary || ""} placeholder="None" onChange={(e) => set({ canary: Number(e.target.value) || 0 })} className={`mt-1.5 ${field}`} />
        </div>
        <div>
          <label className={smallLabel} htmlFor="ro-pause">
            Pause between
          </label>
          <input id="ro-pause" value={typeof rollout.pause_seconds === "number" ? (rollout.pause_seconds ? `${rollout.pause_seconds}s` : "") : rollout.pause_seconds} placeholder="No pause" onChange={(e) => set({ pause_seconds: durationValue(e.target.value) ?? 0 })} className={`mt-1.5 ${field}`} />
        </div>
      </div>
      <p className="-mt-2 text-xs text-muted">Canary hosts run first, on their own. The pause comes after the canary and between batches, like 30s or 5m.</p>
      <div>
        <label className={smallLabel} htmlFor="ro-fail">
          If a host fails
        </label>
        <select
          id="ro-fail"
          value={rollout.on_host_failure ?? ""}
          onChange={(e) => set({ on_host_failure: (e.target.value || undefined) as Rollout["on_host_failure"] })}
          className={`mt-1.5 ${field}`}
        >
          <option value="">{stepPolicy === "finish_then_fail" ? "Keep going (default)" : "Skip the rest (default)"}</option>
          <option value="stop">Skip the rest</option>
          <option value="continue">Keep going</option>
        </select>
        <p className="mt-1.5 text-xs text-muted">
          "Skip the rest" means hosts that haven't started are skipped; hosts already running always finish. The default follows "When a step fails". Either way, the run ends as failed.
        </p>
      </div>
    </Card>
  );
}

/** Only the fields that apply to the chosen mode, as the API expects. */
function cleanRollout(r: Rollout | undefined): Rollout {
  if (!r) return { ...defaultRollout };
  const out: Rollout = { mode: r.mode, canary: Number(r.canary) || 0, pause_seconds: typeof r.pause_seconds === "string" ? (durationValue(r.pause_seconds) ?? 0) : (r.pause_seconds ?? 0) };
  if (r.mode === "parallel") out.max_parallel = Number(r.max_parallel) || 0;
  if (r.mode === "batches") {
    const b = String(r.batch_size ?? "").trim();
    out.batch_size = /^\d+$/.test(b) ? Number(b) : b;
  }
  if (r.on_host_failure) out.on_host_failure = r.on_host_failure;
  return out;
}

const cronPresets: { cron: string; label: string }[] = [
  { cron: "*/15 * * * *", label: "Every 15 minutes" },
  { cron: "0 * * * *", label: "Every hour" },
  { cron: "0 3 * * *", label: "Every day at 03:00" },
  { cron: "0 9 * * 1-5", label: "Weekdays at 09:00" },
  { cron: "0 6 * * 1", label: "Mondays at 06:00" },
];

const localZone = Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC";
const zones = (() => {
  const list = typeof Intl.supportedValuesOf === "function" ? Intl.supportedValuesOf("timeZone") : [];
  return ["UTC", ...list.filter((z) => z !== "UTC")];
})();

/** "2026-11-27T09:30" in the viewer's time zone, for a datetime-local field. */
function toLocalInput(ms: number) {
  const d = new Date(ms - new Date(ms).getTimezoneOffset() * 60_000);
  return d.toISOString().slice(0, 16);
}

const newSchedule = (): Schedule => ({ id: "", name: "", cron: "0 3 * * *", timezone: localZone, enabled: true, inputs: {} });

/** The next times a cron expression fires, asked of the server as you type. */
function useCronPreview(cron: string | undefined, timezone: string) {
  const [state, setState] = useState<{ next: number[]; error: string | null }>({ next: [], error: null });
  useEffect(() => {
    if (cron === undefined) return;
    const timer = setTimeout(() => {
      api.previewSchedule(cron, timezone).then(
        (r) => setState({ next: r.next, error: null }),
        (e) => setState({ next: [], error: e instanceof Error ? e.message : String(e) }),
      );
    }, 300);
    return () => clearTimeout(timer);
  }, [cron, timezone]);
  return state;
}

function ScheduleCard({ s, index, inputs, onChange, onRemove }: { s: Schedule; index: number; inputs: InputDef[]; onChange: (s: Schedule) => void; onRemove: () => void }) {
  const repeat = s.cron !== undefined;
  const preview = useCronPreview(s.cron, s.timezone);
  const preset = cronPresets.find((p) => p.cron === s.cron?.trim().replace(/\s+/g, " "));
  const fillable = inputs.filter((d) => d.type !== "secret");
  return (
    <Card className="space-y-4 p-5 sm:p-6">
      <div className="flex flex-wrap items-center gap-3">
        <div role="radiogroup" aria-label={`Schedule ${index + 1} kind`} className="inline-flex gap-1 rounded-xl bg-surface-2 p-1">
          {[
            { on: repeat, label: "Repeat", pick: () => onChange({ ...s, cron: s.cron ?? "0 3 * * *", at: undefined }) },
            { on: !repeat, label: "Once", pick: () => onChange({ ...s, cron: undefined, at: s.at ?? Date.now() + 3_600_000 }) },
          ].map((o) => (
            <button key={o.label} type="button" role="radio" aria-checked={o.on} onClick={o.pick} className={cx("h-8 rounded-lg px-3 text-xs font-medium", o.on ? "bg-surface text-accent shadow-sm" : "text-muted hover:text-ink")}>
              {o.label}
            </button>
          ))}
        </div>
        <input value={s.name} onChange={(e) => onChange({ ...s, name: e.target.value })} placeholder="Name (optional), like nightly" aria-label={`Schedule ${index + 1} name`} className={`${field} h-9 flex-1 min-w-[10rem]`} />
        <label className="flex items-center gap-2 text-sm">
          <input type="checkbox" checked={s.enabled} onChange={(e) => onChange({ ...s, enabled: e.target.checked })} className="size-4 accent-accent" />
          On
        </label>
        <button type="button" onClick={onRemove} className="rounded-lg p-2 text-muted hover:text-rose-600" aria-label={`Remove schedule ${index + 1}`}>
          <Trash2 className="size-4" />
        </button>
      </div>
      {repeat ? (
        <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
          <div>
            <label className={smallLabel} htmlFor={`sc-preset-${index}`}>
              When
            </label>
            <select id={`sc-preset-${index}`} value={preset?.cron ?? ""} onChange={(e) => e.target.value && onChange({ ...s, cron: e.target.value })} className={`mt-1.5 ${field}`}>
              {cronPresets.map((p) => (
                <option key={p.cron} value={p.cron}>
                  {p.label}
                </option>
              ))}
              <option value="">Custom</option>
            </select>
          </div>
          <div>
            <label className={smallLabel} htmlFor={`sc-cron-${index}`}>
              Cron expression
            </label>
            <input id={`sc-cron-${index}`} value={s.cron} onChange={(e) => onChange({ ...s, cron: e.target.value })} spellCheck={false} className={`mt-1.5 font-mono ${field}`} />
          </div>
          <div>
            <label className={smallLabel} htmlFor={`sc-tz-${index}`}>
              Time zone
            </label>
            <select id={`sc-tz-${index}`} value={s.timezone} onChange={(e) => onChange({ ...s, timezone: e.target.value })} className={`mt-1.5 ${field}`}>
              {(zones.includes(s.timezone) ? zones : [s.timezone, ...zones]).map((z) => (
                <option key={z} value={z}>
                  {z}
                </option>
              ))}
            </select>
          </div>
          <div className="text-xs text-muted sm:pt-6">
            {preview.error ? (
              <span className="text-rose-600 dark:text-rose-400">{preview.error}</span>
            ) : preview.next.length ? (
              <>Next, in your time: {preview.next.slice(0, 3).map(when).join(" · ")}</>
            ) : (
              "Minute, hour, day of month, month, day of week."
            )}
          </div>
        </div>
      ) : (
        <div>
          <label className={smallLabel} htmlFor={`sc-at-${index}`}>
            Run once at (your time)
          </label>
          <input
            id={`sc-at-${index}`}
            type="datetime-local"
            value={s.at ? toLocalInput(s.at) : ""}
            onChange={(e) => onChange({ ...s, at: e.target.value ? new Date(e.target.value).getTime() : undefined })}
            className={`mt-1.5 ${field} sm:max-w-xs`}
          />
        </div>
      )}
      {fillable.length > 0 && (
        <div className="grid grid-cols-1 gap-4 rounded-xl bg-surface-2/60 p-4 sm:grid-cols-2">
          {fillable.map((d) => (
            <div key={d.name}>
              <label className={smallLabel} htmlFor={`sc-in-${index}-${d.name}`}>
                {d.label || d.name}
                {d.required && d.default === null && <span className="text-rose-600"> *</span>}
              </label>
              {d.type === "choice" || d.type === "boolean" ? (
                <select id={`sc-in-${index}-${d.name}`} value={s.inputs[d.name] ?? ""} onChange={(e) => onChange({ ...s, inputs: { ...s.inputs, [d.name]: e.target.value } })} className={`mt-1.5 ${field} bg-surface`}>
                  <option value="">{d.default !== null ? `Default (${d.default})` : "Choose"}</option>
                  {(d.type === "boolean" ? ["true", "false"] : d.choices).map((c) => (
                    <option key={c} value={c}>
                      {c}
                    </option>
                  ))}
                </select>
              ) : (
                <input id={`sc-in-${index}-${d.name}`} value={s.inputs[d.name] ?? ""} onChange={(e) => onChange({ ...s, inputs: { ...s.inputs, [d.name]: e.target.value } })} placeholder={d.default !== null ? `Default: ${d.default}` : ""} className={`mt-1.5 ${field} bg-surface`} />
              )}
            </div>
          ))}
          <p className="text-xs text-muted sm:col-span-2">Values for the runs this schedule starts. Secret inputs can't be scheduled.</p>
        </div>
      )}
    </Card>
  );
}

/** Repeating and one-time runs. */
function SchedulesEditor({ schedules, inputs, onChange }: { schedules: Schedule[]; inputs: InputDef[]; onChange: (s: Schedule[]) => void }) {
  return (
    <div className="space-y-4">
      <div>
        <h3 className="pt-4 text-[19px] font-semibold">Schedules</h3>
        <p className="mt-1 text-sm text-muted">Run this runbook by itself, on repeat or once. Times missed while the server is down are skipped.</p>
      </div>
      {schedules.map((s, i) => (
        <ScheduleCard key={s.id || `new-${i}`} s={s} index={i} inputs={inputs} onChange={(n) => onChange(schedules.map((x, k) => (k === i ? n : x)))} onRemove={() => onChange(schedules.filter((_, k) => k !== i))} />
      ))}
      <button
        type="button"
        onClick={() => onChange([...schedules, newSchedule()])}
        className="flex h-11 items-center gap-2 rounded-xl border border-dashed border-line-strong px-4 text-sm text-muted hover:border-accent hover:text-accent"
      >
        <CalendarClock className="size-4" /> Add schedule
      </button>
    </div>
  );
}

/** Only the values the API needs: no empty inputs, and one of cron or at. */
function cleanSchedule(s: Schedule): Schedule {
  const inputs = Object.fromEntries(Object.entries(s.inputs).filter(([, v]) => v !== ""));
  return s.cron !== undefined ? { ...s, at: undefined, cron: s.cron.trim(), inputs } : { ...s, cron: undefined, inputs };
}

/** Values people fill in when they start a run. */
function InputsEditor({ inputs, onChange }: { inputs: InputDef[]; onChange: (inputs: InputDef[]) => void }) {
  const set = (i: number, patch: Partial<InputDef>) => onChange(inputs.map((d, k) => (k === i ? { ...d, ...patch } : d)));
  const example = inputs.find((d) => d.name.trim())?.name.trim() ?? "service";
  return (
    <div className="space-y-4">
      <div>
        <h3 className="text-[19px] font-semibold">Inputs</h3>
        <p className="mt-1 text-sm text-muted">
          Values asked for when someone starts a run. Use <code className="font-mono text-xs text-ink-2">{`{{ inputs.${example} }}`}</code> in a command (it becomes one safely quoted word), or{" "}
          <code className="font-mono text-xs text-ink-2">"$OPSKILLS_INPUT_{example.toUpperCase()}"</code>.
        </p>
      </div>
      {inputs.map((d, i) => (
        <Card key={i} className="grid grid-cols-1 gap-4 p-5 sm:grid-cols-2 sm:p-6">
          <div>
            <label className={smallLabel} htmlFor={`in-name-${i}`}>
              Name
            </label>
            <input id={`in-name-${i}`} required value={d.name} onChange={(e) => set(i, { name: e.target.value.toLowerCase() })} placeholder="service_name" spellCheck={false} className={`mt-1.5 font-mono ${field}`} />
          </div>
          <div>
            <label className={smallLabel} htmlFor={`in-label-${i}`}>
              Label
            </label>
            <input id={`in-label-${i}`} value={d.label} onChange={(e) => set(i, { label: e.target.value })} placeholder="Service to restart" className={`mt-1.5 ${field}`} />
          </div>
          <div>
            <label className={smallLabel} htmlFor={`in-type-${i}`}>
              Type
            </label>
            <select id={`in-type-${i}`} value={d.type} onChange={(e) => set(i, { type: e.target.value as InputKind, default: null })} className={`mt-1.5 ${field}`}>
              {kinds.map((k) => (
                <option key={k.id} value={k.id}>
                  {k.label}
                </option>
              ))}
            </select>
          </div>
          {d.type !== "secret" && (
            <div>
              <label className={smallLabel} htmlFor={`in-default-${i}`}>
                Default
              </label>
              {d.type === "boolean" ? (
                <select id={`in-default-${i}`} value={d.default ?? ""} onChange={(e) => set(i, { default: e.target.value || null })} className={`mt-1.5 ${field}`}>
                  <option value="">None</option>
                  <option value="true">Yes</option>
                  <option value="false">No</option>
                </select>
              ) : (
                <input id={`in-default-${i}`} value={d.default ?? ""} onChange={(e) => set(i, { default: e.target.value })} inputMode={d.type === "number" ? "decimal" : undefined} className={`mt-1.5 ${field}`} />
              )}
            </div>
          )}
          {d.type === "choice" && (
            <div className="sm:col-span-2">
              <label className={smallLabel} htmlFor={`in-choices-${i}`}>
                Choices, one per line
              </label>
              <textarea
                id={`in-choices-${i}`}
                rows={Math.min(8, Math.max(3, d.choices.length + 1))}
                value={d.choices.join("\n")}
                onChange={(e) => set(i, { choices: e.target.value.split("\n") })}
                className="mt-1.5 w-full resize-y rounded-xl bg-surface-2 px-4 py-3 text-sm outline-none focus:ring-2 focus:ring-accent"
              />
            </div>
          )}
          {(d.type === "text" || d.type === "secret") && (
            <div className="sm:col-span-2">
              <label className={smallLabel} htmlFor={`in-pattern-${i}`}>
                Must match (regular expression, optional)
              </label>
              <input id={`in-pattern-${i}`} value={d.pattern ?? ""} onChange={(e) => set(i, { pattern: e.target.value })} placeholder="[a-z0-9-]+" spellCheck={false} className={`mt-1.5 font-mono ${field}`} />
            </div>
          )}
          <div className="sm:col-span-2">
            <label className={smallLabel} htmlFor={`in-desc-${i}`}>
              Help text
            </label>
            <input id={`in-desc-${i}`} value={d.description} onChange={(e) => set(i, { description: e.target.value })} placeholder="Shown under the field when starting a run" className={`mt-1.5 ${field}`} />
          </div>
          <div className="flex items-center justify-between gap-3 sm:col-span-2">
            <label className="flex items-center gap-2 text-sm">
              <input type="checkbox" checked={d.required} onChange={(e) => set(i, { required: e.target.checked })} className="size-4 accent-accent" />
              Required
            </label>
            <button type="button" onClick={() => onChange(inputs.filter((_, k) => k !== i))} className="flex items-center gap-1.5 rounded-lg px-2 py-1 text-xs text-muted hover:text-rose-600" aria-label={`Remove input ${d.name || i + 1}`}>
              <Trash2 className="size-3.5" /> Remove
            </button>
          </div>
        </Card>
      ))}
      <button
        type="button"
        onClick={() => onChange([...inputs, newInput()])}
        className="flex h-11 items-center gap-2 rounded-xl border border-dashed border-line-strong px-4 text-sm text-muted hover:border-accent hover:text-accent"
      >
        <Plus className="size-4" /> Add input
      </button>
    </div>
  );
}

/** Timeout, retries and error handling for one step. */
function StepOptions({ step, index, onChange }: { step: Step; index: number; onChange: (patch: Partial<Step>) => void }) {
  const summary = stepOptions(step);
  const [open, setOpen] = useState(summary.length > 0);
  return (
    <div className="mt-3">
      <button type="button" onClick={() => setOpen((o) => !o)} aria-expanded={open} className="flex items-center gap-1.5 text-xs font-medium text-muted hover:text-ink">
        <SlidersHorizontal className="size-3.5" /> Options
        {!open && summary.length > 0 && <span className="font-normal">: {summary.join(", ")}</span>}
        <ChevronDown className={cx("size-3.5 transition-transform", open && "rotate-180")} />
      </button>
      {open && (
        <div className="mt-3 grid grid-cols-1 gap-4 rounded-xl bg-surface-2/60 p-4 sm:grid-cols-3">
          <div>
            <label className={smallLabel} htmlFor={`st-timeout-${index}`}>
              Timeout
            </label>
            <input
              id={`st-timeout-${index}`}
              value={step.timeout_seconds ?? ""}
              onChange={(e) => onChange({ timeout_seconds: durationValue(e.target.value) })}
              placeholder="Runbook default"
              className={`mt-1.5 ${field} bg-surface`}
            />
          </div>
          <div>
            <label className={smallLabel} htmlFor={`st-retries-${index}`}>
              Retries
            </label>
            <input
              id={`st-retries-${index}`}
              type="number"
              min={0}
              max={10}
              value={step.retries ?? 0}
              onChange={(e) => onChange({ retries: Math.max(0, Math.min(10, Number(e.target.value) || 0)) })}
              className={`mt-1.5 ${field} bg-surface`}
            />
          </div>
          <div>
            <label className={smallLabel} htmlFor={`st-delay-${index}`}>
              Wait between tries
            </label>
            <input
              id={`st-delay-${index}`}
              value={step.retry_delay_seconds || ""}
              onChange={(e) => onChange({ retry_delay_seconds: durationValue(e.target.value) ?? 0 })}
              placeholder="No wait"
              disabled={!step.retries}
              className={`mt-1.5 ${field} bg-surface disabled:opacity-50`}
            />
          </div>
          <label className="flex items-start gap-2 text-sm sm:col-span-3">
            <input type="checkbox" checked={!!step.continue_on_error} onChange={(e) => onChange({ continue_on_error: e.target.checked })} className="mt-0.5 size-4 accent-accent" />
            <span>
              Continue on error
              <span className="block text-xs text-muted">If this step still fails, record it and carry on as if it had succeeded.</span>
            </span>
          </label>
          <p className="text-xs text-muted sm:col-span-3">Durations: seconds, or with a unit, like 90s, 10m, 2h or 1h30m.</p>
        </div>
      )}
    </div>
  );
}

const kindsOfStep: { id: StepKind; label: string; icon: typeof Terminal }[] = [
  { id: "run", label: "Command", icon: Terminal },
  { id: "copy", label: "Copy a file", icon: FileText },
  { id: "call", label: "Call a runbook", icon: Workflow },
];

function StepKindSwitch({ kind, index, onChange }: { kind: StepKind; index: number; onChange: (k: StepKind) => void }) {
  return (
    <div role="radiogroup" aria-label={`Step ${index + 1} type`} className="mt-4 inline-flex flex-wrap gap-1 rounded-xl bg-surface-2 p-1">
      {kindsOfStep.map((k) => (
        <button
          key={k.id}
          type="button"
          role="radio"
          aria-checked={kind === k.id}
          onClick={() => onChange(k.id)}
          className={cx("flex h-8 items-center gap-1.5 rounded-lg px-3 text-xs font-medium", kind === k.id ? "bg-surface text-accent shadow-sm" : "text-muted hover:text-ink")}
        >
          <k.icon className="size-3.5" /> {k.label}
        </button>
      ))}
    </div>
  );
}

/** Where a copy step writes, and what. */
/** Add a secret whose folder a choice input picks, like `{{ inputs.env }}/DB_PASSWORD`. */
function PickedByInput({ inputs, onAdd }: { inputs: InputDef[]; onAdd: (reference: string) => void }) {
  const choices = inputs.filter((d) => d.type === "choice" && d.name);
  const [inputName, setInputName] = useState("");
  const [rest, setRest] = useState("");
  if (choices.length === 0) return null;
  const pick = inputName || choices[0].name;
  const reference = `{{ inputs.${pick} }}/${rest.replace(/^\/+/, "")}`;
  return (
    <div className="mt-4 rounded-xl bg-surface-2 p-3">
      <p className="text-xs font-medium text-ink-2">Pick the folder with a choice input</p>
      <div className="mt-2 flex flex-wrap items-center gap-2">
        <select aria-label="Choice input" value={pick} onChange={(e) => setInputName(e.target.value)} className="h-9 rounded-lg bg-surface px-2 font-mono text-xs outline-none focus:ring-2 focus:ring-accent">
          {choices.map((d) => (
            <option key={d.name} value={d.name}>
              {`{{ inputs.${d.name} }}`}
            </option>
          ))}
        </select>
        <span className="font-mono text-xs text-muted">/</span>
        <input aria-label="Rest of the secret's name" value={rest} onChange={(e) => setRest(e.target.value)} placeholder="DB_PASSWORD" spellCheck={false} className="h-9 w-48 rounded-lg bg-surface px-3 font-mono text-xs outline-none focus:ring-2 focus:ring-accent" />
        <Button
          size="sm"
          type="button"
          disabled={!/[A-Z0-9_]$/.test(rest)}
          onClick={() => {
            onAdd(reference);
            setRest("");
          }}
        >
          Add
        </Button>
      </div>
      <p className="mt-1.5 text-xs text-muted">Each run uses the secret in the folder its input names, like prod/DB_PASSWORD or staging/DB_PASSWORD. Every choice must have one.</p>
    </div>
  );
}

function CopyFields({ copy, index, vaultNames, onChange, onPickSecret }: { copy: CopyStep; index: number; vaultNames: string[]; onChange: (c: CopyStep) => void; onPickSecret: (name: string) => void }) {
  const fromSecret = copy.secret !== undefined;
  return (
    <div className="mt-4 grid grid-cols-1 gap-4 sm:grid-cols-[minmax(0,1fr)_140px]">
      <div>
        <label className={smallLabel} htmlFor={`cp-to-${index}`}>
          Destination path
        </label>
        <input id={`cp-to-${index}`} required value={copy.to} onChange={(e) => onChange({ ...copy, to: e.target.value })} placeholder="/etc/myapp/config.yml" spellCheck={false} className={`mt-1.5 font-mono ${field}`} />
      </div>
      <div>
        <label className={smallLabel} htmlFor={`cp-mode-${index}`}>
          Mode
        </label>
        <input id={`cp-mode-${index}`} value={copy.mode ?? ""} onChange={(e) => onChange({ ...copy, mode: e.target.value })} placeholder={fromSecret ? "0600" : "0644"} spellCheck={false} className={`mt-1.5 font-mono ${field}`} />
      </div>
      <div className="sm:col-span-2">
        <div role="radiogroup" aria-label="Copy from" className="inline-flex gap-1 rounded-xl bg-surface-2 p-1">
          {[
            { on: !fromSecret, label: "Text", pick: () => onChange({ to: copy.to, mode: copy.mode, content: copy.content ?? "" }) },
            { on: fromSecret, label: "Vault secret", pick: () => onChange({ to: copy.to, mode: copy.mode, secret: copy.secret ?? "" }) },
          ].map((o) => (
            <button key={o.label} type="button" role="radio" aria-checked={o.on} onClick={o.pick} className={cx("h-8 rounded-lg px-3 text-xs font-medium", o.on ? "bg-surface text-accent shadow-sm" : "text-muted hover:text-ink")}>
              {o.label}
            </button>
          ))}
        </div>
        {fromSecret ? (
          <div className="mt-3">
            <select
              aria-label={`Step ${index + 1} secret to copy`}
              required
              value={copy.secret}
              onChange={(e) => {
                onChange({ ...copy, secret: e.target.value });
                if (e.target.value) onPickSecret(e.target.value);
              }}
              className={`font-mono ${field}`}
            >
              <option value="">Choose a secret</option>
              {vaultNames.map((n) => (
                <option key={n} value={n}>
                  {n}
                </option>
              ))}
            </select>
            <p className="mt-1.5 text-xs text-muted">The secret is added to this runbook's secrets. Its value is masked if it shows up in output.</p>
          </div>
        ) : (
          <>
            <textarea
              value={copy.content ?? ""}
              onChange={(e) => onChange({ ...copy, content: e.target.value })}
              placeholder={"server_name {{ host.name }};\nlisten 8080;"}
              aria-label={`Step ${index + 1} file content`}
              rows={Math.min(14, Math.max(4, (copy.content ?? "").split("\n").length + 1))}
              spellCheck={false}
              className="mt-3 w-full resize-y rounded-xl bg-term px-4 py-3 font-mono text-[13px] leading-relaxed text-term-ink outline-none placeholder:text-white/30 focus:ring-2 focus:ring-accent"
            />
            <p className="mt-1.5 text-xs text-muted">{"{{ inputs.x }}"} and other run values are filled in as plain text. Up to 64 KB.</p>
          </>
        )}
      </div>
      <p className="text-xs text-muted sm:col-span-2">A relative path is inside the run's working folder. The file is written in full, then moved into place.</p>
    </div>
  );
}

/** Which runbook a call step runs, and the values for its inputs. */
function CallFields({ call, index, runbooks, onChange }: { call: CallStep; index: number; runbooks: Runbook[]; onChange: (c: CallStep) => void }) {
  const target = runbooks.find((r) => r.id === call.runbook);
  return (
    <div className="mt-4 space-y-4">
      <div>
        <label className={smallLabel} htmlFor={`call-rb-${index}`}>
          Runbook to run
        </label>
        <select id={`call-rb-${index}`} required value={call.runbook} onChange={(e) => onChange({ runbook: e.target.value, inputs: {} })} className={`mt-1.5 ${field}`}>
          <option value="">Choose a runbook</option>
          {runbooks.map((r) => (
            <option key={r.id} value={r.id}>
              {r.name}
            </option>
          ))}
        </select>
        <p className="mt-1.5 text-xs text-muted">It runs on the same host as this step, as its own run, and this step waits for it. A failed or cancelled call fails the step.</p>
      </div>
      {target && target.inputs.length > 0 && (
        <div className="grid grid-cols-1 gap-4 rounded-xl bg-surface-2/60 p-4 sm:grid-cols-2">
          {target.inputs.map((d) => (
            <div key={d.name}>
              <label className={smallLabel} htmlFor={`call-in-${index}-${d.name}`}>
                {d.label || d.name}
                {d.required && d.default === null && <span className="text-rose-600"> *</span>}
              </label>
              <input
                id={`call-in-${index}-${d.name}`}
                value={call.inputs[d.name] ?? ""}
                onChange={(e) => onChange({ ...call, inputs: { ...call.inputs, [d.name]: e.target.value } })}
                placeholder={d.default !== null ? `Default: ${d.default}` : d.type === "choice" ? d.choices.join(" | ") : "{{ inputs.x }} or a value"}
                type={d.type === "secret" ? "password" : "text"}
                spellCheck={false}
                className={`mt-1.5 ${field} bg-surface`}
              />
            </div>
          ))}
          <p className="text-xs text-muted sm:col-span-2">Use this runbook's values, like {"{{ inputs.x }}"} or {"{{ host.name }}"}. Empty fields use the called runbook's defaults.</p>
        </div>
      )}
    </div>
  );
}

/** Drop the fields of the kinds a step isn't, so the API sees exactly one. */
function cleanStep(s: Step): Step {
  const kind = stepKind(s);
  if (kind === "copy") {
    const c = s.copy!;
    const copy: CopyStep = c.secret !== undefined ? { to: c.to, secret: c.secret } : { to: c.to, content: c.content ?? "" };
    if (c.mode?.trim()) copy.mode = c.mode.trim();
    return { ...s, run: "", call: undefined, copy };
  }
  if (kind === "call") {
    const inputs = Object.fromEntries(Object.entries(s.call!.inputs).filter(([, v]) => v !== ""));
    return { ...s, run: "", copy: undefined, call: { runbook: s.call!.runbook, inputs } };
  }
  return { ...s, copy: undefined, call: undefined };
}

export function RunbookEditor({ id, template }: { id?: string; template?: string }) {
  const editing = !!id;
  const [input, setInput] = useState<RunbookInput>(() => structuredClone(templates.find((t) => t.key === template)?.input ?? blank));
  const [tagsText, setTagsText] = useState(input.tags.join(", "));
  const [loaded, setLoaded] = useState(!editing);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const vault = useData(() => api.secrets(), []);
  const others = useData(() => api.runbooks(), []);
  const [workspaceOf, setWorkspaceOf] = useState<string>();
  useFollowWorkspace(workspaceOf);
  const [onHosts, setOnHosts] = useState(false);

  useEffect(() => {
    if (!id) return;
    api.runbook(id).then(
      (rb) => {
        setWorkspaceOf(rb.workspace);
        setInput({
          name: rb.name,
          description: rb.description,
          tags: rb.tags,
          inputs: rb.inputs ?? [],
          steps: rb.steps,
          on_step_failure: rb.on_step_failure ?? "stop",
          output_limit: rb.output_limit,
          rollout: rb.rollout ?? defaultRollout,
          schedules: rb.schedules ?? [],
          notifications: rb.notifications ?? [],
          secrets: rb.secrets ?? [],
          targets: rb.targets ?? { hosts: [], tags: [] },
          timeout_seconds: rb.timeout_seconds,
        });
        setTagsText(rb.tags.join(", "));
        setOnHosts(!runsOnServer(rb.targets));
        setLoaded(true);
      },
      (e) => setError(e.message),
    );
  }, [id]);

  function applyTemplate(key: string) {
    const t = templates.find((x) => x.key === key);
    if (!t) return;
    setInput(structuredClone(t.input));
    setTagsText(t.input.tags.join(", "));
  }

  function setStep(i: number, patch: Partial<RunbookInput["steps"][number]>) {
    setInput((s) => ({ ...s, steps: s.steps.map((st, k) => (k === i ? { ...st, ...patch } : st)) }));
  }

  function moveStep(i: number, by: number) {
    setInput((s) => {
      const steps = [...s.steps];
      const j = i + by;
      if (j < 0 || j >= steps.length) return s;
      [steps[i], steps[j]] = [steps[j], steps[i]];
      return { ...s, steps };
    });
  }

  async function save(e: FormEvent) {
    e.preventDefault();
    if (onHosts && runsOnServer(input.targets)) {
      setError("Pick at least one host or tag to run on, or choose the OpSkills server.");
      window.scrollTo({ top: 0, behavior: "smooth" });
      return;
    }
    setBusy(true);
    setError(null);
    const body: RunbookInput = {
      ...input,
      tags: tagsText
        .split(",")
        .map((t) => t.trim())
        .filter(Boolean),
      inputs: (input.inputs ?? []).map(cleanInput),
      steps: input.steps.map(cleanStep),
      schedules: (input.schedules ?? []).map(cleanSchedule),
      notifications: (input.notifications ?? []).map(cleanNotification),
      rollout: onHosts ? cleanRollout(input.rollout) : { ...defaultRollout },
      timeout_seconds: typeof input.timeout_seconds === "string" ? (durationValue(input.timeout_seconds) ?? undefined) : input.timeout_seconds,
    };
    try {
      const rb = editing ? await api.updateRunbook(id!, body) : await api.createRunbook(body);
      go(`runbooks/${rb.id}`);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
      window.scrollTo({ top: 0, behavior: "smooth" });
    } finally {
      setBusy(false);
    }
  }

  if (!loaded) return <p className="text-sm text-muted">{error ?? "Loading..."}</p>;

  return (
    <form onSubmit={save} className="space-y-8">
      <a href={editing ? `#/runbooks/${id}` : "#/runbooks"} className="inline-flex items-center gap-1 text-sm text-muted hover:text-ink">
        <ArrowLeft className="size-3.5" /> {editing ? "Back to runbook" : "Runbooks"}
      </a>
      <PageHeader
        title={editing ? "Edit runbook" : "New runbook"}
        subtitle="Each step is a shell command. Steps run in order on the hosts you choose. Inputs let people fill in values when they start a run."
        actions={
          <>
            <Button type="button" onClick={() => go(editing ? `runbooks/${id}` : "runbooks")}>
              Cancel
            </Button>
            <Button variant="primary" type="submit" disabled={busy}>
              <Save className="size-4" />
              {busy ? "Saving..." : editing ? "Save changes" : "Create runbook"}
            </Button>
          </>
        }
      />

      {error && <ErrorNote>{error}</ErrorNote>}

      {!editing && (
        <div className="flex flex-wrap items-center gap-2 text-sm">
          <span className="mr-1 text-muted">Start from an example:</span>
          {templates.map((t) => (
            <button key={t.key} type="button" onClick={() => applyTemplate(t.key)} className="h-9 rounded-xl bg-surface px-4 text-xs font-medium text-ink-2 shadow-soft hover:text-accent">
              {t.label}
            </button>
          ))}
        </div>
      )}

      <div className="grid grid-cols-1 gap-8 xl:grid-cols-[minmax(0,1fr)_340px]">
        <div className="space-y-4">
          <InputsEditor inputs={input.inputs ?? []} onChange={(inputs) => setInput((s) => ({ ...s, inputs }))} />
          <h3 className="pt-4 text-[19px] font-semibold">Steps</h3>
          {input.steps.map((step, i) => (
            <Card key={i} className="p-5 sm:p-6">
              <div className="flex items-center gap-3">
                <span className="flex size-9 shrink-0 items-center justify-center rounded-full bg-accent-soft text-sm font-semibold text-accent">{i + 1}</span>
                <input
                  value={step.name}
                  onChange={(e) => setStep(i, { name: e.target.value })}
                  placeholder={`Step ${i + 1} name, for example "Restart the service"`}
                  aria-label={`Step ${i + 1} name`}
                  className={field}
                />
                <div className="flex shrink-0 items-center">
                  <button type="button" onClick={() => moveStep(i, -1)} disabled={i === 0} className="rounded-lg p-2 text-muted hover:text-ink disabled:opacity-30" aria-label="Move step up">
                    <ArrowUp className="size-4" />
                  </button>
                  <button type="button" onClick={() => moveStep(i, 1)} disabled={i === input.steps.length - 1} className="rounded-lg p-2 text-muted hover:text-ink disabled:opacity-30" aria-label="Move step down">
                    <ArrowDown className="size-4" />
                  </button>
                  <button
                    type="button"
                    onClick={() => setInput((s) => ({ ...s, steps: s.steps.filter((_, k) => k !== i) }))}
                    disabled={input.steps.length === 1}
                    className="rounded-lg p-2 text-muted hover:text-rose-600 disabled:opacity-30"
                    aria-label="Remove step"
                  >
                    <Trash2 className="size-4" />
                  </button>
                </div>
              </div>
              <StepKindSwitch
                kind={stepKind(step)}
                index={i}
                onChange={(k) =>
                  setStep(
                    i,
                    k === "run"
                      ? { copy: undefined, call: undefined }
                      : k === "copy"
                        ? { call: undefined, copy: step.copy ?? { to: "", content: "" } }
                        : { copy: undefined, call: step.call ?? { runbook: "", inputs: {} } },
                  )
                }
              />
              {stepKind(step) === "run" && (
                <textarea
                  value={step.run}
                  onChange={(e) => setStep(i, { run: e.target.value })}
                  placeholder="Shell command, for example: df -h"
                  aria-label={`Step ${i + 1} command`}
                  rows={Math.min(12, Math.max(3, step.run.split("\n").length + 1))}
                  spellCheck={false}
                  className="mt-4 w-full resize-y rounded-xl bg-term px-4 py-3 font-mono text-[13px] leading-relaxed text-term-ink outline-none placeholder:text-white/30 focus:ring-2 focus:ring-accent"
                />
              )}
              {step.copy && (
                <CopyFields
                  copy={step.copy}
                  index={i}
                  vaultNames={[...new Set([...(vault.data ?? []).map((s) => s.name), ...input.secrets])]}
                  onChange={(copy) => setStep(i, { copy })}
                  onPickSecret={(name) => setInput((s) => (s.secrets.includes(name) ? s : { ...s, secrets: [...s.secrets, name] }))}
                />
              )}
              {step.call && <CallFields call={step.call} index={i} runbooks={(others.data ?? []).filter((r) => r.id !== id)} onChange={(call) => setStep(i, { call })} />}
              <StepOptions step={step} index={i} onChange={(patch) => setStep(i, patch)} />
            </Card>
          ))}
          <button
            type="button"
            onClick={() => setInput((s) => ({ ...s, steps: [...s.steps, { name: "", run: "" }] }))}
            className="flex h-11 items-center gap-2 rounded-xl border border-dashed border-line-strong px-4 text-sm text-muted hover:border-accent hover:text-accent"
          >
            <Plus className="size-4" /> Add step
          </button>
          <SchedulesEditor schedules={input.schedules ?? []} inputs={input.inputs ?? []} onChange={(schedules) => setInput((s) => ({ ...s, schedules }))} />
          <NotificationsEditor notifications={input.notifications ?? []} onChange={(notifications) => setInput((s) => ({ ...s, notifications }))} />
        </div>

        <aside className="space-y-4">
          <Card className="space-y-4 p-6">
            <div>
              <label className="text-sm font-medium" htmlFor="rb-name">
                Name
              </label>
              <input id="rb-name" required maxLength={100} value={input.name} onChange={(e) => setInput({ ...input, name: e.target.value })} placeholder="Restart the web tier" className={`mt-2 ${field}`} />
            </div>
            <div>
              <label className="text-sm font-medium" htmlFor="rb-desc">
                Description
              </label>
              <textarea
                id="rb-desc"
                maxLength={1000}
                rows={3}
                value={input.description}
                onChange={(e) => setInput({ ...input, description: e.target.value })}
                placeholder="What it does and when to use it"
                className="mt-2 w-full resize-y rounded-xl bg-surface-2 px-4 py-3 text-sm outline-none focus:ring-2 focus:ring-accent"
              />
            </div>
            <div>
              <label className="text-sm font-medium" htmlFor="rb-tags">
                Tags
              </label>
              <input id="rb-tags" value={tagsText} onChange={(e) => setTagsText(e.target.value)} placeholder="web, prod" className={`mt-2 ${field}`} />
              <p className="mt-1.5 text-xs text-muted">Separate with commas.</p>
            </div>
            <div>
              <label className="text-sm font-medium" htmlFor="rb-timeout">
                Step timeout
              </label>
              <input id="rb-timeout" value={input.timeout_seconds ?? ""} onChange={(e) => setInput({ ...input, timeout_seconds: e.target.value })} placeholder="600" className={`mt-2 ${field}`} />
              <p className="mt-1.5 text-xs text-muted">Seconds, or like 10m or 2h. A step still running after this is stopped and fails. Steps can override it.</p>
            </div>
          </Card>
          <Card className="space-y-4 p-6">
            <div>
              <p className="text-sm font-medium">When a step fails</p>
              <div role="radiogroup" aria-label="When a step fails" className="mt-3 space-y-2">
                {(
                  [
                    ["stop", "Stop the run", "Skip every later step."],
                    ["finish_then_fail", "Finish, then fail", "Run the remaining steps, then mark the run as failed. Good for cleanup steps."],
                  ] as const
                ).map(([id, label, hint]) => {
                  const on = (input.on_step_failure ?? "stop") === id;
                  return (
                    <button
                      key={id}
                      type="button"
                      role="radio"
                      aria-checked={on}
                      onClick={() => setInput((s) => ({ ...s, on_step_failure: id }))}
                      className={cx("block w-full rounded-xl px-4 py-3 text-left", on ? "bg-accent-soft ring-1 ring-accent" : "bg-surface-2 hover:bg-surface-3")}
                    >
                      <span className="block text-sm font-medium">{label}</span>
                      <span className="block text-xs text-muted">{hint}</span>
                    </button>
                  );
                })}
              </div>
              <p className="mt-2 text-xs text-muted">Steps set to continue on error never fail the run.</p>
            </div>
            <div>
              <label className="text-sm font-medium" htmlFor="rb-limit">
                Output limit (lines)
              </label>
              <div className="mt-2 grid grid-cols-[minmax(0,1fr)_auto] gap-2">
                <input
                  id="rb-limit"
                  type="number"
                  min={10}
                  max={MAX_OUTPUT_LINES}
                  value={input.output_limit?.lines ?? MAX_OUTPUT_LINES}
                  onChange={(e) => setInput((s) => ({ ...s, output_limit: { lines: Number(e.target.value) || MAX_OUTPUT_LINES, action: s.output_limit?.action ?? "truncate" } }))}
                  className={field}
                />
                <select
                  aria-label="When the limit is reached"
                  value={input.output_limit?.action ?? "truncate"}
                  onChange={(e) => setInput((s) => ({ ...s, output_limit: { lines: s.output_limit?.lines ?? MAX_OUTPUT_LINES, action: e.target.value as "truncate" | "fail" } }))}
                  className={`${field} w-auto`}
                >
                  <option value="truncate">then drop output</option>
                  <option value="fail">then stop the run</option>
                </select>
              </div>
              <p className="mt-1.5 text-xs text-muted">Up to {MAX_OUTPUT_LINES.toLocaleString()} lines per run.</p>
            </div>
          </Card>
          <RunOn targets={input.targets} onChange={(targets) => setInput((s) => ({ ...s, targets }))} onHosts={onHosts} setOnHosts={setOnHosts} />
          {onHosts && <RolloutCard rollout={input.rollout ?? defaultRollout} stepPolicy={input.on_step_failure ?? "stop"} onChange={(rollout) => setInput((s) => ({ ...s, rollout }))} />}
          <Card className="p-6">
            <div className="flex items-center justify-between gap-3">
              <p className="text-sm font-medium">Secrets</p>
              <a href="#/vault" className="text-xs font-medium text-accent hover:underline">
                Open the vault
              </a>
            </div>
            <p className="mt-1 text-xs text-muted">Selected secrets are environment variables in every step, named after their last part. Their values are masked in the output.</p>
            <div className="mt-3 flex flex-wrap gap-2">
              {[...(vault.data ?? []).map((sec) => sec.name), ...input.secrets.filter((r) => !(vault.data ?? []).some((sec) => sec.name === r))].map((name) => {
                const on = input.secrets.includes(name);
                return (
                  <button
                    key={name}
                    type="button"
                    aria-pressed={on}
                    onClick={() => setInput((s) => ({ ...s, secrets: on ? s.secrets.filter((x) => x !== name) : [...s.secrets, name] }))}
                    className={`flex h-8 items-center gap-1.5 rounded-lg px-2.5 font-mono text-xs ${on ? "bg-accent text-accent-ink" : "bg-surface-2 text-ink-2 hover:bg-surface-3"}`}
                  >
                    {on ? <Check className="size-3.5" /> : <KeyRound className="size-3.5" />}
                    {name}
                  </button>
                );
              })}
              {vault.data?.length === 0 && input.secrets.length === 0 && <p className="text-xs text-muted">The vault is empty. Add a secret there first.</p>}
            </div>
            <PickedByInput inputs={input.inputs ?? []} onAdd={(r) => setInput((s) => (s.secrets.includes(r) ? s : { ...s, secrets: [...s.secrets, r] }))} />
            {input.secrets.length > 0 && (
              <p className="mt-3 text-xs text-muted">
                Use it in a command as <code className="font-mono text-ink-2">"${secretEnvName(input.secrets[0])}"</code>.
              </p>
            )}
          </Card>
          <Card className="p-6 text-sm text-muted">
            <p className="font-medium text-ink">How steps run</p>
            <ul className="mt-2 list-disc space-y-1 pl-5">
              <li>
                With <code className="font-mono text-xs">/bin/sh</code>: on the OpSkills server as the user it runs as, or on a host as root through the OpSkills agent.
              </li>
              <li>In a fresh temporary folder for each run, on each host.</li>
              <li>Each host runs all the steps in order. Several hosts run one by one, in parallel or in batches, as the rollout says.</li>
              <li>
                <code className="font-mono text-xs">$OPSKILLS_RUN_ID</code>, <code className="font-mono text-xs">$OPSKILLS_STEP</code>, <code className="font-mono text-xs">$OPSKILLS_HOST_NAME</code> and one{" "}
                <code className="font-mono text-xs">$OPSKILLS_INPUT_...</code> per input are set for each step.
              </li>
              <li>
                <code className="font-mono text-xs">{"{{ host.name }}"}</code>, <code className="font-mono text-xs">{"{{ run.id }}"}</code> and <code className="font-mono text-xs">{"{{ inputs.x }}"}</code> in a command become quoted values.
              </li>
              <li>A step that exits with a non-zero code fails. What happens next is set under "When a step fails".</li>
            </ul>
          </Card>
        </aside>
      </div>
    </form>
  );
}
