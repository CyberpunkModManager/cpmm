//! Tools: Windows programs that work on the modded game (VRAM RipperDoc, texture optimisers,
//! save editors), run through Proton's Wine with the mods mounted.
//!
//! A tool is staged like a mod but never mounted in the game session: its files live in
//! `tools/<id>/files`, laid out game-relative (usually `tools/<Name>/…`, where such tools
//! expect to sit). A run gets its own namespace ([`crate::vfs::sandbox_exec`]): the tool's
//! files over cpmm's overwrite folder (read-only), the enabled mods and the game, with a
//! throwaway upper layer in `tool-runs/<run>/upper` that catches everything the tool writes.
//! After the run, those writes are its outputs: reports (under `tools/`), game files (which
//! [`save`] turns into a mod), load-order edits (cpmm owns modlist.txt; they are not applied)
//! and deletions (not applied either).
//!
//! Wine: the game's own Proton build (Steam's compatibility tool for the game), its `wine`
//! binary run directly. `proton run` detaches console programs from the terminal, and
//! Steam's runtime container isn't needed for a tool. Tools get their own Wine prefix
//! (`tool-prefix`) so they can't upset the game's; the game's settings and saves folders are
//! linked into it. An action that has to see the running game uses the game's prefix.

use crate::state::{Paths, STEAM_APP_ID, now, slugify};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use ts_rs::TS;

/* ------------------------------------------------------------------ where things live */

pub fn tools_dir(paths: &Paths) -> PathBuf {
    paths.data.join("tools")
}
fn tool_dir(paths: &Paths, id: &str) -> PathBuf {
    tools_dir(paths).join(id)
}
/// The tool's files, laid out game-relative: the top layer of its runs.
pub fn tool_files(paths: &Paths, id: &str) -> PathBuf {
    tool_dir(paths, id).join("files")
}
pub fn runs_dir(paths: &Paths) -> PathBuf {
    paths.data.join("tool-runs")
}
fn run_dir(paths: &Paths, run: &str) -> PathBuf {
    runs_dir(paths).join(run)
}
/// The Wine prefix tools run in (not the game's).
pub fn prefix_dir(paths: &Paths) -> PathBuf {
    paths.data.join("tool-prefix")
}
/// The game's own Wine prefix (compatdata next to the game's Steam library).
pub fn game_prefix(paths: &Paths) -> PathBuf {
    let lib = paths.game.parent().and_then(|p| p.parent()).map(Path::to_path_buf).unwrap_or_default();
    lib.join("compatdata").join(STEAM_APP_ID).join("pfx")
}

/// A Unix path as Wine's Z: drive sees it.
pub fn windows_path(p: &Path) -> String {
    format!("Z:{}", p.to_string_lossy().replace('/', "\\"))
}

/* ------------------------------------------------------------------ installed tools */

/// `tools/<id>/tool.json`.
#[derive(Serialize, Deserialize, Clone, Debug, Default, TS)]
#[serde(default)]
pub struct ToolMeta {
    pub id: String,
    pub name: String,
    pub version: String,
    pub mod_id: Option<u64>,
    pub file_id: String,
    /// The Downloads entry it was installed from, if any
    pub archive: String,
    pub installed_at: String,
}

/// Something a tool can do: a known preset, or one of its programs run as is.
#[derive(Serialize, Clone, Debug, PartialEq, TS)]
pub struct ToolAction {
    /// What `cpmm tools run <tool> <action>` names: a preset's id, or the program's path
    pub id: String,
    pub label: String,
    pub description: String,
    /// The program, game-relative ('/'-separated)
    pub program: String,
    /// Arguments; `{game}` is the game folder as Windows sees it, `{game_dir}` as Linux does,
    /// `{input}` the user's answer
    pub args: Vec<String>,
    pub input: Option<ActionInput>,
    pub runner: Runner,
    /// Runs in the game's own Wine prefix (to see the running game)
    pub game_prefix: bool,
    /// Writes go where the game sees them (cpmm's overwrite folder, like the game's own
    /// writes) instead of a scratch layer: for helpers whose steps build on each other
    pub live: bool,
    /// When it can run
    pub when: GameState,
    /// A known action for this tool (shown first); else a program found in its files
    pub preset: bool,
}

/// What runs a tool's program.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq, TS)]
#[serde(rename_all = "snake_case")]
pub enum Runner {
    /// Proton's wine (Windows programs, .bat files)
    Wine,
    /// The system's python3 (Linux-side helpers that come with a mod)
    Python,
}

/// Whether an action needs the game closed or running.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq, TS)]
#[serde(rename_all = "snake_case")]
pub enum GameState {
    Any,
    Closed,
}

#[derive(Serialize, Clone, Debug, PartialEq, TS)]
pub struct ActionInput {
    pub label: String,
    pub default: String,
}

/// An installed tool with what it can do: a standalone tool, or a helper that ships inside
/// an installed mod (whose actions then run in the game's own view).
#[derive(Serialize, Clone, Debug, TS)]
#[ts(rename = "ToolEntry")]
pub struct Tool {
    #[serde(flatten)]
    pub meta: ToolMeta,
    pub dir: PathBuf,
    pub actions: Vec<ToolAction>,
    /// The installed mod this helper comes with (remove that mod to remove it)
    pub from_mod: Option<String>,
    /// How to use it, in order (a helper with steps across game launches)
    pub guide: Option<String>,
    /// Where a step-by-step helper is, read from its state files
    pub progress: Option<Progress>,
}

/// A step-by-step helper's checklist and how far along it is.
#[derive(Serialize, Clone, Debug, PartialEq, TS)]
pub struct Progress {
    /// Where things stand, in a sentence
    pub stage: String,
    pub steps: Vec<Step>,
    /// The step to do next (steps.len() when all are done)
    pub current: usize,
    /// The tweak is in effect: rolling back undoes it
    pub active: bool,
    /// Why no step can run now (another mod owns the setting)
    pub blocked: Option<String>,
}

#[derive(Serialize, Clone, Debug, PartialEq, TS)]
pub struct Step {
    pub title: String,
    pub detail: String,
    /// The action that does it; None: start the game (and quit it)
    pub action: Option<String>,
}

struct Preset {
    /// the program's path, lower case: its file name, or a game-relative path for a helper
    /// that ships inside a mod
    program: &'static str,
    id: &'static str,
    label: &'static str,
    description: &'static str,
    args: &'static [&'static str],
    input: Option<(&'static str, &'static str)>,
    runner: Runner,
    game_prefix: bool,
    live: bool,
    when: GameState,
}

const fn wine(
    program: &'static str,
    id: &'static str,
    label: &'static str,
    description: &'static str,
    args: &'static [&'static str],
) -> Preset {
    Preset {
        program,
        id,
        label,
        description,
        args,
        input: None,
        runner: Runner::Wine,
        game_prefix: false,
        live: false,
        when: GameState::Any,
    }
}

