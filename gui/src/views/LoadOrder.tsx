import { useQuery } from "@tanstack/react-query";
import clsx from "clsx";
import { ArrowRight, CheckCircle2, CircleAlert, EyeOff, Gauge, HeartPulse, Info, ListOrdered, ShieldAlert, Trash2, Wand2 } from "lucide-react";
import { useState } from "react";
import { Badge, Button, Card, Empty, ErrorBox, PageHeader, Spinner, Stat } from "../components/ui";
import { type AppState, api2, type Finding, ownerLabel } from "../lib/cpmm";
import { useJobs } from "../lib/jobs";
import { SafetyPanel } from "./Safety";

const sev: Record<Finding["severity"], { tone: "red" | "amber" | "violet" | "muted" | "cyan"; label: string; icon: typeof CircleAlert }> = {
  crash: { tone: "red", label: "Crash risk", icon: ShieldAlert },
  warning: { tone: "amber", label: "Warning", icon: CircleAlert },
  perf: { tone: "violet", label: "Performance", icon: Gauge },
  info: { tone: "cyan", label: "Info", icon: Info },
  acked: { tone: "muted", label: "Accepted", icon: CheckCircle2 },
};

export function HealthView({ state }: { state: AppState }) {
  const { run, busy } = useJobs();
  const doc = useQuery({ queryKey: ["doctor"], queryFn: api2.doctor });
  const lo = useQuery({ queryKey: ["loadorder"], queryFn: api2.loadorder });
  const [showAcked, setShowAcked] = useState(false);
  const [ruleA, setRuleA] = useState("");
  const [ruleB, setRuleB] = useState("");
  const name = (id: string) => state.mods.find((m) => m.id === id)?.name ?? id;
  const findings = (doc.data ?? []).filter((f) => showAcked || f.severity !== "acked");
  const count = (s: Finding["severity"]) => doc.data?.filter((f) => f.severity === s).length ?? 0;
  const plan = lo.data?.plan;

  return (
    <>
      <PageHeader
        title="Health & load order"
        subtitle="Crash and stutter risks, and an automatic load order: requirements and patches win over what they modify, your rules are enforced, everything else stays put."
        actions={
          <Button
            variant="primary"
            icon={<Wand2 className="size-4" />}
            disabled={busy || !plan || plan.moved.length === 0}
            onClick={() => run("Sort load order", ["sort", "--apply"])}
          >
            {plan && plan.moved.length > 0 ? `Sort (${plan.moved.length} moves)` : "Load order OK"}
          </Button>
        }
      />
      <div className="mb-6 grid grid-cols-2 gap-3 lg:grid-cols-4">
        <Stat label="Crash risks" value={count("crash")} tone={count("crash") ? "red" : "lime"} icon={<ShieldAlert className="size-5" />} />
        <Stat label="Warnings" value={count("warning")} tone={count("warning") ? "amber" : "lime"} icon={<CircleAlert className="size-5" />} />
        <Stat label="Performance" value={count("perf")} tone="violet" icon={<Gauge className="size-5" />} />
        <Stat label="Archives in modlist" value={lo.data?.modlist.length ?? "…"} tone="cyan" icon={<ListOrdered className="size-5" />} />
      </div>

      <SafetyPanel state={state} />

      <section className="mb-8">
        <div className="mb-3 flex items-center justify-between">
          <h2 className="flex items-center gap-2 font-semibold">
            <HeartPulse className="size-4 text-neon-pink" /> Health check
          </h2>
          <Button size="sm" variant="ghost" onClick={() => setShowAcked((v) => !v)}>
            {showAcked ? "Hide accepted" : `Show accepted (${count("acked")})`}
          </Button>
        </div>
        {doc.isLoading && <Spinner label="Reading archive indexes…" />}
        {doc.error && <ErrorBox error={doc.error} />}
        {doc.data && findings.length === 0 && <Empty icon={<CheckCircle2 className="size-6" />} title="No known risks" />}
        <div className="space-y-2">
          {findings.map((f) => {
            const s = sev[f.severity];
            const Icon = s.icon;
            return (
              <Card key={f.title} className={clsx("px-4 py-3", f.severity === "acked" && "opacity-60")}>
                <div className="flex items-start gap-3">
                  <Badge tone={s.tone} className="mt-0.5 w-28 justify-center">
                    <Icon className="size-3" /> {s.label}
                  </Badge>
                  <div className="min-w-0 flex-1">
                    <div className="font-medium">{f.title}</div>
                    {f.detail && <div className="mt-1 text-sm text-ink-400">{f.detail}</div>}
                    {f.fix && <div className="mt-1 text-xs text-neon-amber">{f.fix}</div>}
                    {f.actions.length > 0 && f.severity !== "acked" && (
                      <div className="mt-3 flex flex-wrap gap-2">
                        {f.actions.map((a) => (
                          <Button
                            key={a.label}
                            size="sm"
                            variant={a.style === "primary" ? "primary" : "secondary"}
                            disabled={busy}
                            onClick={() => run(a.label, a.args)}
                          >
                            {a.label}
                          </Button>
                        ))}
                      </div>
                    )}
                  </div>
                  {f.severity === "acked" ? (
                    <Button size="sm" variant="ghost" disabled={busy} onClick={() => run("Un-accept finding", ["doctor", "--unack", f.title])}>
                      Restore
                    </Button>
                  ) : (
                    <Button
                      size="sm"
                      variant="ghost"
                      icon={<EyeOff className="size-3.5" />}
                      disabled={busy}
                      onClick={() => run("Accept finding", ["doctor", "--ack", f.title])}
                      title="Known and fine: stop warning"
                    >
                      Accept
                    </Button>
                  )}
                </div>
              </Card>
            );
          })}
        </div>
      </section>

      <section className="mb-8">
        <h2 className="mb-3 flex items-center gap-2 font-semibold">
          <Wand2 className="size-4 text-neon-cyan" /> Sort preview
        </h2>
        {lo.isLoading && <Spinner />}
        {lo.error && <ErrorBox error={lo.error} />}
        {plan &&
          (plan.moved.length === 0 ? (
            <Card className="px-5 py-4 text-sm text-ink-300">
              <CheckCircle2 className="mr-2 inline size-4 text-neon-lime" />
              The current order already satisfies all {plan.edges.length} constraints between overlapping mods.
            </Card>
          ) : (
            <div className="glass overflow-hidden rounded-2xl">
              {plan.moved.map((m, i) => (
                <div key={m.id} className={clsx("flex items-center gap-3 px-4 py-3", i > 0 && "border-t border-ink-800")}>
                  <span className="min-w-0 flex-1 truncate font-medium">{name(m.id)}</span>
                  <span className="font-mono text-sm text-ink-400">{m.from}</span>
                  <ArrowRight className="size-4 text-neon-cyan" />
                  <span className="font-mono text-sm text-neon-cyan">{m.to}</span>
                  <span className="hidden max-w-md truncate text-xs text-ink-400 lg:inline">{m.because.join("; ")}</span>
                </div>
              ))}
            </div>
          ))}
        {plan && plan.cycles.length > 0 && (
          <p className="mt-2 text-sm text-neon-amber">Circular constraints left in current order: {plan.cycles.map((c) => c.join(" → ")).join("; ")}</p>
        )}
        {plan && plan.unresolved.length > 0 && (
          <div className="mt-4">
            <div className="mb-2 text-xs font-semibold uppercase tracking-wider text-ink-400">Unrelated mods replacing the same resources</div>
            <div className="space-y-1.5">
              {plan.unresolved.map((p) => (
                <div key={p.archive_a + p.archive_b} className="flex flex-wrap items-center gap-2 text-sm">
                  <span>{ownerLabel(p.a)}</span>
                  <span className="text-ink-400">vs</span>
                  <span>{ownerLabel(p.b)}</span>
                  <Badge>{p.shared - p.identical} resources</Badge>
                  <span className="text-xs text-ink-400">winner: {p.winner ?? "game order (mod.io)"}</span>
                  {p.a.kind === "mod" && p.b.kind === "mod" && (
                    <>
                      <Button size="sm" variant="ghost" disabled={busy} onClick={() => run(`Rule: ${p.a.id} wins`, ["rule", p.a.id, "--after", p.b.id])}>
                        {name(p.a.id)} wins
                      </Button>
                      <Button size="sm" variant="ghost" disabled={busy} onClick={() => run(`Rule: ${p.b.id} wins`, ["rule", p.b.id, "--after", p.a.id])}>
                        {name(p.b.id)} wins
                      </Button>
                    </>
                  )}
                </div>
              ))}
            </div>
          </div>
        )}
      </section>

      <div className="grid gap-6 lg:grid-cols-2">
        <Card className="p-5">
          <h2 className="mb-3 font-semibold">Your rules</h2>
          {lo.data?.rules.length === 0 && <p className="mb-3 text-sm text-ink-400">None. Add one when two unrelated mods fight over the same assets.</p>}
          <div className="mb-4 space-y-1.5">
            {lo.data?.rules.map((r) => (
              <div key={r.mod + r.after} className="flex items-center gap-2 text-sm">
                <span className="text-neon-lime">{name(r.mod)}</span> <span className="text-ink-400">wins over</span> {name(r.after)}
                <div className="flex-1" />
                <button
                  type="button"
                  className="rounded-lg p-1.5 text-ink-400 hover:bg-ink-800 hover:text-neon-red cursor-pointer"
                  aria-label="Remove rule"
                  onClick={() => run("Remove rule", ["rule", r.mod, "--after", r.after, "--remove"])}
                >
                  <Trash2 className="size-4" />
                </button>
              </div>
            ))}
          </div>
          <div className="flex flex-wrap items-center gap-2">
            <select
              value={ruleA}
              onChange={(e) => setRuleA(e.target.value)}
              className="h-10 min-w-0 flex-1 rounded-lg border border-ink-700 bg-ink-900 px-2 text-sm"
            >
              <option value="">Winner…</option>
              {state.mods.map((m) => (
                <option key={m.id} value={m.id}>
                  {m.name}
                </option>
              ))}
            </select>
            <span className="text-xs text-ink-400">wins over</span>
            <select
              value={ruleB}
              onChange={(e) => setRuleB(e.target.value)}
              className="h-10 min-w-0 flex-1 rounded-lg border border-ink-700 bg-ink-900 px-2 text-sm"
            >
              <option value="">Loser…</option>
              {state.mods.map((m) => (
                <option key={m.id} value={m.id}>
                  {m.name}
                </option>
              ))}
            </select>
            <Button size="sm" disabled={busy || !ruleA || !ruleB || ruleA === ruleB} onClick={() => run("Add rule", ["rule", ruleA, "--after", ruleB])}>
              Add
            </Button>
          </div>
        </Card>
        <Card className="p-5">
          <h2 className="mb-1 font-semibold">Generated modlist.txt</h2>
          <p className="mb-3 text-xs text-ink-400">
            Archive load order the game uses (first wins). Regenerated at every launch from the priority list; lives in cpmm's overlay layer, not the game
            folder.
          </p>
          <ol className="max-h-64 overflow-y-auto font-mono text-xs leading-5 text-ink-300">
            {lo.data?.modlist.map((a, i) => (
              <li key={a}>
                <span className="inline-block w-8 text-right text-ink-400">{i + 1}</span> {a}
              </li>
            ))}
          </ol>
        </Card>
      </div>
    </>
  );
}
