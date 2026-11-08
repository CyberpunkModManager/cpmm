import { useQuery } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import { Check, ChevronDown, ChevronRight, FileText, Info, PackagePlus, Play, RotateCcw, Save, Trash2, TriangleAlert, Wrench, X } from "lucide-react";
import { useState } from "react";
import { Badge, Button, Card, Confirm, Drawer, Empty, ErrorBox, Input, PageHeader, Spinner } from "../components/ui";
import { type AppState, api, type Progress, type ToolAction, type ToolEntry, type ToolRun, timeAgo } from "../lib/cpmm";
import { useJobs } from "../lib/jobs";

const mb = (b: number) => (b >= 1 << 30 ? `${(b / (1 << 30)).toFixed(1)} GB` : `${(b / (1 << 20)).toFixed(1)} MB`);
const base = (p: string) => p.split("/").pop() ?? p;
/** Reports worth opening: what the tool wrote under tools/, minus its history and the JSON twin of an HTML report. */
const reports = (r: ToolRun) => {
  const all = r.outputs.filter((o) => o.kind === "report" && !o.path.toLowerCase().includes("/history/"));
  const html = new Set(all.filter((o) => /\.html?$/i.test(o.path)).map((o) => o.path.replace(/\.html?$/i, "")));
  return all.filter((o) => /\.(html?|md|txt|log|csv)$/i.test(o.path) || (/\.json$/i.test(o.path) && !html.has(o.path.replace(/\.json$/i, ""))));
};

