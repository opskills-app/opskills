import { useState, type FormEvent } from "react";
import { Play } from "lucide-react";
import type { InputDef, Runbook } from "../api";
import { Button, ErrorNote, Modal } from "./ui";

const field = "h-11 w-full rounded-xl bg-surface-2 px-4 text-sm outline-none focus:ring-2 focus:ring-accent";

/** Help text ends with a full stop so the notes after it read as their own sentences. */
const sentence = (text: string) => (/[.!?]$/.test(text.trim()) ? text.trim() : `${text.trim()}.`);

export function inputLabel(def: InputDef) {
  return def.label || def.name;
}

/** Starting values: an earlier run's (never secrets), else each input's default. */
function initial(rb: Runbook, prefill?: Record<string, string>) {
  const values: Record<string, string> = {};
  for (const def of rb.inputs) {
    const earlier = def.type === "secret" ? undefined : prefill?.[def.name];
    values[def.name] = earlier ?? def.default ?? (def.type === "boolean" ? "false" : def.type === "choice" && def.required ? def.choices[0] ?? "" : "");
  }
  return values;
}

function InputField({ def, value, onChange }: { def: InputDef; value: string; onChange: (v: string) => void }) {
  const id = `input-${def.name}`;
  const control = (() => {
    switch (def.type) {
      case "boolean":
        return (
          <div role="radiogroup" aria-labelledby={`${id}-label`} className="mt-2 grid grid-cols-2 gap-1 rounded-xl bg-surface-2 p-1">
            {["true", "false"].map((v) => (
              <button key={v} type="button" role="radio" aria-checked={value === v} onClick={() => onChange(v)} className={`h-9 rounded-lg text-xs font-medium ${value === v ? "bg-surface text-accent shadow-sm" : "text-muted hover:text-ink"}`}>
                {v === "true" ? "Yes" : "No"}
              </button>
            ))}
          </div>
        );
      case "choice":
        return (
          <select id={id} value={value} required={def.required} onChange={(e) => onChange(e.target.value)} className={`mt-2 ${field}`}>
            {!def.required && <option value="">None</option>}
            {def.choices.map((c) => (
              <option key={c} value={c}>
                {c}
              </option>
            ))}
          </select>
        );
      case "number":
        return <input id={id} type="number" step="any" inputMode="decimal" required={def.required} value={value} onChange={(e) => onChange(e.target.value)} className={`mt-2 ${field}`} />;
      case "secret":
        return <input id={id} type="password" autoComplete="off" required={def.required} value={value} onChange={(e) => onChange(e.target.value)} className={`mt-2 ${field}`} />;
      default:
        return <input id={id} required={def.required} value={value} onChange={(e) => onChange(e.target.value)} className={`mt-2 ${field}`} spellCheck={false} />;
    }
  })();
  return (
    <div>
      <label id={`${id}-label`} htmlFor={id} className="text-sm font-medium">
        {inputLabel(def)}
        {def.required && <span className="text-rose-600"> *</span>}
      </label>
      {control}
      {(def.description || def.type === "secret" || def.pattern) && (
        <p className="mt-1.5 text-xs text-muted">
          {def.description && sentence(def.description)}
          {def.type === "secret" && " Masked in the output and not stored with the run."}
          {def.pattern && (
            <>
              {" "}
              Must match <code className="font-mono">{def.pattern}</code>.
            </>
          )}
        </p>
      )}
    </div>
  );
}

/** Asks for a runbook's inputs, then starts the run. */
export function StartRunDialog({
  rb,
  prefill,
  title,
  onStart,
  onClose,
}: {
  rb: Runbook;
  prefill?: Record<string, string>;
  title?: string;
  onStart: (inputs: Record<string, string>) => Promise<void>;
  onClose: () => void;
}) {
  const [values, setValues] = useState(() => initial(rb, prefill));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function submit(e: FormEvent) {
    e.preventDefault();
    setBusy(true);
    setError(null);
    // Leave out empty optional values so the server applies defaults.
    const inputs = Object.fromEntries(Object.entries(values).filter(([, v]) => v !== ""));
    try {
      await onStart(inputs);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
      setBusy(false);
    }
  }

  return (
    <Modal
      title={title ?? `Run "${rb.name}"`}
      onClose={onClose}
      footer={
        <>
          <Button type="button" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" type="submit" form="start-run" disabled={busy}>
            <Play className="size-3.5" fill="currentColor" /> {busy ? "Starting..." : "Start run"}
          </Button>
        </>
      }
    >
      <form id="start-run" onSubmit={submit} className="space-y-5">
        {error && <ErrorNote>{error}</ErrorNote>}
        {rb.inputs.map((def) => (
          <InputField key={def.name} def={def} value={values[def.name] ?? ""} onChange={(v) => setValues((s) => ({ ...s, [def.name]: v }))} />
        ))}
      </form>
    </Modal>
  );
}
