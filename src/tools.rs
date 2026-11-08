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
//! Saves are the exception: a save editor's writes have to reach them. The save folder the
//! game sees for the active profile is linked into the prefix, an action that can write saves
//! runs after a save snapshot, and every run lists the save files it changed
//! ([`OutputKind::Save`]); restoring the snapshot undoes them.
//!
//! Wine: the game's own Proton build (Steam's compatibility tool for the game), its `wine`
//! binary run directly. `proton run` detaches console programs from the terminal, and
//! Steam's runtime container isn't needed for a tool. Tools get their own Wine prefix
//! (`tool-prefix`) so they can't upset the game's; the game's settings folder and the active
//! profile's saves are linked into it. An action that has to see the running game uses the
//! game's prefix.

use crate::state::{Paths, STEAM_APP_ID, now, slugify};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
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
    /// Arguments; `{game}` is the game folder as Windows sees it, `{input}` the user's answer
    pub args: Vec<String>,
    pub input: Option<ActionInput>,
    /// Runs in the game's own Wine prefix (to see the running game)
    pub game_prefix: bool,
    /// When it can run
    pub when: GameState,
    /// It opens a window and works with the user until they close it (a save editor); else
    /// it runs to the end without questions
    pub interactive: bool,
    /// It can write the saves: the active profile's saves are snapshotted first. A program
    /// cpmm doesn't know counts as one.
    pub saves: bool,
    /// A known action for this tool (shown first); else a program found in its files
    pub preset: bool,
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

/// What a tool is for, from the programs cpmm knows.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq, TS)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    /// Reports what the mods cost and makes lighter textures, saved as a mod
    TextureOptimizer,
    /// Opens the saves for editing
    SaveEditor,
    /// A Windows program cpmm has no presets for
    Unknown,
}

/// An installed tool with what it can do.
#[derive(Serialize, Clone, Debug, TS)]
#[ts(rename = "ToolEntry")]
pub struct Tool {
    #[serde(flatten)]
    pub meta: ToolMeta,
    pub dir: PathBuf,
    pub kind: ToolKind,
    /// What it is for, in a sentence ("" for a tool cpmm doesn't know)
    pub about: String,
    pub actions: Vec<ToolAction>,
}

/// A program cpmm knows (by file name, lower case, so a copy installed from anywhere is
/// recognised).
struct Known {
    program: &'static str,
    kind: ToolKind,
    about: &'static str,
}

const KNOWN: &[Known] = &[
    Known {
        program: "vramripperdoc.exe",
        kind: ToolKind::TextureOptimizer,
        about: "Reports which texture and mesh mods cost the most VRAM, and makes a budget fix: half-size copies of the heaviest textures, \
                which you save as a mod.",
    },
    Known {
        program: "cp2077saveeditor.exe",
        kind: ToolKind::SaveEditor,
        about: "Edits saves: inventory, money, attributes, perks, appearance and quest facts. It opens the active profile's saves; \
                cpmm snapshots them first and lists the save files it changed.",
    },
];

struct Preset {
    /// the program's file name, lower case
    program: &'static str,
    id: &'static str,
    label: &'static str,
    description: &'static str,
    args: &'static [&'static str],
    input: Option<(&'static str, &'static str)>,
    game_prefix: bool,
    when: GameState,
    interactive: bool,
    saves: bool,
}

/// A preset that runs to the end without questions, changing no saves.
const fn unattended(
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
        game_prefix: false,
        when: GameState::Any,
        interactive: false,
        saves: false,
    }
}

/// Actions cpmm knows for particular tools (matched by program, so a copy installed from
/// anywhere gets them). An unattended one runs without questions: a run has no keyboard in
/// Game Mode. An interactive one opens a window for the user.
const PRESETS: &[Preset] = &[
    unattended(
        "vramripperdoc.exe",
        "scan",
        "Scan my mods",
        "Ranks your texture and mesh mods by the VRAM they add and writes a report. Changes nothing.",
        &["--game", "{game}", "--no-open"],
    ),
    unattended(
        "vramripperdoc.exe",
        "budget-fix",
        "Make budget fix",
        "Writes one archive with half-size copies of only the heaviest textures. Save the output as a mod to use it; disable or remove that mod to undo.",
        &["--game", "{game}", "--no-open", "--make-budget-archive"],
    ),
    unattended(
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
        game_prefix: true,
        when: GameState::Any,
        interactive: false,
        saves: false,
    },
    Preset {
        program: "cp2077saveeditor.exe",
        id: "edit",
        label: "Open the save editor",
        description: "Opens the editor's window on the active profile's saves: open a save, change it, save it, then close the window. \
                      Your saves are snapshotted first; restore the snapshot to undo.",
        args: &[],
        input: None,
        game_prefix: false,
        when: GameState::Closed,
        interactive: true,
        saves: true,
    },
];

