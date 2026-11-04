//! cpmm: Cyberpunk 2077 mod manager for Linux/Proton.

mod api;
mod archive;
mod collections;
mod conflicts;
mod downloads;
mod files;
mod fomod;
mod health;
mod import;
mod install;
mod loadorder;
mod manage;
mod modio;
mod nexus;
mod profiles;
mod requirements;
mod safety;
mod settings;
mod state;
mod steam;
mod vanilla;
mod vfs;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use nexus::Nexus;
use requirements::Status;
use state::{InstalledCollection, Mod, NexusSource, Paths, ReqCheck, Requirement, State, now, short_name, slugify};
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
        /// FOMOD choices as JSON: {"Step/Group": ["Option", …]} (see `cpmm api fomod <source>`)
        #[arg(long)]
        fomod_choices: Option<String>,
        /// Take the FOMOD installer's suggested options without asking
        #[arg(long)]
        fomod_defaults: bool,
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
    /// The mod's Nexus changelog; entries newer than the installed version first.
    Changelog {
        r#mod: String,
        /// Show every version, not only the ones newer than what's installed
        #[arg(long)]
        all: bool,
    },
    /// mod.io subscriptions: conflicts with cpmm's mods and changes since you last reviewed them.
    Modio {
        /// Mark the current subscriptions as reviewed
        #[arg(long)]
        ack: bool,
    },
    /// Nexus collections: `show <slug|url>`, `install <slug|url>`, `list`.
    Collection {
        #[command(subcommand)]
        action: CollectionCmd,
    },
    /// Show or change settings: `cpmm settings`, `set <key> <value>`, `reset [key]`.
    Settings {
        #[command(subcommand)]
        action: Option<SettingsCmd>,
    },
    /// Download a mod into the Downloads list without installing it.
    Download {
        /// Nexus mod id or URL
        source: String,
        /// Nexus file: file id, version id, or name fragment (default: the main file)
        #[arg(long)]
        file: Option<String>,
    },
    /// The Downloads list: `cpmm downloads`, `install <#|name>`, `remove <#|name>`, `add <path>`.
    Downloads {
        #[command(subcommand)]
        action: Option<DownloadsCmd>,
    },
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
    /// Show the game's Steam launch options with cpmm in them; --apply sets them.
    LaunchOptions {
        #[arg(long)]
        apply: bool,
    },
    /// Nexus API key: save one (validated first), --check it, or --remove it.
    NexusKey {
        key: Option<String>,
        #[arg(long)]
        check: bool,
        #[arg(long)]
        remove: bool,
    },
    /// Interactive first-run setup in the terminal (the GUI has the same flow).
    Setup,
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
enum DownloadsCmd {
    /// Everything downloaded, with install status (default)
    List,
    /// Install a download (number or name from the list)
    Install {
        key: String,
        /// FOMOD choices as JSON (see `cpmm api fomod <archive>`)
        #[arg(long)]
        fomod_choices: Option<String>,
        #[arg(long)]
        fomod_defaults: bool,
        /// Also install missing requirements
        #[arg(long)]
        with_deps: bool,
    },
    /// Delete a download (installed mods are not affected)
    Remove { key: String },
    /// Put an archive from elsewhere into the Downloads list
    Add {
        path: PathBuf,
        /// The Nexus mod it belongs to (enables update checks)
        #[arg(long)]
        nexus: Option<u64>,
        /// Move instead of copy
        #[arg(long, name = "move")]
        move_it: bool,
        /// How it was obtained (shown in the list)
        #[arg(long, default_value = "local")]
        source: String,
        /// Install it right away if "install downloads automatically" is on
        #[arg(long)]
        auto: bool,
    },
}

#[derive(Subcommand)]
enum CollectionCmd {
    /// What a collection contains and what you already have
    Show { reference: String },
    /// Install a collection: exact file versions, the curator's FOMOD choices and load-order rules
    Install {
        reference: String,
        /// Also install the collection's optional mods
        #[arg(long)]
        optional: bool,
        /// Add to the active profile instead of a new profile named after the collection
        #[arg(long)]
        into_current: bool,
        /// Profile to install into (created if missing)
        #[arg(long)]
        profile: Option<String>,
    },
    /// Collections installed through cpmm
    List,
}

#[derive(Subcommand)]
enum SettingsCmd {
    /// Print every setting (default)
    Show {
        #[arg(long)]
        json: bool,
    },
    Set {
        key: String,
        value: String,
    },
    /// Back to the default (all settings if no key)
    Reset {
        key: Option<String>,
    },
}