/// Save Load Optimizer's helper (Nexus 37424): ships in the mod's tools/ folder; its steps keep
/// an ownership record next to it and write one engine INI, so they run live.
const SLO: &str = "tools/saveloadoptimizer/profile.py";
const fn slo(id: &'static str, label: &'static str, description: &'static str, args: &'static [&'static str], when: GameState) -> Preset {
    Preset { program: SLO, id, label, description, args, input: None, runner: Runner::Python, game_prefix: false, live: true, when }
}

/// Actions cpmm knows for particular tools (matched by program, so a copy installed from
/// anywhere gets them). Each one runs without questions: a run has no keyboard in Game Mode.
const PRESETS: &[Preset] = &[
    wine(
        "vramripperdoc.exe",
        "scan",
        "Scan my mods",
        "Ranks your texture and mesh mods by the VRAM they add and writes a report. Changes nothing.",
        &["--game", "{game}", "--no-open"],
    ),
    wine(
        "vramripperdoc.exe",
        "budget-fix",
        "Make budget fix",
        "Writes one archive with half-size copies of only the heaviest textures. Save the output as a mod to use it; disable or remove that mod to undo.",
        &["--game", "{game}", "--no-open", "--make-budget-archive"],
    ),
    wine(
        "vramripperdoc.exe",
        "budget-fix-all",
        "Make budget fix (with new items)",
        "As Make budget fix, and also halves 4K textures of new clothing, cars and props (they only use VRAM while you use them).",
        &["--game", "{game}", "--no-open", "--make-budget-archive", "--include-new-items"],
    ),
    Preset {
        program: "vramripperdoc.exe",
        id: "measure",
        label: "Measure VRAM in game",
        description: "Records the game's real VRAM use: start it, then launch the game, play a minute and close it to finish. Experimental under cpmm (runs in the game's Wine prefix).",
        args: &["--game", "{game}", "--live", "--label", "{input}"],
        input: Some(("Name this measurement", "before")),
        runner: Runner::Wine,
        game_prefix: true,
        live: false,
        when: GameState::Any,
    },
    slo("status", "Status", "Shows the recorded state. Changes nothing.", &["status", "--game", "{game_dir}"], GameState::Any),
    slo(
        "initialize",
        "1. Fingerprint this setup",
        "Records your game, CET and engine-config fingerprint. Changes no settings. Then start the game once and quit: the collector records your baseline.",
        &["initialize", "--game", "{game_dir}"],
        GameState::Closed,
    ),
    slo(
        "prepare",
        "2. Prepare",
        "After that launch: proposes your baseline +25% (at most +16 MiB). Changes nothing yet.",
        &["prepare", "--game", "{game_dir}"],
        GameState::Closed,
    ),
    slo("activate", "3. Activate", "Writes the one-line engine INI. It takes effect at the next launch.", &["activate"], GameState::Closed),
    slo(
        "verify",
        "4. Verify",
        "After a launch with it active: checks the game read the new value (not a speed measurement).",
        &["verify"],
        GameState::Any,
    ),
    slo(
        "rollback",
        "Roll back",
        "Removes the INI it wrote, if unchanged. A new baseline launch is needed before preparing again.",
        &["rollback"],
        GameState::Closed,
    ),
];

const SLO_GUIDE: &str = "Experimental: raises one decompression-memory setting (how much memory the game uses to unpack data while loading) \
by 25%, at most 16 MiB, over your own setup's value. Follow the steps in order; cpmm shows which one is next. Its state lives in cpmm's overwrite \
folder, so don't clear that while it's active. Updating the game or CET, or changing mods that add engine INIs, invalidates the fingerprint: \
roll back first, then start over.";

/// Save Load Optimizer's files, game-relative (lower case; found case-insensitively).
const SLO_BUILD: &str = "bin/x64/plugins/cyber_engine_tweaks/mods/saveloadoptimizer/build.json";
const SLO_CAPTURE: &str = "bin/x64/plugins/cyber_engine_tweaks/mods/saveloadoptimizer/capture.json";
const SLO_PROFILE: &str = "tools/saveloadoptimizer/profile.json";

fn slo_steps() -> Vec<Step> {
    let s = |title: &str, detail: &str, action: Option<&str>| Step {
        title: title.into(),
        detail: detail.into(),
        action: action.map(String::from),
    };
    vec![
        s(
            "Fingerprint your setup",
            "Records checksums of the game, CET and your engine INIs. Changes no settings. Game closed.",
            Some("initialize"),
        ),
        s("Start the game, then quit", "Reaching the main menu is enough: the collector records your current value (the baseline).", None),
        s("Prepare", "Works out the new value from your baseline. Changes nothing yet.", Some("prepare")),
        s("Activate", "Writes the one-line engine INI with the new value. Game closed.", Some("activate")),
        s(
            "Start the game and load a save",
            "The game now starts with the new value; the collector reads it back. Quit when you've tried it, or keep playing.",
            None,
        ),
        s("Verify", "Checks that the game read the new value. It doesn't measure speed: time a few loads yourself.", Some("verify")),
    ]
}

/// Read a file of the game's view (overwrite folder above the mod's own files).
fn view_json(paths: &Paths, mod_files: &Path, rel_lower: &str) -> Option<serde_json::Value> {
    let p = [paths.upper(), mod_files.to_path_buf()].into_iter().find_map(|r| find_ci(&r, rel_lower).map(|f| r.join(f)))?;
    serde_json::from_slice(&fs::read(p).ok()?).ok()
}

/// Where Save Load Optimizer is, from its fingerprint, capture and ownership record.
/// The value `[section] key = value` sets in an INI text (last one wins), case-insensitively.
pub fn ini_value(text: &str, section: &str, key: &str) -> Option<String> {
    let (mut cur, mut found) = (String::new(), None);
    for line in text.trim_start_matches('\u{feff}').lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with([';', '#']) {
            continue;
        }
        if let Some(rest) = line.strip_prefix('[')
            && let Some(end) = rest.find(']')
        {
            cur = rest[..end].trim().to_lowercase();
        } else if let Some((k, v)) = line.split_once('=')
            && cur == section.to_lowercase()
            && k.trim().eq_ignore_ascii_case(key)
        {
            found = Some(v.split([';', '#']).next().unwrap_or("").trim().to_string());
        }
    }
    found
}

/// Another engine INI in the game's view that sets Save Load Optimizer's option:
/// (who it comes from, its game-relative path, the value).
fn slo_conflict(paths: &Paths, st: &crate::state::State) -> Option<(String, String, String)> {
    let mut roots = vec![("cpmm's overwrite folder".to_string(), paths.upper())];
    roots.extend(st.mods.iter().rev().filter(|m| m.enabled).map(|m| (m.name.clone(), paths.mod_files(&m.id))));
    roots.push(("the game folder".to_string(), paths.game.clone()));
    for (owner, root) in roots {
        for e in walkdir::WalkDir::new(root.join("engine/config")).into_iter().flatten() {
            let name = e.file_name().to_string_lossy().to_lowercase();
            if !e.file_type().is_file() || !name.ends_with(".ini") || name == "zzz_save_load_optimizer.ini" {
                continue;
            }
            let text = String::from_utf8_lossy(&fs::read(e.path()).unwrap_or_default()).into_owned();
            if let Some(v) = ini_value(&text, "ResourceLoaderThrottler", "DedicatedDecompressionMemoryBytes") {
                let rel = e.path().strip_prefix(&root).map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
                return Some((owner, rel, v));
            }
        }
    }
    None
}

pub fn slo_progress(paths: &Paths, st: &crate::state::State, mod_files: &Path) -> Progress {
    let mut p = slo_progress_files(paths, mod_files);
    if let Some((owner, rel, value)) = slo_conflict(paths, st) {
        let mib = value.parse::<f64>().map(|b| format!(" ({:.2} MiB)", b / 1048576.0)).unwrap_or_default();
        let msg = format!(
            "{owner} already sets this setting: DedicatedDecompressionMemoryBytes = {value}{mib}, in {rel}. \
             Save Load Optimizer won't add a second override, so its steps refuse while that is enabled. \
             To use it instead, disable that mod (or remove that line from its INI); otherwise that mod's value stays in effect."
        );
        p.stage = msg.clone();
        p.blocked = Some(msg);
    }
    p
}

fn slo_progress_files(paths: &Paths, mod_files: &Path) -> Progress {
    let build = view_json(paths, mod_files, SLO_BUILD).is_some();
    let capture = view_json(paths, mod_files, SLO_CAPTURE);
    let profile = view_json(paths, mod_files, SLO_PROFILE);
    let captured = capture.as_ref().and_then(|c| c["captured_at"].as_f64()).unwrap_or(0.0);
    let fresh = captured > 0.0 && (crate::safety::now_unix() as f64 - captured) <= 7.0 * 86400.0;
    let num = |k: &str| profile.as_ref().and_then(|p| p[k].as_i64());
    let status = profile.as_ref().and_then(|p| p["status"].as_str()).unwrap_or("");
    let (current, stage, active) = match status {
        _ if !build => (0, "Not set up yet.".to_string(), false),
        "" if capture.is_none() => (1, "Fingerprinted. Start the game once so the collector records your baseline.".into(), false),
        "" if !fresh => (1, "The baseline is more than 7 days old: start the game once for a fresh one.".into(), false),
        "" => (2, "Baseline recorded. Prepare, then activate.".into(), false),
        "staged" => (
            3,
            format!(
                "Prepared: {} → {} bytes. Activate it while the game is closed.",
                num("baseline").unwrap_or(0),
                num("candidate").unwrap_or(0)
            ),
            false,
        ),
        "installing" | "configured_unverified" if captured >= num("activated_at").unwrap_or(i64::MAX) as f64 => {
            (5, "Active, and the game has started with it. Verify that it read the new value.".into(), true)
        }
        "installing" | "configured_unverified" => (4, "Active from the next launch: start the game and load a save.".into(), true),
        "configured_readback_verified" => (
            6,
            format!(
                "Active and verified: the game reads {} bytes (was {}). Roll back if loads aren't faster or NPCs or textures pop in.",
                num("observed_bytes").or(num("candidate")).unwrap_or(0),
                num("baseline").unwrap_or(0)
            ),
            true,
        ),
        "rolled_back" if fresh && captured > num("rolled_back_at").unwrap_or(i64::MAX) as f64 => {
            (2, "Rolled back; a new baseline has been recorded. Prepare again to retry.".into(), false)
        }
        "rolled_back" => (1, "Rolled back. To try again, start the game once for a new baseline.".into(), false),
        other => (0, format!("Unknown state {other:?}: run Status to see the details."), false),
    };
    Progress { stage, steps: slo_steps(), current, active, blocked: None }
}