fn is_program(rel: &str) -> bool {
    let l = rel.to_lowercase();
    l.ends_with(".exe") || l.ends_with(".bat") || l.ends_with(".cmd")
}

fn file_name_lower(rel: &str) -> String {
    rel.rsplit('/').next().unwrap_or(rel).to_lowercase()
}

fn from_preset(p: &Preset, program: &str) -> ToolAction {
    ToolAction {
        id: p.id.into(),
        label: p.label.into(),
        description: p.description.into(),
        program: program.to_string(),
        args: p.args.iter().map(|a| a.to_string()).collect(),
        input: p.input.map(|(label, default)| ActionInput { label: label.into(), default: default.into() }),
        game_prefix: p.game_prefix,
        when: p.when,
        interactive: p.interactive,
        saves: p.saves,
        preset: true,
    }
}

/// A tool's actions: presets for its known programs, then every Windows program in it.
pub fn actions(files: &[String]) -> Vec<ToolAction> {
    let mut out = vec![];
    for f in files {
        let name = file_name_lower(f);
        out.extend(PRESETS.iter().filter(|p| p.program == name).map(|p| from_preset(p, f)));
    }
    for f in files.iter().filter(|f| is_program(f)) {
        let lower = file_name_lower(f);
        if lower.starts_with("unins") || lower.contains("vc_redist") || lower.contains("vcredist") || lower.starts_with("dotnet") {
            continue;
        }
        out.push(ToolAction {
            id: f.clone(),
            label: format!("Run {}", f.rsplit('/').next().unwrap_or(f)),
            description: String::new(),
            program: f.clone(),
            args: vec![],
            input: None,
            game_prefix: false,
            when: GameState::Any,
            interactive: false,
            // cpmm can't tell what an unknown program writes
            saves: true,
            preset: false,
        });
    }
    out
}

/// What the first program cpmm knows among `files` says the tool is for.
fn known(files: &[String]) -> Option<&'static Known> {
    files.iter().find_map(|f| {
        let name = file_name_lower(f);
        KNOWN.iter().find(|k| k.program == name)
    })
}

fn load(paths: &Paths, id: &str) -> Result<Tool> {
    let dir = tool_dir(paths, id);
    let meta: ToolMeta = serde_json::from_slice(&fs::read(dir.join("tool.json")).with_context(|| format!("no tool {id:?}"))?)?;
    let files = crate::files::list_tree(&dir.join("files"));
    let k = known(&files);
    Ok(Tool {
        kind: k.map_or(ToolKind::Unknown, |k| k.kind),
        about: k.map(|k| k.about.to_string()).unwrap_or_default(),
        actions: actions(&files),
        dir,
        meta,
    })
}

/// Installed tools, by name.
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

/// A tool by id, number in the list (1-based), or unique part of its name.
pub fn find(paths: &Paths, key: &str) -> Result<Tool> {
    let all = list(paths);
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

/// The tools prefix, created on first use, with the game's settings and the active profile's
/// saves linked in ([`link_game_folders`]).
fn prepare_prefix(paths: &Paths, st: &crate::state::State, wine: &Wine, out: &mut dyn FnMut(ToolEvent)) -> Result<PathBuf> {
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
        link_game_folders(paths, st, &user)?;
    }
    Ok(pfx)
}

