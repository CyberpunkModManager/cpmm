import { useQuery } from "@tanstack/react-query";
import { FolderOpen, RotateCcw, Save } from "lucide-react";
import { useEffect, useState } from "react";
import { Button, Card, ErrorBox, Input, PageHeader, Spinner, Switch } from "../components/ui";
import { api4, type SettingField } from "../lib/cpmm";
import { useJobs } from "../lib/jobs";

const GROUPS = ["Updates", "Load order", "After installing", "Downloads", "Storage", "Profiles", "Safety", "Launching"];

/** Settings page, rendered from the schema `cpmm api settings` returns. */
export function SettingsView() {
  const { run, busy } = useJobs();
  const q = useQuery({ queryKey: ["settings"], queryFn: api4.settings });
  if (q.isLoading) return <Spinner />;
  if (q.error) return <ErrorBox error={q.error} />;
  const d = q.data!;
  const set = (key: string, value: string) => run(`Setting: ${key}`, ["settings", "set", key, value]);
  return (
    <>
      <PageHeader
        title="Settings"
        subtitle="Saved in ~/.local/share/cpmm/settings.json; the command line (`cpmm settings`) uses the same file."
        actions={
          <Button variant="ghost" icon={<RotateCcw className="size-4" />} disabled={busy} onClick={() => run("Reset settings", ["settings", "reset"])}>
            Reset to defaults
          </Button>
        }
      />
      <div className="grid gap-5 lg:grid-cols-2">
        {GROUPS.map((g) => {
          const fields = d.schema.filter((f) => f.group === g);
          if (!fields.length) return null;
          return (
            <Card key={g} className="p-5">
              <h2 className="mb-4 font-semibold">{g}</h2>
              <div className="space-y-5">
                {fields.map((f) => (
                  <Field key={f.key} f={f} value={d.values[f.key]} resolved={d.resolved} disabled={busy} onSet={set} />
                ))}
              </div>
            </Card>
          );
        })}
        <Card className="p-5">
          <h2 className="mb-4 font-semibold">Nexus Mods</h2>
          <p className="text-sm text-ink-300">
            API key: {d.api_key ? <span className="text-neon-lime">configured</span> : <span className="text-neon-amber">not set</span>} (stored in
            ~/.config/nexusmods/apikey, never printed).
          </p>
          <p className="mt-2 text-xs text-ink-400">Game folder: {d.resolved.game_dir}</p>
        </Card>
      </div>
    </>
  );
}

function Field({
  f,
  value,
  resolved,
  disabled,
  onSet,
}: {
  f: SettingField;
  value: boolean | number | string;
  resolved: Record<string, string>;
  disabled: boolean;
  onSet: (k: string, v: string) => void;
}) {
  const [draft, setDraft] = useState(String(value ?? ""));
  useEffect(() => setDraft(String(value ?? "")), [value]);
  const changed = draft !== String(value ?? "");
  if (f.kind === "bool") {
    return (
      <div className="flex items-start gap-4">
        <div className="min-w-0 flex-1">
          <div className="text-sm font-medium">{f.label}</div>
          <div className="mt-0.5 text-xs text-ink-400">{f.help}</div>
        </div>
        <Switch checked={!!value} label={f.label} disabled={disabled} onChange={(v) => onSet(f.key, v ? "on" : "off")} />
      </div>
    );
  }
  return (
    <div>
      <div className="text-sm font-medium">{f.label}</div>
      <div className="mt-0.5 mb-2 text-xs text-ink-400">{f.help}</div>
      <form
        className="flex gap-2"
        onSubmit={(e) => {
          e.preventDefault();
          if (changed) onSet(f.key, draft);
        }}
      >
        <Input
          icon={f.kind === "path" ? <FolderOpen className="size-4" /> : undefined}
          value={draft}
          inputMode={f.kind === "number" ? "numeric" : undefined}
          placeholder={f.kind === "path" ? (resolved[f.key] ?? "default") : ""}
          onChange={(e) => setDraft(e.target.value)}
          className="min-w-0 flex-1"
        />
        <Button type="submit" size="md" icon={<Save className="size-4" />} disabled={disabled || !changed}>
          Save
        </Button>
      </form>
      {f.kind === "path" && resolved[f.key] && <div className="mt-1 truncate font-mono text-[11px] text-ink-400">now: {resolved[f.key]}</div>}
    </div>
  );
}
