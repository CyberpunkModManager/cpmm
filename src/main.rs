//! cpmm: Cyberpunk 2077 mod manager for Linux/Proton.

mod api;
mod archive;
mod conflicts;
mod files;
mod import;
mod install;
mod loadorder;
mod manage;
mod nexus;
mod profiles;
mod safety;
mod state;
mod steam;
mod vanilla;
mod vfs;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use nexus::Nexus;
use state::{Mod, NexusSource, Paths, ReqCheck, Requirement, State, now, short_name, slugify};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;

#[derive(Parser)]
#[command(version, about = "Cyberpunk 2077 mod manager (overlayfs VFS, Nexus updates and requirements)")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create the data directory; optionally point at a non-default game directory.
    Init {
        #[arg(long)]
        game: Option<PathBuf>,
    },
    /// List managed mods in priority order (later wins), plus mod.io mods.
    List {
        /// Show file counts and conflict counts (slower)
        #[arg(short, long)]
        long: bool,
    },
    /// Details for one mod (id, Nexus id, or unique name fragment).
    Info { r#mod: String },
    /// Enable mods (warns about missing/disabled requirements).
    Enable { mods: Vec<String> },
    /// Disable mods (warns if other enabled mods require them).
    Disable { mods: Vec<String> },
    /// Change a mod's priority.
    Order {
        r#mod: String,
        #[arg(long, group = "pos")]
        top: bool,
        #[arg(long, group = "pos")]
        bottom: bool,
        #[arg(long, group = "pos")]
        before: Option<String>,
        #[arg(long, group = "pos")]
        after: Option<String>,
    },
    /// File overwrites between enabled mods and against the base game, plus redscript conflicts.
    Conflicts {
        /// Only show conflicts involving this mod
        r#mod: Option<String>,
        /// Skip hashing (faster, can't tell identical copies apart)
        #[arg(long)]
        no_hash: bool,
        /// Also list mods that override base-game files
        #[arg(long)]
        base: bool,
    },
    /// Install from Nexus (id or URL) or a local archive.
    Install {
        source: String,
        /// Nexus file: file id, version game-scoped id, or name fragment
        #[arg(long)]
        file: Option<String>,
        #[arg(long)]
        id: Option<String>,
        #[arg(long)]
        name: Option<String>,
        /// Strip this folder from the archive (FOMOD option dirs, odd layouts)
        #[arg(long)]
        root: Option<String>,
        /// Place files under this game-relative folder
        #[arg(long)]
        dest: Option<String>,
        /// Install missing hard requirements too (recursively)
        #[arg(long)]
        with_deps: bool,
        /// Install even if hard requirements are missing
        #[arg(long)]
        ignore_deps: bool,
        /// Install but leave disabled
        #[arg(long)]
        disabled: bool,
        /// Nexus mod id for a local archive (enables update checks)
        #[arg(long)]
        nexus: Option<u64>,
        /// Keep the downloaded archive in the downloads dir
        #[arg(long)]
        keep_archive: bool,
    },
    /// Check Nexus for newer uploads of each installed file.
    Updates {
        /// Ignore the 1h metadata cache
        #[arg(long)]
        refresh: bool,
    },
    /// Install the newest upload of the same Nexus file for the given mods (or --all outdated).
    Update {
        mods: Vec<String>,
        #[arg(long)]
        all: bool,
    },
    /// Give a mod a new id (renames its staging folder; rules and hidden files follow).
    Rename { r#mod: String, new_id: String },
    /// Uninstall: move a mod's staged files to cpmm's trash and forget it.
    #[command(alias = "uninstall")]
    Remove {
        r#mod: String,
        #[arg(long)]
        force: bool,
    },
    /// Check every enabled mod's Nexus requirements are installed and enabled.
    Deps {
        #[arg(long)]
        refresh: bool,
        /// Install missing hard requirements (recursively) and enable disabled ones
        #[arg(long)]
        fix: bool,
        /// Accept a missing requirement: --ignore <mod> <nexus id>
        #[arg(long, num_args = 2, value_names = ["MOD", "REQ_ID"])]
        ignore: Option<Vec<String>>,
        /// Undo --ignore
        #[arg(long, num_args = 2, value_names = ["MOD", "REQ_ID"])]
        unignore: Option<Vec<String>>,
    },
    /// Hide files/folders of a mod from the game (resolves a conflict without reordering).
    Hide { r#mod: String, paths: Vec<String> },
    /// Restore hidden files (all if no paths given).
    Unhide { r#mod: String, paths: Vec<String> },
    /// Make this mod win its conflicts (or just the given paths) by hiding the other copies.
    Prefer { r#mod: String, paths: Vec<String> },
    /// Handle a Nexus "Mod Manager Download" (nxm://) link.
    Nxm {
        url: Option<String>,
        /// Same as `cpmm register-handler`
        #[arg(long, hide = true)]
        register: bool,
    },
    /// Mods on your Nexus "Tracked" list: a link-free way to queue installs from any browser
    /// (including Steam's, which refuses nxm:// links).
    Tracked {
        /// Install the tracked mods that aren't installed yet (with requirements)
        #[arg(long)]
        install: bool,
        /// Untrack mods on Nexus once they're installed
        #[arg(long)]
        untrack: bool,
    },
    /// Make cpmm the default handler for Nexus "Mod Manager Download" (nxm://) links.
    #[command(alias = "registerhandler")]
    RegisterHandler {
        /// Only show which app currently handles nxm:// links
        #[arg(long)]
        check: bool,
        /// Remove cpmm as the handler
        #[arg(long, conflicts_with = "check")]
        unregister: bool,
    },
    /// Compare staged files to the Nexus manifest hashes.
    Verify { mods: Vec<String> },
    /// One-time migration of the existing hand-installed mods into staging.
    Import {
        #[arg(long, default_value = "~/Projects/cyberpunk-modlist")]
        modlist: String,
        /// Actually move files (default is a dry run)
        #[arg(long)]
        apply: bool,
    },
    /// Turn game-dir files or folders into a new manual mod.
    Adopt {
        id: String,
        #[arg(long)]
        name: Option<String>,
        paths: Vec<String>,
    },
    /// Base-game files that are neither vanilla nor managed (and, with --modified, edited vanilla files).
    Unmanaged {
        #[arg(long)]
        modified: bool,
        /// One file per line instead of grouping by folder
        #[arg(long)]
        flat: bool,
    },
    /// Files the game/tools wrote while running (overlay upper dir): list, move into a mod, clear.
    Overwrite {
        #[command(subcommand)]
        action: Option<OwCmd>,
    },
    /// Copy all enabled mods back into the real game dir (escape hatch; reflinks, no extra space).
    Eject {
        #[arg(long)]
        apply: bool,
    },
    /// Free disk space: empty trash/, tmp/ and cached downloads.
    Clean {
        #[arg(long)]
        dry_run: bool,
    },
    /// Steam launch wrapper: cpmm run -- %command%
    Run {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        cmd: Vec<String>,
    },
    /// Run a command seeing the modded game dir (joins the running game session if any).
    Exec {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        cmd: Vec<String>,
    },
    /// Is a session running, with which layers.
    Status,
    /// Print the Steam launch options line to use.
    LaunchOptions,
    /// Add the cpmm GUI to the Steam library as a non-Steam game, with artwork. Works while
    /// Steam runs (through its CEF debugger, e.g. with Decky) or with Steam closed.
    #[command(visible_alias = "add-to-steam")]
    SteamShortcut {
        /// Name in the library
        #[arg(long, default_value = "cpmm")]
        name: String,
        /// GUI binary (default: cpmm-gui next to this cpmm)
        #[arg(long)]
        exe: Option<PathBuf>,
        /// Remove the shortcut instead
        #[arg(long)]
        remove: bool,
        /// Only show what would change
        #[arg(long)]
        dry_run: bool,
    },
    /// Automatic load order: enforce requirements, your rules and patch-over-target, keep the
    /// rest stable. Shows the plan; --apply saves it (modlist.txt follows on next launch).
    Sort {
        #[arg(long)]
        apply: bool,
    },
    /// Load order rules: `cpmm rule <mod> --after <other>`, `--remove`, or `--list`.
    Rule {
        r#mod: Option<String>,
        #[arg(long)]
        after: Option<String>,
        #[arg(long)]
        remove: bool,
        #[arg(long)]
        list: bool,
    },
    /// Archive resource conflicts (which mods replace the same game assets).
    Archives {
        /// Include byte-identical overlaps
        #[arg(long)]
        all: bool,
    },
    /// Health check: crash/stutter risks with suggested fixes.
    Doctor {
        /// Mark a finding as known/accepted (title or unique substring)
        #[arg(long)]
        ack: Option<String>,
        /// Undo --ack
        #[arg(long)]
        unack: Option<String>,
        /// Also show acknowledged findings
        #[arg(long)]
        all: bool,
    },
    /// Earlier versions of a mod kept for rollback.
    History { r#mod: String },
    /// Restore a mod's previous version (or a specific one with --to <ts> from `cpmm history`).
    Rollback {
        r#mod: String,
        #[arg(long)]
        to: Option<i64>,
    },
    /// Undo a whole update run (default: the most recent one).
    UndoUpdate { batch: Option<String> },
    /// Crash reports, with what changed since the last session that didn't crash.
    Crashes {
        #[arg(long, default_value_t = 5)]
        limit: usize,
    },
    /// Errors and warnings in the game/framework logs that are new since the previous launch.
    Errors,
    /// Game patch guard: did the game update, and are the native frameworks ready for it?
    Patch,
    /// Launch the next session without mods (`on`), or cancel (`off`).
    Vanilla { state: Option<String> },
    /// Save snapshots: list, take one (--snapshot) or restore (--restore <name>).
    Saves {
        #[arg(long)]
        snapshot: bool,
        #[arg(long)]
        restore: Option<String>,
    },
    /// Mod list profiles: which mods are enabled, their order and load-order rules.
    /// Installed files are shared; switching never downloads or copies anything.
    Profile {
        #[command(subcommand)]
        action: Option<ProfileCmd>,
    },
    /// JSON views for the GUI and scripts.
    #[command(subcommand)]
    Api(ApiCmd),
}

#[derive(Subcommand)]
enum ProfileCmd {
    /// Profiles, with the active one marked
    List,
    /// New profile: a copy of the active one (or --from), or --empty (every mod disabled)
    Create {
        name: String,
        #[arg(long, conflicts_with = "empty")]
        from: Option<String>,
        #[arg(long)]
        empty: bool,
        /// Give it its own save folder (starts empty unless --copy-saves)
        #[arg(long)]
        own_saves: bool,
        /// With --own-saves: start with a copy of the active profile's saves
        #[arg(long, requires = "own_saves")]
        copy_saves: bool,
        /// Switch to it right away
        #[arg(long)]
        switch: bool,
    },
    /// Make a profile the active one (takes effect at the next launch)
    #[command(visible_alias = "use")]
    Switch {
        name: String,
    },
    Rename {
        old: String,
        new: String,
    },
    /// Own save folder for a profile (`own`), or back to the game's shared one (`shared`).
    /// Turning it off keeps the profile's saves for later.
    Saves {
        name: String,
        #[arg(value_parser = ["own", "shared"])]
        mode: String,
        /// With `own`: copy the active profile's saves in (only into an empty folder)
        #[arg(long)]
        copy: bool,
    },
    /// Delete a profile (not the active one); mods stay installed
    Delete {
        name: String,
    },
}

#[derive(Subcommand)]
enum ApiCmd {
    /// Everything the main view needs
    State,
    /// Files of one mod with conflict info
    Info {
        r#mod: String,
    },
    Conflicts,
    Updates {
        #[arg(long)]
        refresh: bool,
    },
    Overwrite,
    Tracked,
    /// Nexus mod details before installing (files, requirements)
    Lookup {
        source: String,
    },
    /// Sort plan + archive conflicts
    Loadorder,
    Doctor,
    /// Archive resource conflicts between owners, with the current winner
    Archives,
    /// Crashes, new log errors, patch guard, saves and undoable update runs
    Safety,
    History {
        r#mod: String,
    },
}

#[derive(Subcommand)]
enum OwCmd {
    List,
    /// Move files (default: all) into a mod; a new manual mod is created if the name is new.
    Move {
        to: String,
        paths: Vec<String>,
    },
    /// Move files (default: all, including deletion markers) to trash.
    Clear {
        paths: Vec<String>,
    },
}

/// True if a process whose program (argv[0]) is Cyberpunk2077.exe exists.
pub fn game_running() -> bool {
    fs::read_dir("/proc")
        .map(|d| {
            d.flatten().any(|e| {
                fs::read(e.path().join("cmdline"))
                    .map(|c| {
                        let a0 = c.split(|b| *b == 0).next().unwrap_or(&[]);
                        String::from_utf8_lossy(a0).to_lowercase().ends_with("cyberpunk2077.exe")
                    })
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

/// Serialise state-changing commands (two nxm clicks at once, etc.).
fn lock(paths: &Paths) -> Result<fs::File> {
    use std::os::fd::AsRawFd;
    fs::create_dir_all(&paths.data)?;
    let f = fs::OpenOptions::new().create(true).truncate(false).write(true).open(paths.data.join("cpmm.lock"))?;
    if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        eprintln!("waiting for another cpmm command to finish…");
        if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX) } != 0 {
            bail!("flock: {}", std::io::Error::last_os_error());
        }
    }
    Ok(f)
}

fn expand(p: &str) -> PathBuf {
    match p.strip_prefix("~/") {
        Some(r) => PathBuf::from(std::env::var("HOME").unwrap()).join(r),
        None => PathBuf::from(p),
    }
}

fn nexus(paths: &Paths) -> Nexus {
    Nexus::new(paths.api_key().ok(), &paths.cache())
}

fn warn_if_running(paths: &Paths) {
    if vfs::running(paths).is_some() {
        eprintln!("note: a game session is running; changes apply on next launch");
    }
}

fn main() {
    // Behave like a normal CLI under `| head` instead of panicking on EPIPE.
    unsafe { libc::signal(libc::SIGPIPE, libc::SIG_DFL) };
    if let Err(e) = real_main() {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn real_main() -> Result<()> {
    let cli = Cli::parse();
    let paths = Paths::load()?;
    // Commands that inspect the base dir must not run inside the overlay.
    let in_ns = std::env::var_os("CPMM_NS").is_some();
    match cli.cmd {
        Cmd::Run { cmd } => {
            let st = State::load(&paths)?;
            if std::env::var_os("CPMM_NS").is_none() {
                let _ = safety::ingest_errors(&paths); // logs of the previous session
                let vf = safety::vanilla_flag(&paths);
                if vf.exists() {
                    let _ = fs::remove_file(&vf);
                    safety::record_session(&paths, &st, true);
                    eprintln!("cpmm: vanilla launch (no mods mounted)");
                    if let Some(s) = st.active_own_saves(&paths) {
                        // still the profile's own saves
                        return vfs::run(&paths, None, Some(&s), &cmd);
                    }
                    use std::os::unix::process::CommandExt;
                    let err = std::process::Command::new(&cmd[0]).args(&cmd[1..]).exec();
                    bail!("exec {}: {err}", cmd[0]);
                }
                safety::record_session(&paths, &st, false);
                if let Err(e) = loadorder::write_modlist(&paths, &st) {
                    eprintln!("cpmm: modlist.txt not updated: {e:#}");
                }
            }
            return vfs::run(&paths, Some(&st.mods), st.active_own_saves(&paths).as_deref(), &cmd);
        }
        Cmd::Exec { cmd } => {
            let st = State::load(&paths)?;
            if std::env::var_os("CPMM_NS").is_none() && vfs::running(&paths).is_none() {
                let _ = loadorder::write_modlist(&paths, &st);
            }
            return vfs::exec_in(&paths, &st.mods, st.active_own_saves(&paths).as_deref(), &cmd);
        }
        _ if in_ns => bail!("run cpmm management commands outside `cpmm exec`/the game session"),
        _ => {}
    }
    let _lock = match cli.cmd {
        Cmd::List { .. }
        | Cmd::Profile { action: None | Some(ProfileCmd::List) }
        | Cmd::Info { .. }
        | Cmd::Status
        | Cmd::LaunchOptions
        | Cmd::SteamShortcut { .. }
        | Cmd::RegisterHandler { .. }
        | Cmd::Api(_)
        | Cmd::Archives { .. }
        | Cmd::Doctor { ack: None, unack: None, .. }
        | Cmd::History { .. }
        | Cmd::Crashes { .. }
        | Cmd::Patch
        | Cmd::Conflicts { .. }
        | Cmd::Updates { .. }
        | Cmd::Unmanaged { .. } => None,
        _ => Some(lock(&paths)?),
    };
    if let Cmd::Nxm { url: Some(_), .. } = &cli.cmd {
        // Launched by the browser without a terminal: log instead. (The GUI reads stdout itself.)
        if unsafe { libc::isatty(1) } == 0 && std::env::var_os("CPMM_GUI").is_none() {
            use std::os::fd::AsRawFd;
            fs::create_dir_all(&paths.data)?;
            let f = fs::OpenOptions::new().create(true).append(true).open(paths.data.join("nxm.log"))?;
            unsafe {
                libc::dup2(f.as_raw_fd(), 1);
                libc::dup2(f.as_raw_fd(), 2);
            }
            println!("\n== {}", now());
        }
    }
    let mut st = State::load(&paths)?;
    match cli.cmd {
        Cmd::Run { .. } | Cmd::Exec { .. } => unreachable!(),
        Cmd::Init { game } => {
            paths.ensure_dirs()?;
            if let Some(g) = game {
                fs::write(paths.data.join("game_dir"), g.to_string_lossy().as_bytes())?;
            }
            let p = Paths::load()?;
            st.save(&p)?;
            println!("data: {}\ngame: {}", p.data.display(), p.game.display());
            same_fs_check(&p)?;
        }
        Cmd::List { long } => list(&paths, &st, long)?,
        Cmd::Profile { action } => profile(&paths, &mut st, action.unwrap_or(ProfileCmd::List))?,
        Cmd::Info { r#mod } => info(&paths, &st, &r#mod)?,
        Cmd::Enable { mods } | Cmd::Disable { mods } if mods.is_empty() => bail!("name at least one mod"),
        Cmd::Enable { mods } => set_enabled(&paths, &mut st, &mods, true)?,
        Cmd::Disable { mods } => set_enabled(&paths, &mut st, &mods, false)?,
        Cmd::Order { r#mod, top, bottom, before, after } => {
            let i = st.get(&r#mod)?;
            let m = st.mods.remove(i);
            let pos = if top {
                st.mods.len()
            } else if bottom {
                0
            } else if let Some(b) = before {
                st.get(&b)?
            } else if let Some(a) = after {
                st.get(&a)? + 1
            } else {
                st.mods.insert(i, m);
                bail!("give --top, --bottom, --before or --after");
            };
            println!("{} now at priority {}", m.id, pos + 1);
            st.mods.insert(pos, m);
            st.save(&paths)?;
            warn_if_running(&paths);
        }
        Cmd::Conflicts { r#mod, no_hash, base } => show_conflicts(&paths, &st, r#mod.as_deref(), !no_hash, base)?,
        Cmd::Install { source, file, id, name, root, dest, with_deps, ignore_deps, disabled, nexus: nid, keep_archive } => {
            let o = InstallOpts { file, id, name, root, dest, with_deps, ignore_deps, disabled, keep_archive, nxm: None };
            if let Some(gid) = parse_nexus(&source) {
                let nx = nexus(&paths);
                let mut visiting = BTreeSet::new();
                install_nexus(&paths, &mut st, &nx, gid, &o, 0, &mut visiting)?;
            } else {
                install_local(&paths, &mut st, &expand(&source), nid, &o)?;
            }
            warn_if_running(&paths);
        }
        Cmd::Updates { refresh } => updates(&paths, &st, refresh)?,
        Cmd::Update { mods, all } => update(&paths, &mut st, &mods, all)?,
        Cmd::Rename { r#mod, new_id } => {
            manage::ensure_idle(&paths)?;
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
            st.save(&paths)?;
            println!("renamed {old} -> {new_id}");
        }
        Cmd::Remove { r#mod, force } => {
            let i = st.get(&r#mod)?;
            let id = st.mods[i].id.clone();
            if let Some(nid) = st.mods[i].nexus.as_ref().map(|n| n.mod_id) {
                let users: Vec<String> = st
                    .mods
                    .iter()
                    .filter(|m| m.enabled && m.requires.iter().any(|r| r.mod_id == nid && !r.optional))
                    .map(|m| m.id.clone())
                    .collect();
                if !users.is_empty() && !force {
                    bail!("{id} is required by {}; --force to remove anyway", users.join(", "));
                }
            }
            if vfs::running(&paths).is_some() {
                bail!("game session running; quit first (its overlay uses these files)");
            }
            let elsewhere = st.profiles_enabling(&id);
            if !elsewhere.is_empty() {
                println!(
                    "note: also removes it from profile(s) {} where it is enabled (disable it instead to keep it there)",
                    elsewhere.join(", ")
                );
            }
            let _ = safety::snapshot_saves(&paths, &format!("before uninstalling {id}"));
            safety::journal(&paths, "uninstall", std::slice::from_ref(&id), &st.mods[i].version);
            let d = paths.mods().join(&id);
            if d.exists() {
                let t = paths.trash_for(&id);
                fs::rename(&d, &t)?;
                println!("files moved to {} (delete when sure)", t.display());
            }
            st.mods.remove(i);
            st.save(&paths)?;
            println!("removed {id}");
        }
        Cmd::Deps { refresh, fix, ignore, unignore } => {
            for (v, on) in [(ignore, true), (unignore, false)] {
                if let Some(v) = v {
                    let i = st.get(&v[0])?;
                    let rid: u64 = v[1].parse().context("requirement must be a Nexus mod id")?;
                    let m = &mut st.mods[i];
                    m.ignored_reqs.retain(|x| *x != rid);
                    if on {
                        m.ignored_reqs.push(rid);
                    }
                    println!("{} requirement {rid} for {}", if on { "ignoring" } else { "no longer ignoring" }, m.id);
                    st.save(&paths)?;
                }
            }
            deps(&paths, &mut st, refresh)?;
            if fix {
                deps_fix(&paths, &mut st)?;
            }
        }
        Cmd::Hide { r#mod, paths: sel } => {
            manage::ensure_idle(&paths)?;
            let i = st.get(&r#mod)?;
            let d = manage::hide(&paths, &mut st, i, &sel)?;
            safety::journal(&paths, "hide", std::slice::from_ref(&st.mods[i].id), &sel.join(", "));
            st.save(&paths)?;
            println!("hid {} files of {}", d.len(), st.mods[i].id);
        }
        Cmd::Unhide { r#mod, paths: sel } => {
            manage::ensure_idle(&paths)?;
            let i = st.get(&r#mod)?;
            let d = manage::unhide(&paths, &mut st, i, &sel)?;
            st.save(&paths)?;
            println!("restored {} files of {}", d.len(), st.mods[i].id);
        }
        Cmd::Prefer { r#mod, paths: sel } => {
            manage::ensure_idle(&paths)?;
            let i = st.get(&r#mod)?;
            let d = manage::prefer(&paths, &mut st, i, &sel)?;
            safety::journal(&paths, "prefer", std::slice::from_ref(&st.mods[i].id), &sel.join(", "));
            st.save(&paths)?;
            for (o, rel) in &d {
                println!("  hid {rel} in {o}");
            }
            println!("{} now wins {} contested files (undo with `cpmm unhide <other mod>`)", st.mods[i].id, d.len());
        }
        Cmd::Tracked { install, untrack } => {
            let nx = nexus(&paths);
            let ids = nx.tracked()?;
            if ids.is_empty() {
                println!("no tracked Cyberpunk mods (press Track on a mod page on nexusmods.com)");
            }
            let mut done = vec![];
            for gid in ids {
                let name = nx.mod_info(gid).map(|i| i.name).unwrap_or_default();
                match st.by_nexus(gid) {
                    Some(m) => {
                        println!("  {gid:<7} {name}: installed as {}", m.id);
                        done.push(gid);
                    }
                    None if install => {
                        let o = InstallOpts {
                            file: None,
                            id: None,
                            name: None,
                            root: None,
                            dest: None,
                            with_deps: true,
                            ignore_deps: false,
                            disabled: false,
                            keep_archive: false,
                            nxm: None,
                        };
                        let mut visiting = BTreeSet::new();
                        match install_nexus(&paths, &mut st, &nx, gid, &o, 0, &mut visiting) {
                            Ok(()) => done.push(gid),
                            Err(e) => println!("  {gid} {name}: {e:#}"),
                        }
                    }
                    None => println!("  {gid:<7} {name}: not installed"),
                }
            }
            if untrack {
                for gid in done {
                    nx.untrack(gid)?;
                    println!("  untracked {gid}");
                }
            }
            warn_if_running(&paths);
        }
        Cmd::RegisterHandler { check, unregister } => {
            let before = manage::current_nxm_handler();
            if check {
                println!("nxm:// handler: {}", if before.is_empty() { "(none)" } else { &before });
            } else if unregister {
                manage::unregister_nxm()?;
                println!("unregistered (handler was {:?}, now {:?})", before, manage::current_nxm_handler());
            } else {
                let exe = std::env::current_exe()?;
                let d = manage::register_nxm(&exe)?;
                if !before.is_empty() && before != "cpmm-nxm.desktop" {
                    println!("replaced previous handler {before}");
                }
                println!("nxm:// links now open {} nxm <url> ({})", exe.display(), d.display());
                println!("works on the desktop and in Game Mode; progress goes to {}", paths.data.join("nxm.log").display());
            }
        }
        Cmd::Nxm { url, register } => {
            if register {
                let exe = std::env::current_exe()?;
                let d = manage::register_nxm(&exe)?;
                println!("registered {} as the nxm:// handler ({})", exe.display(), d.display());
            }
            if let Some(u) = url {
                let l = manage::parse_nxm(&u)?;
                let o = InstallOpts {
                    file: Some(l.file_gsid.clone()),
                    id: None,
                    name: None,
                    root: None,
                    dest: None,
                    with_deps: true,
                    ignore_deps: false,
                    disabled: false,
                    keep_archive: false,
                    nxm: l.key.clone().zip(l.expires.clone()),
                };
                let nx = nexus(&paths);
                let mut visiting = BTreeSet::new();
                match install_nexus(&paths, &mut st, &nx, l.mod_id, &o, 0, &mut visiting) {
                    Ok(()) => {
                        let name = st.by_nexus(l.mod_id).map(|m| format!("{} {}", m.name, m.version)).unwrap_or_default();
                        manage::notify("Mod installed", &format!("{name}\n(applies on next launch; log: ~/.local/share/cpmm/nxm.log)"));
                    }
                    Err(e) => {
                        manage::notify("Mod install failed", &format!("{e:#}"));
                        return Err(e);
                    }
                }
            } else if !register {
                bail!("give an nxm:// URL or --register");
            }
        }
        Cmd::Verify { mods } => verify(&paths, &st, &mods)?,
        Cmd::Import { modlist, apply } => {
            same_fs_check(&paths)?;
            import::import(&paths, &mut st, &expand(&modlist), apply)?;
            if apply {
                println!("fetching Nexus requirements…");
                deps(&paths, &mut st, true)?;
            }
        }
        Cmd::Adopt { id, name, paths: rels } => {
            let id = slugify(&id);
            let got = import::adopt(&paths, &mut st, &id, name.as_deref().unwrap_or(&id), &rels)?;
            println!("adopted {} files into {id}", got.len());
        }
        Cmd::Unmanaged { modified, flat } => unmanaged(&paths, &st, modified, flat)?,
        Cmd::Overwrite { action } => match action.unwrap_or(OwCmd::List) {
            OwCmd::List => overwrite(&paths, &st)?,
            OwCmd::Move { to, paths: sel } => {
                let (id, moved, skipped) = manage::overwrite_move(&paths, &mut st, &to, &sel)?;
                st.save(&paths)?;
                println!(
                    "moved {moved} files into {id}{}",
                    if skipped > 0 {
                        format!(" ({skipped} deletion markers left; `cpmm overwrite clear` drops them)")
                    } else {
                        String::new()
                    }
                );
            }
            OwCmd::Clear { paths: sel } => {
                let (n, t) = manage::overwrite_clear(&paths, &sel)?;
                println!("moved {n} entries to {}", t.display());
            }
        },
        Cmd::Eject { apply } => eject(&paths, &st, apply)?,
        Cmd::Api(a) => {
            let v = match a {
                ApiCmd::State => api::state(&paths, &st)?,
                ApiCmd::Info { r#mod } => api::info(&paths, &st, &r#mod)?,
                ApiCmd::Conflicts => api::conflicts(&paths, &st)?,
                ApiCmd::Updates { refresh } => api::updates(&paths, &st, refresh)?,
                ApiCmd::Overwrite => api::overwrite(&paths, &st)?,
                ApiCmd::Tracked => api::tracked(&paths, &st)?,
                ApiCmd::Loadorder => {
                    let p = loadorder::plan(&paths, &st);
                    serde_json::json!({"plan": p, "rules": st.rules, "modlist": loadorder::modlist(&paths, &st)})
                }
                ApiCmd::Doctor => serde_json::to_value(loadorder::doctor(&paths, &st))?,
                ApiCmd::Safety => {
                    let nx = nexus(&paths);
                    serde_json::json!({
                        "crashes": safety::crashes(&paths, 5),
                        "errors": safety::error_diff(&paths),
                        "patch": safety::patch_status(&paths, &st, Some(&nx)),
                        "snapshots": safety::list_snapshots(&paths),
                        "save_risks": safety::save_risks(&paths, &st),
                        "batches": safety::batches(&paths, &st),
                        "sessions": safety::sessions(&paths).len(),
                    })
                }
                ApiCmd::History { r#mod } => {
                    let i = st.get(&r#mod)?;
                    serde_json::to_value(safety::list_history(&paths, &st.mods[i].id))?
                }
                ApiCmd::Archives => {
                    let ids: Vec<String> = st.mods.iter().map(|m| m.id.clone()).collect();
                    serde_json::to_value(loadorder::archive_conflicts(&paths, &st, &ids))?
                }
                ApiCmd::Lookup { source } => api::search_info(&paths, parse_nexus(&source).context("not a Nexus mod id or URL")?)?,
            };
            println!("{}", serde_json::to_string(&v)?);
        }
        Cmd::Sort { apply } => {
            let p = loadorder::plan(&paths, &st);
            if p.moved.is_empty() {
                println!("load order already satisfies every constraint ({} constraints)", p.edges.len());
            }
            for mv in &p.moved {
                println!("  {:<34} {:>3} -> {:<3} {}", mv.id, mv.from, mv.to, mv.because.join("; "));
            }
            for c in &p.cycles {
                println!("  cycle (left in current order): {}", c.join(" -> "));
            }
            if !p.unresolved.is_empty() {
                println!("unrelated mods replacing the same resources (current order kept; pin with `cpmm rule`):");
                for u in &p.unresolved {
                    println!(
                        "  {} vs {}: {} resources, winner {}",
                        u.a.label(),
                        u.b.label(),
                        u.shared - u.identical,
                        u.winner.clone().unwrap_or("game order".into())
                    );
                }
            }
            if apply && !p.moved.is_empty() {
                safety::journal(&paths, "sort", &p.moved.iter().map(|m| m.id.clone()).collect::<Vec<_>>(), "");
                loadorder::apply(&mut st, &p.order);
                st.save(&paths)?;
                println!("applied; {} archives in modlist.txt", loadorder::write_modlist(&paths, &st)?);
                warn_if_running(&paths);
            } else if !p.moved.is_empty() {
                println!("dry run: `cpmm sort --apply` to save");
            }
        }
        Cmd::Rule { r#mod, after, remove, list } => {
            let target = r#mod.filter(|_| !list);
            if let Some(target) = target {
                let a = st.mods[st.get(&target)?].id.clone();
                let b = st.mods[st.get(&after.context("give --after <mod>")?)?].id.clone();
                if a == b {
                    bail!("a mod can't load after itself");
                }
                st.rules.retain(|r| !(r.r#mod == a && r.after == b) && !(r.r#mod == b && r.after == a));
                if !remove {
                    st.rules.push(loadorder::Rule { r#mod: a.clone(), after: b.clone() });
                    safety::journal(&paths, "rule", &[a.clone(), b.clone()], "wins over");
                    println!("rule: {a} wins over {b}");
                } else {
                    println!("removed rules between {a} and {b}");
                }
                // A rule only matters once the order reflects it: re-sort right away.
                let p = loadorder::plan(&paths, &st);
                for mv in &p.moved {
                    println!("  moved {} {} -> {}", mv.id, mv.from, mv.to);
                }
                loadorder::apply(&mut st, &p.order);
                st.save(&paths)?;
                println!("load order updated; {} archives in modlist.txt", loadorder::write_modlist(&paths, &st)?);
                warn_if_running(&paths);
            } else {
                if st.rules.is_empty() {
                    println!("no rules");
                }
                for r in &st.rules {
                    println!("  {} after {}", r.r#mod, r.after);
                }
            }
        }
        Cmd::Archives { all } => {
            let ids: Vec<String> = st.mods.iter().map(|m| m.id.clone()).collect();
            let ps = loadorder::archive_conflicts(&paths, &st, &ids);
            let mut n = 0;
            for p in ps.iter().filter(|p| all || p.identical < p.shared) {
                n += 1;
                println!(
                    "  {} ({}) vs {} ({}): {} shared, {} identical{}  winner: {}",
                    p.a.label(),
                    p.archive_a,
                    p.b.label(),
                    p.archive_b,
                    p.shared,
                    p.identical,
                    if p.related { ", related by requirements" } else { "" },
                    p.winner.clone().unwrap_or("game order".into())
                );
            }
            println!("{n} conflicting archive pairs ({} total overlaps)", ps.len());
        }
        Cmd::Doctor { ack, unack, all } => {
            if let Some(k) = ack.or(unack.clone()) {
                let titles: Vec<String> = loadorder::doctor(&paths, &st)
                    .into_iter()
                    .map(|f| f.title)
                    .filter(|t| t.to_lowercase().contains(&k.to_lowercase()))
                    .collect();
                let titles = if unack.is_some() {
                    st.acked.iter().filter(|t| t.to_lowercase().contains(&k.to_lowercase())).cloned().collect()
                } else {
                    titles
                };
                if titles.len() != 1 {
                    bail!("{:?} matches {} findings; be more specific", k, titles.len());
                }
                st.acked.retain(|t| *t != titles[0]);
                if unack.is_none() {
                    st.acked.push(titles[0].clone());
                }
                st.save(&paths)?;
                println!("{} {:?}", if unack.is_none() { "acknowledged" } else { "un-acknowledged" }, titles[0]);
            }
            let fs_: Vec<loadorder::Finding> =
                loadorder::doctor(&paths, &st).into_iter().filter(|f| all || f.severity != "acked").collect();
            for f in &fs_ {
                let tag = match f.severity {
                    "crash" => "CRASH RISK",
                    "warning" => "WARNING",
                    "perf" => "PERF",
                    "acked" => "acked",
                    _ => "info",
                };
                println!("[{tag}] {}", f.title);
                if !f.detail.is_empty() {
                    println!("    {}", f.detail);
                }
                if let Some(x) = &f.fix {
                    println!("    fix: {x}");
                }
                for a in &f.actions {
                    println!(
                        "    {}: cpmm {}",
                        a.label,
                        a.args.iter().map(|x| if x.contains(' ') { format!("{x:?}") } else { x.clone() }).collect::<Vec<_>>().join(" ")
                    );
                }
            }
            println!("{} findings ({} crash risks)", fs_.len(), fs_.iter().filter(|f| f.severity == "crash").count());
        }
        Cmd::History { r#mod } => {
            let i = st.get(&r#mod)?;
            let h = safety::list_history(&paths, &st.mods[i].id);
            if h.is_empty() {
                println!("no earlier versions of {} kept", st.mods[i].id);
            }
            for e in h {
                println!(
                    "  {}  {}  v{}  {} files  {}{}",
                    e.meta.ts,
                    safety::iso(e.meta.ts),
                    e.meta.version,
                    e.files,
                    e.meta.reason,
                    e.meta.batch.map(|b| format!("  [{b}]")).unwrap_or_default()
                );
            }
        }
        Cmd::Rollback { r#mod, to } => {
            manage::ensure_idle(&paths)?;
            let _ = safety::snapshot_saves(&paths, &format!("before rollback of {}", r#mod));
            let msg = safety::rollback(&paths, &mut st, &r#mod, to)?;
            st.save(&paths)?;
            let _ = loadorder::write_modlist(&paths, &st);
            println!("rolled back {msg}");
        }
        Cmd::UndoUpdate { batch } => {
            manage::ensure_idle(&paths)?;
            let b = match batch {
                Some(b) => b,
                None => safety::batches(&paths, &st).into_iter().next().map(|b| b.batch).context("no update run to undo")?,
            };
            let _ = safety::snapshot_saves(&paths, &format!("before undoing {b}"));
            for m in safety::undo_batch(&paths, &mut st, &b)? {
                println!("  rolled back {m}");
            }
            st.save(&paths)?;
            let _ = loadorder::write_modlist(&paths, &st);
            println!("undid {b}");
        }
        Cmd::Crashes { limit } => {
            let cs = safety::crashes(&paths, limit);
            if cs.is_empty() {
                println!("no crash reports");
            }
            for c in cs {
                println!("{}  {}  in {} {}", safety::iso(c.ts), c.message, c.module, c.offset);
                match (c.seconds_in, c.last_good) {
                    (Some(s), Some(g)) => {
                        println!("    {}m{}s into the session; changes since the last good session ({}):", s / 60, s % 60, safety::iso(g))
                    }
                    (Some(s), None) => println!("    {}m{}s into the session (no earlier clean session recorded)", s / 60, s % 60),
                    _ => println!("    before cpmm started recording sessions"),
                }
                for ch in &c.changes {
                    println!("      {ch}");
                }
            }
        }
        Cmd::Errors => {
            let d = safety::error_diff(&paths);
            if d.baseline {
                println!("first log snapshot recorded ({} sources); differences show from the next launch", d.totals.len());
            }
            for (src, n) in &d.totals {
                let new = d.new.get(src).map(|v| v.len()).unwrap_or(0);
                let fixed = d.fixed.get(src).map(|v| v.len()).unwrap_or(0);
                println!("  {src:<14} {n:>4} total  {new:>3} new  {fixed:>3} gone");
            }
            for (src, v) in &d.new {
                for l in v.iter().take(15) {
                    println!("  NEW [{src}] {l}");
                }
            }
        }
        Cmd::Patch => {
            let p = safety::patch_status(&paths, &st, Some(&nexus(&paths)));
            println!(
                "game build {} (last updated {})",
                p.build_id,
                if p.patched_at > 0 { safety::iso(p.patched_at) } else { "unknown".into() }
            );
            if p.patch_detected {
                println!("the game updated since cpmm last saw it (was build {})", p.previous_build.unwrap_or_default());
                for f in &p.at_risk {
                    println!(
                        "  {:<30} {}{}",
                        f.name,
                        if f.released_since_patch { "" } else { "no release since the patch" },
                        if f.update_available { " - update available" } else { "" }
                    );
                }
            } else {
                println!("no game update since cpmm started tracking");
            }
            for e in &p.red4ext_errors {
                println!("  RED4ext: {e}");
            }
            if p.vanilla {
                println!("next launch: VANILLA (cpmm vanilla off to cancel)");
            }
        }
        Cmd::Vanilla { state } => {
            let f = safety::vanilla_flag(&paths);
            match state.as_deref() {
                Some("on") => {
                    fs::write(&f, "1")?;
                    println!("next launch runs without mods (one time)");
                }
                Some("off") => {
                    let _ = fs::remove_file(&f);
                    println!("next launch runs with mods");
                }
                _ => println!("next launch: {}", if f.exists() { "vanilla" } else { "modded" }),
            }
        }
        Cmd::Saves { snapshot, restore } => {
            if snapshot {
                println!("snapshot: {}", safety::snapshot_saves(&paths, "manual")?.unwrap_or("no saves found".into()));
            }
            if let Some(r) = restore {
                if game_running() {
                    bail!("quit the game before restoring saves");
                }
                println!("restored {} files from {r}", safety::restore_snapshot(&paths, &r)?);
            }
            for s in safety::list_snapshots(&paths) {
                let prof = if s.profile.is_empty() || st.profiles.len() < 2 { String::new() } else { format!("  [{}]", s.profile) };
                println!("  {}  {}  {} saves  {}{prof}", s.name, safety::iso(s.ts), s.saves, s.reason);
            }
        }
        Cmd::Clean { dry_run } => {
            let mut total = 0u64;
            for d in [paths.trash(), paths.tmp(), paths.downloads()] {
                let size: u64 = walkdir::WalkDir::new(&d)
                    .into_iter()
                    .flatten()
                    .filter_map(|e| e.metadata().ok())
                    .filter(|m| m.is_file())
                    .map(|m| m.len())
                    .sum();
                total += size;
                if !dry_run && d.exists() {
                    fs::remove_dir_all(&d)?;
                    fs::create_dir_all(&d)?;
                }
            }
            println!("{} {:.1} MB", if dry_run { "would free" } else { "freed" }, total as f64 / 1048576.0);
        }
        Cmd::Status => match vfs::running(&paths) {
            Some((info, pid)) => {
                println!("session running since {} (pid {pid}), {} layers:", info.started, info.layers.len());
                if let Some(s) = &info.saves {
                    println!("  saves: {}", s.display());
                }
                for l in info.layers {
                    println!("  {l}");
                }
            }
            None => println!("no session running"),
        },
        Cmd::SteamShortcut { name, exe, remove, dry_run } => {
            steam::run(steam::Opts { name, exe, remove, dry_run })?;
        }
        Cmd::LaunchOptions => {
            let exe = std::env::current_exe()?;
            println!("Put cpmm right before %command% (after env vars and other wrappers), e.g.:\n");
            println!(
                "VKD3D_LOW_LATENCY_OFFSET=250 PROTON_VKD3D_LOWLATENCY=1 LOW_LATENCY_LAYER=1 WINEDLLOVERRIDES=\"winmm,version=n,b\" game-performance {} run -- %command% --launcher-skip -skipStartScreen",
                exe.display()
            );
        }
    }
    Ok(())
}

fn profile(paths: &Paths, st: &mut State, action: ProfileCmd) -> Result<()> {
    let summary = |st: &State, p: &profiles::Profile| format!("{} of {} mods enabled", p.enabled.len(), st.mods.len());
    match action {
        ProfileCmd::List => {
            st.sync_active_profile();
            for p in &st.profiles {
                let active = p.name == st.active_profile;
                let saves = match p.own_saves_dir(paths) {
                    Some(d) => format!("own saves ({})", profiles::count_saves(&d)),
                    None => "shared saves".into(),
                };
                println!("{} {:<24} {:<26} {saves}", if active { "*" } else { " " }, p.name, summary(st, p));
            }
            return Ok(());
        }
        ProfileCmd::Create { name, from, empty, own_saves, copy_saves, switch } => {
            let name = st.create_profile(&name, from.as_deref(), empty)?;
            println!(
                "created profile {name:?} ({})",
                if empty {
                    "empty: every mod disabled".to_string()
                } else {
                    format!("copy of {:?}", from.unwrap_or(st.active_profile.clone()))
                }
            );
            if own_saves {
                set_profile_saves(paths, st, &name, true, copy_saves)?;
            }
            if switch {
                manage::ensure_idle(paths)?;
                st.switch_profile(&name)?;
                println!("switched to {name:?}");
            }
        }
        ProfileCmd::Switch { name } => {
            let from = st.active_profile.clone();
            if st.profiles[st.profile_index(&name)?].name == from {
                println!("{from:?} is already the active profile");
                return Ok(());
            }
            manage::ensure_idle(paths)?;
            let new = st.switch_profile(&name)?;
            let p = st.profiles[st.profile_index(&st.active_profile)?].clone();
            println!("switched {from:?} -> {:?}: {}", p.name, summary(st, &p));
            if !new.is_empty() {
                println!("{} mod(s) new to this profile, added disabled: {}", new.len(), new.join(", "));
            }
            safety::journal(paths, "profile", std::slice::from_ref(&p.name), &format!("from {from}"));
            println!("takes effect at the next launch; `cpmm deps` / `cpmm doctor` check this list");
        }
        ProfileCmd::Rename { old, new } => {
            let (old, new) = st.rename_profile(&old, &new)?;
            println!("renamed profile {old:?} -> {new:?}");
        }
        ProfileCmd::Saves { name, mode, copy } => {
            set_profile_saves(paths, st, &name, mode == "own", copy)?;
        }
        ProfileCmd::Delete { name } => {
            let (name, saves) = st.delete_profile(paths, &name)?;
            println!("deleted profile {name:?} (mods stay installed)");
            if let Some(d) = saves {
                let t = paths.trash_for(&format!("saves-{}", slugify(&name)));
                fs::create_dir_all(t.parent().unwrap())?;
                fs::rename(&d, &t)?;
                println!("its own saves moved to {} (delete when sure)", t.display());
            }
        }
    }
    st.save(paths)
}

fn set_profile_saves(paths: &Paths, st: &mut State, name: &str, own: bool, copy: bool) -> Result<()> {
    let i = st.profile_index(name)?;
    let name = st.profiles[i].name.clone();
    if name == st.active_profile {
        manage::ensure_idle(paths)?; // the running game has the current save folder in use
    }
    let source = safety::saves_dir(paths); // the active profile's saves, before any change
    let dir = st.set_own_saves(paths, &name, own)?;
    if !own {
        let n = profiles::count_saves(&dir);
        println!(
            "{name:?} now uses the game's shared saves{}",
            if n > 0 { format!(" (its {n} own saves stay in {})", dir.display()) } else { String::new() }
        );
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
        println!("copied {} saves ({n} files) from {}", profiles::count_saves(&dir), source.display());
    }
    println!("{name:?} uses its own saves in {} ({} saves; not synced by Steam Cloud)", dir.display(), profiles::count_saves(&dir));
    Ok(())
}

fn same_fs_check(paths: &Paths) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    fs::create_dir_all(&paths.data)?;
    let a = fs::metadata(&paths.data)?.dev();
    let b = fs::metadata(&paths.game).with_context(|| format!("game dir {}", paths.game.display()))?.dev();
    if a != b {
        eprintln!("warning: data dir and game dir are on different filesystems; import will copy instead of rename");
    }
    Ok(())
}

fn parse_nexus(s: &str) -> Option<u64> {
    if let Ok(n) = s.parse() {
        return Some(n);
    }
    let i = s.find("cyberpunk2077/mods/")?;
    s[i + 19..].split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()
}

fn modio_mods(paths: &Paths) -> Vec<(String, String, String, String)> {
    let p = paths.modio_dir().join("metadata/state.json");
    let Ok(b) = fs::read(&p) else { return vec![] };
    let Ok(v): Result<serde_json::Value, _> = serde_json::from_slice(&b) else { return vec![] };
    v["Mods"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|m| {
            let p = &m["Profile"];
            (
                m["ID"].to_string(),
                p["name"].as_str().unwrap_or("?").to_string(),
                p.pointer("/submitted_by/username").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                p.pointer("/modfile/version").and_then(|x| x.as_str()).unwrap_or("").to_string(),
            )
        })
        .collect()
}

fn list(paths: &Paths, st: &State, long: bool) -> Result<()> {
    let owners = if long { Some(conflicts::ownership(paths, &st.mods, true)) } else { None };
    if st.profiles.len() > 1 {
        println!("profile: {} (cpmm profile list)", st.active_profile);
    }
    println!("{:>3} {:<3} {:<34} {:<12} {:>6}  name", "#", "on", "id", "version", "nexus");
    for (i, m) in st.mods.iter().enumerate() {
        let nid = m.nexus.as_ref().map(|n| n.mod_id.to_string()).unwrap_or("-".into());
        let extra = match &owners {
            Some(o) => {
                let mine: Vec<&Vec<String>> = o.values().map(|(_, v)| v).filter(|v| v.contains(&m.id)).collect();
                let lost = mine.iter().filter(|v| v.last() != Some(&m.id)).count();
                let won = mine.iter().filter(|v| v.len() > 1 && v.last() == Some(&m.id)).count();
                format!(
                    "  [{} files{}{}]",
                    files::list_tree(&paths.mod_files(&m.id)).len(),
                    if won > 0 { format!(", wins {won}") } else { "".into() },
                    if lost > 0 { format!(", loses {lost}") } else { "".into() }
                )
            }
            None => String::new(),
        };
        println!(
            "{:>3} {:<3} {:<34} {:<12} {:>6}  {}{}",
            i + 1,
            if m.enabled { "x" } else { "" },
            m.id,
            m.version.chars().take(12).collect::<String>(),
            nid,
            m.name,
            extra
        );
    }
    let mio = modio_mods(paths);
    if !mio.is_empty() {
        println!("\nmod.io (managed in-game via the Mods menu, outside the game dir):");
        for (id, name, by, ver) in mio {
            println!("    {id:<10} {name} {ver} by {by}");
        }
    }
    Ok(())
}

fn info(paths: &Paths, st: &State, key: &str) -> Result<()> {
    let m = &st.mods[st.get(key)?];
    println!("{}  ({})\n  enabled: {}\n  version: {}\n  installed: {}", m.name, m.id, m.enabled, m.version, m.installed_at);
    if let Some(n) = &m.nexus {
        println!(
            "  nexus: https://www.nexusmods.com/cyberpunk2077/mods/{}  file {:?} (id {}, version id {}, uploaded {})",
            n.mod_id, n.file_name, n.file_id, n.version_id, n.uploaded_at
        );
    }
    for r in &m.requires {
        let have = st.by_nexus(r.mod_id).map(|x| if x.enabled { "ok" } else { "DISABLED" }).unwrap_or("MISSING");
        println!(
            "  requires {} {} [{}]{}{}",
            r.mod_id,
            r.name,
            have,
            if r.optional { " (optional)" } else { "" },
            if r.notes.is_empty() { String::new() } else { format!(" — {}", r.notes) }
        );
    }
    let hid = files::list_tree(&paths.mods().join(&m.id).join("hidden"));
    if !hid.is_empty() {
        println!("  hidden ({}): {}", hid.len(), hid.join(", "));
    }
    let fl = files::list_tree(&paths.mod_files(&m.id));
    println!("  files ({}):", fl.len());
    for f in fl {
        println!("    {f}");
    }
    Ok(())
}

fn set_enabled(paths: &Paths, st: &mut State, keys: &[String], on: bool) -> Result<()> {
    if !on {
        let _ = safety::snapshot_saves(paths, &format!("before disabling {}", keys.join(", ")));
    }
    safety::journal(paths, if on { "enable" } else { "disable" }, keys, "");
    for k in keys {
        let i = st.get(k)?;
        st.mods[i].enabled = on;
        println!("{} {}", if on { "enabled" } else { "disabled" }, st.mods[i].id);
        if !on {
            if let Some(nid) = st.mods[i].nexus.as_ref().map(|n| n.mod_id) {
                let users: Vec<&str> = st
                    .mods
                    .iter()
                    .filter(|m| m.enabled && m.requires.iter().any(|r| r.mod_id == nid && !r.optional))
                    .map(|m| m.id.as_str())
                    .collect();
                if !users.is_empty() {
                    eprintln!("  warning: still required by {}", users.join(", "));
                }
            }
        } else {
            for r in st.mods[i].requires.iter().filter(|r| !r.optional && !st.mods[i].ignored_reqs.contains(&r.mod_id)) {
                match st.by_nexus(r.mod_id) {
                    None => eprintln!("  warning: requires {} {} (not installed)", r.mod_id, r.name),
                    Some(d) if !d.enabled => eprintln!("  warning: requires {} which is disabled", d.id),
                    _ => {}
                }
            }
        }
    }
    st.save(paths)?;
    warn_if_running(paths);
    Ok(())
}

fn show_conflicts(paths: &Paths, st: &State, only: Option<&str>, hash: bool, base: bool) -> Result<()> {
    let only = only.map(|k| st.get(k).map(|i| st.mods[i].id.clone())).transpose()?;
    let fc = conflicts::file_conflicts(paths, &st.mods, hash);
    let mut real = 0;
    let mut same = 0;
    println!("== file overwrites (last listed wins)");
    for c in &fc {
        if let Some(o) = &only
            && !c.mods.contains(o)
        {
            continue;
        }
        if c.mods.len() > 1 {
            if c.identical {
                same += 1;
                continue;
            }
            real += 1;
            println!("  {}  {}", c.path, c.mods.join(" < "));
        }
    }
    println!("  {real} differing, {same} identical copies (harmless)");
    let based: Vec<&conflicts::FileConflict> =
        fc.iter().filter(|c| c.overrides_base && only.as_ref().is_none_or(|o| c.mods.contains(o))).collect();
    if base {
        println!("== overriding base-game files");
        for c in &based {
            println!("  {}  {}", c.path, c.mods.join(" < "));
        }
    } else {
        println!("  {} files override base-game files (--base to list)", based.len());
    }
    println!("== redscript");
    let sc = conflicts::redscript_conflicts(paths, &st.mods, &unmanaged_reds(paths, st));
    let mut n = 0;
    for c in sc {
        if let Some(o) = &only
            && !c.mods.iter().any(|m| m.ends_with(o.as_str()))
        {
            continue;
        }
        n += 1;
        println!("  {} {}: {}", c.kind, c.target, c.mods.join(", "));
    }
    if n == 0 {
        println!("  none");
    }
    Ok(())
}

fn unmanaged_reds(paths: &Paths, st: &State) -> Vec<(String, String)> {
    let owned: BTreeSet<String> = conflicts::ownership(paths, &st.mods, false).into_keys().collect();
    let root = paths.game.join("r6/scripts");
    files::list_tree(&root)
        .into_iter()
        .filter(|f| f.to_lowercase().ends_with(".reds"))
        .map(|f| format!("r6/scripts/{f}"))
        .filter(|f| !owned.contains(&f.to_lowercase()))
        .map(|f| (f.split('/').nth(2).unwrap_or("?").to_string(), f))
        .collect()
}

pub struct InstallOpts {
    file: Option<String>,
    id: Option<String>,
    name: Option<String>,
    root: Option<String>,
    dest: Option<String>,
    with_deps: bool,
    ignore_deps: bool,
    disabled: bool,
    keep_archive: bool,
    /// key/expires from an nxm:// link (lets non-premium accounts download)
    nxm: Option<(String, String)>,
}

fn pick_file(nx: &Nexus, info: &nexus::ModInfo, sel: Option<&str>) -> Result<nexus::Version> {
    let files = nx.mod_files(&info.id)?;
    let mut cands = vec![];
    for f in files.iter().filter(|f| f.is_active) {
        if let Some(v) = nx.versions(&f.id)?.into_iter().next() {
            cands.push(v);
        }
    }
    if let Some(s) = sel {
        if let Some(c) = cands.iter().find(|c| c.file.id == s || c.game_scoped_id == s) {
            return Ok(c.clone());
        }
        if s.chars().all(|c| c.is_ascii_digit()) {
            // an older upload, addressed by its game-scoped version id
            let v = nx.version_by_gsid(s)?;
            if !files.iter().any(|f| f.id == v.file.id) {
                bail!("version {s} does not belong to {}", info.name);
            }
            return Ok(v);
        }
        let ls = s.to_lowercase();
        let hits: Vec<&nexus::Version> = cands
            .iter()
            .filter(|c| c.file.id == s || c.file.name.to_lowercase().contains(&ls) || c.name.to_lowercase().contains(&ls))
            .collect();
        if hits.len() == 1 {
            return Ok(hits[0].clone());
        }
        bail!("--file {s:?} matched {} files", hits.len());
    }
    let mains: Vec<&nexus::Version> = cands.iter().filter(|c| c.category == "main").collect();
    let primary = info.primary_file_version.as_ref().map(|p| p.id.clone()).unwrap_or_default();
    if mains.len() == 1 {
        return Ok(mains[0].clone());
    }
    if let Some(p) = cands.iter().find(|c| c.id == primary)
        && (mains.is_empty() || mains.iter().any(|m| m.id == p.id))
    {
        return Ok(p.clone());
    }
    let mut msg = format!("{} has several files; choose one with --file <gsid>:\n", info.name);
    for c in &cands {
        msg += &format!(
            "  {:>7}  {:<9} v{:<10} {}  {}\n",
            c.game_scoped_id,
            c.category,
            c.version,
            c.uploaded_at.get(..10).unwrap_or(""),
            c.file.name
        );
    }
    bail!(msg)
}

fn to_reqs(rs: &[nexus::Req]) -> Vec<Requirement> {
    rs.iter()
        .filter(|r| !r.external && r.mod_id != 0)
        .map(|r| Requirement { mod_id: r.mod_id, name: r.name.clone(), notes: r.notes.clone(), optional: r.optional() })
        .collect()
}

/// Everything a Nexus mod requires: its mod page's list (GraphQL) plus the file-level
/// dependencies of the installed file version (v3), if known. Returns the external
/// (non-Nexus) requirements for display, and the list to store on the mod.
fn fetch_requirements(
    nx: &Nexus,
    gid: u64,
    version_id: &str,
    force_page: bool,
    force_files: bool,
) -> Result<(Vec<nexus::Req>, Vec<Requirement>)> {
    let reqs = nx.requirements(&[gid], force_page)?.into_iter().next().map(|x| x.2).unwrap_or_default();
    let mut stored = to_reqs(&reqs);
    if !version_id.is_empty() {
        for (dgid, dname, minv) in nx.file_deps(version_id, force_files).unwrap_or_default() {
            if dgid != 0 && dgid != gid && !stored.iter().any(|r| r.mod_id == dgid) {
                stored.push(Requirement {
                    mod_id: dgid,
                    name: dname,
                    notes: format!("file dependency, min version {minv}"),
                    optional: false,
                });
            }
        }
    }
    Ok((reqs.into_iter().filter(|r| r.external).collect(), stored))
}

fn req_notes(r: &Requirement) -> String {
    if r.notes.is_empty() { String::new() } else { format!(" — {}", r.notes) }
}

/// Check a mod's requirements at install time. Missing hard requirements are installed
/// (--with-deps), refused (default), or, with --ignore-deps (updates), reported with the fix.
/// Requirements the user chose to do without for this mod (`ignored`) are skipped.
#[allow(clippy::too_many_arguments)]
fn resolve_requirements(
    paths: &Paths,
    st: &mut State,
    nx: &Nexus,
    label: &str,
    stored: &[Requirement],
    ignored: &[u64],
    o: &InstallOpts,
    depth: usize,
    visiting: &mut BTreeSet<u64>,
) -> Result<()> {
    let ind = "  ".repeat(depth);
    let mut missing = vec![];
    for r in stored {
        if ignored.contains(&r.mod_id) && st.by_nexus(r.mod_id).is_none_or(|d| !d.enabled) {
            println!("{ind}  requires {} {}: not installed, ignored by your choice", r.mod_id, r.name);
            continue;
        }
        match st.by_nexus(r.mod_id) {
            Some(m) if m.enabled => println!("{ind}  requires {} {}: installed", r.mod_id, r.name),
            Some(m) => println!("{ind}  requires {} {}: installed but DISABLED ({}; cpmm enable {})", r.mod_id, r.name, m.id, m.id),
            None if r.optional => println!("{ind}  optional {} {}: not installed{}", r.mod_id, r.name, req_notes(r)),
            None if visiting.contains(&r.mod_id) => {}
            None => missing.push(r.clone()),
        }
    }
    if missing.is_empty() {
        return Ok(());
    }
    if o.with_deps {
        for r in &missing {
            println!("{ind}  installing requirement {} {}{}", r.mod_id, r.name, req_notes(r));
            install_nexus(paths, st, nx, r.mod_id, o, depth + 1, visiting)?;
        }
    } else if o.ignore_deps {
        for r in &missing {
            println!("{ind}  WARNING: requires {} {}, which is NOT installed{}", r.mod_id, r.name, req_notes(r));
            println!("{ind}           `cpmm deps --fix` installs it; `cpmm deps --ignore {label} {}` accepts it", r.mod_id);
        }
    } else {
        let mut msg = String::from("missing hard requirements:\n");
        for r in &missing {
            msg += &format!("  {} {}{}\n", r.mod_id, r.name, req_notes(r));
        }
        msg += "re-run with --with-deps to install them, or --ignore-deps";
        bail!(msg);
    }
    Ok(())
}

fn install_nexus(
    paths: &Paths,
    st: &mut State,
    nx: &Nexus,
    gid: u64,
    o: &InstallOpts,
    depth: usize,
    visiting: &mut BTreeSet<u64>,
) -> Result<()> {
    if !visiting.insert(gid) {
        return Ok(());
    }
    let info = nx.mod_info(gid)?;
    let ind = "  ".repeat(depth);
    let ver = pick_file(nx, &info, if depth == 0 { o.file.as_deref() } else { None })?;
    println!("{ind}{} ({gid}): file {:?} v{} [{}]", info.name, ver.file.name, ver.version, ver.category);

    // Requirements of this exact file version: checked now, then stored on the mod.
    let (external, stored) = fetch_requirements(nx, gid, &ver.id, false, false)?;
    for r in &external {
        println!("{ind}  external requirement: {} {}", r.name, r.url);
    }
    let existing = st.by_nexus(gid).map(|m| m.id.clone());
    let ignored = existing.as_ref().and_then(|e| st.find(e)).map(|i| st.mods[i].ignored_reqs.clone()).unwrap_or_default();
    let label = existing.clone().unwrap_or_else(|| slugify(short_name(&info.name)));
    resolve_requirements(paths, st, nx, &label, &stored, &ignored, o, depth, visiting)?;
    let checked = ReqCheck { at: now(), version_id: ver.id.clone() };

    if let Some(e) = &existing {
        let i = st.get(e)?;
        if st.mods[i].nexus.as_ref().is_some_and(|n| n.version_id == ver.id) {
            println!("{ind}  already installed as {e}");
            st.mods[i].requires = stored;
            st.mods[i].reqs_checked = Some(checked);
            st.save(paths)?;
            return Ok(());
        }
    }
    let id =
        if depth == 0 { o.id.clone() } else { None }.or(existing.clone()).unwrap_or_else(|| st.unique_id(&slugify(short_name(&info.name))));
    let manifest_url = ver.manifest_url.clone().context("version has no manifest")?;
    let man = nx.manifest(&manifest_url)?;
    paths.ensure_dirs()?;
    let arc = paths.downloads().join(format!("{}-{}", ver.game_scoped_id, slugify(&ver.file.name)));
    println!("{ind}  downloading {:.1} MB", man.archive.size as f64 / 1048576.0);
    nx.download(&ver, &man, &arc, o.nxm.as_ref().map(|(k, e)| (k.as_str(), e.as_str())))?;

    let x = install::tmp_extract_dir(paths, &id);
    if x.exists() {
        fs::remove_dir_all(&x)?;
    }
    install::extract(&arc, &x)?;
    let extracted = files::list_tree(&x);
    // per-file hash check against the manifest
    let mut bad = 0;
    for mf in man.files.iter().filter(|f| !f.file_path.ends_with('/')) {
        match files::sha256_file(&x.join(&mf.file_path)) {
            Ok(h) if h == mf.file_hashes.sha256 => {}
            _ => bad += 1,
        }
    }
    if bad > 0 {
        bail!("{bad} extracted files don't match the Nexus manifest");
    }
    let (root, dest) = if depth == 0 { (o.root.as_deref(), o.dest.as_deref()) } else { (None, None) };
    let plan = install::plan(&extracted, &id, root, dest)?;
    let staged = install::stage(paths, st, &id, &x, &plan)?;
    let _ = fs::remove_dir_all(&x);
    if !o.keep_archive {
        let _ = fs::remove_file(&arc);
    }
    let src = NexusSource {
        mod_id: gid,
        mod_uid: info.id.clone(),
        file_id: ver.file.id.clone(),
        file_name: ver.file.name.clone(),
        version_id: ver.id.clone(),
        version_gsid: ver.game_scoped_id.clone(),
        uploaded_at: ver.uploaded_at.clone(),
        manifest_url,
    };
    match existing.and_then(|e| st.find(&e)) {
        Some(i) => {
            let m = &mut st.mods[i];
            println!("{ind}  updated {}: {} -> {}", m.id, m.version, ver.version);
            m.version = ver.version.clone();
            m.nexus = Some(src);
            m.requires = stored;
            m.reqs_checked = Some(checked);
            m.installed_at = now();
        }
        None => {
            st.mods.push(Mod {
                id: id.clone(),
                name: o.name.clone().filter(|_| depth == 0).unwrap_or(info.name.clone()),
                enabled: !o.disabled,
                version: ver.version.clone(),
                nexus: Some(src),
                requires: stored,
                reqs_checked: Some(checked),
                hidden: vec![],
                ignored_reqs: vec![],
                installed_at: now(),
                notes: String::new(),
            });
            safety::journal(paths, "install", std::slice::from_ref(&id), &ver.version);
            println!(
                "{ind}  installed {id}: {} files{}",
                staged.len(),
                if plan.skipped.is_empty() { String::new() } else { format!(" ({} docs skipped)", plan.skipped.len()) }
            );
        }
    }
    let rh = manage::reapply_hidden(paths, st, &id)?;
    if rh > 0 {
        println!("{ind}  re-hid {rh} files you hid earlier");
    }
    st.save(paths)?;
    report_new_conflicts(paths, st, &id, &ind);
    Ok(())
}

fn report_new_conflicts(paths: &Paths, st: &State, id: &str, ind: &str) {
    let fc = conflicts::file_conflicts(paths, &st.mods, true);
    let mine: Vec<&conflicts::FileConflict> =
        fc.iter().filter(|c| c.mods.len() > 1 && !c.identical && c.mods.iter().any(|m| m == id)).collect();
    for c in mine.iter().take(15) {
        println!("{ind}  conflict: {}  {}", c.path, c.mods.join(" < "));
    }
    if mine.len() > 15 {
        println!("{ind}  … {} conflicting files in total (cpmm conflicts {id})", mine.len());
    }
    // shadowed by runtime writes in the overlay upper dir
    let up = paths.upper();
    let shadow: Vec<String> = files::list_tree(&paths.mod_files(id)).into_iter().filter(|f| up.join(f).exists()).collect();
    if !shadow.is_empty() {
        println!("{ind}  note: {} of its files are shadowed by game-written copies in overwrite/ (cpmm overwrite)", shadow.len());
    }
}

fn install_local(paths: &Paths, st: &mut State, arc: &std::path::Path, nid: Option<u64>, o: &InstallOpts) -> Result<()> {
    let stem = arc.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or("mod".into());
    let name = o.name.clone().unwrap_or(stem.clone());
    let existing =
        o.id.clone()
            .map(|i| slugify(&i))
            .filter(|i| st.find(i).is_some())
            .or_else(|| nid.and_then(|g| st.by_nexus(g)).map(|m| m.id.clone()));
    let id = existing.clone().unwrap_or_else(|| o.id.clone().map(|i| slugify(&i)).unwrap_or_else(|| st.unique_id(&slugify(&name))));
    // Tagged with a Nexus mod: check its requirements before anything is staged.
    let mut checked = None;
    if let Some(gid) = nid {
        let nx = nexus(paths);
        let (external, stored) = fetch_requirements(&nx, gid, "", false, false)?;
        for r in &external {
            println!("  external requirement: {} {}", r.name, r.url);
        }
        let ignored = existing.as_ref().and_then(|e| st.find(e)).map(|i| st.mods[i].ignored_reqs.clone()).unwrap_or_default();
        let mut visiting = BTreeSet::from([gid]);
        resolve_requirements(paths, st, &nx, &id, &stored, &ignored, o, 0, &mut visiting)?;
        checked = Some((stored, ReqCheck { at: now(), version_id: String::new() }));
    }
    paths.ensure_dirs()?;
    let x = install::tmp_extract_dir(paths, &id);
    if x.exists() {
        fs::remove_dir_all(&x)?;
    }
    if arc.is_dir() {
        for f in files::list_tree(arc) {
            files::reflink_or_copy(&arc.join(&f), &x.join(&f))?;
        }
    } else {
        install::extract(arc, &x)?;
    }
    let plan = install::plan(&files::list_tree(&x), &id, o.root.as_deref(), o.dest.as_deref())?;
    let staged = install::stage(paths, st, &id, &x, &plan)?;
    let _ = fs::remove_dir_all(&x);
    let (requires, reqs_checked) = match checked {
        Some((r, c)) => (r, Some(c)),
        None => (vec![], None),
    };
    match st.find(&id) {
        Some(i) => {
            st.mods[i].installed_at = now();
            if reqs_checked.is_some() {
                st.mods[i].requires = requires;
                st.mods[i].reqs_checked = reqs_checked;
            }
            println!("replaced files of {id}: {} files", staged.len());
        }
        None => {
            st.mods.push(Mod {
                id: id.clone(),
                name,
                enabled: !o.disabled,
                version: String::new(),
                nexus: nid.map(|g| NexusSource { mod_id: g, ..Default::default() }),
                requires,
                reqs_checked,
                hidden: vec![],
                ignored_reqs: vec![],
                installed_at: now(),
                notes: format!("from {}", arc.display()),
            });
            println!("installed {id}: {} files", staged.len());
        }
    }
    manage::reapply_hidden(paths, st, &id)?;
    st.save(paths)?;
    report_new_conflicts(paths, st, &id, "");
    Ok(())
}

struct UpdateStatus {
    id: String,
    status: String,
    newer: Option<nexus::Version>,
}

fn check_updates(paths: &Paths, st: &State, refresh: bool) -> Vec<UpdateStatus> {
    let mut nx = nexus(paths);
    if refresh {
        nx.ttl = std::time::Duration::ZERO;
    }
    let nx = &nx;
    let work: Vec<&Mod> = st.mods.iter().filter(|m| m.nexus.is_some()).collect();
    let mut out: Vec<UpdateStatus> = vec![];
    std::thread::scope(|s| {
        let hs: Vec<_> = work
            .chunks(work.len().div_ceil(6).max(1))
            .map(|chunk| {
                s.spawn(move || {
                    chunk
                        .iter()
                        .map(|m| {
                            let n = m.nexus.as_ref().unwrap();
                            let r: Result<(String, Option<nexus::Version>)> = (|| {
                                let info = nx.mod_info(n.mod_id)?;
                                if info.status != "published" && !info.status.is_empty() {
                                    return Ok((format!("mod page status: {}", info.status), None));
                                }
                                if n.file_id.is_empty() {
                                    return Ok((format!("file unknown; page version {}", info.version), None));
                                }
                                let vs = nx.versions(&n.file_id)?;
                                let Some(newest) = vs.first() else { return Ok(("no versions".into(), None)) };
                                let ours =
                                    vs.iter().find(|v| v.id == n.version_id || (n.version_id.is_empty() && v.uploaded_at == n.uploaded_at));
                                if ours.is_some_and(|o| o.id == newest.id) {
                                    if newest.category == "archived" {
                                        return Ok((
                                            format!(
                                                "file archived by author; page version {} — check `cpmm install {} --file`",
                                                info.version, n.mod_id
                                            ),
                                            None,
                                        ));
                                    }
                                    return Ok(("up to date".into(), None));
                                }
                                Ok((
                                    format!("UPDATE {} -> {} ({})", m.version, newest.version, newest.uploaded_at.get(..10).unwrap_or("")),
                                    Some(newest.clone()),
                                ))
                            })();
                            match r {
                                Ok((s, v)) => UpdateStatus { id: m.id.clone(), status: s, newer: v },
                                Err(e) => UpdateStatus { id: m.id.clone(), status: format!("error: {e}"), newer: None },
                            }
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        for h in hs {
            out.extend(h.join().unwrap());
        }
    });
    out
}

fn updates(paths: &Paths, st: &State, refresh: bool) -> Result<()> {
    let res = check_updates(paths, st, refresh);
    let mut n = 0;
    for u in &res {
        if u.status != "up to date" {
            n += u.newer.is_some() as usize;
            println!("  {:<34} {}", u.id, u.status);
        }
    }
    let manual = st.mods.iter().filter(|m| m.nexus.is_none()).count();
    println!(
        "{} checked, {n} updates, {} up to date, {manual} manual mods not checked",
        res.len(),
        res.iter().filter(|u| u.status == "up to date").count()
    );
    Ok(())
}

fn update(paths: &Paths, st: &mut State, keys: &[String], all: bool) -> Result<()> {
    let wanted: Vec<String> = if all {
        check_updates(paths, st, false).into_iter().filter(|u| u.newer.is_some()).map(|u| u.id).collect()
    } else {
        keys.iter().map(|k| st.get(k).map(|i| st.mods[i].id.clone())).collect::<Result<_>>()?
    };
    if wanted.is_empty() {
        println!("nothing to update");
        return Ok(());
    }
    if vfs::running(paths).is_some() {
        bail!("game session running; quit first");
    }
    let _ = safety::snapshot_saves(paths, "before updating mods");
    let batch = format!("update-{}", safety::now_unix());
    if let Ok(mut b) = safety::BATCH.lock() {
        *b = Some(batch.clone());
    }
    safety::journal(paths, "update", &wanted, &batch);
    let nx = nexus(paths);
    struct ClearBatch;
    impl Drop for ClearBatch {
        fn drop(&mut self) {
            if let Ok(mut b) = safety::BATCH.lock() {
                *b = None;
            }
        }
    }
    let _clear = ClearBatch;
    for id in wanted.clone() {
        let m = &st.mods[st.get(&id)?];
        let Some(n) = m.nexus.clone() else {
            println!("{id}: not from Nexus, skipping");
            continue;
        };
        if n.file_id.is_empty() {
            println!("{id}: installed file unknown; use `cpmm install {} --file …`", n.mod_id);
            continue;
        }
        let newest = nx.versions(&n.file_id)?.into_iter().next().context("no versions")?;
        let o = InstallOpts {
            file: Some(newest.game_scoped_id.clone()),
            id: Some(id.clone()),
            name: None,
            root: None,
            dest: None,
            with_deps: false,
            ignore_deps: true,
            disabled: false,
            keep_archive: false,
            nxm: None,
        };
        let mut visiting = BTreeSet::new();
        if let Err(e) = install_nexus(paths, st, &nx, n.mod_id, &o, 0, &mut visiting) {
            println!("{id}: {e:#}");
        }
    }
    Ok(())
}

/// Installed mods whose stored requirements need fetching: never checked, checked for a
/// different installed version, or everything with --refresh.
fn stale_requirements(st: &State, refresh: bool) -> Vec<usize> {
    st.mods
        .iter()
        .enumerate()
        .filter_map(|(i, m)| {
            let n = m.nexus.as_ref()?;
            (refresh || m.reqs_checked.as_ref().is_none_or(|c| c.version_id != n.version_id)).then_some(i)
        })
        .collect()
}

fn deps(paths: &Paths, st: &mut State, refresh: bool) -> Result<()> {
    // Requirements stored before check stamps existed were fetched at install: keep them.
    let mut migrated = false;
    for m in st.mods.iter_mut() {
        if let (None, Some(n)) = (&m.reqs_checked, &m.nexus)
            && !m.requires.is_empty()
        {
            m.reqs_checked = Some(ReqCheck { at: m.installed_at.clone(), version_id: n.version_id.clone() });
            migrated = true;
        }
    }
    let stale = stale_requirements(st, refresh);
    let total = st.mods.iter().filter(|m| m.nexus.is_some()).count();
    if !stale.is_empty() {
        let nx = nexus(paths);
        // one batched GraphQL request for all mod pages; per-mod lookups below hit its cache
        let ids: Vec<u64> = stale.iter().filter_map(|&i| st.mods[i].nexus.as_ref().map(|n| n.mod_id)).collect();
        nx.requirements(&ids, refresh)?;
        for &i in &stale {
            let n = st.mods[i].nexus.clone().unwrap();
            match fetch_requirements(&nx, n.mod_id, &n.version_id, false, refresh) {
                Ok((_, stored)) => {
                    st.mods[i].requires = stored;
                    st.mods[i].reqs_checked = Some(ReqCheck { at: now(), version_id: n.version_id.clone() });
                }
                Err(e) => println!("  {}: couldn't check requirements: {e:#}", st.mods[i].id),
            }
        }
        st.save(paths)?;
        println!("checked requirements of {} mod(s) on Nexus; {} cached", stale.len(), total - stale.len());
    } else {
        if migrated {
            st.save(paths)?;
        }
        println!("requirements of all {total} Nexus mods are cached (re-check with --refresh)");
    }
    let mut problems = 0;
    let mut ignored = 0;
    for m in st.mods.iter().filter(|m| m.enabled) {
        for r in &m.requires {
            if m.ignored_reqs.contains(&r.mod_id) && st.by_nexus(r.mod_id).is_none_or(|d| !d.enabled) {
                ignored += 1;
                continue;
            }
            let s = match st.by_nexus(r.mod_id) {
                Some(d) if d.enabled => continue,
                Some(d) => format!("DISABLED ({})", d.id),
                None => "MISSING".into(),
            };
            if r.optional {
                println!("  {:<34} optional {} {} {s}", m.id, r.mod_id, r.name);
            } else {
                problems += 1;
                println!(
                    "  {:<34} requires {} {} {s}{}",
                    m.id,
                    r.mod_id,
                    r.name,
                    if r.notes.is_empty() { String::new() } else { format!(" — {}", r.notes) }
                );
            }
        }
    }
    println!("{problems} unmet hard requirements{}", if ignored > 0 { format!(" ({ignored} ignored by choice)") } else { String::new() });
    Ok(())
}

fn deps_fix(paths: &Paths, st: &mut State) -> Result<()> {
    let mut missing = BTreeSet::new();
    let mut to_enable = BTreeSet::new();
    for m in st.mods.iter().filter(|m| m.enabled) {
        for r in m.requires.iter().filter(|r| !r.optional && !m.ignored_reqs.contains(&r.mod_id)) {
            match st.by_nexus(r.mod_id) {
                None => {
                    missing.insert(r.mod_id);
                }
                Some(d) if !d.enabled => {
                    to_enable.insert(d.id.clone());
                }
                _ => {}
            }
        }
    }
    for id in to_enable {
        let i = st.get(&id)?;
        st.mods[i].enabled = true;
        println!("enabled {id}");
    }
    st.save(paths)?;
    if !missing.is_empty() {
        manage::ensure_idle(paths)?;
        let nx = nexus(paths);
        let o = InstallOpts {
            file: None,
            id: None,
            name: None,
            root: None,
            dest: None,
            with_deps: true,
            ignore_deps: false,
            disabled: false,
            keep_archive: false,
            nxm: None,
        };
        let mut visiting = BTreeSet::new();
        for gid in missing {
            if let Err(e) = install_nexus(paths, st, &nx, gid, &o, 0, &mut visiting) {
                println!("  {gid}: {e:#}");
            }
        }
    }
    Ok(())
}

fn verify(paths: &Paths, st: &State, keys: &[String]) -> Result<()> {
    let nx = nexus(paths);
    let sel: Vec<usize> =
        if keys.is_empty() { (0..st.mods.len()).collect() } else { keys.iter().map(|k| st.get(k)).collect::<Result<_>>()? };
    for i in sel {
        let m = &st.mods[i];
        let Some(url) = m.nexus.as_ref().map(|n| n.manifest_url.clone()).filter(|u| !u.is_empty()) else {
            println!("  {:<34} no manifest (manual mod)", m.id);
            continue;
        };
        let man = nx.manifest(&url)?;
        let by_suffix: Vec<(String, String)> =
            man.files.iter().map(|f| (f.file_path.to_lowercase(), f.file_hashes.sha256.clone())).collect();
        let root = paths.mod_files(&m.id);
        let (mut ok, mut modified, mut extra) = (0, vec![], 0);
        for f in files::list_tree(&root) {
            let lf = f.to_lowercase();
            match by_suffix.iter().find(|(p, _)| *p == lf || p.ends_with(&format!("/{lf}")) || lf.ends_with(&format!("/{p}"))) {
                Some((_, h)) => {
                    if files::sha256_file(&root.join(&f))? == *h {
                        ok += 1
                    } else {
                        modified.push(f)
                    }
                }
                None => extra += 1,
            }
        }
        println!("  {:<34} {ok} ok, {} modified, {extra} not in manifest", m.id, modified.len());
        for f in modified.iter().take(5) {
            println!("      modified: {f}");
        }
    }
    Ok(())
}

fn unmanaged(paths: &Paths, st: &State, modified: bool, flat: bool) -> Result<()> {
    if vfs::running(paths).is_some() {
        eprintln!("note: session running; this shows the real base dir, not the overlay");
    }
    let van = vanilla::load(paths);
    if van.is_empty() {
        bail!("no Steam depot manifest cached; can't tell vanilla files apart");
    }
    let owned: BTreeSet<String> = conflicts::ownership(paths, &st.mods, false).into_keys().collect();
    let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut edited = vec![];
    for f in files::list_tree(&paths.game) {
        let lf = f.to_lowercase();
        if let Some(v) = van.get(&lf) {
            if modified {
                let p = paths.game.join(&f);
                let size = fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
                let changed = size != v.size || (size < 64 << 20 && vanilla::sha1_file(&p).is_some_and(|h| h != v.sha1));
                if changed {
                    edited.push(f);
                }
            }
            continue;
        }
        if owned.contains(&lf) || vanilla::protected_prefix(&lf) {
            continue;
        }
        let key: String = f
            .split('/')
            .take(if lf.starts_with("bin/x64/plugins/cyber_engine_tweaks/mods/") { 6 } else { 3 })
            .collect::<Vec<_>>()
            .join("/");
        groups.entry(key).or_default().push(f);
    }
    println!("== unmanaged non-vanilla files{}", if flat { "" } else { " (by folder)" });
    if flat {
        for f in groups.values().flatten() {
            println!("  {f}");
        }
    }
    for (k, v) in groups.iter().filter(|_| !flat) {
        if v.len() == 1 && v[0] == *k {
            println!("  {k}");
        } else {
            println!("  {k}/  ({} files)", v.len());
        }
    }
    if modified {
        println!("== vanilla files that differ from Steam's copy");
        for f in edited {
            println!("  {f}");
        }
    }
    Ok(())
}

fn overwrite(paths: &Paths, st: &State) -> Result<()> {
    use std::os::unix::fs::FileTypeExt;
    let up = paths.upper();
    let own = conflicts::ownership(paths, &st.mods, true);
    let mut n = 0;
    for e in walkdir::WalkDir::new(&up).into_iter().flatten().filter(|e| !e.file_type().is_dir()) {
        let rel = e.path().strip_prefix(&up)?.to_string_lossy().to_string();
        let what = if e.file_type().is_char_device() {
            "deleted"
        } else if own.contains_key(&rel.to_lowercase()) {
            "edited mod file"
        } else if paths.game.join(&rel).exists() {
            "edited base file"
        } else {
            "new"
        };
        let by = own.get(&rel.to_lowercase()).and_then(|(_, o)| o.last()).map(|s| format!(" ({s})")).unwrap_or_default();
        println!("  {what:<16} {rel}{by}");
        n += 1;
    }
    println!("{n} files in {}", up.display());
    Ok(())
}

fn eject(paths: &Paths, st: &State, apply: bool) -> Result<()> {
    if vfs::running(paths).is_some() {
        bail!("game session running; quit first");
    }
    let own = conflicts::ownership(paths, &st.mods, true);
    let mut n = 0;
    for (rel, owners) in own.values() {
        let winner = owners.last().unwrap();
        if apply {
            let dst = paths.game.join(rel);
            if dst.exists() {
                fs::remove_file(&dst)?;
            }
            files::reflink_or_copy(&paths.mod_files(winner).join(rel), &dst)?;
        }
        n += 1;
    }
    let up = files::list_tree(&paths.upper());
    if apply {
        for rel in &up {
            let src = paths.upper().join(rel);
            if fs::symlink_metadata(&src).map(|m| m.file_type().is_file()).unwrap_or(false) {
                let dst = paths.game.join(rel);
                if dst.exists() {
                    fs::remove_file(&dst)?;
                }
                files::reflink_or_copy(&src, &dst)?;
            }
        }
    }
    println!(
        "{} {n} mod files and {} overwrite files into {}",
        if apply { "copied" } else { "would copy" },
        up.len(),
        paths.game.display()
    );
    if apply {
        println!("game dir is now a classic manual install; remove `cpmm run --` from the launch options to keep it that way");
    }
    Ok(())
}