/// Link the game's settings folder and the save folder the game sees for the active profile
/// (its own saves, else the shared folder) into a prefix user's folder, so tools read the
/// in-game settings and save editors open the right saves. Relinks when the profile changes.
fn link_game_folders(paths: &Paths, st: &crate::state::State, user: &Path) -> Result<()> {
    let game_user = game_prefix(paths).join("drive_c/users/steamuser");
    let cdpr = user.join("Saved Games/CD Projekt Red");
    // earlier prefixes linked the whole CD Projekt Red folder to the shared one
    if cdpr.is_symlink() {
        fs::remove_file(&cdpr)?;
    }
    let links = [
        (user.join("AppData/Local/CD Projekt Red"), game_user.join("AppData/Local/CD Projekt Red")),
        (cdpr.join("Cyberpunk 2077"), crate::safety::active_saves_dir(paths, st)),
    ];
    for (link, target) in links {
        if link.is_symlink() {
            if target.is_dir() && fs::read_link(&link).ok().as_deref() == Some(target.as_path()) {
                continue;
            }
            fs::remove_file(&link)?;
        } else if link.exists() {
            // a real folder there is the user's: leave it
            continue;
        }
        if target.is_dir() {
            fs::create_dir_all(link.parent().unwrap())?;
            std::os::unix::fs::symlink(&target, &link)?;
        }
    }
    Ok(())
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
    /// A save file the run changed or added. Saves aren't caught: this is in effect already
    /// (restoring the run's snapshot undoes it)
    Save,
    /// A save file the run removed
    SaveRemoved,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, TS)]
pub struct Output {
    /// game-relative; a save file's is relative to the save folder (`ManualSave-3/sav.dat`)
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
    /// The save folder it saw: the active profile's
    pub saves_dir: Option<PathBuf>,
    /// The save snapshot taken before it ran (an action that can write saves)
    pub snapshot: Option<String>,
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

/// The save snapshot before an action that can write saves.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SavesSnapshot {
    /// Taken, under this name (restoring it undoes the run's save edits)
    Taken(String),
    /// The save folder holds no saves yet
    NoSaves,
    /// The snapshot_saves setting is off
    Off,
}