fn is_program(rel: &str) -> bool {
    let l = rel.to_lowercase();
    l.ends_with(".exe") || l.ends_with(".bat") || l.ends_with(".cmd")
}

fn preset_matches(p: &Preset, rel_lower: &str) -> bool {
    if p.program.contains('/') { rel_lower == p.program } else { rel_lower.rsplit('/').next() == Some(p.program) }
}

fn from_preset(p: &Preset, program: &str) -> ToolAction {
    ToolAction {
        id: p.id.into(),
        label: p.label.into(),
        description: p.description.into(),
        program: program.to_string(),
        args: p.args.iter().map(|a| a.to_string()).collect(),
        input: p.input.map(|(label, default)| ActionInput { label: label.into(), default: default.into() }),
        runner: p.runner,
        game_prefix: p.game_prefix,
        live: p.live,
        when: p.when,
        preset: true,
    }
}

/// A tool's actions: presets for its known programs, then every Windows program in it.
pub fn actions(files: &[String]) -> Vec<ToolAction> {
    let mut out = vec![];
    for f in files {
        let l = f.to_lowercase();
        out.extend(PRESETS.iter().filter(|p| preset_matches(p, &l)).map(|p| from_preset(p, f)));
    }
    for f in files.iter().filter(|f| is_program(f)) {
        let name = f.rsplit('/').next().unwrap_or(f);
        let lower = name.to_lowercase();
        if lower.starts_with("unins") || lower.contains("vc_redist") || lower.contains("vcredist") || lower.starts_with("dotnet") {
            continue;
        }
        out.push(ToolAction {
            id: f.clone(),
            label: format!("Run {name}"),
            description: String::new(),
            program: f.clone(),
            args: vec![],
            input: None,
            runner: Runner::Wine,
            game_prefix: false,
            live: false,
            when: GameState::Any,
            preset: false,
        });
    }
    out
}

fn load(paths: &Paths, id: &str) -> Result<Tool> {
    let dir = tool_dir(paths, id);
    let meta: ToolMeta = serde_json::from_slice(&fs::read(dir.join("tool.json")).with_context(|| format!("no tool {id:?}"))?)?;
    let files = crate::files::list_tree(&dir.join("files"));
    Ok(Tool { actions: actions(&files), dir, meta, from_mod: None, guide: None, progress: None })
}

/// Installed standalone tools, by name.
pub fn list(paths: &Paths) -> Vec<Tool> {
    let mut v: Vec<Tool> = fs::read_dir(tools_dir(paths))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| load(paths, &e.file_name().to_string_lossy()).ok())
        .collect();
    v.sort_by_key(|t| t.meta.name.to_lowercase());
    v
}

/// Helpers that ship inside enabled mods (cpmm knows where each one's program sits).
pub fn mod_helpers(paths: &Paths, st: &crate::state::State) -> Vec<Tool> {
    let helpers: BTreeSet<&str> = PRESETS.iter().filter(|p| p.program.contains('/')).map(|p| p.program).collect();
    let mut v = vec![];
    for m in st.mods.iter().filter(|m| m.enabled) {
        let files = paths.mod_files(&m.id);
        for h in &helpers {
            // staged paths keep their own case below the canonical game folders
            let Some(rel) = find_ci(&files, h) else { continue };
            let actions: Vec<ToolAction> = PRESETS.iter().filter(|p| p.program == *h).map(|p| from_preset(p, &rel)).collect();
            v.push(Tool {
                meta: ToolMeta {
                    id: m.id.clone(),
                    name: m.name.clone(),
                    version: m.version.clone(),
                    mod_id: m.nexus.as_ref().map(|n| n.mod_id),
                    installed_at: m.installed_at.clone(),
                    ..Default::default()
                },
                dir: files.clone(),
                actions,
                from_mod: Some(m.id.clone()),
                guide: (*h == SLO).then(|| SLO_GUIDE.to_string()),
                progress: (*h == SLO).then(|| slo_progress(paths, st, &files)),
            });
        }
    }
    v
}

/// `rel_lower` under `root`, matching each component without regard to case.
fn find_ci(root: &Path, rel_lower: &str) -> Option<String> {
    let mut at = root.to_path_buf();
    let mut out = vec![];
    for c in rel_lower.split('/') {
        let name =
            fs::read_dir(&at).ok()?.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).find(|n| n.to_lowercase() == c)?;
        at.push(&name);
        out.push(name);
    }
    at.is_file().then(|| out.join("/"))
}

/// Every tool: standalone ones, then helpers in mods.
pub fn all(paths: &Paths, st: &crate::state::State) -> Vec<Tool> {
    let mut v = list(paths);
    v.extend(mod_helpers(paths, st));
    v
}

/// A tool by id, number in the list (1-based), or unique part of its name.
pub fn find(paths: &Paths, st: &crate::state::State, key: &str) -> Result<Tool> {
    let all = all(paths, st);
    if let Ok(n) = key.parse::<usize>()
        && n >= 1
        && n <= all.len()
    {
        return Ok(all[n - 1].clone());
    }
    if let Some(t) = all.iter().find(|t| t.meta.id == key) {
        return Ok(t.clone());
    }
    let k = key.to_lowercase();
    let hits: Vec<&Tool> = all.iter().filter(|t| t.meta.name.to_lowercase().contains(&k) || t.meta.id.contains(&k)).collect();
    match hits.as_slice() {
        [t] => Ok((*t).clone()),
        [] => bail!("no tool matches {key:?} (cpmm tools lists them)"),
        _ => bail!("{key:?} matches several tools: {}", hits.iter().map(|t| t.meta.id.as_str()).collect::<Vec<_>>().join(", ")),
    }
}

/* ------------------------------------------------------------------ is it a tool, and its layout */

const GAME_TOPS: &[&str] = &["archive", "bin", "r6", "red4ext", "engine", "mods"];

/// Is an archive with these files a Windows tool rather than a mod? It has a program, and
/// nothing where the game loads mods from (at its root or behind one wrapper folder).
pub fn is_tool(files: &[String]) -> bool {
    let game_content = files.iter().any(|f| {
        let comps: Vec<String> = f.split('/').map(str::to_lowercase).collect();
        comps.len() > 1 && comps.iter().take(2).take(comps.len() - 1).any(|c| GAME_TOPS.contains(&c.as_str()))
    });
    !game_content && files.iter().any(|f| is_program(f))
}

/// Where each file goes, game-relative: under its `tools/` folder if it has one (wrapper
/// folders dropped), else under `tools/<its one top folder>/` or `tools/<name>/`.
pub fn layout(files: &[String], name: &str) -> Vec<(String, String)> {
    let tools_at = |f: &str| f.split('/').position(|c| c.eq_ignore_ascii_case("tools"));
    let prefix = |f: &str| tools_at(f).map(|i| f.split('/').take(i).collect::<Vec<_>>().join("/"));
    let prefixes: BTreeSet<Option<String>> = files.iter().map(|f| prefix(f)).collect();
    if let [Some(pre)] = prefixes.iter().collect::<Vec<_>>().as_slice()
        && files.iter().all(|f| f.split('/').count() > tools_at(f).unwrap_or(0) + 2)
    {
        let cut = if pre.is_empty() { 0 } else { pre.len() + 1 };
        return files.iter().map(|f| (f.clone(), format!("tools/{}", &f[cut + "tools/".len()..]))).collect();
    }
    let tops: BTreeSet<&str> = files.iter().map(|f| f.split('/').next().unwrap_or("")).collect();
    if tops.len() == 1 && files.iter().all(|f| f.contains('/')) {
        return files.iter().map(|f| (f.clone(), format!("tools/{f}"))).collect();
    }
    let dir = name.replace(['/', '\\'], " ");
    files.iter().map(|f| (f.clone(), format!("tools/{}/{f}", dir.trim()))).collect()
}

