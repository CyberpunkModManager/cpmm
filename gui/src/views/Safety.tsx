import { useQuery } from "@tanstack/react-query";
import clsx from "clsx";
import { Bug, Camera, FileWarning, History, RotateCcw, ShieldCheck, Undo2, Wrench } from "lucide-react";
import { useState } from "react";
import { Badge, Button, Card, Confirm, ErrorBox, Spinner } from "../components/ui";
import { type AppState, api, when } from "../lib/cpmm";
import { useJobs } from "../lib/jobs";

/** Crash triage, new log errors, the game-patch guard, save snapshots and undoable update runs. */
export function SafetyPanel({ state }: { state: AppState }) {
  const { run, busy } = useJobs();
  const q = useQuery({ queryKey: ["safety"], queryFn: api.safety, staleTime: 30_000 });
  const [restore, setRestore] = useState<string | null>(null);
  const [showErr, setShowErr] = useState(false);
  const name = (id: string) => state.mods.find((m) => m.id === id)?.name ?? id;
  const locked = state.game_running || !!state.session;
  if (q.isLoading) return <Spinner label="Checking crash reports, logs and saves…" />;
  if (q.error) return <ErrorBox error={q.error} />;
  const s = q.data!;
  const newErr = Object.entries(s.errors.new);
  const newCount = newErr.reduce((n, [, v]) => n + v.length, 0);
  const fixedCount = Object.values(s.errors.fixed).reduce((n, v) => n + v.length, 0);

  return (
    <section className="mb-8">
      <h2 className="mb-3 flex items-center gap-2 font-semibold">
        <ShieldCheck className="size-4 text-neon-lime" /> Safety net
      </h2>
      <div className="grid gap-4 lg:grid-cols-2">
        {/* Patch guard */}
        <Card className="p-5">
          <h3 className="mb-2 flex items-center gap-2 text-sm font-semibold">
            <Wrench className="size-4 text-neon-amber" /> Game patch guard
          </h3>
          <p className="text-sm text-ink-300">
            Build <span className="font-mono">{s.patch.build_id}</span>, updated {when(s.patch.patched_at)}.
          </p>
          {s.patch.patch_detected ? (
            s.patch.at_risk.length ? (
              <div className="mt-3 space-y-1.5">
                <p className="text-sm text-neon-amber">The game updated; these native mods may not support it yet:</p>
                {s.patch.at_risk.map((f) => (
                  <div key={f.id} className="flex items-center gap-2 text-sm">
                    <span className="min-w-0 flex-1 truncate">{f.name}</span>
                    {f.update_available ? (
                      <Button
                        size="sm"
                        variant="primary"
                        disabled={busy || locked}
                        onClick={() => run(`Update ${f.name}`, { op: "update", mods: [f.id], all: false })}
                      >
                        Update
                      </Button>
                    ) : (
                      <Badge tone="amber">no release yet</Badge>
                    )}
                  </div>
                ))}
              </div>
            ) : (
              <p className="mt-2 text-sm text-neon-lime">The game updated and every native framework has released since.</p>
            )
          ) : (
            <p className="mt-2 text-sm text-ink-400">
              No game update since cpmm started watching. If Steam updates Cyberpunk, cpmm checks RED4ext, redscript, CET and the other native mods before your
              next launch.
            </p>
          )}
          {s.patch.red4ext_errors.length > 0 && <p className="mt-2 font-mono text-xs text-neon-red">{s.patch.red4ext_errors[0]}</p>}
          <div className="mt-4 flex items-center gap-2">
            <Button
              size="sm"
              variant={s.patch.vanilla ? "primary" : "secondary"}
              disabled={busy}
              onClick={() => run(s.patch.vanilla ? "Modded launch" : "Vanilla launch", { op: "vanilla", on: !s.patch.vanilla })}
            >
              {s.patch.vanilla ? "Next launch: vanilla (tap to cancel)" : "Launch vanilla next time"}
            </Button>
          </div>
        </Card>

        {/* New errors */}
        <Card className="p-5">
          <h3 className="mb-2 flex items-center gap-2 text-sm font-semibold">
            <FileWarning className="size-4 text-neon-violet" /> Log errors since the last launch
          </h3>
          {s.errors.baseline ? (
            <p className="text-sm text-ink-400">Baseline recorded from your last session's logs. From the next launch, only new errors show up here.</p>
          ) : (
            <p className="text-sm text-ink-300">
              <span className={newCount ? "text-neon-amber" : "text-neon-lime"}>{newCount} new</span> · {fixedCount} gone · logs from{" "}
              {when(s.errors.latest_log)}
            </p>
          )}
          <div className="mt-3 flex flex-wrap gap-1.5">
            {Object.entries(s.errors.totals).map(([src, n]) => (
              <Badge key={src} tone={s.errors.new[src] ? "amber" : "muted"}>
                {src} {n}
                {s.errors.new[src] ? ` (+${s.errors.new[src].length})` : ""}
              </Badge>
            ))}
          </div>
          {newCount > 0 && (
            <>
              <Button size="sm" variant="ghost" className="mt-3" onClick={() => setShowErr((v) => !v)}>
                {showErr ? "Hide" : "Show"} new errors
              </Button>
              {showErr && (
                <pre className="mt-2 max-h-56 overflow-y-auto rounded-lg bg-ink-900 p-3 font-mono text-[11px] leading-4 whitespace-pre-wrap text-ink-300 select-text">
                  {newErr.map(([src, v]) => v.map((l) => `[${src}] ${l}`).join("\n")).join("\n")}
                </pre>
              )}
            </>
          )}
        </Card>

        {/* Crashes */}
        <Card className="p-5 lg:col-span-2">
          <h3 className="mb-3 flex items-center gap-2 text-sm font-semibold">
            <Bug className="size-4 text-neon-red" /> Crashes
          </h3>
          {s.crashes.length === 0 && <p className="text-sm text-ink-400">No crash reports.</p>}
          <div className="space-y-3">
            {s.crashes.slice(0, 3).map((c, i) => (
              <div key={c.report} className={clsx("rounded-xl border px-4 py-3", i === 0 ? "border-neon-red/30 bg-neon-red/5" : "border-ink-700")}>
                <div className="flex flex-wrap items-center gap-2 text-sm">
                  <span className="font-medium">{when(c.ts)}</span>
                  {c.seconds_in !== null && (
                    <Badge>
                      {Math.floor(c.seconds_in / 60)}m {c.seconds_in % 60}s in
                    </Badge>
                  )}
                  <Badge tone="red">
                    {c.module} {c.offset}
                  </Badge>
                </div>
                <div className="mt-1 text-xs text-ink-400">{c.message}</div>
                {c.session_start === null ? (
                  <div className="mt-2 text-xs text-ink-400">
                    Before cpmm recorded sessions; future crashes show what changed since your last clean session.
                  </div>
                ) : c.changes.length ? (
                  <div className="mt-2 text-xs">
                    <div className="text-ink-300">Changed since the last clean session ({when(c.last_good ?? 0)}):</div>
                    <ul className="mt-1 list-inside list-disc text-ink-400">
                      {c.changes.map((x) => (
                        <li key={x}>{x}</li>
                      ))}
                    </ul>
                  </div>
                ) : (
                  <div className="mt-2 text-xs text-ink-400">No mod changes since the last clean session.</div>
                )}
                {c.suspects.length > 0 && (
                  <div className="mt-3 flex flex-wrap gap-2">
                    {c.suspects.slice(0, 5).map((m) => (
                      <Button
                        key={m}
                        size="sm"
                        icon={<RotateCcw className="size-3.5" />}
                        disabled={busy || locked}
                        onClick={() => run(`Roll back ${name(m)}`, { op: "rollback", mod: m, to: null })}
                      >
                        Roll back {name(m)}
                      </Button>
                    ))}
                  </div>
                )}
              </div>
            ))}
          </div>
        </Card>

        {/* Update runs */}
        <Card className="p-5">
          <h3 className="mb-2 flex items-center gap-2 text-sm font-semibold">
            <Undo2 className="size-4 text-neon-cyan" /> Update runs
          </h3>
          {s.batches.length === 0 && <p className="text-sm text-ink-400">Nothing to undo yet. Every Update / Update all can be undone here in one tap.</p>}
          <div className="space-y-2">
            {s.batches.slice(0, 4).map((b) => (
              <div key={b.batch} className="flex items-center gap-2 text-sm">
                <div className="min-w-0 flex-1">
                  <div>{when(b.ts)}</div>
                  <div className="truncate text-xs text-ink-400">{b.mods.map(([id, v]) => `${name(id)} → ${v}`).join(", ")}</div>
                </div>
                <Button size="sm" disabled={busy || locked} onClick={() => run("Undo update run", { op: "undo_update", batch: b.batch })}>
                  Undo
                </Button>
              </div>
            ))}
          </div>
        </Card>

        {/* Saves */}
        <Card className="p-5">
          <div className="mb-2 flex items-center justify-between">
            <h3 className="flex items-center gap-2 text-sm font-semibold">
              <Camera className="size-4 text-neon-pink" /> Save snapshots
            </h3>
            <Button size="sm" variant="ghost" disabled={busy} onClick={() => run("Snapshot saves", { op: "snapshot_saves" })}>
              Snapshot now
            </Button>
          </div>
          <p className="mb-3 text-xs text-ink-400">
            Taken automatically before disabling, uninstalling, updating or rolling back mods, and before a tool that can edit saves runs. Copy-on-write: no
            extra disk space.
          </p>
          {s.save_risks.map((r) => (
            <p key={r.save} className="mb-2 text-xs text-neon-amber">
              {r.save} was saved with {r.missing.join(", ")}, which {r.missing.length > 1 ? "are" : "is"} now off.
            </p>
          ))}
          <div className="space-y-1.5">
            {s.snapshots.slice(0, 6).map((sn) => (
              <div key={sn.name} className="flex items-center gap-2 text-sm">
                <History className="size-3.5 text-ink-400" />
                <span className="w-36 shrink-0">{when(sn.ts)}</span>
                <span className="min-w-0 flex-1 truncate text-xs text-ink-400">{sn.reason}</span>
                <Button size="sm" variant="ghost" disabled={busy || locked} onClick={() => setRestore(sn.name)}>
                  Restore
                </Button>
              </div>
            ))}
          </div>
        </Card>
      </div>
      <Confirm
        open={!!restore}
        title="Restore these saves?"
        body="Your current saves are snapshotted first, so this can be undone the same way."
        confirmLabel="Restore"
        onCancel={() => setRestore(null)}
        onConfirm={() => {
          const r = restore!;
          setRestore(null);
          run("Restore saves", { op: "restore_saves", name: r });
        }}
      />
    </section>
  );
}

/** Earlier versions of one mod, for its detail drawer. */
export function ModHistory({ id, locked }: { id: string; locked: boolean }) {
  const { run, busy } = useJobs();
  const q = useQuery({ queryKey: ["history", id], queryFn: () => api.history(id) });
  if (!q.data?.length) return <p className="text-sm text-ink-400">No earlier versions kept yet. The last 3 are kept when a mod is updated or replaced.</p>;
  return (
    <div className="space-y-2">
      {q.data.map((h) => (
        <div key={h.ts} className="flex items-center gap-3 rounded-xl border border-ink-700 bg-ink-850 px-3 py-2 text-sm">
          <div className="min-w-0 flex-1">
            <div>
              v{h.version || "?"}{" "}
              <span className="text-xs text-ink-400">
                · {h.files} files · {when(h.ts)}
              </span>
            </div>
            <div className="truncate text-xs text-ink-400">{h.reason}</div>
          </div>
          <Button
            size="sm"
            icon={<RotateCcw className="size-3.5" />}
            disabled={busy || locked}
            onClick={() => run(`Roll back to v${h.version}`, { op: "rollback", mod: id, to: h.ts })}
          >
            Roll back
          </Button>
        </div>
      ))}
    </div>
  );
}
