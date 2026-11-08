import { useQuery } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import clsx from "clsx";
import {
  Activity,
  ArrowDownToLine,
  CheckCircle2,
  CircleAlert,
  Cpu,
  Download,
  Gamepad2,
  GitMerge,
  Globe,
  HeartPulse,
  Layers,
  Loader2,
  LogOut,
  Network,
  PackagePlus,
  Play,
  RefreshCw,
  Save,
  Settings2,
  Wrench,
  X,
} from "lucide-react";
import { type ReactNode, useCallback, useState } from "react";
import { ControlMap } from "./components/ControlMap";
import { ErrorBox, IconButton, Spinner } from "./components/ui";
import { useAutomation } from "./lib/automation";
import { api } from "./lib/cpmm";
import { useSpatialNav } from "./lib/gamepad";
import { useJobs } from "./lib/jobs";
import { openNexus, useNexusBridge } from "./lib/nexus";
import { DownloadsView } from "./views/Downloads";
import { HealthView } from "./views/LoadOrder";
import { ModsView } from "./views/Mods";
import { Onboarding } from "./views/Onboarding";
import { ConflictsView, DepsView, InstallView, OverwriteView, SystemView, UpdatesView } from "./views/Panels";
import { ProfileButton } from "./views/Profiles";
import { SettingsView } from "./views/Settings";
import { ToolsView } from "./views/Tools";

const SECTIONS = [
  { key: "mods", label: "Mods", icon: Layers },
  { key: "health", label: "Health & order", icon: HeartPulse },
  { key: "conflicts", label: "Conflicts", icon: GitMerge },
  { key: "deps", label: "Dependencies", icon: Network },
  { key: "updates", label: "Updates", icon: ArrowDownToLine },
  { key: "overwrite", label: "Overwrite", icon: Save },
  { key: "install", label: "Install", icon: PackagePlus },
  { key: "downloads", label: "Downloads", icon: Download },
  { key: "tools", label: "Tools", icon: Wrench },
  { key: "system", label: "System", icon: Cpu },
  { key: "settings", label: "Settings", icon: Settings2 },
] as const;
type Section = (typeof SECTIONS)[number]["key"];

