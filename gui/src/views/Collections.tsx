import { useQuery } from "@tanstack/react-query";
import { Library, Search } from "lucide-react";
import { useState } from "react";
import { Badge, Button, Card, ErrorBox, Input, Spinner, Switch } from "../components/ui";
import { type AppState, api9 } from "../lib/cpmm";
import { useJobs } from "../lib/jobs";

const tone = { installed: "lime", "other version": "amber", missing: "muted", bundled: "violet", manual: "red" } as const;

/** Install a Nexus collection: exact versions, the curator's installer choices and load order. */
export function CollectionsCard({ state }: { state: AppState }) {
  const { run, busy } = useJobs();
  const [ref, setRef] = useState("");
  const [look, setLook] = useState<string | null>(null);
  const [optional, setOptional] = useState(false);
  const [newProfile, setNewProfile] = useState(true);
  const q = useQuery({ queryKey: ["collection", look], queryFn: () => api9.collection(look!), enabled: !!look });
  const c = q.data;
  const counts: Record<string, number> = {};
  for (const m of c?.mods ?? []) counts[m.status] = (counts[m.status] ?? 0) + 1;
  return (
    <Card className="mb-6 p-5">
      <h2 className="mb-1 flex items-center gap-2 font-semibold">
        <Library className="size-4 text-neon-violet" /> Nexus collection
      </h2>
      <p className="mb-3 text-xs text-ink-400">
        Paste a collection link from nexusmods.com. cpmm installs the curator's exact file versions, their installer options and load-order rules, by default
        into a new profile so your current mod list stays as it is.
      </p>
      <form
        className="flex gap-2"
        onSubmit={(e) => {
          e.preventDefault();
          if (ref.trim()) setLook(ref.trim());
        }}
      >
        <Input
          icon={<Search className="size-4" />}
          value={ref}
          onChange={(e) => setRef(e.target.value)}
          placeholder="https://www.nexusmods.com/games/cyberpunk2077/collections/…"
          className="min-w-0 flex-1"
        />
        <Button type="submit" disabled={!ref.trim()}>
          Look up
        </Button>
      </form>
      {q.isFetching && <Spinner label="Reading the collection…" />}
      {q.error && (
        <div className="mt-3">
          <ErrorBox error={q.error} />
        </div>
      )}
      {c && !q.isFetching && (
        <div className="mt-4 border-t border-ink-700 pt-4">
          <div className="flex flex-wrap items-start justify-between gap-3">
            <div>
              <div className="text-lg font-semibold">{c.name}</div>
              <div className="text-sm text-ink-400">
                by {c.author} · revision {c.revision} · {c.mods.length} mods{c.optional ? ` (${c.optional} optional)` : ""} · {c.rules} load-order rules
              </div>
              <div className="mt-2 flex flex-wrap gap-1.5">
                {Object.entries(counts).map(([k, n]) => (
                  <Badge key={k} tone={tone[k as keyof typeof tone] ?? "muted"}>
                    {n} {k}
                  </Badge>
                ))}
                {c.installed && <Badge tone="cyan">installed before</Badge>}
              </div>
            </div>
            <Button
              variant="primary"
              disabled={busy || state.game_running || !!state.session}
              onClick={() =>
                run(`Install collection ${c.name}`, [
                  "collection",
                  "install",
                  c.slug,
                  ...(optional ? ["--optional"] : []),
                  ...(newProfile ? [] : ["--into-current"]),
                ])
              }
            >
              Install collection
            </Button>
          </div>
          <div className="mt-4 grid gap-3 sm:grid-cols-2">
            <div className="flex items-center gap-3 text-sm">
              <Switch checked={newProfile} label="Install into a new profile" onChange={setNewProfile} />
              Install into a new profile named after it
            </div>
            {c.optional > 0 && (
              <div className="flex items-center gap-3 text-sm">
                <Switch checked={optional} label="Include optional mods" onChange={setOptional} />
                Include the {c.optional} optional mods
              </div>
            )}
          </div>
          {c.summary && <p className="mt-3 text-sm text-ink-300">{c.summary}</p>}
          <div className="mt-3 max-h-64 overflow-y-auto rounded-xl border border-ink-700">
            {c.mods.map((m) => (
              <div key={`${m.mod_id}-${m.file_id}-${m.name}`} className="flex items-center gap-2 border-b border-ink-800 px-3 py-1.5 text-sm last:border-0">
                <Badge tone={tone[m.status as keyof typeof tone] ?? "muted"} className="w-24 justify-center">
                  {m.status}
                </Badge>
                <span className="min-w-0 flex-1 truncate">{m.name}</span>
                {m.fomod && <Badge tone="violet">options</Badge>}
                {m.optional && <Badge>optional</Badge>}
                <span className="font-mono text-xs text-ink-400">{m.version}</span>
              </div>
            ))}
          </div>
          {c.instructions && <p className="mt-3 line-clamp-4 text-xs whitespace-pre-wrap text-ink-400">{c.instructions}</p>}
        </div>
      )}
    </Card>
  );
}