/// What happens during a run, for the adapter.
pub enum ToolEvent {
    PrefixSetup(PathBuf),
    Snapshot(SavesSnapshot),
    Starting {
        label: String,
        wine: String,
        game_prefix: bool,
        interactive: bool,
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

/// Every file under a save folder: its size and modification time.
fn save_listing(dir: &Path) -> BTreeMap<String, (u64, Option<std::time::SystemTime>)> {
    walkdir::WalkDir::new(dir)
        .into_iter()
        .flatten()
        .filter(|e| e.file_type().is_file())
        .filter_map(|e| {
            let rel = e.path().strip_prefix(dir).ok()?.to_string_lossy().into_owned();
            let md = e.metadata().ok()?;
            Some((rel, (md.len(), md.modified().ok())))
        })
        .collect()
}

/// The save files that differ between two listings of the save folder: changed or added
/// ([`OutputKind::Save`]), or removed.
fn changed_saves(
    before: &BTreeMap<String, (u64, Option<std::time::SystemTime>)>,
    after: &BTreeMap<String, (u64, Option<std::time::SystemTime>)>,
) -> Vec<Output> {
    let mut v: Vec<Output> = after
        .iter()
        .filter(|(rel, now)| before.get(*rel) != Some(*now))
        .map(|(rel, (size, _))| Output { path: rel.clone(), size: *size, kind: OutputKind::Save })
        .collect();
    v.extend(before.keys().filter(|rel| !after.contains_key(*rel)).map(|rel| Output {
        path: rel.clone(),
        size: 0,
        kind: OutputKind::SaveRemoved,
    }));
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
    args.iter().map(|a| a.replace("{game}", &windows_path(game)).replace("{input}", input)).collect()
}

/// Run a tool action with `wine`. `cpmm` is the binary that enters the sandbox (`cpmm tools
/// sandbox`); `extra` arguments go after the action's own. With a terminal on stdin, the tool
/// gets the terminal (it may ask things); otherwise its output is read line by line into `out`.
///
/// Saves aren't caught by the sandbox (a save editor's edits are the point). So an action
/// that can write saves runs after a save snapshot (unless the snapshot_saves setting is
/// off), and every run lists the save files it changed among its outputs.
#[allow(clippy::too_many_arguments)]
pub fn run(
    paths: &Paths,
    st: &crate::state::State,
    wine: &Wine,
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
    let prefix = if a.game_prefix {
        let p = game_prefix(paths);
        if !p.join("system.reg").is_file() {
            bail!("the game's Wine prefix {} isn't set up (start the game once from Steam)", p.display());
        }
        p
    } else {
        prepare_prefix(paths, st, wine, out)?
    };
    let mut env: Vec<(String, String)> = vec![
        ("WINEPREFIX".into(), prefix.to_string_lossy().into_owned()),
        ("WINEDEBUG".into(), "-all".into()),
        // .NET (5 and later) programs look for Windows' ICU, which Wine doesn't have, and stop
        // ("Cannot get symbol u_charsToUChars from libicuuc"); Windows' older NLS works in Wine
        ("DOTNET_SYSTEM_GLOBALIZATION_USENLS".into(), "1".into()),
    ];
    if let Some(l) = &wine.lib {
        env.push(("WINEDLLPATH".into(), l.to_string_lossy().into_owned()));
    }
    let mut program = vec![wine.wine.to_string_lossy().into_owned()];
    if a.program.to_lowercase().ends_with(".exe") {
        program.push(program_host.to_string_lossy().into_owned());
    } else {
        program.extend(["cmd".to_string(), "/c".to_string(), windows_path(&program_host)]);
    }
    program.extend(args);

    let saves_dir = crate::safety::active_saves_dir(paths, st);
    let snapshot = if !a.saves {
        None
    } else if !paths.cfg.snapshot_saves {
        Some(SavesSnapshot::Off)
    } else {
        let reason = format!("before {}: {}", crate::state::short_name(&tool.meta.name), a.label);
        // no snapshot, no run: its edits couldn't be undone
        let name = crate::safety::snapshot_saves(paths, &reason).context("snapshotting your saves before the run")?;
        Some(name.map_or(SavesSnapshot::NoSaves, SavesSnapshot::Taken))
    };
    if let Some(s) = &snapshot {
        out(ToolEvent::Snapshot(s.clone()));
    }
    let saves_before = save_listing(&saves_dir);

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
        wine: wine.name.clone(),
        started: now(),
        upper: dir.join("upper"),
        saves_dir: Some(saves_dir.clone()),
        snapshot: match &snapshot {
            Some(SavesSnapshot::Taken(name)) => Some(name.clone()),
            _ => None,
        },
        ..Default::default()
    };
    fs::write(dir.join("run.json"), serde_json::to_vec_pretty(&rec)?)?;
    // the generated modlist.txt is a layer of the view; a running session's stays as mounted
    if crate::vfs::running(paths).is_none() {
        let _ = crate::loadorder::write_modlist(paths, st);
    }
    out(ToolEvent::Starting {
        label: format!("{}: {}", tool.meta.name, a.label),
        wine: wine.name.clone(),
        game_prefix: a.game_prefix,
        interactive: a.interactive,
    });

    let launch = Launch {
        tool_files: tool_files(paths, &tool.meta.id),
        cwd: program_host.parent().map(Path::to_path_buf).unwrap_or(paths.game.clone()),
        program,
        env,
        // the tools prefix links the active profile's saves; the game's prefix has the shared
        // folder, so the profile's own saves are bound over it, as in the game session
        saves: if a.game_prefix { st.active_own_saves(paths) } else { None },
    };
    fs::write(dir.join("launch.json"), serde_json::to_vec_pretty(&launch)?)?;
    let mut c = Command::new(cpmm);
    c.env_remove("LD_PRELOAD").args(["tools", "sandbox"]).arg(&dir);
    let terminal = unsafe { libc::isatty(0) == 1 && libc::isatty(1) == 1 };
    let code = if terminal { c.status().context("starting the tool")?.code() } else { stream(c, &dir.join("log.txt"), out)? };
    rec.finished = Some(now());
    rec.exit_code = code;
    rec.outputs = scan_upper(&rec.upper);
    if let Ok(m) = fs::read_to_string(rec.upper.join("archive/pc/mod/modlist.txt")) {
        rec.load_first = load_first(&m, &rec.outputs);
    }
    rec.outputs.extend(changed_saves(&saves_before, &save_listing(&saves_dir)));
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
        let f = s(&["tools/VRAM RipperDoc/1 - Scan my mods.bat", "tools/VRAM RipperDoc/VramRipperdoc.exe"]);
        let a = actions(&f);
        assert_eq!(a[0].id, "scan");
        assert!(a.iter().take_while(|a| a.preset).count() == 4);
        assert!(a.iter().any(|a| !a.preset && a.program.ends_with(".bat")));
        let m = a.iter().find(|a| a.id == "measure").unwrap();
        assert!(m.game_prefix && m.input.is_some());
        // it reports and makes a texture mod: no window, no saves
        assert!(a.iter().filter(|a| a.preset).all(|a| !a.interactive && !a.saves));
        assert_eq!(known(&f).map(|k| k.kind), Some(ToolKind::TextureOptimizer));
    }

