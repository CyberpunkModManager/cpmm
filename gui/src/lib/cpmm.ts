import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export type ReqStatus = "ok" | "disabled" | "ignored" | "optional-missing" | "missing";

export interface Requirement {
  mod_id: number;
  name: string;
  notes: string;
  optional: boolean;
  status: ReqStatus;
  installed_as: string | null;
  url: string;
}

export interface ModRow {
  priority: number;
  id: string;
  name: string;
  enabled: boolean;
  version: string;
  installed_at: string;
  notes: string;
  nexus: { mod_id: number; file_name: string; file_id: string; version_gsid: string; uploaded_at: string; url: string } | null;
  fomod: Record<string, string[]> | null;
  files: number;
  hidden: number;
  wins: number;
  loses: number;
  requires: Requirement[];
  dep_problems: number;
}

export interface AppState {
  game_dir: string;
  data_dir: string;
  game_running: boolean;
  session: { pid: number; started: string; layers: number } | null;
  mods: ModRow[];
  modio: { id: string; name: string; author: string; version: string }[];
  overwrite_count: number;
  nxm_handler: boolean;
  active_profile: string;
  profiles: { name: string; enabled: number; active: boolean; created_at: string; own_saves: boolean; saves: number }[];
}

export interface ModInfo {
  id: string;
  name: string;
  files: { path: string; others: string[]; winner: string; wins: boolean }[];
  hidden: string[];
  ignored_reqs: number[];
}

export interface Conflicts {
  files: { path: string; mods: string[]; winner: string; identical: boolean; overrides_base: boolean }[];
  scripts: { kind: string; target: string; mods: string[]; severe: boolean }[];
  base_overrides: number;
}

export interface UpdateRow {
  id: string;
  status: string;
  update: { version: string; uploaded_at: string; gsid: string } | null;
}

export interface OverwriteRow {
  path: string;
  kind: "deleted" | "edited mod file" | "edited base file" | "new";
  shadows: string | null;
}

export interface TrackedRow {
  mod_id: number;
  name: string;
  installed_as: string | null;
  url: string;
}

export interface Lookup {
  mod_id: number;
  name: string;
  version: string;
  author: string;
  url: string;
  files: { gsid: string; name: string; version: string; category: string; uploaded_at: string }[];
  requires: { mod_id: number; name: string; notes: string; optional: boolean; external: boolean }[];
}

export const api = {
  state: () => invoke<AppState>("api", { args: ["state"] }),
  info: (id: string) => invoke<ModInfo>("api", { args: ["info", id] }),
  conflicts: () => invoke<Conflicts>("api", { args: ["conflicts"] }),
  updates: (refresh = false) => invoke<UpdateRow[]>("api", { args: refresh ? ["updates", "--refresh"] : ["updates"] }),
  overwrite: () => invoke<OverwriteRow[]>("api", { args: ["overwrite"] }),
  tracked: () => invoke<TrackedRow[]>("api", { args: ["tracked"] }),
  lookup: (source: string) => invoke<Lookup>("api", { args: ["lookup", source] }),
};

export interface RunResult {
  code: number;
  output: string;
}

export interface LogLine {
  job: number;
  line: string;
  stderr: boolean;
}

let nextJob = 1;

/** Run a cpmm CLI action; lines stream to `onLine` as they arrive. */
export async function runCpmm(args: string[], onLine?: (l: LogLine) => void): Promise<RunResult & { job: number }> {
  const job = nextJob++;
  const unlisten = onLine ? await listen<LogLine>("cpmm-log", (e) => e.payload.job === job && onLine(e.payload)) : null;
  try {
    const r = await invoke<RunResult>("run", { job, args });
    return { ...r, job };
  } finally {
    unlisten?.();
  }
}

export function timeAgo(iso: string): string {
  if (!iso) return "";
  const t = Date.parse(iso);
  if (Number.isNaN(t)) return iso;
  const s = (Date.now() - t) / 1000;
  const units: [number, string][] = [
    [31536000, "y"],
    [2592000, "mo"],
    [86400, "d"],
    [3600, "h"],
    [60, "m"],
  ];
  for (const [n, u] of units) if (s >= n) return `${Math.floor(s / n)}${u} ago`;
  return "just now";
}

