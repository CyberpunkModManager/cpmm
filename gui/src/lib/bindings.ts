// Generated from the Rust types of the Operations interface (src/ops/bindings.rs). Do not edit:
// `UPDATE_BINDINGS=1 cargo test bindings` regenerates it.

/**
 * How an action ended. `output` is the log as one text, stdout lines then stderr lines (the
 * last line of a failed action is its error, `error: …`); `installed` the mods it newly
 * installed (what the GUI names in "Installed …").
 */
export type ActResult = { code: number, output: string, installed: Array<string>, };

/**
 * Every change to the mod setup and cpmm's data that the CLI or the GUI can ask for
 * (`{"op": "enable", "mods": ["…"]}` from the GUI).
 */
export type Action = { "op": "init", game: string | null, } | { "op": "enable", mods: Array<string>, } | { "op": "disable", mods: Array<string>, } | { "op": "order", mod: string, place: Place | null, } | { "op": "install" } & InstallRequest | { "op": "update", mods: Array<string>, all: boolean, } | { "op": "rename", mod: string, new_id: string, } | { "op": "remove", mod: string, force: boolean, } | { "op": "deps", refresh: boolean, fix: boolean, ignore: [string, string] | null, unignore: [string, string] | null, } | { "op": "hide", mod: string, paths: Array<string>, } | { "op": "unhide", mod: string, paths: Array<string>, } | { "op": "prefer", mod: string, paths: Array<string>, } | { "op": "nxm", url: string | null, register: boolean, } | { "op": "tracked", install: boolean, untrack: boolean, } | { "op": "register_handler" } | { "op": "unregister_handler" } | { "op": "import", modlist: string, apply: boolean, } | { "op": "adopt", id: string, name: string | null, paths: Array<string>, } | { "op": "overwrite_move", to: string, paths: Array<string>, } | { "op": "overwrite_clear", paths: Array<string>, } | { "op": "eject", apply: boolean, } | { "op": "collection_install", reference: string, optional: boolean, into_current: boolean, profile: string | null, } | { "op": "settings_set", key: string, value: string, } | { "op": "settings_reset", key: string | null, } | { "op": "download", source: string, file: string | null, } | { "op": "downloads_install", key: string, fomod: { [key in string]: Array<string> } | null, ask: boolean, with_deps: boolean, } | { "op": "downloads_remove", key: string, } | { "op": "downloads_add", path: string, nexus: number | null, move_it: boolean, source: string, auto: boolean, } | { "op": "clean", dry_run: boolean, } | { "op": "sort", apply: boolean, } | { "op": "rule", mod: string, after: string, remove: boolean, } | { "op": "ack", finding: string, } | { "op": "unack", finding: string, } | { "op": "rollback", mod: string, to: number | null, } | { "op": "undo_update", batch: string | null, } | { "op": "errors" } | { "op": "vanilla", on: boolean, } | { "op": "snapshot_saves" } | { "op": "restore_saves", name: string, } | { "op": "profile" } & ProfileAction | { "op": "modio_ack" } | { "op": "launch_options_apply" } | { "op": "nexus_key_set", key: string, } | { "op": "nexus_key_remove" } | { "op": "nexus_key", check: boolean, } | { "op": "steam_shortcut", name: string, exe: string | null, remove: boolean, dry_run: boolean, } | { "op": "setup" };

export type AppState = { game_dir: string, data_dir: string, game_running: boolean, session: SessionInfo | null, mods: Array<ModRow>, modio: Array<ModioSub>, overwrite_count: number, nxm_handler: boolean, active_profile: string, profiles: Array<ProfileRow>, };

export type ArchivePair = { a: Owner, b: Owner, archive_a: string, archive_b: string, shared: number, identical: number, 
/**
 * resources in the smaller archive (to tell "patch" from "duplicate")
 */
smaller: number, related: boolean, winner: string | null, };

export type Batch = { batch: string, ts: number, mods: Array<[string, string]>, };

export type Changelog = { id: string, name: string, installed: string, entries: Array<ChangelogEntry>, };

/**
 * One changelog entry, placed relative to the installed version.
 */
export type ChangelogEntry = { version: string, changes: Array<string>, 
/**
 * later than the installed version (never, when that version is unknown)
 */
newer: boolean, 
/**
 * the installed version
 */
installed: boolean, };