/// The files in an archive, without extracting it.
pub fn archive_files(archive: &Path) -> Result<Vec<String>> {
    let out = Command::new("7z")
        .args(["l", "-ba", "-slt"])
        .arg(archive)
        .env_remove("LD_PRELOAD")
        .output()
        .context("running 7z (install p7zip/7zip)")?;
    if !out.status.success() {
        bail!("7z couldn't list {}: {}", archive.display(), String::from_utf8_lossy(&out.stderr));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut files = vec![];
    // blocks of "Key = value" lines, one per entry
    for block in text.split("\n\n") {
        let get = |k: &str| block.lines().find_map(|l| l.strip_prefix(k).and_then(|r| r.strip_prefix(" = ")).map(str::to_string));
        if let Some(p) = get("Path")
            && get("Folder").as_deref() != Some("+")
            && get("Attributes").is_none_or(|a| !a.starts_with('D'))
        {
            files.push(p.replace('\\', "/"));
        }
    }
    Ok(files)
}

/// Install (or replace) a tool from an archive or folder.
pub fn install(paths: &Paths, source: &Path, mut meta: ToolMeta) -> Result<(ToolMeta, bool)> {
    paths.ensure_dirs()?;
    let tmp = crate::install::stage::tmp_extract_dir(paths, &format!("tool-{}", slugify(&meta.name)));
    let _ = fs::remove_dir_all(&tmp);
    let root = if source.is_dir() {
        source.to_path_buf()
    } else {
        crate::install::stage::extract(source, &tmp)?;
        tmp.clone()
    };
    let files = crate::files::list_tree(&root);
    if files.is_empty() {
        bail!("{} has no files", source.display());
    }
    if meta.id.is_empty() {
        meta.id = slugify(crate::state::short_name(&meta.name));
    }
    let dir = tool_dir(paths, &meta.id);
    let replaced = dir.exists();
    let staging = tools_dir(paths).join(format!(".{}-new", meta.id));
    let _ = fs::remove_dir_all(&staging);
    for (from, to) in layout(&files, &meta.name) {
        let (src, dst) = (root.join(&from), staging.join("files").join(&to));
        fs::create_dir_all(dst.parent().unwrap())?;
        if source.is_dir() {
            crate::files::reflink_or_copy(&src, &dst)?;
        } else {
            crate::files::move_file(&src, &dst)?;
        }
    }
    meta.installed_at = now();
    fs::write(staging.join("tool.json"), serde_json::to_vec_pretty(&meta)?)?;
    if replaced {
        fs::create_dir_all(paths.trash())?;
        fs::rename(&dir, paths.trash_for(&format!("tool-{}", meta.id)))?;
    }
    fs::rename(&staging, &dir)?;
    let _ = fs::remove_dir_all(&tmp);
    Ok((meta, replaced))
}

/// Uninstall: the tool's folder goes to cpmm's trash. Its runs stay until discarded.
pub fn remove(paths: &Paths, t: &Tool) -> Result<PathBuf> {
    fs::create_dir_all(paths.trash())?;
    let to = paths.trash_for(&format!("tool-{}", t.meta.id));
    fs::rename(&t.dir, &to)?;
    Ok(to)
}

/* ------------------------------------------------------------------ Wine */

/// The Wine that runs tools.
#[derive(Serialize, Clone, Debug, TS)]
pub struct Wine {
    /// e.g. "proton-cachyos-slr", "Proton - Experimental", "system wine"
    pub name: String,
    pub wine: PathBuf,
    /// Its Wine libraries (WINEDLLPATH), for a Proton build
    pub lib: Option<PathBuf>,
    /// It's the compatibility tool Steam runs the game with
    pub games: bool,
}

/// A minimal text VDF tree (Steam's config.vdf, compatibilitytool.vdf).
#[derive(Debug, PartialEq)]
enum Vdf {
    S(String),
    M(Vec<(String, Vdf)>),
}

fn vdf_tokens(s: &str) -> Vec<String> {
    let mut out = vec![];
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '"' => {
                let mut t = String::new();
                while let Some(c) = it.next() {
                    match c {
                        '\\' => {
                            if let Some(n) = it.next() {
                                t.push(n)
                            }
                        }
                        '"' => break,
                        c => t.push(c),
                    }
                }
                out.push(format!("\"{t}"));
            }
            '{' | '}' => out.push(c.to_string()),
            '/' if it.peek() == Some(&'/') => {
                for c in it.by_ref() {
                    if c == '\n' {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

fn vdf_parse(t: &[String], i: &mut usize) -> Vec<(String, Vdf)> {
    let mut m = vec![];
    while *i < t.len() {
        let k = &t[*i];
        if k == "}" {
            *i += 1;
            break;
        }
        *i += 1;
        let Some(key) = k.strip_prefix('"') else { continue };
        match t.get(*i).map(String::as_str) {
            Some("{") => {
                *i += 1;
                m.push((key.to_string(), Vdf::M(vdf_parse(t, i))));
            }
            Some(v) if v.starts_with('"') => {
                *i += 1;
                m.push((key.to_string(), Vdf::S(v[1..].to_string())));
            }
            _ => {}
        }
    }
    m
}

fn vdf(s: &str) -> Vec<(String, Vdf)> {
    let t = vdf_tokens(s);
    let mut i = 0;
    vdf_parse(&t, &mut i)
}

fn vdf_get<'a>(m: &'a [(String, Vdf)], path: &[&str]) -> Option<&'a Vdf> {
    let (first, rest) = path.split_first()?;
    let v = m.iter().find(|(k, _)| k.eq_ignore_ascii_case(first)).map(|(_, v)| v)?;
    match (rest.is_empty(), v) {
        (true, v) => Some(v),
        (false, Vdf::M(sub)) => vdf_get(sub, rest),
        _ => None,
    }
}

/// The compatibility tool Steam runs the game with (config.vdf's CompatToolMapping).
pub fn game_compat_tool(config_vdf: &str) -> Option<String> {
    let tree = vdf(config_vdf);
    let at = |root: &[&str]| -> Option<String> {
        let mut path = root.to_vec();
        path.extend(["CompatToolMapping", STEAM_APP_ID, "name"]);
        match vdf_get(&tree, &path)? {
            Vdf::S(s) if !s.is_empty() => Some(s.clone()),
            _ => None,
        }
    };
    at(&["InstallConfigStore", "Software", "Valve", "Steam"]).or_else(|| at(&["InstallConfigStore", "Software", "valve", "Steam"]))
}

/// The internal names a Proton folder answers to: the ones its compatibilitytool.vdf
/// declares, or for Valve's builds the name Steam uses ("Proton 9.0" is proton_9, "Proton
/// 5.13" proton_513, "Proton - Experimental" proton_experimental).
pub fn compat_names(dir_name: &str, compat_vdf: Option<&str>) -> Vec<String> {
    let mut names = vec![dir_name.to_string()];
    if let Some(Vdf::M(tools)) = compat_vdf.map(vdf).as_deref().and_then(|t| vdf_get(t, &["compatibilitytools", "compat_tools"])) {
        names.extend(tools.iter().map(|(k, _)| k.clone()));
    }
    let l = dir_name.to_lowercase();
    if l == "proton - experimental" {
        names.push("proton_experimental".into());
    } else if l == "proton hotfix" {
        names.push("proton_hotfix".into());
    } else if let Some(v) = l.strip_prefix("proton ")
        && v.trim().starts_with(|c: char| c.is_ascii_digit())
    {
        let v = v.trim();
        let n = v.strip_suffix(".0").unwrap_or(v).replace('.', "");
        names.push(format!("proton_{n}"));
    }
    names
}

fn wine_in(dir: &Path) -> Option<(PathBuf, PathBuf)> {
    ["files", "dist"]
        .iter()
        .map(|d| dir.join(d))
        .find_map(|d| d.join("bin/wine").is_file().then(|| (d.join("bin/wine"), d.join("lib/wine"))))
}

/// The game's Proton build if it can be found; else the newest Valve Proton; else any
/// compatibility tool; else the system's wine.
pub fn find_wine(paths: &Paths) -> Result<Wine> {
    let steam = crate::steam::steam_root();
    let mapped = fs::read_to_string(steam.join("config/config.vdf")).ok().and_then(|c| game_compat_tool(&c));
    let mut roots =
        vec![steam.join("compatibilitytools.d"), PathBuf::from("/usr/share/steam/compatibilitytools.d"), steam.join("steamapps/common")];
    if let Some(common) = paths.game.parent() {
        roots.push(common.to_path_buf());
    }
    let mut found: Vec<(String, Vec<String>, PathBuf, PathBuf)> = vec![];
    for r in roots {
        for e in fs::read_dir(&r).into_iter().flatten().flatten() {
            let dir = e.path();
            if !dir.join("proton").is_file() {
                continue;
            }
            let Some((wine, lib)) = wine_in(&dir) else { continue };
            let name = e.file_name().to_string_lossy().into_owned();
            let compat = fs::read_to_string(dir.join("compatibilitytool.vdf")).ok();
            found.push((name.clone(), compat_names(&name, compat.as_deref()), wine, lib));
        }
    }
    if let Some(m) = &mapped
        && let Some((name, _, wine, lib)) = found.iter().find(|(_, names, ..)| names.iter().any(|n| n.eq_ignore_ascii_case(m)))
    {
        return Ok(Wine { name: name.clone(), wine: wine.clone(), lib: Some(lib.clone()), games: true });
    }
    let version = |n: &str| -> Option<(u32, u32)> {
        let v = n.strip_prefix("Proton ")?;
        let (a, b) = v.split_once('.').unwrap_or((v, "0"));
        Some((a.trim().parse().ok()?, b.trim().parse().unwrap_or(0)))
    };
    let best = found
        .iter()
        .filter_map(|f| version(&f.0).map(|v| (v, f)))
        .max_by_key(|(v, _)| *v)
        .map(|(_, f)| f)
        .or_else(|| found.iter().find(|f| f.0 == "Proton - Experimental"))
        .or_else(|| found.first());
    if let Some((name, _, wine, lib)) = best {
        return Ok(Wine { name: name.clone(), wine: wine.clone(), lib: Some(lib.clone()), games: false });
    }
    for w in ["/usr/bin/wine", "/usr/local/bin/wine"] {
        if Path::new(w).is_file() {
            return Ok(Wine { name: "system wine".into(), wine: w.into(), lib: None, games: false });
        }
    }
    bail!("no Proton or Wine found (install Proton from Steam, or wine)")
}

fn wine_env(cmd: &mut Command, wine: &Wine, prefix: &Path) {
    cmd.env("WINEPREFIX", prefix).env("WINEDEBUG", "-all").env_remove("LD_PRELOAD");
    if let Some(l) = &wine.lib {
        cmd.env("WINEDLLPATH", l);
    }
}

/// The tools prefix, created on first use, with the game's settings and saves folders
/// linked in (tools read the in-game settings; save editors find the saves).
fn prepare_prefix(paths: &Paths, wine: &Wine, out: &mut dyn FnMut(ToolEvent)) -> Result<PathBuf> {
    let pfx = prefix_dir(paths);
    if !pfx.join("system.reg").is_file() {
        out(ToolEvent::PrefixSetup(pfx.clone()));
        fs::create_dir_all(&pfx)?;
        let mut c = Command::new(&wine.wine);
        c.args(["wineboot", "-i"]);
        wine_env(&mut c, wine, &pfx);
        let o = c.stdin(Stdio::null()).output().with_context(|| format!("running {}", wine.wine.display()))?;
        if !o.status.success() || !pfx.join("system.reg").is_file() {
            bail!("setting up the Wine prefix failed: {}", String::from_utf8_lossy(&o.stderr).lines().last().unwrap_or(""));
        }
    }
    let users = pfx.join("drive_c/users");
    let me = std::env::var("USER").unwrap_or_default();
    let user = [me.as_str(), "steamuser"]
        .iter()
        .map(|u| users.join(u))
        .find(|p| !p.as_os_str().is_empty() && p.is_dir())
        .or_else(|| fs::read_dir(&users).ok()?.flatten().map(|e| e.path()).find(|p| p.is_dir() && !p.ends_with("Public")));
    if let Some(user) = user {
        let game_user = game_prefix(paths).join("drive_c/users/steamuser");
        let saves = crate::safety::shared_saves_dir(paths);
        let links = [
            (user.join("AppData/Local/CD Projekt Red"), game_user.join("AppData/Local/CD Projekt Red")),
            (user.join("Saved Games/CD Projekt Red"), saves.parent().map(Path::to_path_buf).unwrap_or(saves.clone())),
        ];
        for (link, target) in links {
            // a real folder there is the user's: leave it
            if !target.is_dir() || (link.exists() && !link.is_symlink()) {
                continue;
            }
            if fs::read_link(&link).ok().as_deref() == Some(target.as_path()) {
                continue;
            }
            let _ = fs::remove_file(&link);
            fs::create_dir_all(link.parent().unwrap())?;
            std::os::unix::fs::symlink(&target, &link)?;
        }
    }
    Ok(pfx)
}

/* ------------------------------------------------------------------ runs */

/// What a run wrote, by kind.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, TS)]
#[serde(rename_all = "snake_case")]
pub enum OutputKind {
    /// Under `tools/`: reports and the tool's own files
    Report,
    /// A game file: [`save`] makes a mod of these
    GameFile,
    /// modlist.txt and its backups: cpmm writes the load order, so these aren't applied
    LoadOrder,
    /// The tool deleted something; not applied (remove or disable the mod instead)
    Deleted,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, TS)]
pub struct Output {
    /// game-relative
    pub path: String,
    pub size: u64,
    pub kind: OutputKind,
}

/// One run of a tool action (`tool-runs/<id>/run.json`).
#[derive(Serialize, Deserialize, Clone, Debug, Default, TS)]
#[serde(default)]
pub struct ToolRun {
    pub id: String,
    pub tool: String,
    pub tool_name: String,
    pub action: String,
    pub label: String,
    pub wine: String,
    pub started: String,
    pub finished: Option<String>,
    /// None: still running, or killed
    pub exit_code: Option<i32>,
    pub outputs: Vec<Output>,
    /// Archives the tool asked to load first (it put them at the top of modlist.txt)
    pub load_first: Vec<String>,
    /// The mod its game files were saved as
    pub saved_as: Option<String>,
    /// Where its files are (reports open from here)
    pub upper: PathBuf,
    /// It wrote where the game sees them (cpmm's overwrite folder); nothing to review or save
    pub live: bool,
}

impl ToolRun {
    pub fn count(&self, k: OutputKind) -> usize {
        self.outputs.iter().filter(|o| o.kind == k).count()
    }
}

/// What the sandboxed child needs (`tool-runs/<id>/launch.json`).
#[derive(Serialize, Deserialize)]
struct Launch {
    tool_files: PathBuf,
    cwd: PathBuf,
    program: Vec<String>,
    env: Vec<(String, String)>,
    saves: Option<PathBuf>,
}

/// What happens during a run, for the adapter.
pub enum ToolEvent {
    PrefixSetup(PathBuf),
    Starting {
        label: String,
        wine: String,
        game_prefix: bool,
        live: bool,
    },
    /// a line the tool printed (stderr: on its error stream)
    Output {
        line: String,
        stderr: bool,
    },
}

fn kind_of(rel: &str) -> OutputKind {
    let l = rel.to_lowercase();
    if l.starts_with("tools/") {
        OutputKind::Report
    } else if l == "archive/pc/mod/modlist.txt" || l.starts_with("archive/pc/mod/modlist.txt.") {
        OutputKind::LoadOrder
    } else {
        OutputKind::GameFile
    }
}

/// What a run's upper layer holds: its outputs. A whiteout (char device 0:0) is a deletion.
pub fn scan_upper(upper: &Path) -> Vec<Output> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    let mut v = vec![];
    for e in walkdir::WalkDir::new(upper).into_iter().flatten() {
        let ft = e.file_type();
        if ft.is_dir() {
            continue;
        }
        let Ok(rel) = e.path().strip_prefix(upper) else { continue };
        let rel = rel.to_string_lossy().into_owned();
        let md = e.metadata().ok();
        let kind = if ft.is_char_device() && md.as_ref().is_some_and(|m| m.rdev() == 0) { OutputKind::Deleted } else { kind_of(&rel) };
        v.push(Output { size: if kind == OutputKind::Deleted { 0 } else { md.map(|m| m.size()).unwrap_or(0) }, path: rel, kind });
    }
    v.sort_by(|a, b| a.path.cmp(&b.path));
    v
}

/// Archives a tool's modlist.txt lists first that it also wrote: what it wants to win.
pub fn load_first(modlist: &str, outputs: &[Output]) -> Vec<String> {
    let mine: BTreeSet<String> = outputs
        .iter()
        .filter(|o| o.kind == OutputKind::GameFile)
        .filter_map(|o| o.path.to_lowercase().strip_prefix("archive/pc/mod/").map(str::to_string))
        .collect();
    modlist.lines().map(str::trim).filter(|l| !l.is_empty()).take_while(|l| mine.contains(&l.to_lowercase())).map(str::to_string).collect()
}

/// An action's arguments with `{game}` and `{input}` filled in.
pub fn fill_args(args: &[String], game: &Path, input: &str) -> Vec<String> {
    let unix = game.to_string_lossy();
    args.iter().map(|a| a.replace("{game_dir}", &unix).replace("{game}", &windows_path(game)).replace("{input}", input)).collect()
}

fn which(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).map(|d| d.join(name)).find(|f| f.is_file()))
}

