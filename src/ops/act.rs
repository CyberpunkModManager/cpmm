//! Actions: every change an adapter can ask for, as one request type, run under the cpmm lock
//! and through the Change module. Progress and step results go to the adapter's [`Ui`] as
//! [`Event`]s; the text the CLI prints is their rendering ([`super::render`]).

use super::{Event, Note, ToolNote, Ui};
use crate::change::{Change, Edit, Idle, Lock};
use crate::fomod::Choices;
use crate::install::{self, OnMissing};
use crate::nexus::{self, Catalogue};
use crate::requirements::{self, Status};
use crate::state::{InstalledCollection, Paths, ReqCheck, State, now, slugify};
use crate::{collections, downloads, files, health, import, loadorder, manage, modio, profiles, safety, settings, steam, tools, vfs};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use ts_rs::TS;

/// Where `Action::Order` puts a mod.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum Place {
    Top,
    Bottom,
    Before(String),
    After(String),
}

/// What to install and how: `cpmm install`'s options. Only `source` is required.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(default)]
pub struct InstallRequest {
    /// A Nexus mod id or page URL, or a local archive or folder (`~/` expands).
    pub source: String,
    /// Nexus file: a file or version id, or part of its name (default: the main file)
    #[ts(optional = nullable)]
    pub file: Option<String>,
    #[ts(optional = nullable)]
    pub id: Option<String>,
    #[ts(optional = nullable)]
    pub name: Option<String>,
    /// Strip this folder from the archive / place files under this game-relative folder
    #[ts(optional = nullable)]
    pub root: Option<String>,
    #[ts(optional = nullable)]
    pub dest: Option<String>,
    #[ts(optional = nullable)]
    pub fomod: Option<Choices>,
    /// Ask the user for FOMOD options the adapter can ask (else the installer's suggestions)
    #[ts(as = "Option<bool>", optional)]
    pub ask: bool,
    #[ts(as = "Option<OnMissing>", optional)]
    pub missing: OnMissing,
    #[ts(as = "Option<bool>", optional)]
    pub disabled: bool,
    /// The Nexus mod a local archive belongs to (enables update checks)
    #[ts(optional = nullable)]
    pub nexus: Option<u64>,
    #[ts(as = "Option<bool>", optional)]
    pub keep_archive: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "do", rename_all = "snake_case")]
pub enum ProfileAction {
    Create {
        name: String,
        from: Option<String>,
        empty: bool,
        own_saves: bool,
        copy_saves: bool,
        switch: bool,
    },
    Switch {
        name: String,
    },
    Rename {
        old: String,
        new: String,
    },
    /// Own save folder (`own`) or the game's shared one; `copy` copies the active saves in.
    Saves {
        name: String,
        own: bool,
        copy: bool,
    },
    Delete {
        name: String,
    },
}

/// Every change to the mod setup and cpmm's data that the CLI or the GUI can ask for
/// (`{"op": "enable", "mods": ["…"]}` from the GUI).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Action {
    /// Create the data directory; optionally record a non-default game directory.
    Init {
        game: Option<PathBuf>,
    },
    Enable {
        mods: Vec<String>,
    },
    Disable {
        mods: Vec<String>,
    },
    /// `place` None fails (after the mod is found), as `cpmm order` without a position does.
    Order {
        r#mod: String,
        place: Option<Place>,
    },
    Install(InstallRequest),
    Update {
        mods: Vec<String>,
        all: bool,
    },
    Rename {
        r#mod: String,
        new_id: String,
    },
    Remove {
        r#mod: String,
        force: bool,
    },
    /// Refresh stored requirements and report unmet ones; `ignore`/`unignore` (mod, Nexus id)
    /// first, `fix` (install missing ones, enable disabled ones) after.
    Deps {
        refresh: bool,
        fix: bool,
        ignore: Option<(String, String)>,
        unignore: Option<(String, String)>,
    },
    Hide {
        r#mod: String,
        paths: Vec<String>,
    },
    Unhide {
        r#mod: String,
        paths: Vec<String>,
    },
    Prefer {
        r#mod: String,
        paths: Vec<String>,
    },
    /// A Nexus "Mod Manager Download" link: into the Downloads list (installed if the
    /// setting says so). `register` makes cpmm the link handler first.
    Nxm {
        url: Option<String>,
        register: bool,
    },
    /// The Nexus Tracked list; `install` the missing ones, `untrack` the installed ones.
    Tracked {
        install: bool,
        untrack: bool,
    },
    RegisterHandler,
    UnregisterHandler,
    Import {
        modlist: PathBuf,
        apply: bool,
    },
    Adopt {
        id: String,
        name: Option<String>,
        paths: Vec<String>,
    },
    OverwriteMove {
        to: String,
        paths: Vec<String>,
    },
    OverwriteClear {
        paths: Vec<String>,
    },
    Eject {
        apply: bool,
    },
    CollectionInstall {
        reference: String,
        optional: bool,
        into_current: bool,
        profile: Option<String>,
    },
    SettingsSet {
        key: String,
        value: String,
    },
    /// One setting, or all (keeping `onboarded`) for None
    SettingsReset {
        key: Option<String>,
    },
    Download {
        source: String,
        file: Option<String>,
    },
    DownloadsInstall {
        key: String,
        fomod: Option<Choices>,
        ask: bool,
        with_deps: bool,
    },
    DownloadsRemove {
        key: String,
    },
    /// Put an archive into the Downloads list; with `auto`, install it right away if the
    /// "install downloads automatically" setting is on.
    DownloadsAdd {
        path: PathBuf,
        nexus: Option<u64>,
        move_it: bool,
        source: String,
        auto: bool,
    },
    Clean {
        dry_run: bool,
    },
    /// Show the sort plan; `apply` saves it.
    Sort {
        apply: bool,
    },
    /// `r#mod` loads after (wins over) `after`; `remove` drops the rules between them.
    Rule {
        r#mod: String,
        after: String,
        remove: bool,
    },
    /// Mark a doctor finding (title or unique part of it) as known, or undo that.
    Ack {
        finding: String,
    },
    Unack {
        finding: String,
    },
    Rollback {
        r#mod: String,
        to: Option<i64>,
    },
    /// Undo an update run (the most recent one for None).
    UndoUpdate {
        batch: Option<String>,
    },
    /// Record the current game logs and report what is new since the previous launch.
    Errors,
    Vanilla {
        on: bool,
    },
    SnapshotSaves,
    RestoreSaves {
        name: String,
    },
    Profile(ProfileAction),
    ModioAck,
    LaunchOptionsApply,
    NexusKeySet {
        key: String,
    },
    NexusKeyRemove,
    /// Is a key set; with `check`, is it valid (asks Nexus)?
    NexusKey {
        check: bool,
    },
    SteamShortcut {
        name: String,
        exe: Option<PathBuf>,
        remove: bool,
        dry_run: bool,
    },
    /// The terminal first-run flow (asks through the adapter; without answers, the defaults).
    Setup,
    /// Install a Windows tool: a Nexus mod id or URL, a Downloads entry, or an archive or folder.
    ToolInstall {
        source: String,
    },
    ToolRemove {
        tool: String,
    },
    /// Run a tool action in its own view of the modded game. `extra` arguments follow the
    /// action's own; `input` answers its question (default: the suggested answer).
    ToolRun {
        tool: String,
        action: String,
        input: Option<String>,
        extra: Vec<String>,
    },
    /// Install the game files a run wrote as a mod (named `name`, default after the action).
    ToolSave {
        run: String,
        name: Option<String>,
    },
    /// Delete a run and what it wrote.
    ToolDiscard {
        run: String,
    },
}

impl Action {
    /// Does this change State or cpmm's data? Those run under the cpmm lock, taken before
    /// State is read; the rest only read (or touch things outside cpmm's data, like the
    /// desktop's link handler or Steam's files). Exhaustive on purpose: a new action has to
    /// choose, and a change made by an action declared read-only fails.
    pub fn changes_state(&self) -> bool {
        match self {
            // a run writes only its own folder (and the tools' Wine prefix), never State
            Action::SteamShortcut { .. } | Action::RegisterHandler | Action::UnregisterHandler | Action::NexusKey { .. } | Action::ToolRun { .. } => false,
            Action::Init { .. }
            | Action::Enable { .. }
            | Action::Disable { .. }
            | Action::Order { .. }
            | Action::Install(_)
            | Action::Update { .. }
            | Action::Rename { .. }
            | Action::Remove { .. }
            | Action::Deps { .. }
            | Action::Hide { .. }
            | Action::Unhide { .. }
            | Action::Prefer { .. }
            | Action::Nxm { .. }
            | Action::Adopt { .. }
            | Action::OverwriteMove { .. }
            | Action::OverwriteClear { .. }
            | Action::CollectionInstall { .. }
            | Action::SettingsSet { .. }
            | Action::SettingsReset { .. }
            | Action::Download { .. }
            | Action::DownloadsInstall { .. }
            | Action::DownloadsRemove { .. }
            | Action::DownloadsAdd { .. }
            | Action::Rule { .. }
            | Action::Ack { .. }
            | Action::Unack { .. }
            | Action::Rollback { .. }
            | Action::UndoUpdate { .. }
            | Action::Errors // records the log baseline
            | Action::Vanilla { .. }
            | Action::SnapshotSaves
            | Action::RestoreSaves { .. }
            | Action::Profile(_)
            | Action::ModioAck
            | Action::LaunchOptionsApply
            | Action::NexusKeySet { .. }
            | Action::NexusKeyRemove
            | Action::Setup
            | Action::ToolInstall { .. }
            | Action::ToolRemove { .. }
            | Action::ToolSave { .. }
            | Action::ToolDiscard { .. } => true,
            Action::Tracked { install, untrack } => *install || *untrack,
            Action::Import { apply, .. } | Action::Eject { apply } | Action::Sort { apply } => *apply,
            Action::Clean { dry_run } => !dry_run,
        }
    }