export type CollectionInfo = { slug: string, revision: number | null, name: string, author: string, summary: string | null, 
/**
 * Bytes
 */
total_size: number, 
/**
 * The curator's notes (first 2000 characters)
 */
instructions: string, 
/**
 * Load-order rules
 */
rules: number, 
/**
 * Optional mods
 */
optional: number, 
/**
 * Installed through cpmm before
 */
installed: boolean, mods: Array<CollectionMod>, };

export type CollectionMod = { name: string, version: string, mod_id: number, file_id: string, optional: boolean, phase: number, source: string, 
/**
 * Comes with the curator's FOMOD choices
 */
fomod: boolean, 
/**
 * The curator patches files (Vortex-only binary patches)
 */
patches: boolean, status: "installed" | "missing" | "other version" | "bundled" | "manual", };

export type Conflicts = { files: Array<FileRow>, scripts: Array<ScriptRow>, base_overrides: number, };

export type Crash = { report: string, ts: number, message: string, module: string, offset: string, session_start: number | null, seconds_in: number | null, last_good: number | null, changes: Array<string>, 
/**
 * mods that changed since the last good session and can be rolled back
 */
suspects: Array<string>, };

export type DownloadEntry = { 
/**
 * archive file name inside the download folder (the key for install/remove)
 */
file: string, path: string, size: number, 
/**
 * "installed", "other version installed", "not installed"
 */
status: "installed" | "other version installed" | "not installed", installed_as: string | null, 
/**
 * display name (mod name)
 */
name: string, mod_id: number | null, 
/**
 * Nexus file (stable across its uploads) and the exact upload
 */
file_id: string, version_gsid: string, version: string, file_name: string, uploaded_at: string, 
/**
 * where it came from: "nexus" (cpmm download / install), "nxm" (link), "browser", "local"
 */
source: string, downloaded_at: string, };

export type Edge = { before: string, after: string, why: string, strength: "requirement" | "rule" | "heuristic", };

export type ErrDiff = { baseline: boolean, latest_log: number, new: { [key in string]: Array<string> }, fixed: { [key in string]: Array<string> }, totals: { [key in string]: number }, };

export type FileRow = { path: string, 
/**
 * In priority order; the last one wins
 */
mods: Array<string>, winner: string, identical: boolean, overrides_base: boolean, };

export type Finding = { severity: "crash" | "warning" | "perf" | "info" | "acked", title: string, detail: string, 
/**
 * What to do when there is no in-app action (e.g. mod.io mods)
 */
fix: string | null, actions: Array<Remedy>, };

/**
 * The folders in effect (settings resolved against the defaults).
 */
export type Folders = { staging_dir: string, download_dir: string, data_dir: string, game_dir: string, };

/**
 * A FOMOD installer's options, or only `fomod: false` for an archive without one.
 */
export type FomodDesc = { fomod: boolean, 
/**
 * The installer's name
 */
name?: string, 
/**
 * The archive inspected (in the Downloads list for a Nexus mod)
 */
archive?: string, steps?: Array<FomodStep>, };

export type FomodGroup = { 
/**
 * `"<step>/<group>"`, the key of its entry in the choices
 */
key: string, name: string, kind: string, plugins: Array<FomodPlugin>, };

export type FomodPlugin = { name: string, description: string, kind: string, selected: boolean, 
/**
 * A small preview as a data: URI
 */
image: string | null, };

export type FomodStep = { name: string, 
/**
 * Shown for the choices so far
 */
visible: boolean, groups: Array<FomodGroup>, };

export type Framework = { id: string, name: string, version: string, latest_upload: string | null, update_available: boolean, released_since_patch: boolean, };

export type HistEntry = { files: number, ts: number, id: string, version: string, nexus: NexusSource | null, reason: string, batch: string | null, };

/**
 * What to install and how: `cpmm install`'s options. Only `source` is required.
 */
export type InstallRequest = { 
/**
 * A Nexus mod id or page URL, or a local archive or folder (`~/` expands).
 */
source: string, 
/**
 * Nexus file: a file or version id, or part of its name (default: the main file)
 */
file?: string | null, id?: string | null, name?: string | null, 
/**
 * Strip this folder from the archive / place files under this game-relative folder
 */
root?: string | null, dest?: string | null, fomod?: { [key in string]: Array<string> } | null, 
/**
 * Ask the user for FOMOD options the adapter can ask (else the installer's suggestions)
 */
ask?: boolean, missing?: OnMissing, disabled?: boolean, 
/**
 * The Nexus mod a local archive belongs to (enables update checks)
 */
nexus?: number | null, keep_archive?: boolean, };