/// Run a tool action. `cpmm` is the binary that enters the sandbox (`cpmm tools sandbox`);
/// `extra` arguments go after the action's own. With a terminal on stdin, the tool gets the
/// terminal (it may ask things); otherwise its output is read line by line into `out`.
#[allow(clippy::too_many_arguments)]
pub fn run(
    paths: &Paths,
    st: &crate::state::State,
    tool: &Tool,
    action: &str,
    input: Option<&str>,
    extra: &[String],
    cpmm: &Path,
    out: &mut dyn FnMut(ToolEvent),
) -> Result<ToolRun> {
    let a = tool
        .actions
        .iter()
        .find(|a| a.id == action || a.id.eq_ignore_ascii_case(action))
        .or_else(|| tool.actions.iter().find(|a| a.label.eq_ignore_ascii_case(action)))
        .with_context(|| {
            format!(
                "{} has no action {action:?} (it has: {})",
                tool.meta.name,
                tool.actions.iter().map(|a| a.id.as_str()).collect::<Vec<_>>().join(", ")
            )
        })?
        .clone();
    if a.when == GameState::Closed && crate::vfs::game_running() {
        bail!("{} needs the game closed; quit Cyberpunk first", a.label);
    }
    let input = input.map(str::to_string).or_else(|| a.input.as_ref().map(|i| i.default.clone())).unwrap_or_default();
    let mut args = fill_args(&a.args, &paths.game, &input);
    args.extend(extra.iter().cloned());
    let program_host = paths.game.join(&a.program);
    let mut env: Vec<(String, String)> = vec![];
    let (runner_name, mut program) = match a.runner {
        Runner::Python => {
            let py = which("python3").context("python3 not found (install Python 3.11 or later)")?;
            ("python3".to_string(), vec![py.to_string_lossy().into_owned(), program_host.to_string_lossy().into_owned()])
        }
        Runner::Wine => {
            let wine = find_wine(paths)?;
            let prefix = if a.game_prefix {
                let p = game_prefix(paths);
                if !p.join("system.reg").is_file() {
                    bail!("the game's Wine prefix {} isn't set up (start the game once from Steam)", p.display());
                }
                p
            } else {
                prepare_prefix(paths, &wine, out)?
            };
            env.push(("WINEPREFIX".into(), prefix.to_string_lossy().into_owned()));
            env.push(("WINEDEBUG".into(), "-all".into()));
            if let Some(l) = &wine.lib {
                env.push(("WINEDLLPATH".into(), l.to_string_lossy().into_owned()));
            }
            let mut p = vec![wine.wine.to_string_lossy().into_owned()];
            if a.program.to_lowercase().ends_with(".exe") {
                p.push(program_host.to_string_lossy().into_owned());
            } else {
                p.extend(["cmd".to_string(), "/c".to_string(), windows_path(&program_host)]);
            }
            (wine.name, p)
        }
    };
    program.extend(args);

    let base = format!("{}-{}", tool.meta.id, now().replace([':', '-'], "").replace('T', "-").trim_end_matches('Z'));
    // two runs in the same second each get their own folder
    let (mut id, mut n) = (base.clone(), 2);
    while run_dir(paths, &id).exists() {
        id = format!("{base}-{n}");
        n += 1;
    }
    let dir = run_dir(paths, &id);
    fs::create_dir_all(&dir)?;
    let mut rec = ToolRun {
        id: id.clone(),
        tool: tool.meta.id.clone(),
        tool_name: tool.meta.name.clone(),
        action: a.id.clone(),
        label: a.label.clone(),
        wine: runner_name.clone(),
        started: now(),
        live: a.live,
        upper: if a.live { PathBuf::new() } else { dir.join("upper") },
        ..Default::default()
    };
    fs::write(dir.join("run.json"), serde_json::to_vec_pretty(&rec)?)?;
    // the generated modlist.txt is a layer of the view; a running session's stays as mounted
    if crate::vfs::running(paths).is_none() {
        let _ = crate::loadorder::write_modlist(paths, st);
    }
    out(ToolEvent::Starting {
        label: format!("{}: {}", tool.meta.name, a.label),
        wine: runner_name,
        game_prefix: a.game_prefix,
        live: a.live,
    });

    let mut c = Command::new(cpmm);
    c.env_remove("LD_PRELOAD");
    if a.live {
        // the game's own view: joins a running session, else mounts one (writes go to overwrite/)
        let cwd = if tool.from_mod.is_some() {
            paths.game.clone()
        } else {
            program_host.parent().map(Path::to_path_buf).unwrap_or(paths.game.clone())
        };
        c.arg("exec").arg("--").args(&program).current_dir(&cwd).env("PWD", &cwd);
        for (k, v) in &env {
            c.env(k, v);
        }
    } else {
        let launch = Launch {
            tool_files: tool_files(paths, &tool.meta.id),
            cwd: program_host.parent().map(Path::to_path_buf).unwrap_or(paths.game.clone()),
            program,
            env,
            saves: st.active_own_saves(paths),
        };
        fs::write(dir.join("launch.json"), serde_json::to_vec_pretty(&launch)?)?;
        c.args(["tools", "sandbox"]).arg(&dir);
    }
    let terminal = unsafe { libc::isatty(0) == 1 && libc::isatty(1) == 1 };
    let code = if terminal { c.status().context("starting the tool")?.code() } else { stream(c, &dir.join("log.txt"), out)? };
    rec.finished = Some(now());
    rec.exit_code = code;
    if !a.live {
        rec.outputs = scan_upper(&rec.upper);
        if let Ok(m) = fs::read_to_string(rec.upper.join("archive/pc/mod/modlist.txt")) {
            rec.load_first = load_first(&m, &rec.outputs);
        }
    }
    fs::write(dir.join("run.json"), serde_json::to_vec_pretty(&rec)?)?;
    let _ = fs::remove_dir_all(dir.join("work"));
    prune(paths, 20);
    Ok(rec)
}

