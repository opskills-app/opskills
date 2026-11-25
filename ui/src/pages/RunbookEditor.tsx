import { useEffect, useState, type FormEvent } from "react";
import { ArrowDown, ArrowLeft, ArrowUp, Check, KeyRound, Plus, Save, Trash2 } from "lucide-react";
import { api, useData, type RunbookInput } from "../api";
import { Button, Card, ErrorNote, PageHeader } from "../components/ui";
import { templates } from "../templates";
import { go } from "../router";

const blank: RunbookInput = { name: "", description: "", tags: [], steps: [{ name: "", run: "" }], secrets: [], timeout_seconds: 600 };

const field = "h-11 w-full rounded-xl bg-surface-2 px-4 text-sm outline-none focus:ring-2 focus:ring-accent";

export function RunbookEditor({ id, template }: { id?: string; template?: string }) {
  const editing = !!id;
  const [input, setInput] = useState<RunbookInput>(() => structuredClone(templates.find((t) => t.key === template)?.input ?? blank));
  const [tagsText, setTagsText] = useState(input.tags.join(", "));
  const [loaded, setLoaded] = useState(!editing);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const vault = useData(() => api.secrets(), []);

  useEffect(() => {
    if (!id) return;
    api.runbook(id).then(
      (rb) => {
        setInput({ name: rb.name, description: rb.description, tags: rb.tags, steps: rb.steps, secrets: rb.secrets ?? [], timeout_seconds: rb.timeout_seconds });
        setTagsText(rb.tags.join(", "));
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
    setBusy(true);
    setError(null);
    const body: RunbookInput = {
      ...input,
      tags: tagsText
        .split(",")
        .map((t) => t.trim())
        .filter(Boolean),
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
        subtitle="Each step is a shell command. Steps run in order on this OpSkills server, and the first failing step stops the run."
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
          <h3 className="text-[19px] font-semibold">Steps</h3>
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
              <textarea
                value={step.run}
                onChange={(e) => setStep(i, { run: e.target.value })}
                placeholder="Shell command, for example: df -h"
                aria-label={`Step ${i + 1} command`}
                rows={Math.min(12, Math.max(3, step.run.split("\n").length + 1))}
                spellCheck={false}
                className="mt-4 w-full resize-y rounded-xl bg-term px-4 py-3 font-mono text-[13px] leading-relaxed text-term-ink outline-none placeholder:text-white/30 focus:ring-2 focus:ring-accent"
              />
            </Card>
          ))}
          <button
            type="button"
            onClick={() => setInput((s) => ({ ...s, steps: [...s.steps, { name: "", run: "" }] }))}
            className="flex h-11 items-center gap-2 rounded-xl border border-dashed border-line-strong px-4 text-sm text-muted hover:border-accent hover:text-accent"
          >
            <Plus className="size-4" /> Add step
          </button>
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
                Step timeout (seconds)
              </label>
              <input
                id="rb-timeout"
                type="number"
                min={1}
                max={86400}
                value={input.timeout_seconds ?? 600}
                onChange={(e) => setInput({ ...input, timeout_seconds: Number(e.target.value) || undefined })}
                className={`mt-2 ${field}`}
              />
              <p className="mt-1.5 text-xs text-muted">A step still running after this is stopped and the run fails.</p>
            </div>
          </Card>
          <Card className="p-6">
            <div className="flex items-center justify-between gap-3">
              <p className="text-sm font-medium">Secrets</p>
              <a href="#/vault" className="text-xs font-medium text-accent hover:underline">
                Open the vault
              </a>
            </div>
            <p className="mt-1 text-xs text-muted">Selected secrets are environment variables in every step. Their values are masked in the output.</p>
            <div className="mt-3 flex flex-wrap gap-2">
              {(vault.data ?? []).map((sec) => {
                const on = input.secrets.includes(sec.name);
                return (
                  <button
                    key={sec.name}
                    type="button"
                    aria-pressed={on}
                    onClick={() => setInput((s) => ({ ...s, secrets: on ? s.secrets.filter((x) => x !== sec.name) : [...s.secrets, sec.name] }))}
                    className={`flex h-8 items-center gap-1.5 rounded-lg px-2.5 font-mono text-xs ${on ? "bg-accent text-accent-ink" : "bg-surface-2 text-ink-2 hover:bg-surface-3"}`}
                  >
                    {on ? <Check className="size-3.5" /> : <KeyRound className="size-3.5" />}
                    {sec.name}
                  </button>
                );
              })}
              {vault.data?.length === 0 && <p className="text-xs text-muted">The vault is empty. Add a secret there first.</p>}
            </div>
            {input.secrets.length > 0 && (
              <p className="mt-3 text-xs text-muted">
                Use it in a command as <code className="font-mono text-ink-2">"${input.secrets[0]}"</code>.
              </p>
            )}
          </Card>
          <Card className="p-6 text-sm text-muted">
            <p className="font-medium text-ink">How steps run</p>
            <ul className="mt-2 list-disc space-y-1 pl-5">
              <li>
                With <code className="font-mono text-xs">/bin/sh</code> on the OpSkills server, as the user the server runs as.
              </li>
              <li>In a fresh temporary folder for each run.</li>
              <li>
                <code className="font-mono text-xs">$OPSKILLS_RUN_ID</code> and <code className="font-mono text-xs">$OPSKILLS_STEP</code> are set for each step.
              </li>
              <li>A step that exits with a non-zero code stops the run.</li>
            </ul>
          </Card>
        </aside>
      </div>
    </form>
  );
}
