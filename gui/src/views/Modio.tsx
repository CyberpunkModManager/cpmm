import { useQuery } from "@tanstack/react-query";
import clsx from "clsx";
import { Radar } from "lucide-react";
import { Badge, Button, Card } from "../components/ui";
import { type AppState, api } from "../lib/cpmm";
import { useJobs } from "../lib/jobs";

/** mod.io watcher: subscriptions made in the in-game Mods menu, what changed, and collisions. */
export function ModioCard({ state }: { state: AppState }) {
  const { run, busy } = useJobs();
  const q = useQuery({ queryKey: ["modio"], queryFn: api.modio, staleTime: 30_000 });
  const d = q.data;
  if (!d) return null;
  const name = (id: string) => state.mods.find((m) => m.id === id)?.name ?? id;
  const changed = !d.changes.first_look && d.changes.added.length + d.changes.updated.length + d.changes.removed.length > 0;
  return (
    <Card className="mb-8 p-5">
      <div className="mb-3 flex flex-wrap items-center justify-between gap-2">
        <h2 className="flex items-center gap-2 font-semibold">
          <Radar className="size-4 text-neon-cyan" /> mod.io watcher
        </h2>
        {(changed || (d.changes.first_look && d.mods.length > 0)) && (
          <Button size="sm" disabled={busy} onClick={() => run("Mark mod.io as reviewed", ["modio", "--ack"])}>
            Mark as reviewed
          </Button>
        )}
      </div>
      <p className="mb-3 text-xs text-ink-400">
        Mods subscribed in the game's Mods menu load outside cpmm. cpmm watches them for new subscriptions and checks them against your mods. Unsubscribe in the
        in-game Mods menu (then restart the game).
      </p>
      {d.mods.length === 0 ? (
        <p className="text-sm text-neon-lime">No mod.io subscriptions.</p>
      ) : (
        <div className="space-y-2">
          {changed && (
            <div className="rounded-lg border border-neon-amber/30 bg-neon-amber/5 px-3 py-2 text-sm text-neon-amber">
              Since your last review:{" "}
              {[
                ...d.changes.added.map((a) => `new ${a}`),
                ...d.changes.updated.map((u) => `updated ${u}`),
                ...d.changes.removed.map((r) => `removed ${r}`),
              ].join(" · ")}
            </div>
          )}
          {d.mods.map((m) => {
            const cs = d.conflicts.filter((c) => c.modio === m.name);
            return (
              <div key={m.id} className="rounded-xl border border-ink-700 px-3 py-2">
                <div className="flex flex-wrap items-center gap-2 text-sm">
                  <span className="font-medium">{m.name}</span>
                  <span className="text-xs text-ink-400">
                    {m.version} · {m.author}
                  </span>
                  {d.changes.added.includes(m.name) && <Badge tone="amber">new</Badge>}
                  {cs.length === 0 && <Badge tone="lime">no conflicts</Badge>}
                </div>
                {cs.map((c) => (
                  <div key={c.kind + c.with} className={clsx("mt-1 text-xs", c.severe ? "text-neon-red" : "text-ink-400")}>
                    {c.kind} with {name(c.with)}: {c.detail}
                  </div>
                ))}
              </div>
            );
          })}
        </div>
      )}
    </Card>
  );
}