/// Run `c` with its output piped, line by line into `out` and `log`. Lines ended by a
/// carriage return (progress counters) come at most every half second.
fn stream(mut c: Command, log: &Path, out: &mut dyn FnMut(ToolEvent)) -> Result<Option<i32>> {
    use std::io::Write;
    use std::sync::mpsc;
    let mut child = c.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().context("starting the tool sandbox")?;
    let (tx, rx) = mpsc::channel::<(String, bool, bool)>();
    let mut readers = vec![];
    for (pipe, stderr) in [
        (child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>), false),
        (child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>), true),
    ] {
        let Some(mut pipe) = pipe else { continue };
        let tx = tx.clone();
        readers.push(std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            let mut split = Lines::default();
            while let Ok(n) = pipe.read(&mut buf) {
                if n == 0 {
                    break;
                }
                for &b in &buf[..n] {
                    if let Some((l, cr)) = split.push(b) {
                        let _ = tx.send((l, stderr, cr));
                    }
                }
            }
            if let Some((l, cr)) = split.finish() {
                let _ = tx.send((l, stderr, cr));
            }
        }));
    }
    drop(tx);
    let mut logf = fs::File::create(log).ok();
    let mut last_cr = std::time::Instant::now() - std::time::Duration::from_secs(1);
    let mut pending: Option<(String, bool)> = None;
    for (line, stderr, cr) in rx {
        if line.is_empty() {
            continue;
        }
        if cr {
            if last_cr.elapsed() < std::time::Duration::from_millis(500) {
                pending = Some((line, stderr));
                continue;
            }
            last_cr = std::time::Instant::now();
        }
        pending = None;
        if let Some(f) = logf.as_mut() {
            let _ = writeln!(f, "{line}");
        }
        out(ToolEvent::Output { line, stderr });
    }
    if let Some((line, stderr)) = pending {
        out(ToolEvent::Output { line, stderr });
    }
    for r in readers {
        let _ = r.join();
    }
    Ok(child.wait()?.code())
}

