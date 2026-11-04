import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
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

/** The `cpmm api` argv of a query (the Tauri shell runs it). */
function apiArgs(q: Query): string[] {
  switch (q.view) {
    case "info":
    case "history":
    case "changelog":
      return [q.view, q.mod];
    case "updates":
      return q.refresh ? ["updates", "--refresh"] : ["updates"];
    case "lookup":
      return ["lookup", q.source];
    case "collection":
      return ["collection", q.reference];
    case "fomod":
      return [
        "fomod",
        q.source,
        ...(q.file ? ["--file", q.file] : []),
        ...(q.choices && Object.keys(q.choices).length ? ["--choices", JSON.stringify(q.choices)] : []),
      ];
    default:
      return [q.view];
  }
}

function query<T>(q: Query): Promise<T> {
  return invoke<T>("api", { args: apiArgs(q) });
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
  fomod: (source: string, file?: string | null, choices?: Record<string, string[]>) => query<FomodDesc>({ view: "fomod", source, file, choices }),
  collection: (reference: string) => query<CollectionInfo>({ view: "collection", reference }),
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

export const ownerLabel = (o: Owner) => (o.kind === "mod" ? o.id : o.kind === "modio" ? `mod.io: ${o.id}` : "(unmanaged)");

export const when = (unix: number) => (unix ? new Date(unix * 1000).toLocaleString([], { dateStyle: "medium", timeStyle: "short" }) : "—");