/** Windows tools (VRAM RipperDoc, …) run through Proton with the mods mounted; their writes are caught for review. */
export function ToolsView({ state }: { state: AppState }) {
  const { run, busy, jobs } = useJobs();
  const q = useQuery({ queryKey: ["tools"], queryFn: api.tools, refetchInterval: busy ? 3000 : 15_000 });
  const [source, setSource] = useState("");
  const [ask, setAsk] = useState<{ tool: ToolEntry; action: ToolAction; value: string } | null>(null);
  const [save, setSave] = useState<{ run: ToolRun; name: string } | null>(null);
  const [remove, setRemove] = useState<ToolEntry | null>(null);
  const [discard, setDiscard] = useState<ToolRun | null>(null);
  const [text, setText] = useState<{ run: ToolRun; path: string } | null>(null);
  const [others, setOthers] = useState<Record<string, boolean>>({});
  const live = jobs.find((j) => j.status === "running" && j.command === "tools");
  const settings = useQuery({ queryKey: ["settings"], queryFn: api.settings, staleTime: 60_000 });

  const start = (tool: ToolEntry, action: ToolAction, input: string | null = null) =>
    run(`${tool.name}: ${action.label}`, { op: "tool_run", tool: tool.id, action: action.id, input, extra: [] });
  const onAction = (tool: ToolEntry, action: ToolAction) => (action.input ? setAsk({ tool, action, value: action.input.default }) : start(tool, action));
  const openReport = (r: ToolRun, path: string) => (/\.html?$/i.test(path) ? invoke("open_report", { path: `${r.upper}/${path}` }) : setText({ run: r, path }));

  const v = q.data;
  return (
    <>
      <PageHeader
        title="Tools"
        subtitle={
          <>
            Windows programs that work on your modded game, run through {v?.wine ? <span className="text-neon-cyan">{v.wine.name}</span> : "Proton"} with your
            mods mounted. Whatever a tool writes is caught, so nothing changes until you save it as a mod.
          </>
        }
      />
      {q.isLoading && <Spinner />}
      {q.error && <ErrorBox error={q.error} />}
      {v?.wine_error && (
        <Card className="mb-4 flex items-center gap-3 border border-neon-amber/30 p-4 text-sm text-neon-amber">
          <TriangleAlert className="size-5 shrink-0" />
          {v.wine_error}
        </Card>
      )}

      {live && (
        <Card className="mb-4 p-4">
          <div className="mb-2 flex items-center gap-2 text-sm font-medium">
            <Spinner /> {live.label}
          </div>
          <pre className="max-h-56 overflow-y-auto whitespace-pre-wrap rounded-lg bg-ink-950 p-3 font-mono text-xs text-ink-300">
            {live.lines
              .slice(-14)
              .map((l) => l.text)
              .join("\n") || "starting…"}
          </pre>
        </Card>
      )}

      {v && v.tools.length === 0 && (
        <Empty icon={<Wrench className="size-6" />} title="No tools installed">
          A tool from your Downloads list installs here when you install it. Or give a Nexus mod id or page below.
        </Empty>
      )}

      <div className="space-y-4">
        {v?.tools.map((t) => {
          const presets = t.actions.filter((a) => a.preset);
          const rest = t.actions.filter((a) => !a.preset);
          // a step-by-step helper shows its checklist; every step stays runnable by hand below
          const shown = t.progress ? [] : presets.length ? presets : rest;
          const hidden = t.progress ? t.actions : presets.length ? rest : [];
          return (
            <Card key={t.id} className="p-5">
              <div className="flex items-start gap-3">
                <Wrench className="mt-1 size-5 text-neon-violet" />
                <div className="min-w-0 flex-1">
                  <div className="font-semibold">{t.name}</div>
                  <div className="text-xs text-ink-400">
                    {[t.version && `v${t.version}`, t.mod_id && `Nexus ${t.mod_id}`, t.installed_at && `installed ${timeAgo(t.installed_at)}`]
                      .filter(Boolean)
                      .join(" · ")}
                  </div>
                </div>
                {t.from_mod ? (
                  <Badge tone="violet" title="Remove or disable that mod to remove this helper">
                    comes with mod {t.from_mod}
                  </Badge>
                ) : (
                  <Button size="sm" variant="ghost" icon={<Trash2 className="size-3.5" />} disabled={busy} onClick={() => setRemove(t)}>
                    Remove
                  </Button>
                )}
              </div>
              {t.guide && <p className="mt-3 text-sm text-ink-300">{t.guide}</p>}
              {t.progress && (
                <Checklist
                  tool={t}
                  progress={t.progress}
                  busy={busy}
                  gameRunning={state.game_running}
                  closesOnLaunch={settings.data?.values.close_on_launch !== false}
                  onAction={(id) => {
                    const a = t.actions.find((x) => x.id === id);
                    if (a) onAction(t, a);
                  }}
                />
              )}
              <div className={shown.length ? "mt-4 grid gap-3 md:grid-cols-2" : "hidden"}>
                {shown.map((a, i) => (
                  <ActionRow key={a.id} action={a} primary={i === 0 && !t.from_mod} busy={busy} gameRunning={state.game_running} onRun={() => onAction(t, a)} />
                ))}
              </div>
              {hidden.length > 0 && (
                <div className="mt-3">
                  <Button
                    size="sm"
                    variant="ghost"
                    icon={others[t.id] ? <ChevronDown className="size-3.5" /> : <ChevronRight className="size-3.5" />}
                    onClick={() => setOthers((o) => ({ ...o, [t.id]: !o[t.id] }))}
                  >
                    {t.progress ? `Run a step by hand (${hidden.length})` : `Its programs as they are (${hidden.length})`}
                  </Button>
                  {others[t.id] && (
                    <div className="mt-2 grid gap-2 md:grid-cols-2">
                      {hidden.map((a) => (
                        <ActionRow key={a.id} action={a} busy={busy} gameRunning={state.game_running} onRun={() => onAction(t, a)} />
                      ))}
                    </div>
                  )}
                </div>
              )}
            </Card>
          );
        })}
      </div>

      <Card className="mt-4 flex flex-wrap items-center gap-3 p-4">
        <PackagePlus className="size-5 text-ink-400" />
        <Input
          className="min-w-64 flex-1"
          placeholder="Install a tool: Nexus mod id or page URL"
          value={source}
          onChange={(e) => setSource(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && source.trim() && run(`Install tool ${source}`, { op: "tool_install", source: source.trim() })}
        />
        <Button
          variant="primary"
          disabled={busy || !source.trim()}
          onClick={() => run(`Install tool ${source}`, { op: "tool_install", source: source.trim() }).then((ok) => ok && setSource(""))}
        >
          Install tool
        </Button>
      </Card>

      {v && v.runs.length > 0 && (
        <>
          <h2 className="mt-8 mb-3 text-sm font-semibold uppercase tracking-wide text-ink-400">Recent runs</h2>
          <div className="glass overflow-hidden rounded-2xl">
            {v.runs.map((r, i) => {
              const games = r.outputs.filter((o) => o.kind === "game_file");
              const edited = r.outputs.some((o) => o.kind === "load_order");
              const deleted = r.outputs.filter((o) => o.kind === "deleted").length;
              const done = r.finished != null;
              return (
                <div key={r.id} className={i > 0 ? "border-t border-ink-800 px-4 py-3" : "px-4 py-3"}>
                  <div className="flex flex-wrap items-center gap-3">
                    <Badge
                      tone={!done ? "cyan" : r.exit_code === 0 ? "lime" : "red"}
                      className="w-24 justify-center"
                      title={done ? `exit code ${r.exit_code ?? "none"}` : "running"}
                    >
                      {!done ? "running" : r.exit_code === 0 ? "finished" : r.exit_code == null ? "killed" : `exit ${r.exit_code}`}
                    </Badge>
                    <div className="min-w-0 flex-1">
                      <div className="truncate font-medium">{r.label}</div>
                      <div className="truncate text-xs text-ink-400">
                        {[
                          r.tool_name,
                          timeAgo(r.started),
                          r.wine,
                          r.live && "wrote where the game sees it",
                          games.length > 0 && `${games.length} game file(s), ${mb(games.reduce((s, o) => s + o.size, 0))}`,
                        ]
                          .filter(Boolean)
                          .join(" · ")}
                      </div>
                    </div>
                    {done && (
                      <Button size="sm" variant="ghost" icon={<FileText className="size-3.5" />} onClick={() => setText({ run: r, path: "@log" })}>
                        Log
                      </Button>
                    )}
                    {reports(r).map((o) => (
                      <Button key={o.path} size="sm" icon={<FileText className="size-3.5" />} onClick={() => openReport(r, o.path)}>
                        {base(o.path)}
                      </Button>
                    ))}
                    {games.length > 0 &&
                      (r.saved_as ? (
                        <Badge tone="lime">saved as {r.saved_as}</Badge>
                      ) : (
                        <Button
                          size="sm"
                          variant="primary"
                          icon={<Save className="size-3.5" />}
                          disabled={busy}
                          onClick={() => setSave({ run: r, name: `${shortName(r.tool_name)} (${r.label})` })}
                        >
                          Save as mod
                        </Button>
                      ))}
                    <Button size="sm" variant="ghost" icon={<X className="size-3.5" />} disabled={busy || !done} onClick={() => setDiscard(r)}>
                      Discard
                    </Button>
                  </div>
                  {(games.length > 0 || edited || deleted > 0) && !r.saved_as && (
                    <ul className="mt-2 ml-28 space-y-0.5 text-xs text-ink-400">
                      {games.slice(0, 6).map((o) => (
                        <li key={o.path} className="truncate font-mono">
                          {o.path} <span className="text-ink-500">({mb(o.size)})</span>
                        </li>
                      ))}
                      {edited && (
                        <li>
                          It edited modlist.txt: cpmm writes the load order, so that edit isn't applied
                          {r.load_first.length > 0 && "; saving makes its archive win instead"}.
                        </li>
                      )}
                      {deleted > 0 && <li>It deleted {deleted} file(s); not applied. To undo a saved output, disable or remove its mod.</li>}
                    </ul>
                  )}
                </div>
              );
            })}
          </div>
        </>
      )}

      {ask && (
        <Confirm
          open
          title={`${ask.action.label}`}
          body={
            <div className="space-y-3">
              <p>{ask.action.description}</p>
              <label className="block text-xs text-ink-400">
                {ask.action.input?.label}
                <Input className="mt-1" value={ask.value} onChange={(e) => setAsk({ ...ask, value: e.target.value })} />
              </label>
            </div>
          }
          confirmLabel="Run"
          onCancel={() => setAsk(null)}
          onConfirm={() => {
            const a = ask;
            setAsk(null);
            start(a.tool, a.action, a.value.trim() || null);
          }}
        />
      )}
      {save && (
        <Confirm
          open
          title="Save as mod"
          body={
            <div className="space-y-3">
              <p>
                Installs the game files this run wrote as a mod. It applies at the next launch; disable or remove the mod to undo it.
                {save.run.load_first.length > 0 && " The tool wants its archive to win, so the mod gets top priority."}
              </p>
              <label className="block text-xs text-ink-400">
                Mod name
                <Input className="mt-1" value={save.name} onChange={(e) => setSave({ ...save, name: e.target.value })} />
              </label>
            </div>
          }
          confirmLabel="Save as mod"
          onCancel={() => setSave(null)}
          onConfirm={() => {
            const s = save;
            setSave(null);
            run(`Save ${s.run.label} as a mod`, { op: "tool_save", run: s.run.id, name: s.name.trim() || null });
          }}
        />
      )}
      <Confirm
        open={!!remove}
        title={`Remove ${remove?.name}?`}
        body="Its files go to cpmm's trash. Runs and mods saved from it stay."
        confirmLabel="Remove"
        danger
        onCancel={() => setRemove(null)}
        onConfirm={() => {
          const t = remove!;
          setRemove(null);
          run(`Remove tool ${t.name}`, { op: "tool_remove", tool: t.id });
        }}
      />
      <Confirm
        open={!!discard}
        title={`Discard ${discard?.label}?`}
        body={
          discard?.live
            ? "Its log is deleted. What it wrote stays: it went where the game sees it (use the tool's own steps to undo)."
            : discard?.saved_as
              ? `Its reports are deleted; the mod ${discard.saved_as} stays.`
              : "Its reports and the files it wrote are deleted."
        }
        confirmLabel="Discard"
        danger
        onCancel={() => setDiscard(null)}
        onConfirm={() => {
          const r = discard!;
          setDiscard(null);
          run(`Discard ${r.label}`, { op: "tool_discard", run: r.id });
        }}
      />
      {text && <TextReport run={text.run} path={text.path} onClose={() => setText(null)} />}
    </>
  );
}

/** A step-by-step helper: where it is, and the one thing to do next. */
function Checklist({
  tool,
  progress,
  busy,
  gameRunning,
  closesOnLaunch,
  onAction,
}: {
  tool: ToolEntry;
  progress: Progress;
  busy: boolean;
  gameRunning: boolean;
  closesOnLaunch: boolean;
  onAction: (id: string) => void;
}) {
  const closed = (id: string) => tool.actions.find((a) => a.id === id)?.when === "closed";
  const done = progress.current >= progress.steps.length;
  const conflict = progress.blocked != null;
  return (
    <div className="mt-4">
      <div
        className={
          conflict
            ? "flex gap-2 rounded-xl border border-neon-amber/40 bg-neon-amber/5 p-3 text-sm text-neon-amber"
            : done
              ? "rounded-xl border border-neon-lime/30 bg-neon-lime/5 p-3 text-sm text-neon-lime"
              : "rounded-xl border border-neon-cyan/30 bg-neon-cyan/5 p-3 text-sm text-neon-cyan"
        }
      >
        {conflict && <TriangleAlert className="mt-0.5 size-4 shrink-0" />}
        {progress.stage}
      </div>
      <ol className="mt-3 space-y-2">
        {progress.steps.map((s, i) => {
          const state = i < progress.current ? "done" : i === progress.current ? "now" : "later";
          const blocked = !!s.action && closed(s.action) && gameRunning;
          return (
            <li
              key={s.title}
              className={
                state === "now"
                  ? "flex items-start gap-3 rounded-xl border border-neon-cyan/50 bg-ink-900/60 p-3"
                  : "flex items-start gap-3 rounded-xl border border-ink-800 bg-ink-900/30 p-3"
              }
            >
              <span
                className={
                  state === "done"
                    ? "flex size-7 shrink-0 items-center justify-center rounded-full bg-neon-lime/20 text-neon-lime"
                    : state === "now"
                      ? "flex size-7 shrink-0 items-center justify-center rounded-full bg-neon-cyan text-sm font-semibold text-ink-950"
                      : "flex size-7 shrink-0 items-center justify-center rounded-full bg-ink-800 text-sm text-ink-400"
                }
              >
                {state === "done" ? <Check className="size-4" /> : i + 1}
              </span>
              <div className={state === "later" ? "min-w-0 flex-1 opacity-60" : "min-w-0 flex-1"}>
                <div className="font-medium">{s.title}</div>
                <div className="text-xs text-ink-400">
                  {s.detail}
                  {state === "now" && !s.action && closesOnLaunch && " cpmm closes while the game runs: open it again afterwards."}
                  {state === "now" && blocked && <span className="text-neon-amber"> Quit the game first.</span>}
                </div>
              </div>
              {state === "now" &&
                (s.action ? (
                  <Button
                    size="sm"
                    variant="primary"
                    icon={<Play className="size-3.5" />}
                    disabled={busy || blocked || conflict}
                    onClick={() => onAction(s.action!)}
                  >
                    {s.title}
                  </Button>
                ) : (
                  <Button
                    size="sm"
                    variant="primary"
                    icon={<Play className="size-3.5" />}
                    disabled={gameRunning || conflict}
                    onClick={() => invoke("launch_game", { close: closesOnLaunch }).catch((e) => alert(String(e)))}
                  >
                    {gameRunning ? "Game is running" : "Launch Cyberpunk"}
                  </Button>
                ))}
            </li>
          );
        })}
      </ol>
      <div className="mt-3 flex flex-wrap gap-2">
        <Button size="sm" variant="ghost" icon={<Info className="size-3.5" />} disabled={busy} onClick={() => onAction("status")}>
          Status
        </Button>
        <Button
          size="sm"
          variant={progress.active ? "danger" : "ghost"}
          icon={<RotateCcw className="size-3.5" />}
          disabled={busy || !progress.active || gameRunning}
          title={progress.active ? (gameRunning ? "Quit the game first" : "Removes the INI it wrote") : "Nothing to roll back"}
          onClick={() => onAction("rollback")}
        >
          Roll back
        </Button>
      </div>
    </div>
  );
}

function ActionRow({
  action,
  primary,
  busy,
  gameRunning,
  onRun,
}: {
  action: ToolAction;
  primary?: boolean;
  busy: boolean;
  gameRunning: boolean;
  onRun: () => void;
}) {
  return (
    <div className="flex items-start gap-3 rounded-xl border border-ink-800 bg-ink-900/40 p-3">
      <Button
        size="sm"
        variant={primary ? "primary" : "secondary"}
        icon={<Play className="size-3.5" />}
        disabled={busy || (action.when === "closed" && gameRunning)}
        onClick={onRun}
      >
        {action.label}
      </Button>
      <div className="min-w-0 flex-1 text-xs text-ink-400">
        {action.description || <span className="font-mono">{action.program}</span>}
        {action.when === "closed" && gameRunning && <div className="mt-1 text-neon-amber">Quit the game first.</div>}
      </div>
    </div>
  );
}

function TextReport({ run, path, onClose }: { run: ToolRun; path: string; onClose: () => void }) {
  const q = useQuery({ queryKey: ["tool-file", run.id, path], queryFn: () => api.toolFile(run.id, path) });
  return (
    <Drawer open onClose={onClose} title={<div className="truncate font-semibold">{path === "@log" ? `${run.label}: log` : base(path)}</div>} width="max-w-4xl">
      {q.isLoading && <Spinner />}
      {q.error && <ErrorBox error={q.error} />}
      {q.data && (
        <pre className="whitespace-pre-wrap font-mono text-xs text-ink-200">
          {q.data.text}
          {q.data.truncated && "\n… (truncated)"}
        </pre>
      )}
    </Drawer>
  );
}

/** "VRAM RipperDoc - Performance Optimizer" → "VRAM RipperDoc" (as cpmm's short names). */
function shortName(n: string) {
  const cut = [" - ", " | ", " – ", " ("].map((s) => n.indexOf(s)).filter((i) => i >= 4);
  return cut.length ? n.slice(0, Math.min(...cut)) : n;
}