/// Splits a program's output into lines. Windows programs end lines with CRLF; a lone CR
/// (a progress counter redrawing its line) ends a line that the next one replaces, which
/// `push` marks with `true`.
#[derive(Default)]
struct Lines {
    line: Vec<u8>,
    cr: bool,
}

impl Lines {
    fn take(&mut self, cr: bool) -> Option<(String, bool)> {
        self.cr = false;
        let l = String::from_utf8_lossy(&std::mem::take(&mut self.line)).trim_end().to_string();
        Some((l, cr))
    }
    fn push(&mut self, b: u8) -> Option<(String, bool)> {
        match (b, self.cr) {
            (b'\n', _) => self.take(false),
            (b'\r', true) => self.take(true),
            (b'\r', false) => {
                self.cr = true;
                None
            }
            (b, true) => {
                let out = self.take(true);
                self.line.push(b);
                out
            }
            (b, false) => {
                self.line.push(b);
                None
            }
        }
    }
    fn finish(&mut self) -> Option<(String, bool)> {
        if self.line.is_empty() { None } else { self.take(false) }
    }
}

/// The child side of a run (`cpmm tools sandbox <run dir>`): mount the run's view and become
/// the tool. Called single-threaded, before anything else.
pub fn sandbox(paths: &Paths, dir: &Path) -> Result<()> {
    let l: Launch = serde_json::from_slice(&fs::read(dir.join("launch.json")).context("reading launch.json")?)?;
    let st = crate::state::State::load(paths)?;
    let mut cmd = Command::new(&l.program[0]);
    cmd.args(&l.program[1..]).env_remove("LD_PRELOAD");
    for (k, v) in &l.env {
        cmd.env(k, v);
    }
    crate::vfs::sandbox_exec(paths, &st.mods, &l.tool_files, &dir.join("upper"), &dir.join("work"), l.saves.as_deref(), &l.cwd, &mut cmd)
}

/// Recorded runs, newest first.
pub fn runs(paths: &Paths) -> Vec<ToolRun> {
    let mut v: Vec<ToolRun> = fs::read_dir(runs_dir(paths))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| serde_json::from_slice::<ToolRun>(&fs::read(e.path().join("run.json")).ok()?).ok())
        .collect();
    v.sort_by(|a, b| b.started.cmp(&a.started));
    v
}

/// A run by id, or the newest one for "last".
pub fn find_run(paths: &Paths, key: &str) -> Result<ToolRun> {
    let all = runs(paths);
    if key == "last" {
        return all.into_iter().next().context("no tool runs yet");
    }
    if let Some(r) = all.iter().find(|r| r.id == key) {
        return Ok(r.clone());
    }
    let hits: Vec<ToolRun> = all.into_iter().filter(|r| r.id.contains(key)).collect();
    match hits.len() {
        1 => Ok(hits.into_iter().next().unwrap()),
        0 => bail!("no tool run {key:?} (cpmm tools runs lists them)"),
        _ => bail!("{key:?} matches several runs"),
    }
}

/// Keep the newest `keep` runs; older ones are deleted (they are scratch: a saved run's game
/// files are a mod by then).
fn prune(paths: &Paths, keep: usize) {
    for r in runs(paths).into_iter().skip(keep) {
        let _ = fs::remove_dir_all(run_dir(paths, &r.id));
    }
}

/// Delete a run's files.
pub fn discard(paths: &Paths, r: &ToolRun) -> Result<()> {
    fs::remove_dir_all(run_dir(paths, &r.id)).with_context(|| format!("deleting run {}", r.id))
}

/// Copy a run's game files into a fresh folder, laid out game-relative, to install as a mod.
pub fn export_game_files(paths: &Paths, r: &ToolRun) -> Result<PathBuf> {
    let files: Vec<&Output> = r.outputs.iter().filter(|o| o.kind == OutputKind::GameFile).collect();
    if files.is_empty() {
        bail!("run {} wrote no game files (only {} report file(s))", r.id, r.count(OutputKind::Report));
    }
    let dir = paths.tmp().join(format!("tool-output-{}", r.id));
    let _ = fs::remove_dir_all(&dir);
    for o in files {
        crate::files::reflink_or_copy(&r.upper.join(&o.path), &dir.join(&o.path))?;
    }
    Ok(dir)
}

/// Mark a run as saved as `mod_id`.
pub fn mark_saved(paths: &Paths, r: &ToolRun, mod_id: &str) -> Result<()> {
    let mut r = r.clone();
    r.saved_as = Some(mod_id.to_string());
    fs::write(run_dir(paths, &r.id).join("run.json"), serde_json::to_vec_pretty(&r)?)?;
    Ok(())
}

