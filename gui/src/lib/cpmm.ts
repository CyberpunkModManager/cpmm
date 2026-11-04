import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
  Action,
  ActResult,
  AppState,
  ArchivePair,
  Changelog,
  CollectionInfo,
  Conflicts,
  DownloadEntry,
  Finding,
  FomodDesc,
  HistEntry,
  LoadOrder,
  LogLine,
  Lookup,
  ModInfo,
  ModioData,
  Onboarding,
  OverwriteRow,
  Owner,
  Query,
  Safety,
  SettingsData,
  TrackedRow,
  UpdateRow,
} from "./bindings";

// The shapes cpmm answers with are generated from its Rust types.
export type * from "./bindings";

/** A read-only view, answered in-process by the core (what `cpmm api …` prints). */
function query<T>(q: Query): Promise<T> {
  return invoke<T>("query", { q });
}

/** cpmm's read-only views, one typed call each. */
export const api = {
  state: () => query<AppState>({ view: "state" }),
  info: (id: string) => query<ModInfo>({ view: "info", mod: id }),
  conflicts: () => query<Conflicts>({ view: "conflicts" }),
  updates: (refresh = false) => query<UpdateRow[]>({ view: "updates", refresh }),
  overwrite: () => query<OverwriteRow[]>({ view: "overwrite" }),
  tracked: () => query<TrackedRow[]>({ view: "tracked" }),
  lookup: (source: string) => query<Lookup>({ view: "lookup", source }),
  loadorder: () => query<LoadOrder>({ view: "loadorder" }),
  doctor: () => query<Finding[]>({ view: "doctor" }),
  archives: () => query<ArchivePair[]>({ view: "archives" }),
  safety: () => query<Safety>({ view: "safety" }),
  history: (id: string) => query<HistEntry[]>({ view: "history", mod: id }),
  settings: () => query<SettingsData>({ view: "settings" }),
  downloads: () => query<DownloadEntry[]>({ view: "downloads" }),
  onboarding: () => query<Onboarding>({ view: "onboarding" }),
  changelog: (id: string) => query<Changelog>({ view: "changelog", mod: id }),
  modio: () => query<ModioData>({ view: "modio" }),
  fomod: (source: string, file?: string | null, choices?: Record<string, string[]>) =>
    query<FomodDesc>({ view: "fomod", source, file, choices: choices && Object.keys(choices).length ? choices : null }),
  collection: (reference: string) => query<CollectionInfo>({ view: "collection", reference }),
};

export interface RunResult {
  code: number;
  output: string;
}

let nextJob = 1;

async function asJob<R>(onLine: ((l: LogLine) => void) | undefined, start: (job: number) => Promise<R>): Promise<R & { job: number }> {
  const job = nextJob++;
  const unlisten = onLine ? await listen<LogLine>("cpmm-log", (e) => e.payload.job === job && onLine(e.payload)) : null;
  try {
    return { ...(await start(job)), job };
  } finally {
    unlisten?.();
  }
}

/** Run an action in the core; its log lines stream to `onLine` as they happen. */
export function act(action: Action, onLine?: (l: LogLine) => void): Promise<ActResult & { job: number }> {
  return asJob(onLine, (job) => invoke<ActResult>("act", { job, action }));
}

/** Run a cpmm CLI command (the doctor's remedies, still argv); lines stream to `onLine`. */
export function runCpmm(args: string[], onLine?: (l: LogLine) => void): Promise<RunResult & { job: number }> {
  return asJob(onLine, (job) => invoke<RunResult>("run", { job, args }));
}

/** The `cpmm` command an action is (the job log names it). */
const COMMAND: Partial<Record<Action["op"], string>> = {
  register_handler: "register-handler",
  unregister_handler: "register-handler",
  overwrite_move: "overwrite",
  overwrite_clear: "overwrite",
  collection_install: "collection",
  settings_set: "settings",
  settings_reset: "settings",
  downloads_install: "downloads",
  downloads_remove: "downloads",
  downloads_add: "downloads",
  ack: "doctor",
  unack: "doctor",
  undo_update: "undo-update",
  snapshot_saves: "saves",
  restore_saves: "saves",
  modio_ack: "modio",
  launch_options_apply: "launch-options",
  nexus_key_set: "nexus-key",
  nexus_key_remove: "nexus-key",
  nexus_key: "nexus-key",
  steam_shortcut: "steam-shortcut",
};
export const command = (a: Action) => COMMAND[a.op] ?? a.op;

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

export const ownerLabel = (o: Owner) => (o.kind === "mod" ? o.id : o.kind === "modio" ? `mod.io: ${o.id}` : "(unmanaged)");

export const when = (unix: number) => (unix ? new Date(unix * 1000).toLocaleString([], { dateStyle: "medium", timeStyle: "short" }) : "—");