export default function App() {
  const [section, setSection] = useState<Section>("mods");
  const [logOpen, setLogOpen] = useState(false);
  const state = useQuery({ queryKey: ["state"], queryFn: api.state, refetchInterval: 5000 });
  const { jobs, busy } = useJobs();
  const settings = useQuery({ queryKey: ["settings"], queryFn: api.settings, staleTime: 60_000 });
  useAutomation(state.data, settings.data);
  const [setupOpen, setSetupOpen] = useState(false);
  const showSetup = setupOpen || settings.data?.values.onboarded === false;
  const doctor = useQuery({ queryKey: ["doctor"], queryFn: api.doctor, staleTime: 60_000 });

  const onSection = useCallback((d: number) => {
    setSection((s) => {
      const i = SECTIONS.findIndex((x) => x.key === s);
      return SECTIONS[(i + d + SECTIONS.length) % SECTIONS.length].key;
    });
  }, []);
  const onBack = useCallback(() => {
    // Close the topmost layer by clicking its backdrop (drawers/dialogs listen for that)
    const layers = document.querySelectorAll<HTMLElement>("[data-nav-layer]");
    const top = layers[layers.length - 1];
    if (top) (top.firstElementChild as HTMLElement | null)?.click();
    else setLogOpen(false);
  }, []);
  const [controlsOpen, setControlsOpen] = useState(false);
  const onStart = useCallback(() => setLogOpen((v) => !v), []);
  const onSelect = useCallback(() => setControlsOpen((v) => !v), []);
  useSpatialNav(onSection, onBack, onStart, onSelect);
  useNexusBridge();

  const s = state.data;
  const badge: Partial<Record<Section, ReactNode>> = s
    ? {
        deps: s.mods.reduce((n, m) => n + m.dep_problems, 0) || null,
        health: doctor.data?.filter((f) => f.severity === "crash" || f.severity === "warning").length || null,
        overwrite: s.overwrite_count || null,
      }
    : {};

  return (
    <div className="flex h-full">
      <aside className="flex w-60 shrink-0 flex-col border-r border-ink-800 bg-ink-900/70 p-4">
        <div className="mb-5 flex items-center gap-3 px-2">
          <img src="/icon.png" alt="" className="size-9 rounded-lg" />
          <div>
            <div className="font-semibold tracking-tight">cpmm</div>
            <div className="text-xs text-ink-400">Cyberpunk mod manager</div>
          </div>
        </div>
        {settings.data?.values.profiles_enabled !== false && <ProfileButton state={s} />}
        <nav className="-mx-1 flex min-h-0 flex-1 flex-col gap-1 overflow-y-auto px-1">
          {SECTIONS.map(({ key, label, icon: Icon }) => (
            <button
              type="button"
              key={key}
              onClick={() => setSection(key)}
              className={clsx(
                "flex h-11 shrink-0 items-center gap-3 rounded-xl px-3 text-sm cursor-pointer",
                section === key ? "bg-neon-cyan/12 text-neon-cyan" : "text-ink-300 hover:bg-ink-800 hover:text-ink-100",
              )}
            >
              <Icon className="size-4.5" />
              <span className="flex-1 text-left">{label}</span>
              {badge[key] && (
                <span
                  className={clsx(
                    "rounded-md px-1.5 text-xs tabular-nums",
                    key === "deps" || key === "health" ? "bg-neon-red/20 text-neon-red" : "bg-ink-700 text-ink-300",
                  )}
                >
                  {badge[key]}
                </span>
              )}
            </button>
          ))}
        </nav>
        <button
          type="button"
          onClick={() => setControlsOpen(true)}
          className="mt-3 flex items-center gap-1.5 rounded-lg px-2 py-1 text-[11px] text-ink-400 hover:bg-ink-800 hover:text-ink-200 cursor-pointer"
        >
          <Gamepad2 className="size-3.5" /> Select or ? · controls
        </button>
        <button
          type="button"
          onClick={() => openNexus().catch((e) => alert(String(e)))}
          className="mt-4 flex h-11 items-center gap-3 rounded-xl border border-neon-cyan/30 px-3 text-sm text-neon-cyan hover:bg-neon-cyan/12 cursor-pointer"
        >
          <Globe className="size-4.5" /> Browse Nexus
        </button>
        <button
          type="button"
          onClick={() => invoke("quit")}
          className="mt-1 flex h-11 items-center gap-3 rounded-xl px-3 text-sm text-ink-300 hover:bg-neon-red/10 hover:text-neon-red cursor-pointer"
        >
          <LogOut className="size-4.5" /> Exit
        </button>
      </aside>

      <div className="flex min-w-0 flex-1 flex-col">
        <header className="flex h-14 shrink-0 items-center gap-3 border-b border-ink-800 px-6">
          {s?.session ? (
            <span className="inline-flex items-center gap-2 rounded-full border border-neon-lime/30 bg-neon-lime/10 px-3 py-1 text-xs text-neon-lime">
              <span className="size-2 animate-pulse rounded-full bg-neon-lime" /> Game running · {s.session.layers} layers mounted
            </span>
          ) : s ? (
            <span className="inline-flex items-center gap-2 rounded-full border border-ink-700 px-3 py-1 text-xs text-ink-300">
              <span className="size-2 rounded-full bg-ink-400" /> Game not running
            </span>
          ) : null}
          {s && (
            <span className="text-xs text-ink-400">
              {s.mods.filter((m) => m.enabled).length} of {s.mods.length} mods enabled
            </span>
          )}
          <div className="flex-1" />
          <button
            type="button"
            disabled={!s || s.game_running || !!s.session || busy}
            onClick={() => invoke("launch_game", { close: settings.data?.values.close_on_launch !== false }).catch((e) => alert(String(e)))}
            title={
              s?.game_running || s?.session
                ? "Cyberpunk is already running"
                : busy
                  ? "Wait for running actions to finish"
                  : "Launch Cyberpunk 2077 through Steam (cpmm closes)"
            }
            className="inline-flex h-9 items-center gap-2 rounded-lg bg-neon-cyan px-4 text-sm font-semibold text-ink-950 glow-cyan hover:bg-cyan-300 disabled:opacity-40 disabled:pointer-events-none cursor-pointer"
          >
            <Play className="size-4" /> Launch Cyberpunk
          </button>
          <IconButton label="Refresh" onClick={() => state.refetch()}>
            <RefreshCw className={clsx("size-4", state.isFetching && "animate-spin")} />
          </IconButton>
          <button
            type="button"
            onClick={() => setLogOpen((v) => !v)}
            className={clsx(
              "inline-flex h-9 items-center gap-2 rounded-lg px-3 text-sm cursor-pointer",
              logOpen ? "bg-ink-800 text-ink-100" : "text-ink-300 hover:bg-ink-800",
            )}
          >
            {busy ? <Loader2 className="size-4 animate-spin text-neon-cyan" /> : <Activity className="size-4" />} Activity
            {jobs.length > 0 && <span className="text-xs text-ink-400">{jobs.length}</span>}
          </button>
        </header>

        <div className="flex min-h-0 flex-1">
          <main className="min-w-0 flex-1 overflow-y-auto px-8 py-7">
            {state.isLoading && <Spinner label="Reading cpmm state…" />}
            {state.error && <ErrorBox error={state.error} />}
            {s && (
              <div className="mx-auto max-w-6xl">
                {section === "mods" && <ModsView state={s} />}
                {section === "health" && <HealthView state={s} />}
                {section === "conflicts" && <ConflictsView state={s} />}
                {section === "deps" && <DepsView state={s} />}
                {section === "updates" && <UpdatesView state={s} />}
                {section === "overwrite" && <OverwriteView state={s} />}
                {section === "install" && <InstallView state={s} />}
                {section === "downloads" && <DownloadsView state={s} />}
                {section === "tools" && <ToolsView state={s} />}
                {section === "system" && <SystemView state={s} />}
                {section === "settings" && <SettingsView onSetup={() => setSetupOpen(true)} />}
              </div>
            )}
          </main>
          {logOpen && <ActivityPanel onClose={() => setLogOpen(false)} />}
        </div>
      </div>
      {showSetup && <Onboarding onClose={() => setSetupOpen(false)} />}
      {controlsOpen && <ControlMap onClose={() => setControlsOpen(false)} />}
      <Toasts />
    </div>
  );
}