#[derive(Subcommand)]
enum ApiCmd {
    /// Settings values, their schema and the resolved folders
    Settings,
    /// A Nexus collection's contents with install status
    Collection {
        reference: String,
    },
    /// FOMOD installer options of an archive or Nexus mod (downloads it if needed)
    Fomod {
        source: String,
        #[arg(long)]
        file: Option<String>,
        /// current choices as JSON, to see which later steps become visible
        #[arg(long)]
        choices: Option<String>,
    },
    /// mod.io subscriptions, changes since the last review, conflicts
    Modio,
    /// Nexus changelog of one mod, with entries newer than the installed version marked
    Changelog {
        r#mod: String,
    },
    /// First-run status: game found, Nexus key, launch options, link handler
    Onboarding,
    /// The Downloads list with install status
    Downloads,
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
        | Cmd::LaunchOptions { apply: false }
        | Cmd::NexusKey { key: None, remove: false, .. }
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
        Cmd::Install {
            source,
            file,
            id,
            name,
            root,
            dest,
            with_deps,
            ignore_deps,
            disabled,
            nexus: nid,
            keep_archive,
            fomod_choices,
            fomod_defaults,
        } => {
            let fomod = fomod_choices
                .map(|j| serde_json::from_str::<fomod::Choices>(&j))
                .transpose()
                .context("--fomod-choices must be JSON: {\"Step/Group\": [\"Option\"]}")?;
            let o = InstallOpts {
                file,
                id,
                name,
                root,
                dest,
                with_deps,
                ignore_deps,
                disabled,
                keep_archive,
                nxm: None,
                hooks: true,
                fomod,
                fomod_defaults,
            };
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
            let users: Vec<&str> = requirements::dependents(&st, &st.mods[i]).iter().map(|m| m.id.as_str()).collect();
            if !users.is_empty() && !force {
                bail!("{id} is required by {}; --force to remove anyway", users.join(", "));
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
            safety::auto_snapshot(&paths, &format!("before uninstalling {id}"));
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
                            hooks: true,
                            fomod: None,
                            fomod_defaults: true,
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
                let nx = nexus(&paths);
                // Mod Manager Download = download into the Downloads list; install only if the
                // user asked for that (Settings → Install downloads automatically).
                let arc = match download_nexus(&paths, &nx, l.mod_id, Some(&l.file_gsid), l.key.as_deref().zip(l.expires.as_deref()), "nxm")
                {
                    Ok(p) => p,
                    Err(e) => {
                        manage::notify("Download failed", &format!("{e:#}"));
                        return Err(e);
                    }
                };
                let file = arc.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let d = downloads::find(&paths, &st, &file)?;
                if paths.cfg.auto_install_downloads {
                    match install_download(&paths, &mut st, &d, None, true, true) {
                        Ok(()) => {
                            let name = st.by_nexus(l.mod_id).map(|m| format!("{} {}", m.name, m.version)).unwrap_or_default();
                            manage::notify("Mod installed", &format!("{name}\n(applies on next launch)"));
                        }
                        Err(e) => {
                            manage::notify("Mod install failed", &format!("{e:#}"));
                            return Err(e);
                        }
                    }
                } else {
                    println!("downloaded {} {}; install it from Downloads (cpmm downloads install {file:?})", d.meta.name, d.meta.version);
                    manage::notify(
                        "Mod downloaded",
                        &format!("{} {}\nInstall it from cpmm's Downloads list.", d.meta.name, d.meta.version),
                    );
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
                ApiCmd::Doctor => serde_json::to_value(health::check(&paths, &st))?,
                ApiCmd::Downloads => serde_json::to_value(downloads::list(&paths, &st))?,
                ApiCmd::Onboarding => {
                    let exe = installed_exe().unwrap_or_default();
                    let lo = steam::read_launch_options();
                    serde_json::json!({
                        "game_dir": paths.game,
                        "game_found": paths.game.join("bin/x64/Cyberpunk2077.exe").is_file(),
                        "api_key": paths.api_key().is_ok(),
                        "launch_options": lo,
                        "launch_ok": lo.as_deref().is_some_and(steam::launch_options_ok),
                        "launch_suggested": steam::with_cpmm(lo.as_deref().unwrap_or(""), &exe),
                        "steam_running": steam::is_running(),
                        "steam_cef": steam::cef_available(),
                        "nxm_handler": manage::current_nxm_handler() == "cpmm-nxm.desktop",
                        "onboarded": paths.cfg.onboarded,
                        "mods_installed": st.mods.len(),
                    })
                }
                ApiCmd::Changelog { r#mod } => {
                    let i = st.get(&r#mod)?;
                    let m = &st.mods[i];
                    let gid = m.nexus.as_ref().map(|n| n.mod_id).context("not a Nexus mod")?;
                    let entries = nexus::changelog_entries(nexus(&paths).changelog(gid)?, &m.version);
                    serde_json::json!({"id": m.id, "name": m.name, "installed": m.version, "entries": entries})
                }
                ApiCmd::Modio => serde_json::json!({
                    "mods": modio::list(&paths).into_iter().map(|m| serde_json::json!({"id": m.id, "name": m.name, "author": m.author, "version": m.version, "files": m.files.len()})).collect::<Vec<_>>(),
                    "changes": modio::changes(&paths),
                    "conflicts": modio::conflicts(&paths, &st),
                }),
                ApiCmd::Fomod { source, file, choices } => {
                    let arc = fetch_archive(&paths, &source, file.as_deref())?;
                    let x = paths.tmp().join("fomod-inspect");
                    if x.exists() {
                        fs::remove_dir_all(&x)?;
                    }
                    install::extract(&arc, &x)?;
                    let extracted = files::list_tree(&x);
                    let v = match fomod::detect(&extracted) {
                        None => serde_json::json!({"fomod": false}),
                        Some(cfg) => {
                            let f = fomod::parse(&fs::read(x.join(&cfg))?, &cfg)?;
                            let c: fomod::Choices = choices.map(|j| serde_json::from_str(&j)).transpose()?.unwrap_or_default();
                            let fa = file_active(&paths, &st);
                            let mut d = fomod::describe(&f, &c, &x, &fa);
                            d["fomod"] = true.into();
                            d["archive"] = serde_json::json!(arc);
                            d
                        }
                    };
                    let _ = fs::remove_dir_all(&x);
                    v
                }
                ApiCmd::Collection { reference } => collection_json(&paths, &st, &reference)?,
                ApiCmd::Settings => serde_json::json!({
                    "values": paths.cfg,
                    "schema": settings::schema(),
                    "resolved": {
                        "staging_dir": paths.mods(),
                        "download_dir": paths.downloads(),
                        "data_dir": paths.data,
                        "game_dir": paths.game,
                    },
                    "api_key": paths.api_key().is_ok(),
                }),
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
                let titles: Vec<String> = health::check(&paths, &st)
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
            let fs_: Vec<health::Finding> = health::check(&paths, &st).into_iter().filter(|f| all || f.severity != "acked").collect();
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
            safety::auto_snapshot(&paths, &format!("before rollback of {}", r#mod));
            let msg = safety::rollback(&paths, &mut st, &r#mod, to)?;
            st.save(&paths)?;
            if paths.cfg.auto_sort {
                auto_sort(&paths, &mut st);
            }
            let _ = loadorder::write_modlist(&paths, &st);
            println!("rolled back {msg}");
        }
        Cmd::UndoUpdate { batch } => {
            manage::ensure_idle(&paths)?;
            let b = match batch {
                Some(b) => b,
                None => safety::batches(&paths, &st).into_iter().next().map(|b| b.batch).context("no update run to undo")?,
            };
            safety::auto_snapshot(&paths, &format!("before undoing {b}"));
            for m in safety::undo_batch(&paths, &mut st, &b)? {
                println!("  rolled back {m}");
            }
            st.save(&paths)?;
            if paths.cfg.auto_sort {
                auto_sort(&paths, &mut st);
            }
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
        Cmd::Changelog { r#mod, all } => {
            let i = st.get(&r#mod)?;
            let m = &st.mods[i];
            let gid = m.nexus.as_ref().map(|n| n.mod_id).context("not a Nexus mod")?;
            let entries = nexus::changelog_entries(nexus(&paths).changelog(gid)?, &m.version);
            if entries.is_empty() {
                println!("{} has no changelog on Nexus", m.name);
            }
            for (shown, e) in entries.iter().enumerate() {
                if !all && !e.newer && shown > 0 {
                    break;
                }
                let tag = if e.installed {
                    "  (installed)"
                } else if e.newer {
                    "  (new)"
                } else {
                    ""
                };
                println!("{}{tag}", e.version);
                for l in &e.changes {
                    println!("  - {l}");
                }
                if !all && !e.newer {
                    break; // the installed/older entry gives context; stop there
                }
            }
        }
        Cmd::Modio { ack } => {
            if ack {
                println!("marked {} mod.io subscriptions as reviewed", modio::ack(&paths)?);
                return Ok(());
            }
            let mods = modio::list(&paths);
            if mods.is_empty() {
                println!("no mod.io subscriptions (the in-game Mods menu manages them)");
            }
            let cs = modio::conflicts(&paths, &st);
            for m in &mods {
                let mine: Vec<&modio::Conflict> = cs.iter().filter(|c| c.modio == m.name).collect();
                println!(
                    "  {:<10} {} {} by {}{}",
                    m.id,
                    m.name,
                    m.version,
                    m.author,
                    if mine.is_empty() { String::new() } else { format!("  [{} conflicts]", mine.len()) }
                );
                for c in mine {
                    println!("      {}{} with {}: {}", if c.severe { "! " } else { "" }, c.kind, c.with, c.detail);
                }
            }
            let ch = modio::changes(&paths);
            if ch.first_look && !mods.is_empty() {
                println!("first look: `cpmm modio --ack` marks these as reviewed; cpmm then reports new subscriptions");
            } else {
                for a in &ch.added {
                    println!("  NEW since last review: {a}");
                }
                for u in &ch.updated {
                    println!("  updated: {u}");
                }
                for r in &ch.removed {
                    println!("  unsubscribed: {r}");
                }
            }
        }
        Cmd::Collection { action } => match action {
            CollectionCmd::Show { reference } => {
                let v = collection_json(&paths, &st, &reference)?;
                println!("{} (revision {}) by {}", v["name"].as_str().unwrap_or(""), v["revision"], v["author"].as_str().unwrap_or(""));
                println!(
                    "{} mods ({} optional), {:.1} GB, {} load-order rules",
                    v["mods"].as_array().map_or(0, |a| a.len()),
                    v["optional"],
                    v["total_size"].as_f64().unwrap_or(0.0) / 1e9,
                    v["rules"]
                );
                let mut by: BTreeMap<String, usize> = BTreeMap::new();
                for m in v["mods"].as_array().cloned().unwrap_or_default() {
                    *by.entry(m["status"].as_str().unwrap_or("").to_string()).or_default() += 1;
                }
                println!("status: {}", by.iter().map(|(k, n)| format!("{n} {k}")).collect::<Vec<_>>().join(", "));
                println!("install with: cpmm collection install {reference}");
            }
            CollectionCmd::Install { reference, optional, into_current, profile } => {
                collection_install(&paths, &mut st, &reference, optional, into_current, profile)?
            }
            CollectionCmd::List => {
                if st.collections.is_empty() {
                    println!("no collections installed through cpmm");
                }
                for c in &st.collections {
                    println!(
                        "  {}  {} (revision {}), {} mods, profile {:?}, {}",
                        c.slug,
                        c.name,
                        c.revision,
                        c.mods.len(),
                        c.profile,
                        c.installed_at
                    );
                }
            }
        },
        Cmd::Settings { action } => settings_cmd(&paths, &st, action.unwrap_or(SettingsCmd::Show { json: false }))?,
        Cmd::Download { source, file } => {
            let gid = parse_nexus(&source).context("give a Nexus mod id or URL")?;
            let p = download_nexus(&paths, &nexus(&paths), gid, file.as_deref(), None, "nexus")?;
            println!("downloaded to {} (cpmm downloads install … to install it)", p.display());
        }
        Cmd::Downloads { action } => match action.unwrap_or(DownloadsCmd::List) {
            DownloadsCmd::List => {
                let all = downloads::list(&paths, &st);
                if all.is_empty() {
                    println!("no downloads in {}", paths.downloads().display());
                }
                for (i, d) in all.iter().enumerate() {
                    println!(
                        "{:>3}  {:<16} {:<42} {:<10} {:>8.1} MB  {}{}",
                        i + 1,
                        d.status,
                        d.meta.name.chars().take(42).collect::<String>(),
                        d.meta.version.chars().take(10).collect::<String>(),
                        d.size as f64 / 1048576.0,
                        d.meta.source,
                        d.installed_as.as_ref().map(|x| format!("  ({x})")).unwrap_or_default()
                    );
                }
            }
            DownloadsCmd::Install { key, fomod_choices, fomod_defaults, with_deps } => {
                let d = downloads::find(&paths, &st, &key)?;
                let fomod = fomod_choices
                    .map(|j| serde_json::from_str::<fomod::Choices>(&j))
                    .transpose()
                    .context("--fomod-choices must be JSON")?;
                install_download(&paths, &mut st, &d, fomod, fomod_defaults, with_deps)?;
            }
            DownloadsCmd::Remove { key } => {
                let d = downloads::find(&paths, &st, &key)?;
                downloads::remove(&d)?;
                println!("deleted {}{}", d.file, d.installed_as.map(|x| format!(" ({x} stays installed)")).unwrap_or_default());
            }
            DownloadsCmd::Add { path, nexus: nid, move_it, source, auto } => {
                let p = downloads::add(&paths, &expand(&path.to_string_lossy()), nid, &source, move_it)?;
                let file = p.strip_prefix(paths.downloads()).unwrap_or(&p).to_string_lossy().to_string();
                println!("added {file} to Downloads");
                if auto && paths.cfg.auto_install_downloads {
                    let d = downloads::find(&paths, &st, &file)?;
                    install_download(&paths, &mut st, &d, None, true, true)?;
                }
            }
        },
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
        Cmd::LaunchOptions { apply } => {
            let exe = installed_exe()?;
            let cur = steam::read_launch_options();
            if apply {
                let new = steam::apply_launch_options(&exe)?;
                println!("launch options set:\n  {new}");
                if steam::is_running() {
                    println!("(applied live in Steam)");
                }
            } else {
                match &cur {
                    Some(c) if steam::launch_options_ok(c) => println!("already launching through cpmm:\n  {c}"),
                    Some(c) => println!("current:  {c}\nwith cpmm: {}\n(`cpmm launch-options --apply` sets it)", steam::with_cpmm(c, &exe)),
                    None => println!(
                        "Cyberpunk isn't in Steam's local config yet; start it once from Steam. Use:\n  {}",
                        steam::with_cpmm("", &exe)
                    ),
                }
            }
        }
        Cmd::NexusKey { key, check, remove } => nexus_key_cmd(&paths, key, check, remove)?,
        Cmd::Setup => setup_cmd(&paths, &mut st)?,
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
            if own_saves || paths.cfg.new_profiles_own_saves {
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
    let mio = modio::list(paths);
    if !mio.is_empty() {
        println!("\nmod.io (managed in-game via the Mods menu, outside the game dir):");
        for m in mio {
            println!("    {:<10} {} {} by {}", m.id, m.name, m.version, m.author);
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
    for c in requirements::of(st, m) {
        let r = c.req;
        let have = match c.status {
            Status::Ok => "ok",
            Status::Disabled | Status::OptionalDisabled => "DISABLED",
            Status::Missing | Status::OptionalMissing => "MISSING",
            Status::Ignored if c.dep.is_some() => "DISABLED",
            Status::Ignored => "MISSING",
        };
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
        safety::auto_snapshot(paths, &format!("before disabling {}", keys.join(", ")));
    }
    safety::journal(paths, if on { "enable" } else { "disable" }, keys, "");
    for k in keys {
        let i = st.get(k)?;
        st.mods[i].enabled = on;
        println!("{} {}", if on { "enabled" } else { "disabled" }, st.mods[i].id);
        if !on {
            let users: Vec<&str> = requirements::dependents(st, &st.mods[i]).iter().map(|m| m.id.as_str()).collect();
            if !users.is_empty() {
                eprintln!("  warning: still required by {}", users.join(", "));
            }
        } else {
            for c in requirements::of(st, &st.mods[i]) {
                match (c.status, c.dep) {
                    (Status::Missing, _) => eprintln!("  warning: requires {} {} (not installed)", c.req.mod_id, c.req.name),
                    (Status::Disabled, Some(d)) => eprintln!("  warning: requires {} which is disabled", d.id),
                    _ => {}
                }
            }
        }
    }
    st.save(paths)?;
    if paths.cfg.auto_sort {
        auto_sort(paths, st);
    }
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
    let sc = conflicts::redscript_conflicts(paths, &st.mods);
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
    /// Run the after-install tasks from settings (off for bulk updates, which sort once at the end)
    hooks: bool,
    /// FOMOD choices; None: the mod's saved choices, else ask (terminal) or the installer's defaults
    fomod: Option<fomod::Choices>,
    fomod_defaults: bool,
}

fn pick_file(nx: &Nexus, info: &nexus::ModInfo, sel: Option<&str>) -> Result<nexus::Version> {
    let files = nx.mod_files(&info.id)?;
    // A numeric selection that is a version id (collections, nxm links): one lookup instead of
    // listing every file's versions.
    if let Some(s) = sel.filter(|s| s.chars().all(|c| c.is_ascii_digit()) && !files.iter().any(|f| f.id == *s))
        && let Ok(v) = nx.version_by_gsid(s)
        && files.iter().any(|f| f.id == v.file.id)
    {
        return Ok(v);
    }
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
    for c in requirements::check(st, stored, ignored) {
        let r = c.req;
        match (c.status, c.dep) {
            (Status::Ignored, _) => println!("{ind}  requires {} {}: not installed, ignored by your choice", r.mod_id, r.name),
            (Status::Ok, _) => println!("{ind}  requires {} {}: installed", r.mod_id, r.name),
            (Status::Disabled | Status::OptionalDisabled, Some(m)) => {
                println!("{ind}  requires {} {}: installed but DISABLED ({}; cpmm enable {})", r.mod_id, r.name, m.id, m.id)
            }
            (Status::OptionalMissing, _) => println!("{ind}  optional {} {}: not installed{}", r.mod_id, r.name, req_notes(r)),
            (Status::Missing, _) if visiting.contains(&r.mod_id) => {}
            (Status::Missing, _) => missing.push(r.clone()),
            (Status::Disabled | Status::OptionalDisabled, None) => unreachable!("a disabled requirement has an installed mod"),
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
    // The same install only if it's the same Nexus *file*: a page can offer several (a car and
    // its tuning-parts FOMOD), and installing one must not replace the other.
    let existing = st
        .mods
        .iter()
        .find(|m| m.nexus.as_ref().is_some_and(|n| n.mod_id == gid && (n.file_id.is_empty() || n.file_id == ver.file.id)))
        .map(|m| m.id.clone());
    let sibling = existing.is_none() && st.by_nexus(gid).is_some();
    let ignored = existing.as_ref().and_then(|e| st.find(e)).map(|i| st.mods[i].ignored_reqs.clone()).unwrap_or_default();
    let label = existing.clone().unwrap_or_else(|| slugify(short_name(&info.name)));
    resolve_requirements(paths, st, nx, &label, &stored, &ignored, o, depth, visiting)?;
    let checked = ReqCheck { at: now(), version_id: ver.id.clone() };

    if let Some(e) = &existing {
        let i = st.get(e)?;
        // explicit FOMOD choices mean "reinstall with these options"
        let reconfigure = depth == 0 && o.fomod.is_some();
        if !reconfigure && st.mods[i].nexus.as_ref().is_some_and(|n| n.version_id == ver.id) {
            println!("{ind}  already installed as {e}");
            st.mods[i].requires = stored;
            st.mods[i].reqs_checked = Some(checked);
            st.save(paths)?;
            return Ok(());
        }
    }
    let id = if depth == 0 { o.id.clone() } else { None }.or(existing.clone()).unwrap_or_else(|| {
        let base = slugify(short_name(&info.name));
        st.unique_id(&if sibling { slugify(&format!("{base} {}", ver.file.name)) } else { base })
    });
    let manifest_url = ver.manifest_url.clone().context("version has no manifest")?;
    let man = nx.manifest(&manifest_url)?;
    paths.ensure_dirs()?;
    let arc = paths.downloads().join(format!("{}-{}", ver.game_scoped_id, slugify(&ver.file.name)));
    if !arc.exists() {
        println!("{ind}  downloading {:.1} MB", man.archive.size as f64 / 1048576.0);
    }
    nx.download(&ver, &man, &arc, o.nxm.as_ref().map(|(k, e)| (k.as_str(), e.as_str())))?;
    record_download(&arc, &info.name, gid, &ver, if o.nxm.is_some() { "nxm" } else { "nexus" });

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
    let saved = st.find(&id).and_then(|i| st.mods[i].fomod.clone()).and_then(|v| serde_json::from_value(v).ok());
    let (plan, fomod_choices) = match fomod::detect(&extracted) {
        Some(cfg) if root.is_none() && dest.is_none() => {
            let (p, c) = fomod_plan(paths, st, &x, &extracted, &cfg, o, saved, &ind)?;
            (p, Some(c))
        }
        _ => (install::plan(&extracted, &id, root, dest)?, None),
    };
    let staged = install::stage(paths, st, &id, &x, &plan)?;
    let _ = fs::remove_dir_all(&x);
    let archive = if !o.keep_archive && !paths.cfg.keep_archives {
        let _ = fs::remove_file(&arc);
        let _ = fs::remove_file(format!("{}.cpmm.json", arc.display()));
        None
    } else {
        arc.file_name().map(|n| n.to_string_lossy().into_owned())
    };
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
            m.archive = archive.clone();
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
                fomod: None,
                archive: archive.clone(),
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
    if let (Some(c), Some(i)) = (&fomod_choices, st.find(&id)) {
        st.mods[i].fomod = serde_json::to_value(c).ok();
    }
    let rh = manage::reapply_hidden(paths, st, &id)?;
    if rh > 0 {
        println!("{ind}  re-hid {rh} files you hid earlier");
    }
    st.save(paths)?;
    report_new_conflicts(paths, st, &id, &ind);
    if depth == 0 && o.hooks {
        after_install(paths, st, &id);
    }
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
    let extracted = files::list_tree(&x);
    let saved = st.find(&id).and_then(|i| st.mods[i].fomod.clone()).and_then(|v| serde_json::from_value(v).ok());
    let (plan, fomod_choices) = match fomod::detect(&extracted) {
        Some(cfg) if o.root.is_none() && o.dest.is_none() => {
            let (p, c) = fomod_plan(paths, st, &x, &extracted, &cfg, o, saved, "")?;
            (p, Some(c))
        }
        _ => (install::plan(&extracted, &id, o.root.as_deref(), o.dest.as_deref())?, None),
    };
    let staged = install::stage(paths, st, &id, &x, &plan)?;
    let _ = fs::remove_dir_all(&x);
    let (requires, reqs_checked) = match checked {
        Some((r, c)) => (r, Some(c)),
        None => (vec![], None),
    };
    // installed from the Downloads list? remember which download
    let from_library = arc.strip_prefix(paths.downloads()).ok().map(|r| r.to_string_lossy().into_owned());
    match st.find(&id) {
        Some(i) => {
            st.mods[i].installed_at = now();
            st.mods[i].archive = from_library.clone();
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
                fomod: None,
                archive: from_library.clone(),
                installed_at: now(),
                notes: format!("from {}", arc.display()),
            });
            println!("installed {id}: {} files", staged.len());
        }
    }
    if let (Some(c), Some(i)) = (&fomod_choices, st.find(&id)) {
        st.mods[i].fomod = serde_json::to_value(c).ok();
    }
    manage::reapply_hidden(paths, st, &id)?;
    st.save(paths)?;
    report_new_conflicts(paths, st, &id, "");
    if o.hooks {
        after_install(paths, st, &id);
    }
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
    safety::auto_snapshot(paths, "before updating mods");
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
            hooks: false,
            fomod: None,
            fomod_defaults: true,
        };
        let mut visiting = BTreeSet::new();
        if let Err(e) = install_nexus(paths, st, &nx, n.mod_id, &o, 0, &mut visiting) {
            println!("{id}: {e:#}");
        }
    }
    if paths.cfg.auto_sort {
        auto_sort(paths, st);
    }
    Ok(())
}

fn deps(paths: &Paths, st: &mut State, refresh: bool) -> Result<()> {
    let requirements::Stale { mods: stale, migrated } = requirements::stale(st, refresh);
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
        for c in requirements::of(st, m) {
            let r = c.req;
            let s = match (c.status, c.dep) {
                (Status::Ok, _) => continue,
                (Status::Ignored, _) => {
                    ignored += 1;
                    continue;
                }
                (Status::Disabled | Status::OptionalDisabled, Some(d)) => format!("DISABLED ({})", d.id),
                _ => "MISSING".into(),
            };
            if !c.status.is_problem() {
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

/// For FOMOD file conditions: is this game-relative file present (game dir or an enabled mod)?
fn file_active<'a>(paths: &'a Paths, st: &'a State) -> impl Fn(&str) -> bool + 'a {
    let own = conflicts::ownership(paths, &st.mods, true);
    move |f: &str| own.contains_key(&f.to_lowercase()) || paths.game.join(f).exists()
}

/// Plan an install from a FOMOD installer: explicit choices, else the mod's saved choices,
/// else ask in a terminal, else the installer's defaults.
#[allow(clippy::too_many_arguments)]
fn fomod_plan(
    paths: &Paths,
    st: &State,
    x: &std::path::Path,
    extracted: &[String],
    cfg: &str,
    o: &InstallOpts,
    saved: Option<fomod::Choices>,
    ind: &str,
) -> Result<(install::Plan, fomod::Choices)> {
    let f = fomod::parse(&fs::read(x.join(cfg))?, cfg)?;
    let fa = file_active(paths, st);
    let tty = unsafe { libc::isatty(0) } == 1 && unsafe { libc::isatty(1) } == 1;
    let choices = match (&o.fomod, saved) {
        (Some(c), _) => c.clone(),
        (None, Some(s)) => {
            println!("{ind}  FOMOD: using the options you chose last time");
            s
        }
        (None, None) if tty && !o.fomod_defaults => fomod::ask(&f, &fa)?,
        _ => {
            println!("{ind}  FOMOD: using the installer's suggested options");
            fomod::Choices::new()
        }
    };
    // saved choices may name options a newer installer dropped: fall back to defaults
    let r = match fomod::resolve(&f, &choices, &fa) {
        Ok(r) => r,
        Err(e) if o.fomod.is_none() => {
            println!("{ind}  FOMOD: saved options no longer fit ({e:#}); using the suggested ones");
            fomod::resolve(&f, &fomod::Choices::new(), &fa)?
        }
        Err(e) => return Err(e),
    };
    let moves = fomod::moves(&f, &r.ops, extracted)?;
    for (k, v) in &r.choices {
        println!("{ind}  FOMOD {k}: {}", if v.is_empty() { "(none)".into() } else { v.join(", ") });
    }
    Ok((install::Plan { moves, skipped: vec![] }, r.choices))
}

/// The archive for a source: a local path, or the Nexus file downloaded into the cache.
fn fetch_archive(paths: &Paths, source: &str, file: Option<&str>) -> Result<PathBuf> {
    match parse_nexus(source) {
        Some(gid) => download_nexus(paths, &nexus(paths), gid, file, None, "nexus"),
        None => {
            // a name or number from the Downloads list, or a path
            let p = expand(source);
            if p.exists() {
                return Ok(p);
            }
            let st = State::load(paths)?;
            Ok(downloads::find(paths, &st, source)?.path)
        }
    }
}

/// Write the Downloads-list record for a Nexus archive.
fn record_download(arc: &std::path::Path, name: &str, gid: u64, ver: &nexus::Version, source: &str) {
    // keep when and how it first arrived (an nxm download stays "nxm" after installing)
    let prev = downloads::read_meta(arc);
    let keep = prev.as_ref().map(|m| m.downloaded_at.clone()).filter(|d| !d.is_empty());
    let source = prev.map(|m| m.source).filter(|s| !s.is_empty()).unwrap_or_else(|| source.to_string());
    let _ = downloads::write_meta(
        arc,
        &downloads::DlMeta {
            name: name.to_string(),
            mod_id: Some(gid),
            file_id: ver.file.id.clone(),
            version_gsid: ver.game_scoped_id.clone(),
            version: ver.version.clone(),
            file_name: ver.file.name.clone(),
            uploaded_at: ver.uploaded_at.clone(),
            source,
            downloaded_at: keep.unwrap_or_else(now),
        },
    );
}

/// Download one Nexus file into the Downloads list (no install).
fn download_nexus(paths: &Paths, nx: &Nexus, gid: u64, file: Option<&str>, nxm: Option<(&str, &str)>, source: &str) -> Result<PathBuf> {
    let info = nx.mod_info(gid)?;
    let ver = pick_file(nx, &info, file)?;
    let man = nx.manifest(ver.manifest_url.as_deref().context("version has no manifest")?)?;
    paths.ensure_dirs()?;
    let arc = paths.downloads().join(format!("{}-{}", ver.game_scoped_id, slugify(&ver.file.name)));
    if !arc.exists() {
        println!("downloading {} {} ({:.1} MB)", info.name, ver.version, man.archive.size as f64 / 1048576.0);
    }
    nx.download(&ver, &man, &arc, nxm)?;
    record_download(&arc, &info.name, gid, &ver, source);
    Ok(arc)
}

/// Install an entry of the Downloads list. Nexus downloads go through the Nexus install (which
/// reuses the archive on disk and keeps update tracking exact); others install as archives.
fn install_download(
    paths: &Paths,
    st: &mut State,
    d: &downloads::Download,
    fomod: Option<fomod::Choices>,
    fomod_defaults: bool,
    with_deps: bool,
) -> Result<()> {
    let o = InstallOpts {
        file: (!d.meta.version_gsid.is_empty()).then(|| d.meta.version_gsid.clone()),
        id: None,
        name: (!d.meta.name.is_empty()).then(|| d.meta.name.clone()),
        root: None,
        dest: None,
        with_deps,
        ignore_deps: !with_deps,
        disabled: false,
        keep_archive: true,
        nxm: None,
        hooks: true,
        fomod,
        fomod_defaults,
    };
    match (d.meta.mod_id, d.meta.version_gsid.is_empty()) {
        (Some(gid), false) => {
            let mut visiting = BTreeSet::new();
            install_nexus(paths, st, &nexus(paths), gid, &InstallOpts { name: None, ..o }, 0, &mut visiting)
        }
        (gid, _) => install_local(paths, st, &d.path, gid, &o),
    }
}

/// Revision info + manifest of a collection (the archive is cached per revision).
fn fetch_collection(paths: &Paths, nx: &Nexus, reference: &str) -> Result<(String, serde_json::Value, collections::Manifest, PathBuf)> {
    let (slug, rev) = collections::parse_ref(reference)?;
    let rv = nx.collection_revision(&slug, rev)?;
    let n = rv["revisionNumber"].as_u64().unwrap_or(0);
    let dir = paths.cache().join("collections").join(format!("{slug}-{n}"));
    if !dir.join("collection.json").is_file() {
        fs::create_dir_all(&dir)?;
        let arc = dir.join("collection.7z");
        nx.download_collection(rv["downloadLink"].as_str().context("no download link for this collection")?, &arc)?;
        install::extract(&arc, &dir)?;
        let _ = fs::remove_file(&arc);
    }
    let man = collections::read_manifest(&dir)?;
    Ok((slug, rv, man, dir))
}

fn coll_status(st: &State, m: &collections::CollMod) -> &'static str {
    let same = st.mods.iter().filter(|x| x.nexus.as_ref().is_some_and(|n| n.mod_id == m.mod_id)).collect::<Vec<_>>();
    if m.source != "nexus" {
        if m.source == "bundle" { "bundled" } else { "manual" }
    } else if same.iter().any(|x| x.nexus.as_ref().is_some_and(|n| n.version_gsid == m.file_id)) {
        "installed"
    } else if same.is_empty() {
        "missing"
    } else {
        "other version"
    }
}

fn collection_json(paths: &Paths, st: &State, reference: &str) -> Result<serde_json::Value> {
    let nx = nexus(paths);
    let (slug, rv, man, _) = fetch_collection(paths, &nx, reference)?;
    Ok(serde_json::json!({
        "slug": slug,
        "revision": rv["revisionNumber"],
        "name": rv.pointer("/collection/name").cloned().unwrap_or(serde_json::json!(man.name)),
        "author": rv.pointer("/collection/user/name").cloned().unwrap_or(serde_json::json!(man.author)),
        "summary": rv.pointer("/collection/summary"),
        "total_size": rv["totalSize"].as_str().and_then(|s| s.parse::<f64>().ok()).or(rv["totalSize"].as_f64()).unwrap_or(0.0),
        "instructions": man.instructions.chars().take(2000).collect::<String>(),
        "rules": man.rules.len(),
        "optional": man.mods.iter().filter(|m| m.optional).count(),
        "installed": st.collections.iter().any(|c| c.slug == slug),
        "mods": man.mods.iter().map(|m| serde_json::json!({
            "name": m.name, "version": m.version, "mod_id": m.mod_id, "file_id": m.file_id, "optional": m.optional,
            "phase": m.phase, "source": m.source, "fomod": m.choices.is_some(), "patches": m.has_patches,
            "status": coll_status(st, m),
        })).collect::<Vec<_>>(),
    }))
}

/// Install a collection: exact files in phase order with the curator's FOMOD choices, enable
/// them in the target profile, add the curator's load-order rules, then sort.
fn collection_install(
    paths: &Paths,
    st: &mut State,
    reference: &str,
    optional: bool,
    into_current: bool,
    profile: Option<String>,
) -> Result<()> {
    manage::ensure_idle(paths)?;
    let nx = nexus(paths);
    let (slug, rv, man, dir) = fetch_collection(paths, &nx, reference)?;
    let name = rv.pointer("/collection/name").and_then(|v| v.as_str()).unwrap_or(&man.name).to_string();
    let rev = rv["revisionNumber"].as_u64().unwrap_or(0);
    println!("{name} (revision {rev}) by {}: {} mods", man.author, man.mods.len());
    if !into_current && paths.cfg.profiles_enabled {
        let pname = profile.unwrap_or_else(|| name.clone());
        let pname = profiles::valid_name(&pname).unwrap_or_else(|_| format!("Collection {slug}"));
        if st.profile_index(&pname).is_err() {
            st.create_profile(&pname, None, true)?;
            println!("created profile {pname:?} (your other profiles are untouched)");
        }
        st.switch_profile(&pname)?;
        st.save(paths)?;
        println!("installing into profile {pname:?}");
    }
    safety::auto_snapshot(paths, &format!("before installing collection {name}"));
    if let Ok(mut b) = safety::BATCH.lock() {
        *b = Some(format!("collection-{slug}-{rev}"));
    }
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
        println!("[{n}/{total}] {} {}", m.name, m.version);
        let r: Result<Option<String>> = match m.source.as_str() {
            "nexus" if m.mod_id > 0 => {
                let o = InstallOpts {
                    file: Some(m.file_id.clone()),
                    id: None,
                    name: None,
                    root: None,
                    dest: None,
                    with_deps: false,
                    ignore_deps: true,
                    disabled: false,
                    keep_archive: false,
                    nxm: None,
                    hooks: false,
                    fomod: m.choices.clone(),
                    fomod_defaults: true,
                };
                let mut visiting = BTreeSet::new();
                install_nexus(paths, st, &nx, m.mod_id, &o, 0, &mut visiting).map(|_| {
                    st.mods
                        .iter()
                        .find(|x| x.nexus.as_ref().is_some_and(|nx| nx.mod_id == m.mod_id && nx.version_gsid == m.file_id))
                        .or_else(|| st.by_nexus(m.mod_id))
                        .map(|x| x.id.clone())
                })
            }
            "bundle" => {
                let p = dir.join("bundled").join(&m.bundle);
                if p.exists() {
                    let o = InstallOpts {
                        file: None,
                        id: None,
                        name: Some(m.name.clone()),
                        root: None,
                        dest: None,
                        with_deps: false,
                        ignore_deps: true,
                        disabled: false,
                        keep_archive: true,
                        nxm: None,
                        hooks: false,
                        fomod: m.choices.clone(),
                        fomod_defaults: true,
                    };
                    install_local(paths, st, &p, None, &o).map(|_| st.mods.iter().rev().find(|x| x.name == m.name).map(|x| x.id.clone()))
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
                println!("  failed: {e:#}");
                failed.push(format!("{}: {e:#}", m.name));
            }
        }
    }
    if let Ok(mut b) = safety::BATCH.lock() {
        *b = None;
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
        slug: slug.clone(),
        name: name.clone(),
        revision: rev,
        profile: st.active_profile.clone(),
        mods: ids.iter().cloned().collect(),
        installed_at: now(),
    });
    st.save(paths)?;
    auto_sort(paths, st);
    let _ = loadorder::write_modlist(paths, st);
    println!(
        "\n{name}: {} installed, {skipped} optional skipped{}, {added} load-order rules",
        ids.len(),
        if optional { "" } else { " (--optional adds them)" }
    );
    if !failed.is_empty() {
        println!("{} failed (run the same command again to retry just those):", failed.len());
        for f in &failed {
            println!("  {f}");
        }
    }
    for m in &manual {
        println!("  manual: {m}");
    }
    if !man.instructions.trim().is_empty() {
        println!("\nCurator's notes: {}", man.instructions.chars().take(400).collect::<String>().replace('\n', " "));
    }
    Ok(())
}

/// The cpmm binary Steam should run (the installed copy, not a cargo target dir).
fn installed_exe() -> Result<PathBuf> {
    let local = expand("~/.local/bin/cpmm");
    if local.is_file() {
        return Ok(local);
    }
    Ok(std::env::current_exe()?)
}

fn key_file() -> PathBuf {
    expand("~/.config/nexusmods/apikey")
}

fn nexus_key_cmd(paths: &Paths, key: Option<String>, check: bool, remove: bool) -> Result<()> {
    if remove {
        let _ = fs::remove_file(key_file());
        println!("Nexus API key removed");
        return Ok(());
    }
    if let Some(k) = key {
        let k = k.trim().to_string();
        let (name, premium) = Nexus::new(Some(k.clone()), &paths.cache()).validate().context("validating the key")?;
        let f = key_file();
        fs::create_dir_all(f.parent().unwrap())?;
        fs::write(&f, &k)?;
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&f, fs::Permissions::from_mode(0o600))?;
        println!("saved the key for {name}{}", if premium { " (premium)" } else { "" });
        return Ok(());
    }
    match paths.api_key() {
        Ok(k) if check => {
            let (name, premium) = Nexus::new(Some(k), &paths.cache()).validate()?;
            println!(
                "key OK: {name}{}",
                if premium { " (premium: downloads work from cpmm)" } else { " (free: use Mod Manager Download links)" }
            );
        }
        Ok(_) => println!("a Nexus API key is set ({}); --check validates it", key_file().display()),
        Err(_) => {
            println!("no Nexus API key; get yours at https://www.nexusmods.com/users/myaccount?tab=api and run `cpmm nexus-key <KEY>`")
        }
    }
    Ok(())
}

fn ask(q: &str, default: &str) -> String {
    use std::io::Write as _;
    print!("{q}{} ", if default.is_empty() { String::new() } else { format!(" [{default}]") });
    let _ = std::io::stdout().flush();
    let mut l = String::new();
    let _ = std::io::stdin().read_line(&mut l);
    let l = l.trim();
    if l.is_empty() { default.to_string() } else { l.to_string() }
}
fn yes(q: &str, default: bool) -> bool {
    let a = ask(&format!("{q} (y/n)"), if default { "y" } else { "n" });
    a.to_lowercase().starts_with('y')
}

/// Interactive first-run setup for terminal users.
fn setup_cmd(paths: &Paths, st: &mut State) -> Result<()> {
    println!("cpmm setup\n");
    // 1. game
    let mut game = paths.game.clone();
    if !game.join("bin/x64/Cyberpunk2077.exe").is_file() {
        let g = ask("Where is Cyberpunk 2077 installed?", &game.to_string_lossy());
        game = expand(&g);
        if !game.join("bin/x64/Cyberpunk2077.exe").is_file() {
            bail!("no bin/x64/Cyberpunk2077.exe under {}", game.display());
        }
        fs::create_dir_all(&paths.data)?;
        fs::write(paths.data.join("game_dir"), game.to_string_lossy().as_bytes())?;
    }
    println!("game: {}", game.display());
    // 2. Nexus key
    if paths.api_key().is_err() {
        println!("\ncpmm needs your personal Nexus Mods API key (https://www.nexusmods.com/users/myaccount?tab=api).");
        let k = ask("Paste it (or leave empty to skip):", "");
        if !k.is_empty() {
            nexus_key_cmd(paths, Some(k), false, false)?;
        }
    } else {
        println!("Nexus API key: set");
    }
    // 3. launch options
    let exe = installed_exe()?;
    let lo = steam::read_launch_options();
    if !lo.as_deref().is_some_and(steam::launch_options_ok)
        && yes("\nMake Steam launch the game through cpmm (keeps your existing launch options)?", true)
    {
        match steam::apply_launch_options(&exe) {
            Ok(n) => println!("launch options: {n}"),
            Err(e) => println!("{e:#}"),
        }
    }
    // 4. link handler + preferences
    if manage::current_nxm_handler() != "cpmm-nxm.desktop" && yes("Open Nexus \"Mod Manager Download\" links with cpmm?", true) {
        manage::register_nxm(&exe)?;
    }
    let mut cfg = paths.cfg.clone();
    cfg.check_updates_on_start = yes("Check for mod updates when cpmm starts?", cfg.check_updates_on_start);
    cfg.auto_install_updates = yes("Install updates automatically (undoable)?", cfg.auto_install_updates);
    cfg.auto_sort = yes("Keep the load order sorted automatically?", cfg.auto_sort);
    cfg.snapshot_saves = yes("Snapshot your saves before risky changes?", cfg.snapshot_saves);
    cfg.onboarded = true;
    settings::save(&paths.data, &cfg)?;
    let _ = st;
    println!("\nAll set. `cpmm install <nexus url>` installs a mod; `cpmm-gui` opens the app.");
    Ok(())
}

fn settings_cmd(paths: &Paths, st: &State, action: SettingsCmd) -> Result<()> {
    let cur = paths.cfg.clone();
    let new = match action {
        SettingsCmd::Show { json } => {
            if json {
                println!("{}", serde_json::to_string_pretty(&cur)?);
            } else {
                let v = serde_json::to_value(&cur)?;
                for f in settings::schema().as_array().unwrap() {
                    let k = f["key"].as_str().unwrap();
                    println!("  {:<24} {:<10} {}", k, v[k].to_string().trim_matches('"'), f["label"].as_str().unwrap_or(""));
                }
                println!("staging: {}\ndownloads: {}", paths.mods().display(), paths.downloads().display());
            }
            return Ok(());
        }
        SettingsCmd::Set { key, value } => settings::set(&cur, &key, &value)?,
        SettingsCmd::Reset { key: None } => settings::Settings { onboarded: cur.onboarded, ..Default::default() },
        SettingsCmd::Reset { key: Some(k) } => {
            let d = serde_json::to_value(settings::Settings::default())?;
            let v = d.get(&k).with_context(|| format!("unknown setting {k:?}"))?;
            settings::set(&cur, &k, v.as_str().map(String::from).unwrap_or(v.to_string()).as_str())?
        }
    };
    // Changing the staging folder moves the installed mods with it.
    let mut np = paths.clone();
    np.cfg = new.clone();
    let (old_dir, new_dir) = (paths.mods(), np.mods());
    if old_dir != new_dir && old_dir.exists() {
        manage::ensure_idle(paths)?;
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
        println!("moved {moved} mods to {}", new_dir.display());
        use std::os::unix::fs::MetadataExt;
        if fs::metadata(&new_dir).map(|m| m.dev()).ok() != fs::metadata(&paths.game).map(|m| m.dev()).ok() {
            println!("note: the new folder is on a different drive than the game; installs and imports will copy instead of move");
        }
    }
    settings::save(&paths.data, &new)?;
    let _ = st;
    println!("saved");
    Ok(())
}

/// Settings-driven tasks after a mod is installed.
fn after_install(paths: &Paths, st: &mut State, id: &str) {
    if paths.cfg.auto_sort {
        auto_sort(paths, st);
    }
    if paths.cfg.after_install_verify {
        let _ = verify(paths, st, &[id.to_string()]);
    }
    if paths.cfg.after_install_doctor {
        let name = st.find(id).map(|i| st.mods[i].name.clone()).unwrap_or_default();
        let hits: Vec<health::Finding> = health::check(paths, st)
            .into_iter()
            .filter(|f| {
                matches!(f.severity, "crash" | "warning") && (f.title.contains(id) || (!name.is_empty() && f.title.contains(&name)))
            })
            .collect();
        for f in &hits {
            println!("  health: [{}] {}", f.severity, f.title);
        }
        if hits.is_empty() {
            println!("  health: no new risks");
        }
    }
    let cmd = paths.cfg.after_install_command.trim();
    if !cmd.is_empty() {
        let m = st.find(id).map(|i| st.mods[i].clone());
        let r = std::process::Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .env("CPMM_MOD_ID", id)
            .env("CPMM_MOD_NAME", m.as_ref().map(|m| m.name.as_str()).unwrap_or(""))
            .env("CPMM_MOD_VERSION", m.as_ref().map(|m| m.version.as_str()).unwrap_or(""))
            .status();
        match r {
            Ok(s) if s.success() => {}
            Ok(s) => println!("  after-install command exited with {s}"),
            Err(e) => println!("  after-install command failed: {e}"),
        }
    }
}

/// Apply the automatic load order if it would move anything.
fn auto_sort(paths: &Paths, st: &mut State) {
    let p = loadorder::plan(paths, st);
    if !p.moved.is_empty() {
        loadorder::apply(st, &p.order);
        if st.save(paths).is_ok() {
            let _ = loadorder::write_modlist(paths, st);
            println!("  auto-sorted the load order ({} moves)", p.moved.len());
        }
    }
}

fn deps_fix(paths: &Paths, st: &mut State) -> Result<()> {
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
            hooks: true,
            fomod: None,
            fomod_defaults: true,
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
    let ow = manage::overwrite_files(paths, st);
    for e in &ow {
        let by = e.shadows.as_ref().map(|s| format!(" ({s})")).unwrap_or_default();
        println!("  {:<16} {}{by}", e.kind.as_str(), e.rel);
    }
    println!("{} files in {}", ow.len(), paths.upper().display());
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