export type LoadOrder = { plan: SortPlan, rules: Array<Rule>, 
/**
 * Archives in modlist.txt order
 */
modlist: Array<string>, };

/**
 * One line of a job's log, as the GUI receives it (Tauri event `cpmm-log`).
 */
export type LogLine = { job: number, line: string, stderr: boolean, };

export type Lookup = { mod_id: number, name: string, version: string, author: string, url: string, files: Array<LookupFile>, requires: Array<LookupReq>, };

export type LookupFile = { gsid: string, name: string, version: string, category: string, uploaded_at: string, };

export type LookupReq = { mod_id: number, name: string, notes: string, optional: boolean, 
/**
 * Outside Nexus
 */
external: boolean, };

export type ModInfo = { id: string, name: string, files: Array<OwnedFile>, hidden: Array<string>, ignored_reqs: Array<number>, };

/**
 * One installed mod in priority order (1 = lowest).
 */
export type ModRow = { priority: number, id: string, name: string, enabled: boolean, version: string, installed_at: string, notes: string, nexus: NexusRef | null, 
/**
 * The FOMOD choices it was installed with
 */
fomod: Record<string, string[]> | null, files: number, hidden: number, 
/**
 * Conflicting files it wins and loses
 */
wins: number, loses: number, requires: Array<Requirement>, 
/**
 * Unmet hard requirements (0 for a disabled mod)
 */
dep_problems: number, };

export type ModioChanges = { first_look: boolean, added: Array<string>, removed: Array<string>, updated: Array<string>, };

export type ModioConflict = { modio: string, kind: "same mod" | "files" | "archive" | "script", with: string, detail: string, severe: boolean, };

export type ModioData = { mods: Array<ModioRow>, changes: ModioChanges, conflicts: Array<ModioConflict>, };

export type ModioRow = { id: string, name: string, author: string, version: string, files: number, };

export type ModioSub = { id: string, name: string, author: string, version: string, };

export type Moved = { id: string, from: number, to: number, because: Array<string>, };

export type NexusRef = { mod_id: number, file_name: string, file_id: string, version_gsid: string, uploaded_at: string, url: string, };

export type NexusSource = { 
/**
 * Game-scoped mod id, the number in the mod page URL.
 */
mod_id: number, 
/**
 * v3 mod uid.
 */
mod_uid: string, 
/**
 * v3 mod file id (stable across uploads of the same file).
 */
file_id: string, file_name: string, 
/**
 * v3 mod file version id (one specific upload).
 */
version_id: string, 
/**
 * Game-scoped id of that upload (used for download links).
 */
version_gsid: string, uploaded_at: string, manifest_url: string, };

/**
 * What to do about a hard requirement that is not installed.
 */
export type OnMissing = "refuse" | "install" | "warn";

export type Onboarding = { game_dir: string, game_found: boolean, api_key: boolean, launch_options: string | null, launch_ok: boolean, launch_suggested: string, steam_running: boolean, steam_cef: boolean, nxm_handler: boolean, onboarded: boolean, mods_installed: number, };

export type OverwriteRow = { path: string, kind: OwKind, shadows: string | null, };

export type OwKind = "deleted" | "edited mod file" | "edited base file" | "new";

/**
 * One staged file of a mod and who else ships it.
 */
export type OwnedFile = { path: string, others: Array<string>, 
/**
 * The enabled mod whose copy the game sees ("" if none is enabled)
 */
winner: string, wins: boolean, };

export type Owner = { "kind": "mod", "id": string } | { "kind": "base", "id": string } | { "kind": "modio", "id": string };

export type PatchStatus = { build_id: string, previous_build: string | null, patched_at: number, 
/**
 * the game changed build since cpmm first saw it
 */
patch_detected: boolean, at_risk: Array<Framework>, red4ext_errors: Array<string>, vanilla: boolean, };

/**
 * Where `Action::Order` puts a mod.
 */
export type Place = "top" | "bottom" | { "before": string } | { "after": string };