    /// The `cpmm` command line that does the same (how `cpmm doctor` shows a remedy).
    pub fn argv(&self) -> Vec<String> {
        let mut a: Vec<String> = vec![];
        let mut put = |xs: &[&str]| a.extend(xs.iter().map(|x| x.to_string()));
        let on = |b: bool, flag: &'static str| if b { vec![flag] } else { vec![] };
        let opt =
            |v: &Option<String>, flag: &'static str| -> Vec<String> { v.iter().flat_map(|x| [flag.to_string(), x.clone()]).collect() };
        fn strs(v: &[String]) -> Vec<&str> {
            v.iter().map(String::as_str).collect()
        }
        match self {
            Action::Init { game } => {
                put(&["init"]);
                if let Some(g) = game {
                    put(&["--game", &g.to_string_lossy()]);
                }
            }
            Action::Enable { mods } => put(&[&["enable"], &strs(mods)[..]].concat()),
            Action::Disable { mods } => put(&[&["disable"], &strs(mods)[..]].concat()),
            Action::Order { r#mod, place } => {
                put(&["order", r#mod]);
                match place {
                    Some(Place::Top) => put(&["--top"]),
                    Some(Place::Bottom) => put(&["--bottom"]),
                    Some(Place::Before(b)) => put(&["--before", b]),
                    Some(Place::After(x)) => put(&["--after", x]),
                    None => {}
                }
            }
            Action::Install(r) => {
                put(&["install", &r.source]);
                for (v, flag) in [(&r.file, "--file"), (&r.id, "--id"), (&r.name, "--name"), (&r.root, "--root"), (&r.dest, "--dest")] {
                    put(&opt(v, flag).iter().map(String::as_str).collect::<Vec<_>>());
                }
                match r.missing {
                    OnMissing::Install => put(&["--with-deps"]),
                    OnMissing::Warn => put(&["--ignore-deps"]),
                    OnMissing::Refuse => {}
                }
                put(&on(r.disabled, "--disabled"));
                if let Some(n) = r.nexus {
                    put(&["--nexus", &n.to_string()]);
                }
                put(&on(r.keep_archive, "--keep-archive"));
                if let Some(c) = &r.fomod {
                    put(&["--fomod-choices", &serde_json::to_string(c).unwrap_or_default()]);
                }
                put(&on(!r.ask, "--fomod-defaults"));
            }
            Action::Update { mods, all } => {
                put(&[&["update"], &strs(mods)[..]].concat());
                put(&on(*all, "--all"));
            }
            Action::Rename { r#mod, new_id } => put(&["rename", r#mod, new_id]),
            Action::Remove { r#mod, force } => {
                put(&["remove", r#mod]);
                put(&on(*force, "--force"));
            }
            Action::Deps { refresh, fix, ignore, unignore } => {
                put(&["deps"]);
                put(&on(*refresh, "--refresh"));
                put(&on(*fix, "--fix"));
                for (v, flag) in [(ignore, "--ignore"), (unignore, "--unignore")] {
                    if let Some((m, r)) = v {
                        put(&[flag, m, r]);
                    }
                }
            }
            Action::Hide { r#mod, paths } => put(&[&["hide", r#mod.as_str()], &strs(paths)[..]].concat()),
            Action::Unhide { r#mod, paths } => put(&[&["unhide", r#mod.as_str()], &strs(paths)[..]].concat()),
            Action::Prefer { r#mod, paths } => put(&[&["prefer", r#mod.as_str()], &strs(paths)[..]].concat()),
            Action::Nxm { url, register } => {
                put(&["nxm"]);
                put(&url.iter().map(String::as_str).collect::<Vec<_>>());
                put(&on(*register, "--register"));
            }
            Action::Tracked { install, untrack } => {
                put(&["tracked"]);
                put(&on(*install, "--install"));
                put(&on(*untrack, "--untrack"));
            }
            Action::RegisterHandler => put(&["register-handler"]),
            Action::UnregisterHandler => put(&["register-handler", "--unregister"]),
            Action::Import { modlist, apply } => {
                put(&["import", "--modlist", &modlist.to_string_lossy()]);
                put(&on(*apply, "--apply"));
            }
            Action::Adopt { id, name, paths } => {
                put(&["adopt", id]);
                put(&opt(name, "--name").iter().map(String::as_str).collect::<Vec<_>>());
                put(&strs(paths));
            }
            Action::OverwriteMove { to, paths } => put(&[&["overwrite", "move", to.as_str()], &strs(paths)[..]].concat()),
            Action::OverwriteClear { paths } => put(&[&["overwrite", "clear"], &strs(paths)[..]].concat()),
            Action::Eject { apply } => {
                put(&["eject"]);
                put(&on(*apply, "--apply"));
            }
            Action::CollectionInstall { reference, optional, into_current, profile } => {
                put(&["collection", "install", reference]);
                put(&on(*optional, "--optional"));
                put(&on(*into_current, "--into-current"));
                put(&opt(profile, "--profile").iter().map(String::as_str).collect::<Vec<_>>());
            }
            Action::SettingsSet { key, value } => put(&["settings", "set", key, value]),
            Action::SettingsReset { key } => {
                put(&["settings", "reset"]);
                put(&key.iter().map(String::as_str).collect::<Vec<_>>());
            }
            Action::Download { source, file } => {
                put(&["download", source]);
                put(&opt(file, "--file").iter().map(String::as_str).collect::<Vec<_>>());
            }
            Action::DownloadsInstall { key, fomod, ask, with_deps } => {
                put(&["downloads", "install", key]);
                if let Some(c) = fomod {
                    put(&["--fomod-choices", &serde_json::to_string(c).unwrap_or_default()]);
                }
                put(&on(!ask, "--fomod-defaults"));
                put(&on(*with_deps, "--with-deps"));
            }
            Action::DownloadsRemove { key } => put(&["downloads", "remove", key]),
            Action::DownloadsAdd { path, nexus, move_it, source, auto } => {
                put(&["downloads", "add", &path.to_string_lossy()]);
                if let Some(n) = nexus {
                    put(&["--nexus", &n.to_string()]);
                }
                put(&on(*move_it, "--move"));
                if source != "local" {
                    put(&["--source", source]);
                }
                put(&on(*auto, "--auto"));
            }
            Action::Clean { dry_run } => {
                put(&["clean"]);
                put(&on(*dry_run, "--dry-run"));
            }
            Action::Sort { apply } => {
                put(&["sort"]);
                put(&on(*apply, "--apply"));
            }
            Action::Rule { r#mod, after, remove } => {
                put(&["rule", r#mod, "--after", after]);
                put(&on(*remove, "--remove"));
            }
            Action::Ack { finding } => put(&["doctor", "--ack", finding]),
            Action::Unack { finding } => put(&["doctor", "--unack", finding]),
            Action::Rollback { r#mod, to } => {
                put(&["rollback", r#mod]);
                if let Some(t) = to {
                    put(&["--to", &t.to_string()]);
                }
            }
            Action::UndoUpdate { batch } => {
                put(&["undo-update"]);
                put(&batch.iter().map(String::as_str).collect::<Vec<_>>());
            }
            Action::Errors => put(&["errors"]),
            Action::Vanilla { on: v } => put(&["vanilla", if *v { "on" } else { "off" }]),
            Action::SnapshotSaves => put(&["saves", "--snapshot"]),
            Action::RestoreSaves { name } => put(&["saves", "--restore", name]),
            Action::Profile(p) => match p {
                ProfileAction::Create { name, from, empty, own_saves, copy_saves, switch } => {
                    put(&["profile", "create", name]);
                    put(&opt(from, "--from").iter().map(String::as_str).collect::<Vec<_>>());
                    put(&on(*empty, "--empty"));
                    put(&on(*own_saves, "--own-saves"));
                    put(&on(*copy_saves, "--copy-saves"));
                    put(&on(*switch, "--switch"));
                }
                ProfileAction::Switch { name } => put(&["profile", "switch", name]),
                ProfileAction::Rename { old, new } => put(&["profile", "rename", old, new]),
                ProfileAction::Saves { name, own, copy } => {
                    put(&["profile", "saves", name, if *own { "own" } else { "shared" }]);
                    put(&on(*copy, "--copy"));
                }
                ProfileAction::Delete { name } => put(&["profile", "delete", name]),
            },
            Action::ModioAck => put(&["modio", "--ack"]),
            Action::LaunchOptionsApply => put(&["launch-options", "--apply"]),
            Action::NexusKeySet { key } => put(&["nexus-key", key]),
            Action::NexusKeyRemove => put(&["nexus-key", "--remove"]),
            Action::NexusKey { check } => {
                put(&["nexus-key"]);
                put(&on(*check, "--check"));
            }
            Action::SteamShortcut { name, exe, remove, dry_run } => {
                put(&["steam-shortcut"]);
                if name != "cpmm" {
                    put(&["--name", name]);
                }
                if let Some(e) = exe {
                    put(&["--exe", &e.to_string_lossy()]);
                }
                put(&on(*remove, "--remove"));
                put(&on(*dry_run, "--dry-run"));
            }
            Action::Setup => put(&["setup"]),
            Action::ToolInstall { source } => put(&["tools", "install", source]),
            Action::ToolRemove { tool } => put(&["tools", "remove", tool]),
            Action::ToolRun { tool, action, input, extra } => {
                put(&["tools", "run", tool, action]);
                put(&opt(input, "--input").iter().map(String::as_str).collect::<Vec<_>>());
                if !extra.is_empty() {
                    put(&["--"]);
                    put(&extra.iter().map(String::as_str).collect::<Vec<_>>());
                }
            }
            Action::ToolSave { run, name } => {
                put(&["tools", "save", run]);
                put(&opt(name, "--name").iter().map(String::as_str).collect::<Vec<_>>());
            }
            Action::ToolDiscard { run } => put(&["tools", "discard", run]),
        }
        a
    }
}

/// Run an action: take the cpmm lock if it changes state (saying so if it has to wait), read
/// State, do it. Events go to `ui` as they happen.
pub fn act(paths: &Paths, action: Action, ui: &mut dyn Ui) -> Result<()> {
    let mut ui = super::Warned(ui);
    let _lock = if action.changes_state() {
        Some(Lock::acquire_noting(paths, &mut || ui.event(&Event::Note(Note::WaitingForLock)))?)
    } else {
        None
    };
    let mut st = State::load(paths)?;
    let r = run(paths, &mut st, action, &mut ui);
    ui.flush();
    r
}

/// Import and adopt move files out of the game dir, which only the game process holds.
const GAME_DIR_IDLE: Idle = Idle::NoGame("Cyberpunk is running; quit it first");

fn say(ui: &mut dyn Ui, n: Note) {
    ui.event(&Event::Note(n))
}

fn run(paths: &Paths, st: &mut State, action: Action, ui: &mut dyn Ui) -> Result<()> {
    match action {
        Action::Init { game } => {
            paths.ensure_dirs()?;
            if let Some(g) = game {
                fs::write(paths.data.join("game_dir"), g.to_string_lossy().as_bytes())?;
            }
            let p = Paths::load()?;
            Change::records().apply(&p, st, ui, |_, _| Ok(()))?;
            say(ui, Note::Initialized { data: p.data.clone(), game: p.game.clone() });
            same_fs_check(&p, ui)?;
        }
        Action::Enable { mods } | Action::Disable { mods } if mods.is_empty() => bail!("name at least one mod"),
        Action::Enable { mods } => set_enabled(paths, st, ui, &mods, true)?,
        Action::Disable { mods } => set_enabled(paths, st, ui, &mods, false)?,
        // a manual placement: auto-sort would undo it
        Action::Order { r#mod, place } => Change::setup().idle(Idle::NotNeeded).keep_order().apply(paths, st, ui, |cx, st| {
            let i = st.get(&r#mod)?;
            let m = st.mods.remove(i);
            let pos = match place {
                Some(Place::Top) => st.mods.len(),
                Some(Place::Bottom) => 0,
                Some(Place::Before(b)) => st.get(&b)?,
                Some(Place::After(a)) => st.get(&a)? + 1,
                None => bail!("give --top, --bottom, --before or --after"),
            };
            cx.note(Note::Placed { id: m.id.clone(), priority: pos + 1 });
            cx.journal("order", std::slice::from_ref(&m.id), &format!("priority {}", pos + 1));
            st.mods.insert(pos, m);
            Ok(())
        })?,
        Action::Install(req) => {
            let policy = install::Policy {
                id: req.id,
                name: req.name,
                root: req.root,
                dest: req.dest,
                fomod: req.fomod,
                ask: req.ask,
                missing: req.missing,
                disabled: req.disabled,
                keep_archive: req.keep_archive,
                ..Default::default()
            };
            let source = match nexus::catalogue::mod_id(&req.source) {
                Some(gid) => install::Source::Nexus { mod_id: gid, file: req.file },
                None => install::Source::Local { path: files::expand_home(&req.source), nexus: req.nexus },
            };
            // a new mod doesn't touch a running session; replacing a mounted one is refused by staging
            Change::setup()
                .idle(Idle::NotNeeded)
                .apply(paths, st, ui, |cx, st| install::run(cx, st, &Catalogue::open(cx.paths()), source, &policy))?;
        }
        Action::Update { mods, all } => update(paths, st, ui, &mods, all)?,
        Action::Rename { r#mod, new_id } => Change::setup().keep_order().apply(paths, st, ui, |cx, st| {
            let i = st.get(&r#mod)?;
            let new_id = slugify(&new_id);
            if st.find(&new_id).is_some() {
                bail!("{new_id} already exists");
            }
            let old = st.mods[i].id.clone();
            if paths.mods().join(&old).exists() {
                fs::rename(paths.mods().join(&old), paths.mods().join(&new_id))?;
            }
            for r in st.rules.iter_mut() {
                if r.r#mod == old {
                    r.r#mod = new_id.clone();
                }
                if r.after == old {
                    r.after = new_id.clone();
                }
            }
            st.mods[i].id = new_id.clone();
            st.rename_mod_in_profiles(&old, &new_id);
            cx.journal("rename", std::slice::from_ref(&new_id), &format!("from {old}"));
            cx.note(Note::Renamed { old, new: new_id });
            Ok(())
        })?,
        Action::Remove { r#mod, force } => {
            let i = st.get(&r#mod)?;
            let id = st.mods[i].id.clone();
            let users: Vec<&str> = requirements::dependents(st, &st.mods[i]).iter().map(|m| m.id.as_str()).collect();
            if !users.is_empty() && !force {
                bail!("{id} is required by {}; --force to remove anyway", users.join(", "));
            }
            // staged files are in use only through a session's overlay
            Change::setup()
                .idle(Idle::NoSession("game session running; quit first (its overlay uses these files)"))
                .snapshot(format!("before uninstalling {id}"))
                .keep_order()
                .apply(paths, st, ui, |cx, st| {
                    let elsewhere = st.profiles_enabling(&id);
                    if !elsewhere.is_empty() {
                        cx.note(Note::AlsoInProfiles(elsewhere));
                    }
                    cx.journal("uninstall", std::slice::from_ref(&id), &st.mods[i].version);
                    let d = cx.paths().mods().join(&id);
                    if d.exists() {
                        let t = cx.paths().trash_for(&id);
                        fs::rename(&d, &t)?;
                        cx.note(Note::Trashed(t));
                    }
                    st.mods.remove(i);
                    Ok(())
                })?;
            say(ui, Note::Removed(id));
        }
        Action::Deps { refresh, fix, ignore, unignore } => {
            if ignore.is_some() || unignore.is_some() {
                Change::records().apply(paths, st, ui, |cx, st| {
                    for (v, on) in [(ignore, true), (unignore, false)] {
                        if let Some((key, req)) = v {
                            let i = st.get(&key)?;
                            let rid: u64 = req.parse().context("requirement must be a Nexus mod id")?;
                            let m = &mut st.mods[i];
                            m.ignored_reqs.retain(|x| *x != rid);
                            if on {
                                m.ignored_reqs.push(rid);
                            }
                            let id = m.id.clone();
                            cx.note(Note::Ignoring { id: id.clone(), req: rid, on });
                            cx.journal(if on { "ignore" } else { "unignore" }, std::slice::from_ref(&id), &format!("requirement {rid}"));
                            cx.save(st)?;
                        }
                    }
                    Ok(())
                })?;
            }
            Change::records().apply(paths, st, ui, |cx, st| deps(cx, st, refresh))?;
            if fix {
                deps_fix(paths, st, ui)?;
            }
        }
        Action::Hide { r#mod, paths: sel } => Change::setup().keep_order().apply(paths, st, ui, |cx, st| {
            let i = st.get(&r#mod)?;
            let d = manage::hide(cx.paths(), st, i, &sel)?;
            cx.journal("hide", std::slice::from_ref(&st.mods[i].id), &sel.join(", "));
            cx.note(Note::Hid { id: st.mods[i].id.clone(), files: d.len() });
            Ok(())
        })?,
        Action::Unhide { r#mod, paths: sel } => Change::setup().keep_order().apply(paths, st, ui, |cx, st| {
            let i = st.get(&r#mod)?;
            let d = manage::unhide(cx.paths(), st, i, &sel)?;
            cx.journal("unhide", std::slice::from_ref(&st.mods[i].id), &sel.join(", "));
            cx.note(Note::Unhid { id: st.mods[i].id.clone(), files: d.len() });
            Ok(())
        })?,
        Action::Prefer { r#mod, paths: sel } => Change::setup().keep_order().apply(paths, st, ui, |cx, st| {
            let i = st.get(&r#mod)?;
            let d = manage::prefer(cx.paths(), st, i, &sel)?;
            cx.journal("prefer", std::slice::from_ref(&st.mods[i].id), &sel.join(", "));
            cx.note(Note::Preferred { id: st.mods[i].id.clone(), hid: d });
            Ok(())
        })?,
        Action::Tracked { install: true, untrack } => {
            Change::setup().idle(Idle::NotNeeded).apply(paths, st, ui, |cx, st| tracked(cx.paths(), st, Install::Into(cx), untrack))?
        }
        Action::Tracked { install: false, untrack } => tracked(paths, st, Install::No(ui), untrack)?,
        Action::RegisterHandler => {
            let before = manage::current_nxm_handler();
            let exe = ui.this_exe()?;
            let desktop = manage::register_nxm(&exe)?;
            let replaced = Some(before).filter(|b| !b.is_empty() && b != "cpmm-nxm.desktop");
            say(ui, Note::HandlerSet { exe, desktop, replaced, log: paths.data.join("nxm.log") });
        }
        Action::UnregisterHandler => {
            let before = manage::current_nxm_handler();
            manage::unregister_nxm()?;
            say(ui, Note::HandlerRemoved { before, now: manage::current_nxm_handler() });
        }
        Action::Nxm { url, register } => {
            if register {
                let exe = ui.this_exe()?;
                let desktop = manage::register_nxm(&exe)?;
                say(ui, Note::Registered { exe, desktop });
            }
            match url {
                Some(u) => nxm(paths, st, ui, &u)?,
                None if !register => bail!("give an nxm:// URL or --register"),
                None => {}
            }
        }
        Action::Import { modlist, apply } => {
            same_fs_check(paths, ui)?;
            let modlist = files::expand_home(&modlist.to_string_lossy());
            if !apply {
                import::import(paths, st, &modlist, false, &mut |p| say(ui, Note::Import(p)))?;
            } else {
                Change::setup()
                    .idle(GAME_DIR_IDLE)
                    .keep_order()
                    .apply(paths, st, ui, |cx, st| import::import(cx.paths(), st, &modlist, true, &mut |p| cx.note(Note::Import(p))))?;
                say(ui, Note::FetchingRequirements);
                Change::records().apply(paths, st, ui, |cx, st| deps(cx, st, true))?;
            }
        }
        Action::Adopt { id, name, paths: rels } => {
            let id = slugify(&id);
            let got = Change::setup().idle(GAME_DIR_IDLE).keep_order().apply(paths, st, ui, |cx, st| {
                let got = import::adopt(cx.paths(), st, &id, name.as_deref().unwrap_or(&id), &rels)?;
                cx.journal("adopt", std::slice::from_ref(&id), &format!("{} files", got.len()));
                Ok(got)
            })?;
            say(ui, Note::Adopted { id, files: got.len() });
        }
        Action::OverwriteMove { to, paths: sel } => {
            let (id, moved, skipped) = Change::setup().keep_order().apply(paths, st, ui, |cx, st| {
                let r = manage::overwrite_move(cx.paths(), st, &to, &sel)?;
                if r.1 > 0 {
                    cx.journal("overwrite", std::slice::from_ref(&r.0), &format!("{} game-written files moved in", r.1));
                }
                Ok(r)
            })?;
            say(ui, Note::OverwriteMoved { id, moved, skipped });
        }
        Action::OverwriteClear { paths: sel } => {
            let (n, t) = manage::overwrite_clear(paths, &sel)?;
            say(ui, Note::OverwriteCleared { entries: n, trash: t });
        }
        Action::Eject { apply } => {
            let (mods, overwrite) = manage::eject(paths, st, apply)?;
            say(ui, Note::Ejected { mods, overwrite, game: paths.game.clone(), applied: apply });
        }
        Action::CollectionInstall { reference, optional, into_current, profile } => {
            collection_install(paths, st, ui, &reference, optional, into_current, profile)?
        }
        Action::SettingsSet { key, value } => {
            let new = settings::set(&paths.cfg, &key, &value)?;
            save_settings(paths, ui, new)?;
        }
        Action::SettingsReset { key: None } => {
            save_settings(paths, ui, settings::Settings { onboarded: paths.cfg.onboarded, ..Default::default() })?
        }
        Action::SettingsReset { key: Some(k) } => {
            let d = serde_json::to_value(settings::Settings::default())?;
            let v = d.get(&k).with_context(|| format!("unknown setting {k:?}"))?;
            let new = settings::set(&paths.cfg, &k, v.as_str().map(String::from).unwrap_or(v.to_string()).as_str())?;
            save_settings(paths, ui, new)?;
        }
        Action::Download { source, file } => {
            let gid = nexus::catalogue::mod_id(&source).context("give a Nexus mod id or URL")?;
            let p = fetch_nexus(paths, &Catalogue::open(paths), ui, gid, file.as_deref(), None, "nexus")?;
            say(ui, Note::DownloadedTo(p));
        }
        Action::DownloadsInstall { key, fomod, ask, with_deps } => {
            let d = downloads::find(paths, st, &key)?;
            let policy =
                install::Policy { fomod, ask, missing: if with_deps { OnMissing::Install } else { OnMissing::Warn }, ..Default::default() };
            install_download(paths, st, ui, &d, &policy)?;
        }
        Action::DownloadsRemove { key } => {
            let d = downloads::find(paths, st, &key)?;
            downloads::remove(&d)?;
            say(ui, Note::DownloadDeleted { file: d.file, installed_as: d.installed_as });
        }
        Action::DownloadsAdd { path, nexus: nid, move_it, source, auto } => {
            let p = downloads::add(paths, &files::expand_home(&path.to_string_lossy()), nid, &source, move_it)?;
            let file = p.strip_prefix(paths.downloads()).unwrap_or(&p).to_string_lossy().to_string();
            say(ui, Note::DownloadAdded(file.clone()));
            if auto && paths.cfg.auto_install_downloads {
                let d = downloads::find(paths, st, &file)?;
                let policy = install::Policy { missing: OnMissing::Install, ..Default::default() };
                install_download(paths, st, ui, &d, &policy)?;
            }
        }
        Action::Clean { dry_run } => {
            let freed = manage::clean(paths, dry_run)?;
            say(ui, Note::Cleaned { bytes: freed, dry_run });
        }
        Action::Sort { apply } => {
            let p = loadorder::plan(paths, st);
            say(ui, Note::SortPlan(p.clone()));
            if apply && !p.moved.is_empty() {
                // the edit is the sort
                Change::setup().idle(Idle::NotNeeded).keep_order().apply(paths, st, ui, |cx, st| {
                    cx.journal("sort", &p.moved.iter().map(|m| m.id.clone()).collect::<Vec<_>>(), "");
                    loadorder::apply(st, &p.order);
                    Ok(())
                })?;
                say(ui, Note::Sorted { archives: loadorder::modlist(paths, st).len() });
            } else if !p.moved.is_empty() {
                say(ui, Note::SortDryRun);
            }
        }
        Action::Rule { r#mod, after, remove } => {
            // the edit sorts: a rule only matters once the order reflects it
            Change::setup().idle(Idle::NotNeeded).keep_order().apply(paths, st, ui, |cx, st| {
                let a = st.mods[st.get(&r#mod)?].id.clone();
                let b = st.mods[st.get(&after)?].id.clone();
                if a == b {
                    bail!("a mod can't load after itself");
                }
                st.rules.retain(|r| !(r.r#mod == a && r.after == b) && !(r.r#mod == b && r.after == a));
                if !remove {
                    st.rules.push(loadorder::Rule { r#mod: a.clone(), after: b.clone() });
                    cx.journal("rule", &[a.clone(), b.clone()], "wins over");
                } else {
                    cx.journal("rule", &[a.clone(), b.clone()], "removed");
                }
                cx.note(Note::Ruled { a, b, removed: remove });
                let p = loadorder::plan(cx.paths(), st);
                for mv in &p.moved {
                    cx.note(Note::Moved(mv.clone()));
                }
                loadorder::apply(st, &p.order);
                Ok(())
            })?;
            say(ui, Note::RulesApplied { archives: loadorder::modlist(paths, st).len() });
        }
        Action::Ack { finding } => ack(paths, st, ui, &finding, true)?,
        Action::Unack { finding } => ack(paths, st, ui, &finding, false)?,
        Action::Rollback { r#mod, to } => {
            let msg = Change::setup()
                .snapshot(format!("before rollback of {}", r#mod))
                .apply(paths, st, ui, |cx, st| safety::rollback(cx, st, &r#mod, to))?;
            say(ui, Note::RolledBack(msg));
        }
        Action::UndoUpdate { batch } => {
            let b = match batch {
                Some(b) => b,
                None => safety::batches(paths, st).into_iter().next().map(|b| b.batch).context("no update run to undo")?,
            };
            Change::setup().snapshot(format!("before undoing {b}")).apply(paths, st, ui, |cx, st| {
                for m in safety::undo_batch(cx, st, &b)? {
                    cx.note(Note::RolledBackMod(m));
                }
                Ok(())
            })?;
            say(ui, Note::Undid(b));
        }
        Action::Errors => say(ui, Note::Errors(safety::error_diff(paths))),
        Action::Vanilla { on } => {
            let f = safety::vanilla_flag(paths);
            if on {
                fs::write(&f, "1")?;
            } else {
                let _ = fs::remove_file(&f);
            }
            say(ui, Note::Vanilla(on));
        }
        Action::SnapshotSaves => say(ui, Note::Snapshot(safety::snapshot_saves(paths, "manual")?)),
        Action::RestoreSaves { name } => {
            // saves, not the mod setup: only the game process holds them
            Idle::NoGame("quit the game before restoring saves").check(paths)?;
            let n = safety::restore_snapshot(paths, &name)?;
            say(ui, Note::Restored { files: n, from: name });
        }
        Action::Profile(a) => profile(paths, st, ui, a)?,
        Action::ModioAck => say(ui, Note::ModioReviewed(modio::ack(paths)?)),
        Action::LaunchOptionsApply => {
            let exe = installed_exe(ui)?;
            let new = steam::apply_launch_options(&exe)?;
            say(ui, Note::LaunchOptionsSet { options: new, live: steam::is_running() });
        }
        Action::NexusKeySet { key } => {
            let (name, premium) = save_key(paths, &key)?;
            say(ui, Note::KeySaved { name, premium });
        }
        Action::NexusKey { check } => say(ui, Note::Key(super::query::nexus_key(paths, check)?)),
        Action::NexusKeyRemove => {
            let _ = fs::remove_file(nexus::key_file());
            say(ui, Note::KeyRemoved);
        }
        Action::SteamShortcut { name, exe, remove, dry_run } => {
            steam::run(steam::Opts { name, exe, remove, dry_run }, &mut |s| say(ui, Note::Shortcut(s)))?
        }
        Action::Setup => setup(paths, ui)?,
        Action::ToolInstall { source } => tool_install(paths, st, ui, &source)?,
        Action::ToolRemove { tool } => {
            let t = tools::find(paths, &tool)?;
            let trash = tools::remove(paths, &t)?;
            say(ui, Note::Tool(ToolNote::Removed { id: t.meta.id, trash }));
        }
        Action::ToolRun { tool, action, input, extra } => {
            let t = tools::find(paths, &tool)?;
            let wine = tools::find_wine(paths)?;
            let cpmm = ui.this_exe()?;
            // the tool's last words: usually why it stopped
            let mut last = String::new();
            let r = tools::run(paths, st, &wine, &t, &action, input.as_deref(), &extra, &cpmm, &mut |e| {
                let n = match e {
                    tools::ToolEvent::PrefixSetup(p) => ToolNote::PrefixSetup(p),
                    tools::ToolEvent::Snapshot(s) => ToolNote::Snapshot(s),
                    tools::ToolEvent::Starting { label, wine, game_prefix, interactive } => {
                        ToolNote::Starting { label, wine, game_prefix, interactive }
                    }
                    tools::ToolEvent::Output { line, stderr } => {
                        if !line.trim().is_empty() && line.trim() != "ntsync: up and running." {
                            last = line.trim().to_string();
                        }
                        ToolNote::Output { line, stderr }
                    }
                };
                say(ui, Note::Tool(n))
            })?;
            let (code, label) = (r.exit_code, r.label.clone());
            say(ui, Note::Tool(ToolNote::Finished(Box::new(r))));
            let kept = "; what it wrote is kept (cpmm tools runs)";
            match code {
                Some(0) => {}
                Some(_) if !last.is_empty() => bail!("{label}: {last}"),
                Some(c) => bail!("{label}: the tool exited with code {c}{kept}"),
                None => bail!("{label}: the tool was killed{kept}"),
            }
        }
        Action::ToolSave { run, name } => tool_save(paths, st, ui, &run, name)?,
        Action::ToolDiscard { run } => {
            let r = tools::find_run(paths, &run)?;
            tools::discard(paths, &r)?;
            say(ui, Note::Tool(ToolNote::Discarded(r.id)));
        }
    }
    Ok(())
}

/// `cpmm tools sandbox <run dir>`: the child side of a tool run.
pub fn tool_sandbox(paths: &Paths, dir: &Path) -> Result<()> {
    tools::sandbox(paths, dir)
}

/// What a Downloads entry says about the tool in it.
fn tool_meta(d: &downloads::Download) -> tools::ToolMeta {
    tools::ToolMeta {
        name: if d.meta.name.is_empty() { nexus::catalogue::download_name(&d.file) } else { d.meta.name.clone() },
        version: d.meta.version.clone(),
        mod_id: d.meta.mod_id,
        file_id: d.meta.file_id.clone(),
        archive: d.file.clone(),
        ..Default::default()
    }
}

fn install_tool(paths: &Paths, ui: &mut dyn Ui, source: &Path, meta: tools::ToolMeta) -> Result<()> {
    let (meta, replaced) = tools::install(paths, source, meta)?;
    let t = tools::find(paths, &meta.id)?;
    say(
        ui,
        Note::Tool(ToolNote::Installed {
            id: meta.id,
            name: meta.name,
            version: meta.version,
            actions: t.actions.iter().map(|a| a.id.clone()).collect(),
            replaced,
        }),
    );
    Ok(())
}

/// Install a tool from a Nexus mod (into Downloads first), a Downloads entry, or a local
/// archive or folder.
fn tool_install(paths: &Paths, st: &State, ui: &mut dyn Ui, source: &str) -> Result<()> {
    let local = files::expand_home(source);
    if local.exists() {
        let name = local.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "tool".into());
        return install_tool(paths, ui, &local, tools::ToolMeta { name: nexus::catalogue::download_name(&name), ..Default::default() });
    }
    let d = match nexus::catalogue::mod_id(source) {
        // a number or a mod page: from Nexus, through the Downloads list like any download
        Some(gid) => {
            let arc = fetch_nexus(paths, &Catalogue::open(paths), ui, gid, None, None, "nexus")?;
            downloads::find(paths, st, &arc.file_name().unwrap_or_default().to_string_lossy())?
        }
        None => downloads::find(paths, st, source)?,
    };
    install_tool(paths, ui, &d.path, tool_meta(&d))
}

/// A Downloads entry that is a Windows tool goes under Tools, not into the mod list. True if
/// it was one.
fn tool_download(paths: &Paths, ui: &mut dyn Ui, d: &downloads::Download) -> Result<bool> {
    if !tools::archive_files(&d.path).is_ok_and(|f| tools::is_tool(&f)) {
        return Ok(false);
    }
    say(ui, Note::Tool(ToolNote::IsTool(d.meta.name.clone())));
    install_tool(paths, ui, &d.path, tool_meta(d))?;
    Ok(true)
}

/// Install what a run wrote to the game folder as a mod. If the tool put its archives first
/// in modlist.txt, it wants them to win: the new mod gets top priority and a rule over each
/// mod whose archives it overlaps.
fn tool_save(paths: &Paths, st: &mut State, ui: &mut dyn Ui, run: &str, name: Option<String>) -> Result<()> {
    let r = tools::find_run(paths, run)?;
    let dir = tools::export_game_files(paths, &r)?;
    let name = name.unwrap_or_else(|| format!("{} ({})", crate::state::short_name(&r.tool_name), r.label));
    let policy = install::Policy { id: Some(slugify(&name)), name: Some(name), missing: OnMissing::Warn, ..Default::default() };
    let res = Change::setup().idle(Idle::NotNeeded).apply(paths, st, ui, |cx, st| {
        let rep = install::run(cx, st, &Catalogue::open(cx.paths()), install::Source::Local { path: dir.clone(), nexus: None }, &policy)?;
        let id = rep.requested().id.clone();
        let mut rules = 0;
        if !r.load_first.is_empty() {
            if let Some(i) = st.find(&id) {
                let m = st.mods.remove(i);
                st.mods.push(m);
            }
            let ids: Vec<String> = st.mods.iter().map(|m| m.id.clone()).collect();
            for p in loadorder::archive_conflicts(cx.paths(), st, &ids) {
                let other = match (&p.a, &p.b) {
                    (loadorder::Owner::Mod(a), loadorder::Owner::Mod(b)) if *a == id => b.clone(),
                    (loadorder::Owner::Mod(a), loadorder::Owner::Mod(b)) if *b == id => a.clone(),
                    _ => continue,
                };
                let rule = loadorder::Rule { r#mod: id.clone(), after: other };
                if !st.rules.contains(&rule) {
                    st.rules.push(rule);
                    rules += 1;
                }
            }
        }
        cx.journal("tool-save", std::slice::from_ref(&id), &r.id);
        Ok((id, rules))
    });
    let _ = fs::remove_dir_all(&dir);
    let (id, rules) = res?;
    tools::mark_saved(paths, &r, &id)?;
    say(ui, Note::Tool(ToolNote::Saved { run: r.id, id, rules }));
    Ok(())
}

fn same_fs_check(paths: &Paths, ui: &mut dyn Ui) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    fs::create_dir_all(&paths.data)?;
    let a = fs::metadata(&paths.data)?.dev();
    let b = fs::metadata(&paths.game).with_context(|| format!("game dir {}", paths.game.display()))?.dev();
    if a != b {
        say(ui, Note::DifferentFilesystems);
    }
    Ok(())
}

/// The cpmm binary Steam should run (the installed copy, not a cargo target dir).
pub fn installed_exe(ui: &dyn Ui) -> Result<PathBuf> {
    let local = files::expand_home("~/.local/bin/cpmm");
    if local.is_file() {
        return Ok(local);
    }
    ui.this_exe()
}

fn set_enabled(paths: &Paths, st: &mut State, ui: &mut dyn Ui, keys: &[String], on: bool) -> Result<()> {
    let mut change = Change::setup().idle(Idle::NotNeeded);
    if !on {
        change = change.snapshot(format!("before disabling {}", keys.join(", ")));
    }
    change.apply(paths, st, ui, |cx, st| {
        cx.journal(if on { "enable" } else { "disable" }, keys, "");
        for k in keys {
            let i = st.get(k)?;
            st.mods[i].enabled = on;
            cx.note(Note::Toggled { id: st.mods[i].id.clone(), on });
            if !on {
                let users: Vec<String> = requirements::dependents(st, &st.mods[i]).iter().map(|m| m.id.clone()).collect();
                if !users.is_empty() {
                    cx.note(Note::StillRequired(users));
                }
            } else {
                for c in requirements::of(st, &st.mods[i]) {
                    match (c.status, c.dep) {
                        (Status::Missing, _) => cx.note(Note::RequiresMissing { mod_id: c.req.mod_id, name: c.req.name.clone() }),
                        (Status::Disabled, Some(d)) => cx.note(Note::RequiresDisabled(d.id.clone())),
                        _ => {}
                    }
                }
            }
        }
        Ok(())
    })
}

fn update(paths: &Paths, st: &mut State, ui: &mut dyn Ui, keys: &[String], all: bool) -> Result<()> {
    let wanted: Vec<String> = if all {
        super::query::check_updates(paths, st, false).into_iter().filter(|u| u.newer.is_some()).map(|u| u.id).collect()
    } else {
        keys.iter().map(|k| st.get(k).map(|i| st.mods[i].id.clone())).collect::<Result<_>>()?
    };
    if wanted.is_empty() {
        say(ui, Note::NothingToUpdate);
        return Ok(());
    }
    // staged files are in use only through a session's overlay
    Change::setup()
        .idle(Idle::NoSession("game session running; quit first"))
        .snapshot("before updating mods")
        .batch(format!("update-{}", safety::now_unix()))
        .apply(paths, st, ui, |cx, st| update_edit(cx, st, wanted))
}

fn update_edit(cx: &mut Edit, st: &mut State, wanted: Vec<String>) -> Result<()> {
    let batch = cx.batch().unwrap_or_default();
    cx.journal("update", &wanted, batch);
    let nx = Catalogue::open(cx.paths());
    for id in wanted {
        let m = &st.mods[st.get(&id)?];
        let Some(n) = m.nexus.clone() else {
            cx.note(Note::NotFromNexus(id));
            continue;
        };
        if n.file_id.is_empty() {
            cx.note(Note::FileUnknown { id, mod_id: n.mod_id });
            continue;
        }
        let newest = nx.gate().versions(&n.file_id)?.into_iter().next().context("no versions")?;
        let source = install::Source::Nexus { mod_id: n.mod_id, file: Some(newest.game_scoped_id.clone()) };
        let policy = install::Policy { id: Some(id.clone()), missing: OnMissing::Warn, hooks: false, ..Default::default() };
        if let Err(e) = install::run(cx, st, &nx, source, &policy) {
            cx.note(Note::UpdateFailed { id, error: format!("{e:#}") });
        }
    }
    Ok(())
}

/// Refresh the stored requirements (inside a records change) and report unmet ones.
fn deps(cx: &mut Edit, st: &mut State, refresh: bool) -> Result<()> {
    let paths = cx.paths();
    let requirements::Stale { mods: stale, migrated } = requirements::stale(st, refresh);
    let total = st.mods.iter().filter(|m| m.nexus.is_some()).count();
    if !stale.is_empty() {
        let nx = Catalogue::open(paths);
        // one batched GraphQL request for all mod pages; per-mod lookups below hit its cache
        let ids: Vec<u64> = stale.iter().filter_map(|&i| st.mods[i].nexus.as_ref().map(|n| n.mod_id)).collect();
        nx.gate().requirements(&ids, refresh)?;
        for &i in &stale {
            let n = st.mods[i].nexus.clone().unwrap();
            match nx.requirements(n.mod_id, &n.version_id, refresh) {
                Ok((_, stored)) => {
                    st.mods[i].requires = stored;
                    st.mods[i].reqs_checked = Some(ReqCheck { at: now(), version_id: n.version_id.clone() });
                }
                Err(e) => cx.note(Note::ReqCheckFailed { id: st.mods[i].id.clone(), error: format!("{e:#}") }),
            }
        }
        cx.save(st)?;
        cx.note(Note::ReqsChecked { checked: stale.len(), cached: total - stale.len() });
    } else {
        if migrated {
            cx.save(st)?;
        }
        cx.note(Note::ReqsCached { total });
    }
    cx.note(Note::Unmet(super::query::unmet(st)));
    Ok(())
}

fn deps_fix(paths: &Paths, st: &mut State, ui: &mut dyn Ui) -> Result<()> {
    let mut missing = BTreeSet::new();
    let mut to_enable = BTreeSet::new();
    for m in st.mods.iter().filter(|m| m.enabled) {
        for c in requirements::of(st, m) {
            match (c.status, c.dep) {
                (Status::Missing, _) => {
                    missing.insert(c.req.mod_id);
                }
                (Status::Disabled, Some(d)) => {
                    to_enable.insert(d.id.clone());
                }
                _ => {}
            }
        }
    }
    if !to_enable.is_empty() {
        let ids: Vec<String> = to_enable.into_iter().collect();
        Change::setup().idle(Idle::NotNeeded).apply(paths, st, ui, |cx, st| {
            cx.journal("enable", &ids, "");
            for id in &ids {
                let i = st.get(id)?;
                st.mods[i].enabled = true;
                cx.note(Note::Toggled { id: id.clone(), on: true });
            }
            Ok(())
        })?;
    }
    if missing.is_empty() {
        return Ok(());
    }
    Change::setup().apply(paths, st, ui, |cx, st| {
        let nx = Catalogue::open(cx.paths());
        let policy = install::Policy { missing: OnMissing::Install, ..Default::default() };
        for gid in missing {
            // pulled in as a requirement of one installed earlier in this loop
            if st.by_nexus(gid).is_some() {
                continue;
            }
            if let Err(e) = install::run(cx, st, &nx, install::Source::Nexus { mod_id: gid, file: None }, &policy) {
                cx.note(Note::FixFailed { mod_id: gid, error: format!("{e:#}") });
            }
        }
        Ok(())
    })
}

/// Where `tracked` installs: inside the caller's change, or not at all (just the list).
enum Install<'c, 'e, 'u> {
    Into(&'c mut Edit<'e>),
    No(&'u mut dyn Ui),
}

impl Install<'_, '_, '_> {
    fn note(&mut self, n: Note) {
        match self {
            Install::Into(cx) => cx.note(n),
            Install::No(ui) => say(*ui, n),
        }
    }
}

/// Mods on the Nexus "Tracked" list; inside a change, install the missing ones.
fn tracked(paths: &Paths, st: &mut State, mut install: Install, untrack: bool) -> Result<()> {
    let nx = Catalogue::open(paths);
    let ids = nx.gate().tracked()?;
    if ids.is_empty() {
        install.note(Note::NoTracked);
    }
    let mut done = vec![];
    for gid in ids {
        let name = nx.gate().mod_info(gid).map(|i| i.name).unwrap_or_default();
        if let Some(m) = st.by_nexus(gid) {
            let n = Note::Tracked { mod_id: gid, name, installed_as: Some(m.id.clone()) };
            install.note(n);
            done.push(gid);
            continue;
        }
        let Install::Into(cx) = &mut install else {
            install.note(Note::Tracked { mod_id: gid, name, installed_as: None });
            continue;
        };
        let source = install::Source::Nexus { mod_id: gid, file: None };
        let policy = install::Policy { missing: OnMissing::Install, ..Default::default() };
        match install::run(cx, st, &nx, source, &policy) {
            Ok(_) => done.push(gid),
            Err(e) => cx.note(Note::TrackedFailed { mod_id: gid, name, error: format!("{e:#}") }),
        }
    }
    if untrack {
        for gid in done {
            nx.gate().untrack(gid)?;
            install.note(Note::Untracked(gid));
        }
    }
    Ok(())
}

/// A Nexus "Mod Manager Download": into the Downloads list; installed only if the user asked
/// for that (Settings → Install downloads automatically). Desktop notifications say how it went.
fn nxm(paths: &Paths, st: &mut State, ui: &mut dyn Ui, url: &str) -> Result<()> {
    let l = manage::parse_nxm(url)?;
    let nx = Catalogue::open(paths);
    let arc = match fetch_nexus(paths, &nx, ui, l.mod_id, Some(&l.file_gsid), l.key.as_deref().zip(l.expires.as_deref()), "nxm") {
        Ok(p) => p,
        Err(e) => {
            manage::notify("Download failed", &format!("{e:#}"));
            return Err(e);
        }
    };
    let file = arc.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let d = downloads::find(paths, st, &file)?;
    if paths.cfg.auto_install_downloads && tool_download(paths, ui, &d).unwrap_or(false) {
        manage::notify("Tool installed", &format!("{}\nRun it from cpmm's Tools page.", d.meta.name));
    } else if paths.cfg.auto_install_downloads {
        let policy = install::Policy { missing: OnMissing::Install, ..Default::default() };
        // a new mod doesn't touch a running session; replacing a mounted one is refused by staging
        let r = Change::setup()
            .idle(Idle::NotNeeded)
            .apply(paths, st, ui, |cx, st| install::run(cx, st, &nx, install::Source::Download(&d), &policy));
        match r {
            Ok(_) => {
                let name = st.by_nexus(l.mod_id).map(|m| format!("{} {}", m.name, m.version)).unwrap_or_default();
                manage::notify("Mod installed", &format!("{name}\n(applies on next launch)"));
            }
            Err(e) => {
                manage::notify("Mod install failed", &format!("{e:#}"));
                return Err(e);
            }
        }
    } else {
        say(ui, Note::Downloaded { name: d.meta.name.clone(), version: d.meta.version.clone(), file });
        manage::notify("Mod downloaded", &format!("{} {}\nInstall it from cpmm's Downloads list.", d.meta.name, d.meta.version));
    }
    Ok(())
}

fn install_download(paths: &Paths, st: &mut State, ui: &mut dyn Ui, d: &downloads::Download, policy: &install::Policy) -> Result<()> {
    if tool_download(paths, ui, d)? {
        return Ok(());
    }
    // a new mod doesn't touch a running session; replacing a mounted one is refused by staging
    Change::setup()
        .idle(Idle::NotNeeded)
        .apply(paths, st, ui, |cx, st| install::run(cx, st, &Catalogue::open(cx.paths()), install::Source::Download(d), policy))?;
    Ok(())
}

/// Download one Nexus file into the Downloads list (no install).
pub(super) fn fetch_nexus(
    paths: &Paths,
    nx: &Catalogue,
    ui: &mut dyn Ui,
    gid: u64,
    file: Option<&str>,
    nxm: Option<(&str, &str)>,
    source: &str,
) -> Result<PathBuf> {
    let (info, ver) = nx.pick(gid, file)?;
    let man = nx.manifest(&ver)?;
    paths.ensure_dirs()?;
    if !nx.archive_path(&ver).exists() {
        say(ui, Note::Fetching { name: info.name.clone(), version: ver.version.clone(), bytes: man.archive.size });
    }
    let arc = nx.fetch_archive_with(&ver, &man, nxm, &mut |t| ui.event(&Event::Install(&install::Event::Transfer(t))))?;
    downloads::record_nexus(&arc, &info.name, gid, &ver, source);
    Ok(arc)
}

fn ack(paths: &Paths, st: &mut State, ui: &mut dyn Ui, k: &str, on: bool) -> Result<()> {
    let lk = k.to_lowercase();
    let titles: Vec<String> = if on {
        health::check(paths, st).into_iter().map(|f| f.title).filter(|t| t.to_lowercase().contains(&lk)).collect()
    } else {
        st.acked.iter().filter(|t| t.to_lowercase().contains(&lk)).cloned().collect()
    };
    if titles.len() != 1 {
        bail!("{:?} matches {} findings; be more specific", k, titles.len());
    }
    Change::records().apply(paths, st, ui, |_, st| {
        st.acked.retain(|t| *t != titles[0]);
        if on {
            st.acked.push(titles[0].clone());
        }
        Ok(())
    })?;
    say(ui, Note::Acked { title: titles[0].clone(), on });
    Ok(())
}

/// Write new settings; a new staging folder takes the installed mods with it.
fn save_settings(paths: &Paths, ui: &mut dyn Ui, new: settings::Settings) -> Result<()> {
    let mut np = paths.clone();
    np.cfg = new.clone();
    let (old_dir, new_dir) = (paths.mods(), np.mods());
    if old_dir != new_dir && old_dir.exists() {
        Idle::Required.check(paths)?; // the staged mods are mounted while the game runs
        if new_dir.starts_with(&old_dir) {
            bail!("the new staging folder can't be inside the current one");
        }
        fs::create_dir_all(&new_dir)?;
        let mut moved = 0;
        for e in fs::read_dir(&old_dir)?.flatten() {
            let to = new_dir.join(e.file_name());
            if to.exists() {
                bail!("{} already exists", to.display());
            }
            if fs::rename(e.path(), &to).is_err() {
                for f in files::list_tree(&e.path()) {
                    files::reflink_or_copy(&e.path().join(&f), &to.join(&f))?;
                }
                fs::remove_dir_all(e.path())?;
            }
            moved += 1;
        }
        let _ = fs::remove_dir(&old_dir);
        say(ui, Note::StagingMoved { mods: moved, to: new_dir.clone() });
        use std::os::unix::fs::MetadataExt;
        if fs::metadata(&new_dir).map(|m| m.dev()).ok() != fs::metadata(&paths.game).map(|m| m.dev()).ok() {
            say(ui, Note::StagingOtherDrive);
        }
    }
    settings::save(&paths.data, &new)?;
    say(ui, Note::SettingsSaved);
    Ok(())
}

/// Validate a Nexus API key and save it; returns the account name and whether it's premium.
fn save_key(paths: &Paths, key: &str) -> Result<(String, bool)> {
    let k = key.trim().to_string();
    let (name, premium) = Catalogue::with_key(paths, Some(k.clone())).gate().validate().context("validating the key")?;
    let f = nexus::key_file();
    fs::create_dir_all(f.parent().unwrap())?;
    fs::write(&f, &k)?;
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&f, fs::Permissions::from_mode(0o600))?;
    Ok((name, premium))
}

fn yes(ui: &mut dyn Ui, q: &str, default: bool) -> bool {
    let a = ask(ui, &format!("{q} (y/n)"), if default { "y" } else { "n" });
    a.to_lowercase().starts_with('y')
}

fn ask(ui: &mut dyn Ui, q: &str, default: &str) -> String {
    ui.ask(q, default).filter(|a| !a.is_empty()).unwrap_or_else(|| default.to_string())
}

/// Interactive first-run setup for terminal users.
fn setup(paths: &Paths, ui: &mut dyn Ui) -> Result<()> {
    say(ui, Note::Setup(super::SetupStep::Start));
    // 1. game
    let mut game = paths.game.clone();
    if !game.join("bin/x64/Cyberpunk2077.exe").is_file() {
        let g = ask(ui, "Where is Cyberpunk 2077 installed?", &game.to_string_lossy());
        game = files::expand_home(&g);
        if !game.join("bin/x64/Cyberpunk2077.exe").is_file() {
            bail!("no bin/x64/Cyberpunk2077.exe under {}", game.display());
        }
        fs::create_dir_all(&paths.data)?;
        fs::write(paths.data.join("game_dir"), game.to_string_lossy().as_bytes())?;
    }
    say(ui, Note::Setup(super::SetupStep::Game(game)));
    // 2. Nexus key
    if paths.api_key().is_err() {
        say(ui, Note::Setup(super::SetupStep::KeyNeeded));
        let k = ask(ui, "Paste it (or leave empty to skip):", "");
        if !k.is_empty() {
            let (name, premium) = save_key(paths, &k)?;
            say(ui, Note::KeySaved { name, premium });
        }
    } else {
        say(ui, Note::Setup(super::SetupStep::KeySet));
    }
    // 3. launch options
    let exe = installed_exe(ui)?;
    let lo = steam::read_launch_options();
    if !lo.as_deref().is_some_and(steam::launch_options_ok)
        && yes(ui, "\nMake Steam launch the game through cpmm (keeps your existing launch options)?", true)
    {
        let r = steam::apply_launch_options(&exe).map_err(|e| format!("{e:#}"));
        say(ui, Note::Setup(super::SetupStep::LaunchOptions(r)));
    }
    // 4. link handler + preferences
    if manage::current_nxm_handler() != "cpmm-nxm.desktop" && yes(ui, "Open Nexus \"Mod Manager Download\" links with cpmm?", true) {
        manage::register_nxm(&exe)?;
    }
    let mut cfg = paths.cfg.clone();
    cfg.check_updates_on_start = yes(ui, "Check for mod updates when cpmm starts?", cfg.check_updates_on_start);
    cfg.auto_install_updates = yes(ui, "Install updates automatically (undoable)?", cfg.auto_install_updates);
    cfg.auto_sort = yes(ui, "Keep the load order sorted automatically?", cfg.auto_sort);
    cfg.snapshot_saves = yes(ui, "Snapshot your saves before risky changes?", cfg.snapshot_saves);
    cfg.onboarded = true;
    settings::save(&paths.data, &cfg)?;
    say(ui, Note::Setup(super::SetupStep::Done));
    Ok(())
}

fn summary(st: &State, p: &profiles::Profile) -> String {
    format!("{} of {} mods enabled", p.enabled.len(), st.mods.len())
}

/// Profile actions; a setup change when the active list moves, else a records change.
fn profile(paths: &Paths, st: &mut State, ui: &mut dyn Ui, action: ProfileAction) -> Result<()> {
    let active = |st: &State, name: &str| st.profile_index(name).is_ok_and(|i| st.profiles[i].name == st.active_profile);
    let change = match &action {
        ProfileAction::Switch { name } if active(st, name) => {
            say(ui, Note::Profile(super::ProfileNote::AlreadyActive(st.active_profile.clone())));
            return Ok(());
        }
        // a different list for the next launch; the profile keeps its own order
        ProfileAction::Switch { .. } | ProfileAction::Create { switch: true, .. } => Change::setup().keep_order(),
        // the running game has the active profile's save folder in use
        ProfileAction::Saves { name, .. } if active(st, name) => Change::records().idle(Idle::Required),
        _ => Change::records(),
    };
    change.apply(paths, st, ui, |cx, st| profile_edit(cx, st, action))
}

fn profile_edit(cx: &mut Edit, st: &mut State, action: ProfileAction) -> Result<()> {
    use super::ProfileNote as P;
    let paths = cx.paths();
    match action {
        ProfileAction::Create { name, from, empty, own_saves, copy_saves, switch } => {
            let name = st.create_profile(&name, from.as_deref(), empty)?;
            let from = if empty { None } else { Some(from.unwrap_or(st.active_profile.clone())) };
            cx.note(Note::Profile(P::Created { name: name.clone(), from }));
            if own_saves || paths.cfg.new_profiles_own_saves {
                set_profile_saves(cx, st, &name, true, copy_saves)?;
            }
            if switch {
                let from = st.active_profile.clone();
                st.switch_profile(&name)?;
                cx.journal("profile", std::slice::from_ref(&name), &format!("from {from}"));
                cx.note(Note::Profile(P::SwitchedTo(name)));
            }
        }
        ProfileAction::Switch { name } => {
            let from = st.active_profile.clone();
            let new = st.switch_profile(&name)?;
            let p = st.profiles[st.profile_index(&st.active_profile)?].clone();
            cx.note(Note::Profile(P::Switched { from: from.clone(), to: p.name.clone(), summary: summary(st, &p), new }));
            cx.journal("profile", std::slice::from_ref(&p.name), &format!("from {from}"));
        }
        ProfileAction::Rename { old, new } => {
            let (old, new) = st.rename_profile(&old, &new)?;
            cx.note(Note::Profile(P::Renamed { old, new }));
        }
        ProfileAction::Saves { name, own, copy } => set_profile_saves(cx, st, &name, own, copy)?,
        ProfileAction::Delete { name } => {
            let (name, saves) = st.delete_profile(paths, &name)?;
            cx.note(Note::Profile(P::Deleted(name.clone())));
            if let Some(d) = saves {
                let t = paths.trash_for(&format!("saves-{}", slugify(&name)));
                fs::create_dir_all(t.parent().unwrap())?;
                fs::rename(&d, &t)?;
                cx.note(Note::Profile(P::SavesTrashed(t)));
            }
        }
    }
    Ok(())
}

fn set_profile_saves(cx: &mut Edit, st: &mut State, name: &str, own: bool, copy: bool) -> Result<()> {
    use super::ProfileNote as P;
    let paths = cx.paths();
    let i = st.profile_index(name)?;
    let name = st.profiles[i].name.clone();
    let source = safety::saves_dir(paths); // the active profile's saves, before any change
    let dir = st.set_own_saves(paths, &name, own)?;
    if !own {
        let kept = profiles::count_saves(&dir);
        cx.note(Note::Profile(P::SharedSaves { name, kept, dir }));
        return Ok(());
    }
    let have = profiles::count_saves(&dir);
    if copy {
        if have > 0 {
            bail!("{} already holds {have} saves; not copying over them", dir.display());
        }
        let mut n = 0;
        for f in files::list_tree(&source) {
            files::reflink_or_copy(&source.join(&f), &dir.join(&f))?;
            n += 1;
        }
        cx.note(Note::Profile(P::SavesCopied { saves: profiles::count_saves(&dir), files: n, from: source }));
    }
    let saves = profiles::count_saves(&dir);
    cx.note(Note::Profile(P::OwnSaves { name, dir, saves }));
    Ok(())
}

/// Install a collection: exact files in phase order with the curator's FOMOD choices, enable
/// them in the target profile, add the curator's load-order rules, then sort.
fn collection_install(
    paths: &Paths,
    st: &mut State,
    ui: &mut dyn Ui,
    reference: &str,
    optional: bool,
    into_current: bool,
    profile: Option<String>,
) -> Result<()> {
    use super::CollectionNote as C;
    let nx = Catalogue::open(paths);
    let (slug, rv, man, dir) = nx.collection(reference)?;
    let name = rv.pointer("/collection/name").and_then(|v| v.as_str()).unwrap_or(&man.name).to_string();
    let rev = rv["revisionNumber"].as_u64().unwrap_or(0);
    say(ui, Note::Collection(C::Start { name: name.clone(), revision: rev, author: man.author.clone(), mods: man.mods.len() }));
    if !into_current && paths.cfg.profiles_enabled {
        let pname = profile.unwrap_or_else(|| name.clone());
        let pname = profiles::valid_name(&pname).unwrap_or_else(|_| format!("Collection {slug}"));
        // a change of its own, so the install's save snapshot is of this profile's saves
        Change::setup().keep_order().apply(paths, st, ui, |cx, st| {
            if st.profile_index(&pname).is_err() {
                st.create_profile(&pname, None, true)?;
                cx.note(Note::Collection(C::ProfileCreated(pname.clone())));
            }
            let from = st.active_profile.clone();
            st.switch_profile(&pname)?;
            cx.journal("profile", std::slice::from_ref(&pname), &format!("from {from}"));
            Ok(())
        })?;
        say(ui, Note::Collection(C::Into(pname)));
    }
    let done = Change::setup()
        .snapshot(format!("before installing collection {name}"))
        .batch(format!("collection-{slug}-{rev}"))
        .always_sort() // the curator's rules only matter once the order reflects them
        .apply(paths, st, ui, |cx, st| collection_edit(cx, st, &nx, &slug, &name, rev, &man, &dir, optional))?;
    say(ui, Note::Collection(C::Done { name, optional, instructions: man.instructions.clone(), done }));
    Ok(())
}

/// What a collection install did.
#[derive(Clone, Debug, Default)]
pub struct CollectionDone {
    pub installed: usize,
    /// Optional mods left out
    pub skipped: usize,
    /// Load-order rules added
    pub rules: usize,
    pub failed: Vec<String>,
    /// Steps left to the user
    pub manual: Vec<String>,
}

/// The collection's mods in phase order, then enabled in the active profile with the
/// curator's rules. Runs inside the collection's change (its batch tags replaced versions).
#[allow(clippy::too_many_arguments)]
fn collection_edit(
    cx: &mut Edit,
    st: &mut State,
    nx: &Catalogue,
    slug: &str,
    name: &str,
    rev: u64,
    man: &collections::Manifest,
    dir: &Path,
    optional: bool,
) -> Result<CollectionDone> {
    use super::CollectionNote as C;
    let mut mods = man.mods.clone();
    mods.sort_by_key(|m| m.phase);
    let (mut ok_ids, mut failed, mut manual, mut skipped) = (BTreeMap::<u64, Vec<String>>::new(), vec![], vec![], 0);
    let total = mods.iter().filter(|m| optional || !m.optional).count();
    let mut n = 0;
    for m in &mods {
        if m.optional && !optional {
            skipped += 1;
            continue;
        }
        n += 1;
        cx.note(Note::Collection(C::Step { n, total, name: m.name.clone(), version: m.version.clone() }));
        let policy = install::Policy { fomod: m.choices.clone(), missing: OnMissing::Warn, hooks: false, ..Default::default() };
        let r: Result<Option<String>> = match m.source.as_str() {
            "nexus" if m.mod_id > 0 => {
                let source = install::Source::Nexus { mod_id: m.mod_id, file: Some(m.file_id.clone()) };
                install::run(cx, st, nx, source, &policy).map(|r| Some(r.requested().id.clone()))
            }
            "bundle" => {
                let p = dir.join("bundled").join(&m.bundle);
                if p.exists() {
                    let source = install::Source::Local { path: p, nexus: None };
                    let policy = install::Policy { name: Some(m.name.clone()), ..policy };
                    install::run(cx, st, nx, source, &policy).map(|r| Some(r.requested().id.clone()))
                } else {
                    Err(anyhow::anyhow!("bundled file {} is missing from the collection", m.bundle))
                }
            }
            _ => {
                manual.push(format!("{} ({}): {}", m.name, m.source, m.instructions.lines().next().unwrap_or("install it by hand")));
                Ok(None)
            }
        };
        match r {
            Ok(Some(id)) => {
                ok_ids.entry(m.mod_id).or_default().push(id);
                if m.has_patches {
                    manual.push(format!(
                        "{}: the curator patches some of its files with Vortex-only binary patches, which cpmm doesn't apply",
                        m.name
                    ));
                }
            }
            Ok(None) => {}
            Err(e) => {
                cx.note(Note::Collection(C::Failed(format!("{e:#}"))));
                failed.push(format!("{}: {e:#}", m.name));
            }
        }
    }
    // every collection mod is on in this profile, including ones installed before
    let ids: BTreeSet<String> = ok_ids.values().flatten().cloned().collect();
    for m in st.mods.iter_mut().filter(|m| ids.contains(&m.id)) {
        m.enabled = true;
    }
    // the curator's load order
    let mut added = 0;
    for r in &man.rules {
        let (Some(src), Some(refm)) = (ok_ids.get(&r.source.0).and_then(|v| v.first()), ok_ids.get(&r.reference.0).and_then(|v| v.first()))
        else {
            continue;
        };
        let (a, b) = if r.kind == "after" { (src.clone(), refm.clone()) } else { (refm.clone(), src.clone()) };
        if a != b && !st.rules.iter().any(|x| x.r#mod == a && x.after == b) {
            st.rules.retain(|x| !(x.r#mod == b && x.after == a));
            st.rules.push(loadorder::Rule { r#mod: a, after: b });
            added += 1;
        }
    }
    st.collections.retain(|c| c.slug != slug);
    st.collections.push(InstalledCollection {
        slug: slug.to_string(),
        name: name.to_string(),
        revision: rev,
        profile: st.active_profile.clone(),
        mods: ids.iter().cloned().collect(),
        installed_at: now(),
    });
    cx.journal("collection", &[slug.to_string()], &format!("revision {rev}, {} mods, {added} load-order rules", ids.len()));
    Ok(CollectionDone { installed: ids.len(), skipped, rules: added, failed, manual })
}

/// `cpmm run -- %command%`: the Steam launch wrapper. Outside a session: ingest the previous
/// session's logs, record this one, honour a one-time vanilla launch, refresh modlist.txt;
/// then mount and run the command (this process becomes it).
pub fn launch(paths: &Paths, cmd: &[String], ui: &mut dyn Ui) -> Result<()> {
    let st = State::load(paths)?;
    if std::env::var_os("CPMM_NS").is_none() {
        let _ = safety::ingest_errors(paths); // logs of the previous session
        let vf = safety::vanilla_flag(paths);
        if vf.exists() {
            let _ = fs::remove_file(&vf);
            safety::record_session(paths, &st, true);
            say(ui, Note::Launch(super::LaunchNote::Vanilla));
            if let Some(s) = st.active_own_saves(paths) {
                // still the profile's own saves
                return vfs::run(paths, None, Some(&s), cmd, &mut |_| say(ui, Note::Launch(super::LaunchNote::Saves(s.clone()))));
            }
            use std::os::unix::process::CommandExt;
            let err = std::process::Command::new(&cmd[0]).args(&cmd[1..]).exec();
            bail!("exec {}: {err}", cmd[0]);
        }
        safety::record_session(paths, &st, false);
        if let Err(e) = loadorder::write_modlist(paths, &st) {
            say(ui, Note::Launch(super::LaunchNote::ModlistFailed(format!("{e:#}"))));
        }
    }
    let saves = st.active_own_saves(paths);
    vfs::run(paths, Some(&st.mods), saves.as_deref(), cmd, &mut |layers| {
        if let Some(n) = layers {
            say(ui, Note::Launch(super::LaunchNote::Mounted(n)));
        }
        if let Some(s) = &saves {
            say(ui, Note::Launch(super::LaunchNote::Saves(s.clone())));
        }
    })
}

/// `cpmm exec -- cmd`: a command seeing the modded game dir (joins a running session).
pub fn exec(paths: &Paths, cmd: &[String]) -> Result<()> {
    let st = State::load(paths)?;
    if std::env::var_os("CPMM_NS").is_none() && vfs::running(paths).is_none() {
        let _ = loadorder::write_modlist(paths, &st);
    }
    vfs::exec_in(paths, &st.mods, st.active_own_saves(paths).as_deref(), cmd)
}