/// A file a run wrote, for viewing (reports). Only paths inside the run's upper layer.
pub fn output_path(paths: &Paths, r: &ToolRun, rel: &str) -> Result<PathBuf> {
    if rel == "@log" {
        return Ok(run_dir(paths, &r.id).join("log.txt"));
    }
    if rel.split('/').any(|c| c == ".." || c.is_empty()) {
        bail!("bad path {rel:?}");
    }
    let p = r.upper.join(rel);
    if !p.is_file() {
        bail!("run {} has no file {rel}", r.id);
    }
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn a_tools_folder_archive_keeps_its_layout() {
        let f = s(&["tools/VRAM RipperDoc/VramRipperdoc.exe", "tools/VRAM RipperDoc/README.md"]);
        assert!(is_tool(&f));
        let l = layout(&f, "VRAM RipperDoc - Performance Optimizer");
        assert_eq!(l[0].1, "tools/VRAM RipperDoc/VramRipperdoc.exe");
        // behind a wrapper folder too
        let w = s(&["Cyberpunk 2077/tools/X/x.exe", "Cyberpunk 2077/tools/X/y.dll"]);
        assert_eq!(layout(&w, "X")[1].1, "tools/X/y.dll");
    }

    #[test]
    fn a_bare_program_goes_under_tools() {
        let one_dir = s(&["Editor/editor.exe", "Editor/lib.dll"]);
        assert_eq!(layout(&one_dir, "Save Editor")[0].1, "tools/Editor/editor.exe");
        let loose = s(&["editor.exe", "lib.dll"]);
        assert_eq!(layout(&loose, "Save Editor")[1].1, "tools/Save Editor/lib.dll");
    }

    #[test]
    fn mods_with_programs_are_not_tools() {
        // CET, RED4ext and friends ship .exe/.dll under bin/ and red4ext/
        assert!(!is_tool(&s(&["bin/x64/plugins/thing.dll", "bin/x64/helper.exe"])));
        assert!(!is_tool(&s(&["Mod/archive/pc/mod/a.archive", "Mod/setup.exe"])));
        // no program at all
        assert!(!is_tool(&s(&["tools/notes/readme.txt"])));
    }

    #[test]
    fn ripperdoc_gets_its_presets_first() {
        let a = actions(&s(&["tools/VRAM RipperDoc/1 - Scan my mods.bat", "tools/VRAM RipperDoc/VramRipperdoc.exe"]));
        assert_eq!(a[0].id, "scan");
        assert!(a.iter().take_while(|a| a.preset).count() == 4);
        assert!(a.iter().any(|a| !a.preset && a.program.ends_with(".bat")));
        let m = a.iter().find(|a| a.id == "measure").unwrap();
        assert!(m.game_prefix && m.input.is_some());
    }

    #[test]
    fn save_load_optimizers_helper_runs_live_with_python() {
        let a = actions(&s(&["bin/x64/plugins/cyber_engine_tweaks/mods/saveLoadOptimizer/init.lua", "tools/SaveLoadOptimizer/profile.py"]));
        let ids: Vec<&str> = a.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["status", "initialize", "prepare", "activate", "verify", "rollback"]);
        assert!(a.iter().all(|a| a.runner == Runner::Python && a.live && a.program == "tools/SaveLoadOptimizer/profile.py"));
        assert_eq!(a.iter().find(|a| a.id == "activate").unwrap().when, GameState::Closed);
        // a profile.py anywhere else is somebody else's
        assert!(actions(&s(&["tools/Other/profile.py"])).is_empty());
    }

    #[test]
    fn save_load_optimizer_progress_follows_its_files() {
        let d = std::env::temp_dir().join(format!("cpmm-slo-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        let paths = Paths { data: d.join("data"), game: d.join("game"), cfg: Default::default() };
        let (ow, files) = (paths.upper(), d.join("mod"));
        let cet = "bin/x64/plugins/cyber_engine_tweaks/mods/saveLoadOptimizer";
        fs::create_dir_all(files.join(cet)).unwrap();
        fs::create_dir_all(ow.join(cet)).unwrap();
        fs::create_dir_all(ow.join("tools/SaveLoadOptimizer")).unwrap();
        let at = |p: Progress| (p.current, p.active);
        assert_eq!(at(slo_progress_files(&paths, &files)), (0, false));
        fs::write(ow.join(cet).join("build.json"), "{}").unwrap();
        assert_eq!(at(slo_progress_files(&paths, &files)), (1, false));
        let now = crate::safety::now_unix();
        fs::write(ow.join(cet).join("capture.json"), format!(r#"{{"captured_at": {now}}}"#)).unwrap();
        assert_eq!(at(slo_progress_files(&paths, &files)), (2, false));
        let profile = |p: &str| fs::write(ow.join("tools/SaveLoadOptimizer/profile.json"), p).unwrap();
        profile(r#"{"status": "staged", "baseline": 1048576, "candidate": 1310720}"#);
        assert_eq!(at(slo_progress_files(&paths, &files)), (3, false));
        profile(&format!(r#"{{"status": "configured_unverified", "activated_at": {}}}"#, now + 10));
        assert_eq!(at(slo_progress_files(&paths, &files)), (4, true));
        profile(&format!(r#"{{"status": "configured_unverified", "activated_at": {}}}"#, now - 10));
        assert_eq!(at(slo_progress_files(&paths, &files)), (5, true));
        profile(r#"{"status": "configured_readback_verified"}"#);
        assert_eq!(at(slo_progress_files(&paths, &files)), (6, true));
        profile(&format!(r#"{{"status": "rolled_back", "rolled_back_at": {}}}"#, now + 10));
        assert_eq!(at(slo_progress_files(&paths, &files)), (1, false));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn an_ini_setting_is_read_from_its_section() {
        let t = "\u{feff}[Rendering]\nDedicatedDecompressionMemoryBytes = 1\n\n[ResourceLoaderThrottler]\ndedicateddecompressionmemorybytes = 16777216\t\t; ~16MB\n";
        assert_eq!(ini_value(t, "ResourceLoaderThrottler", "DedicatedDecompressionMemoryBytes").as_deref(), Some("16777216"));
        assert_eq!(
            ini_value(
                "; [ResourceLoaderThrottler]\n; DedicatedDecompressionMemoryBytes = 5",
                "ResourceLoaderThrottler",
                "DedicatedDecompressionMemoryBytes"
            ),
            None
        );
    }

    #[test]
    fn a_helper_is_found_in_a_mod_whatever_its_case() {
        let d = std::env::temp_dir().join(format!("cpmm-tools-ci-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(d.join("tools/SaveLoadOptimizer")).unwrap();
        fs::write(d.join("tools/SaveLoadOptimizer/profile.py"), "").unwrap();
        assert_eq!(find_ci(&d, SLO).as_deref(), Some("tools/SaveLoadOptimizer/profile.py"));
        assert_eq!(find_ci(&d, "tools/saveloadoptimizer/missing.py"), None);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn args_get_the_game_folder_as_windows_sees_it() {
        let a = fill_args(&s(&["--game", "{game}", "--label", "{input}"]), Path::new("/home/u/Cyberpunk 2077"), "after");
        assert_eq!(a, s(&["--game", "Z:\\home\\u\\Cyberpunk 2077", "--label", "after"]));
        assert_eq!(
            fill_args(&s(&["--game", "{game_dir}"]), Path::new("/home/u/Cyberpunk 2077"), ""),
            s(&["--game", "/home/u/Cyberpunk 2077"])
        );
    }

    #[test]
    fn outputs_are_sorted_by_kind() {
        assert_eq!(kind_of("tools/VRAM RipperDoc/VRAM-RipperDoc-Report.html"), OutputKind::Report);
        assert_eq!(kind_of("archive/pc/mod/modlist.txt"), OutputKind::LoadOrder);
        assert_eq!(kind_of("archive/pc/mod/modlist.txt.ripperdoc.bak"), OutputKind::LoadOrder);
        assert_eq!(kind_of("archive/pc/mod/! !RipperDoc_Budget.archive"), OutputKind::GameFile);
    }

    #[test]
    fn a_tool_putting_its_archive_first_wants_it_to_win() {
        let outs = vec![Output { path: "archive/pc/mod/! !RipperDoc_Budget.archive".into(), size: 1, kind: OutputKind::GameFile }];
        assert_eq!(load_first("! !RipperDoc_Budget.archive\r\nother.archive\r\n", &outs), s(&["! !RipperDoc_Budget.archive"]));
        assert!(load_first("other.archive\r\n! !RipperDoc_Budget.archive\r\n", &outs).is_empty());
    }

    #[test]
    fn the_games_compat_tool_is_read_from_config_vdf() {
        let c = r#""InstallConfigStore"
{
	"Software"
	{
		"Valve"
		{
			"Steam"
			{
				"CompatToolMapping"
				{
					"0" { "name" "proton_experimental" "config" "" "priority" "75" }
					"1091500"
					{
						"name"		"proton-cachyos-slr"
						"config"		""
					}
				}
			}
		}
	}
}"#;
        assert_eq!(game_compat_tool(c).as_deref(), Some("proton-cachyos-slr"));
        assert_eq!(game_compat_tool("\"InstallConfigStore\" { }"), None);
    }

    #[test]
    fn valve_proton_folders_answer_to_steams_names() {
        assert!(compat_names("Proton 9.0", None).contains(&"proton_9".to_string()));
        assert!(compat_names("Proton 5.13", None).contains(&"proton_513".to_string()));
        assert!(compat_names("Proton - Experimental", None).contains(&"proton_experimental".to_string()));
        let ge = r#""compatibilitytools" { "compat_tools" { "GE-Proton10-1" { "install_path" "." } } }"#;
        assert!(compat_names("GE-Proton10-1-dir", Some(ge)).contains(&"GE-Proton10-1".to_string()));
    }

    #[test]
    fn crlf_ends_a_line_and_a_lone_cr_redraws_it() {
        let mut l = Lines::default();
        let mut got = vec![];
        for b in b"Scanning 1\rScanning 2\r\nGame: X\r\nlast" {
            got.extend(l.push(*b));
        }
        got.extend(l.finish());
        let want = vec![("Scanning 1".to_string(), true), ("Scanning 2".into(), false), ("Game: X".into(), false), ("last".into(), false)];
        assert_eq!(got, want);
    }

    #[test]
    fn files_in_the_upper_layer_are_outputs() {
        let d = std::env::temp_dir().join(format!("cpmm-tools-upper-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(d.join("tools/T")).unwrap();
        fs::write(d.join("tools/T/report.html"), "x").unwrap();
        let o = scan_upper(&d);
        assert_eq!(o, vec![Output { path: "tools/T/report.html".into(), size: 1, kind: OutputKind::Report }]);
        let _ = fs::remove_dir_all(&d);
    }
}