    #[test]
    fn the_save_editor_opens_a_window_on_the_saves_with_the_game_closed() {
        let f = s(&[
            "tools/Save Editor (Project CyberCAT-SimpleGUI)/CP2077SaveEditor.exe",
            "tools/Save Editor (Project CyberCAT-SimpleGUI)/kraken.dll",
        ]);
        let a = actions(&f);
        let ids: Vec<&str> = a.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["edit", "tools/Save Editor (Project CyberCAT-SimpleGUI)/CP2077SaveEditor.exe"]);
        assert!(a[0].preset && a[0].interactive && a[0].saves && a[0].when == GameState::Closed && a[0].args.is_empty());
        assert_eq!(known(&f).map(|k| k.kind), Some(ToolKind::SaveEditor));
        // a program cpmm doesn't know may write saves too
        let other = actions(&s(&["tools/X/x.exe"]));
        assert!(!other[0].preset && other[0].saves);
        assert_eq!(known(&s(&["tools/X/x.exe"])).map(|k| k.kind), None);
    }

    /// A data folder whose game sits in a Steam library, so the shared save folder (in the
    /// game's compatdata) is inside it too.
    fn fixture(tag: &str) -> Paths {
        let d = std::env::temp_dir().join(format!("cpmm-tools-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        let paths = Paths { data: d.join("data"), game: d.join("steamapps/common/Cyberpunk 2077"), cfg: Default::default() };
        fs::create_dir_all(&paths.game).unwrap();
        paths
    }

    fn put_save(saves: &Path, name: &str, text: &str) {
        fs::create_dir_all(saves.join(name)).unwrap();
        fs::write(saves.join(name).join("sav.dat"), text).unwrap();
    }

    #[test]
    fn the_prefix_links_the_saves_the_game_sees_for_the_active_profile() {
        let paths = fixture("link");
        let shared = crate::safety::shared_saves_dir(&paths);
        put_save(&shared, "ManualSave-1", "x");
        let user = paths.data.join("pfx/drive_c/users/steamuser");
        let link = user.join("Saved Games/CD Projekt Red/Cyberpunk 2077");
        // an earlier prefix linked the whole CD Projekt Red folder
        fs::create_dir_all(user.join("Saved Games")).unwrap();
        std::os::unix::fs::symlink(shared.parent().unwrap(), user.join("Saved Games/CD Projekt Red")).unwrap();

        let mut st = crate::state::State::load(&paths).unwrap();
        link_game_folders(&paths, &st, &user).unwrap();
        assert!(!user.join("Saved Games/CD Projekt Red").is_symlink());
        assert_eq!(fs::read_link(&link).unwrap(), shared);

        // a profile with its own saves: those, not the shared ones
        st.create_profile("Story", None, false).unwrap();
        let own = st.set_own_saves(&paths, "Story", true).unwrap();
        st.switch_profile("Story").unwrap();
        link_game_folders(&paths, &st, &user).unwrap();
        assert_eq!(fs::read_link(&link).unwrap(), own);
        assert!(shared.join("ManualSave-1/sav.dat").is_file(), "relinking leaves the saves alone");

        // back to a shared profile
        st.switch_profile(crate::profiles::DEFAULT).unwrap();
        link_game_folders(&paths, &st, &user).unwrap();
        assert_eq!(fs::read_link(&link).unwrap(), shared);
        let _ = fs::remove_dir_all(paths.data.parent().unwrap());
    }

    #[test]
    fn changed_saves_are_found_by_size_and_time() {
        let paths = fixture("changed");
        let saves = paths.data.join("saves");
        put_save(&saves, "ManualSave-1", "one");
        put_save(&saves, "ManualSave-2", "two");
        put_save(&saves, "AutoSave-0", "auto");
        let before = save_listing(&saves);
        fs::write(saves.join("ManualSave-1/sav.dat"), "edited").unwrap();
        // same size, newer time
        let f = fs::File::options().write(true).open(saves.join("ManualSave-2/sav.dat")).unwrap();
        f.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(5)).unwrap();
        fs::remove_dir_all(saves.join("AutoSave-0")).unwrap();
        put_save(&saves, "ManualSave-3", "new");
        let got: Vec<(String, OutputKind)> = changed_saves(&before, &save_listing(&saves)).into_iter().map(|o| (o.path, o.kind)).collect();
        assert_eq!(
            got,
            vec![
                ("AutoSave-0/sav.dat".to_string(), OutputKind::SaveRemoved),
                ("ManualSave-1/sav.dat".into(), OutputKind::Save),
                ("ManualSave-2/sav.dat".into(), OutputKind::Save),
                ("ManualSave-3/sav.dat".into(), OutputKind::Save),
            ]
        );
        assert!(changed_saves(&before, &before).is_empty());
        let _ = fs::remove_dir_all(paths.data.parent().unwrap());
    }

    #[test]
    fn a_run_that_can_write_saves_snapshots_them_first_and_lists_what_it_changed() {
        use std::os::unix::fs::PermissionsExt;
        let mut paths = fixture("run");
        let d = paths.data.parent().unwrap().to_path_buf();
        let shared = crate::safety::shared_saves_dir(&paths);
        put_save(&shared, "ManualSave-1", "original");
        // a tool cpmm doesn't know, so it may write saves
        fs::create_dir_all(tool_files(&paths, "editor").join("tools/Editor")).unwrap();
        fs::write(tool_files(&paths, "editor").join("tools/Editor/editor.exe"), "").unwrap();
        fs::write(tool_dir(&paths, "editor").join("tool.json"), r#"{"id": "editor", "name": "Editor"}"#).unwrap();
        // a prefix that exists already (no wineboot)
        fs::create_dir_all(prefix_dir(&paths).join("drive_c/users/steamuser")).unwrap();
        fs::write(prefix_dir(&paths).join("system.reg"), "").unwrap();
        // stands in for `cpmm tools sandbox`: notes the snapshots there are, then edits saves
        let cpmm = d.join("cpmm");
        let snaps = paths.data.join("save-snapshots");
        fs::write(
            &cpmm,
            format!(
                "#!/bin/sh\nls '{}' > '{}/seen'\nprintf edited > '{}/ManualSave-1/sav.dat'\nmkdir -p '{}/ManualSave-2' && printf new > '{}/ManualSave-2/sav.dat'\n",
                snaps.display(),
                d.display(),
                shared.display(),
                shared.display(),
                shared.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&cpmm, fs::Permissions::from_mode(0o755)).unwrap();
        let wine = Wine { name: "test wine".into(), wine: "/bin/false".into(), lib: None, games: false };
        let st = crate::state::State::load(&paths).unwrap();
        let tool = find(&paths, "editor").unwrap();

        let mut events = vec![];
        let r = run(&paths, &st, &wine, &tool, "tools/Editor/editor.exe", None, &[], &cpmm, &mut |e| match e {
            ToolEvent::Snapshot(s) => events.push(format!("{s:?}")),
            ToolEvent::Starting { .. } => events.push("starting".into()),
            _ => {}
        })
        .unwrap();
        let name = r.snapshot.clone().expect("a snapshot");
        assert_eq!(events, [format!("Taken({name:?})"), "starting".into()]);
        assert_eq!(fs::read_to_string(d.join("seen")).unwrap().trim(), name, "taken before the tool started");
        assert_eq!(fs::read_to_string(snaps.join(&name).join("ManualSave-1/sav.dat")).unwrap(), "original");
        assert_eq!(r.saves_dir.as_deref(), Some(shared.as_path()));
        let saves: Vec<&str> = r.outputs.iter().filter(|o| o.kind == OutputKind::Save).map(|o| o.path.as_str()).collect();
        assert_eq!(saves, ["ManualSave-1/sav.dat", "ManualSave-2/sav.dat"]);

        // with snapshots turned off, the run says so and takes none
        paths.cfg.snapshot_saves = false;
        events.clear();
        let r = run(&paths, &st, &wine, &tool, "tools/Editor/editor.exe", None, &[], &cpmm, &mut |e| {
            if let ToolEvent::Snapshot(s) = e {
                events.push(format!("{s:?}"))
            }
        })
        .unwrap();
        assert_eq!((r.snapshot, events), (None, vec!["Off".to_string()]));
        assert_eq!(fs::read_dir(&snaps).unwrap().count(), 1);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn args_get_the_game_folder_as_windows_sees_it() {
        let a = fill_args(&s(&["--game", "{game}", "--label", "{input}"]), Path::new("/home/u/Cyberpunk 2077"), "after");
        assert_eq!(a, s(&["--game", "Z:\\home\\u\\Cyberpunk 2077", "--label", "after"]));
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