export type Owner = { kind: "mod" | "base" | "modio"; id: string };
export interface ArchivePair {
  a: Owner;
  b: Owner;
  archive_a: string;
  archive_b: string;
  shared: number;
  identical: number;
  smaller: number;
  related: boolean;
  winner: string | null;
}
export interface LoadOrder {
  plan: {
    order: string[];
    moved: { id: string; from: number; to: number; because: string[] }[];
    edges: { before: string; after: string; why: string; strength: string }[];
    cycles: string[][];
    unresolved: ArchivePair[];
  };
  rules: { mod: string; after: string }[];
  modlist: string[];
}
export interface Finding {
  severity: "crash" | "warning" | "perf" | "info" | "acked";
  title: string;
  detail: string;
  fix: string | null;
  actions: { label: string; args: string[]; style: "primary" | "secondary" }[];
}
export const api2 = {
  loadorder: () => invoke<LoadOrder>("api", { args: ["loadorder"] }),
  doctor: () => invoke<Finding[]>("api", { args: ["doctor"] }),
  archives: () => invoke<ArchivePair[]>("api", { args: ["archives"] }),
};
export const ownerLabel = (o: Owner) => (o.kind === "mod" ? o.id : o.kind === "modio" ? `mod.io: ${o.id}` : "(unmanaged)");

export interface Crash {
  report: string;
  ts: number;
  message: string;
  module: string;
  offset: string;
  session_start: number | null;
  seconds_in: number | null;
  last_good: number | null;
  changes: string[];
  suspects: string[];
}
export interface Safety {
  crashes: Crash[];
  errors: { baseline: boolean; latest_log: number; new: Record<string, string[]>; fixed: Record<string, string[]>; totals: Record<string, number> };
  patch: {
    build_id: string;
    previous_build: string | null;
    patched_at: number;
    patch_detected: boolean;
    at_risk: { id: string; name: string; version: string; latest_upload: string | null; update_available: boolean; released_since_patch: boolean }[];
    red4ext_errors: string[];
    vanilla: boolean;
  };
  snapshots: { name: string; ts: number; reason: string; saves: number }[];
  save_risks: { save: string; saved_at: number; missing: string[] }[];
  batches: { batch: string; ts: number; mods: [string, string][] }[];
  sessions: number;
}
export interface HistEntry {
  ts: number;
  id: string;
  version: string;
  reason: string;
  batch: string | null;
  files: number;
}
export const api3 = {
  safety: () => invoke<Safety>("api", { args: ["safety"] }),
  history: (id: string) => invoke<HistEntry[]>("api", { args: ["history", id] }),
};
export const when = (unix: number) => (unix ? new Date(unix * 1000).toLocaleString([], { dateStyle: "medium", timeStyle: "short" }) : "—");

export interface SettingField {
  key: string;
  group: string;
  label: string;
  help: string;
  kind: "bool" | "number" | "text" | "path";
}
export interface SettingsData {
  values: Record<string, boolean | number | string>;
  schema: SettingField[];
  resolved: { staging_dir: string; download_dir: string; watch_dir: string; data_dir: string; game_dir: string };
  api_key: boolean;
}
export const api4 = {
  settings: () => invoke<SettingsData>("api", { args: ["settings"] }),
  downloads: () => invoke<{ path: string; nexus_id: number | null }[]>("api", { args: ["downloads"] }),
};

export interface OnboardingStatus {
  game_dir: string;
  game_found: boolean;
  api_key: boolean;
  launch_options: string | null;
  launch_ok: boolean;
  launch_suggested: string;
  steam_running: boolean;
  steam_cef: boolean;
  nxm_handler: boolean;
  onboarded: boolean;
  mods_installed: number;
}
export const api5 = {
  onboarding: () => invoke<OnboardingStatus>("api", { args: ["onboarding"] }),
};

export interface Changelog {
  id: string;
  name: string;
  installed: string;
  entries: { version: string; changes: string[]; newer: boolean; installed: boolean }[];
}
export const api6 = {
  changelog: (id: string) => invoke<Changelog>("api", { args: ["changelog", id] }),
};

export interface ModioData {
  mods: { id: string; name: string; author: string; version: string; files: number }[];
  changes: { first_look: boolean; added: string[]; removed: string[]; updated: string[] };
  conflicts: { modio: string; kind: string; with: string; detail: string; severe: boolean }[];
}
export const api7 = {
  modio: () => invoke<ModioData>("api", { args: ["modio"] }),
};

export interface FomodPlugin {
  name: string;
  description: string;
  kind: string;
  selected: boolean;
  image: string | null;
}
export interface FomodDesc {
  fomod: boolean;
  name?: string;
  archive?: string;
  steps?: { name: string; visible: boolean; groups: { key: string; name: string; kind: string; plugins: FomodPlugin[] }[] }[];
}
export const api8 = {
  fomod: (source: string, file?: string | null, choices?: Record<string, string[]>) =>
    invoke<FomodDesc>("api", {
      args: ["fomod", source, ...(file ? ["--file", file] : []), ...(choices && Object.keys(choices).length ? ["--choices", JSON.stringify(choices)] : [])],
    }),
};