export type ProfileAction = { "do": "create", name: string, from: string | null, empty: boolean, own_saves: boolean, copy_saves: boolean, switch: boolean, } | { "do": "switch", name: string, } | { "do": "rename", old: string, new: string, } | { "do": "saves", name: string, own: boolean, copy: boolean, } | { "do": "delete", name: string, };

export type ProfileRow = { name: string, 
/**
 * Mods enabled in it
 */
enabled: number, active: boolean, created_at: string, own_saves: boolean, saves: number, };

/**
 * A read-only view, by name (`{"view": "info", "mod": "…"}` from the GUI; `cpmm api info …`).
 */
export type Query = { "view": "state" } | { "view": "info", mod: string, } | { "view": "conflicts" } | { "view": "updates", refresh: boolean, } | { "view": "overwrite" } | { "view": "tracked" } | { "view": "lookup", source: string, } | { "view": "loadorder" } | { "view": "doctor" } | { "view": "archives" } | { "view": "safety" } | { "view": "history", mod: string, } | { "view": "settings" } | { "view": "downloads" } | { "view": "onboarding" } | { "view": "changelog", mod: string, } | { "view": "modio" } | { "view": "fomod", source: string, file?: string | null, choices?: { [key in string]: Array<string> } | null, } | { "view": "collection", reference: string, };

/**
 * A one-click fix: the operation the GUI runs when the user picks `label`. `args` is the
 * same as a `cpmm` command line (what `cpmm doctor` prints; for scripts reading `api doctor`).
 */
export type Remedy = { label: string, op: Action, args: Array<string>, 
/**
 * "primary" for the recommended choice
 */
style: "primary" | "secondary", };

/**
 * Where one requirement of a mod stands.
 *
 * The rule: a dependency that is installed and enabled is `Ok`; otherwise a requirement
 * the user chose to do without is `Ignored`; otherwise it is disabled or missing, and
 * optional requirements get their own variants because they are never a problem.
 */
export type ReqStatus = "ok" | "ignored" | "disabled" | "optional-disabled" | "missing" | "optional-missing";

export type Requirement = { mod_id: number, name: string, notes: string, optional: boolean, status: ReqStatus, installed_as: string | null, url: string, };

export type Rule = { 
/**
 * This mod must load after (win over) `after`.
 */
mod: string, after: string, };

export type Safety = { crashes: Array<Crash>, errors: ErrDiff, patch: PatchStatus, snapshots: Array<Snapshot>, save_risks: Array<SaveRisk>, batches: Array<Batch>, 
/**
 * Game sessions recorded
 */
sessions: number, };

export type SaveRisk = { save: string, saved_at: number, missing: Array<string>, };

export type ScriptRow = { kind: string, target: string, mods: Array<string>, severe: boolean, };

export type SessionInfo = { pid: number, started: string, layers: number, };

/**
 * One setting as the GUI's Settings page shows it.
 */
export type SettingField = { key: keyof Settings, group: string, label: string, help: string, kind: "bool" | "number" | "text" | "path", };

export type Settings = { check_updates_on_start: boolean, auto_install_updates: boolean, refresh_minutes: number, auto_sort: boolean, after_install_verify: boolean, after_install_doctor: boolean, after_install_command: string, auto_install_downloads: boolean, keep_archives: boolean, staging_dir: string, download_dir: string, profiles_enabled: boolean, new_profiles_own_saves: boolean, snapshot_saves: boolean, keep_versions: number, keep_save_snapshots: number, close_on_launch: boolean, onboarded: boolean, };

export type SettingsData = { values: Settings, schema: Array<SettingField>, resolved: Folders, api_key: boolean, };

export type Snapshot = { name: string, ts: number, reason: string, saves: number, 
/**
 * Profile whose saves these are ("" for snapshots from before profiles).
 */
profile: string, };

export type SortPlan = { order: Array<string>, moved: Array<Moved>, edges: Array<Edge>, cycles: Array<Array<string>>, unresolved: Array<ArchivePair>, };

export type TrackedRow = { mod_id: number, name: string, installed_as: string | null, url: string, };

export type UpdateInfo = { version: string, uploaded_at: string, 
/**
 * The upload's game-scoped id (`cpmm install <mod> --file <gsid>`)
 */
gsid: string, };

export type UpdateRow = { id: string, status: string, update: UpdateInfo | null, };