function ActivityPanel({ onClose }: { onClose: () => void }) {
  const { jobs, clearFinished } = useJobs();
  return (
    <aside className="flex w-[26rem] shrink-0 flex-col border-l border-ink-800 bg-ink-900/80">
      <div className="flex h-12 items-center justify-between border-b border-ink-800 px-4">
        <div className="text-sm font-semibold">Activity</div>
        <div className="flex items-center gap-1">
          <button type="button" onClick={clearFinished} className="h-8 rounded-lg px-2 text-xs text-ink-400 hover:bg-ink-800 cursor-pointer">
            Clear
          </button>
          <IconButton label="Close activity" onClick={onClose}>
            <X className="size-4" />
          </IconButton>
        </div>
      </div>
      <div className="flex-1 space-y-3 overflow-y-auto p-4">
        {jobs.length === 0 && <div className="py-10 text-center text-sm text-ink-400">Actions you run show their output here.</div>}
        {jobs.map((j) => (
          <div key={j.id} className="rounded-xl border border-ink-700 bg-ink-850">
            <div className="flex items-center gap-2 border-b border-ink-800 px-3 py-2 text-sm">
              {j.status === "running" ? (
                <Loader2 className="size-4 animate-spin text-neon-cyan" />
              ) : j.status === "ok" ? (
                <CheckCircle2 className="size-4 text-neon-lime" />
              ) : (
                <CircleAlert className="size-4 text-neon-red" />
              )}
              <span className="min-w-0 flex-1 truncate">{j.label}</span>
              <span className="font-mono text-[10px] text-ink-400">cpmm {j.command}</span>
            </div>
            <pre className="max-h-64 overflow-y-auto px-3 py-2 font-mono text-[11px] leading-4 whitespace-pre-wrap select-text">
              {j.lines.length === 0 ? (
                <span className="text-ink-400">…</span>
              ) : (
                j.lines.map((l, i) => (
                  <div key={i} className={l.err ? "text-neon-amber" : "text-ink-300"}>
                    {l.text}
                  </div>
                ))
              )}
            </pre>
          </div>
        ))}
      </div>
    </aside>
  );
}

function Toasts() {
  const { toasts, dismiss } = useJobs();
  return (
    <div className="pointer-events-none fixed right-6 bottom-6 z-[60] flex w-96 flex-col gap-2">
      {toasts.map((t) => (
        <div
          key={t.id}
          onClick={() => dismiss(t.id)}
          className={clsx(
            "pointer-events-auto flex items-start gap-3 rounded-xl border px-4 py-3 text-sm shadow-2xl backdrop-blur cursor-pointer",
            t.kind === "ok" && "border-neon-lime/30 bg-ink-850/95 text-ink-100",
            t.kind === "error" && "border-neon-red/40 bg-ink-850/95 text-ink-100",
            t.kind === "info" && "border-ink-700 bg-ink-850/95",
          )}
        >
          {t.kind === "ok" ? (
            <CheckCircle2 className="mt-0.5 size-4 shrink-0 text-neon-lime" />
          ) : t.kind === "error" ? (
            <CircleAlert className="mt-0.5 size-4 shrink-0 text-neon-red" />
          ) : (
            <Download className="mt-0.5 size-4 shrink-0" />
          )}
          <span className="break-words">{t.text}</span>
        </div>
      ))}
    </div>
  );
}
