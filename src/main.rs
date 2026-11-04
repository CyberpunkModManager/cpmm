//! cpmm: Cyberpunk 2077 mod manager for Linux/Proton. The CLI adapter at the Operations seam:
//! clap parsing, the terminal's [`Ui`](cpmm::ops::Ui) (printing events, asking questions),
//! printing query results, exit codes. Everything else is the `cpmm` library.

mod cli;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use cpmm::fomod;
use cpmm::install::OnMissing;
use cpmm::ops::{self, Action, InstallRequest, Place, ProfileAction};
use cpmm::state::{Paths, State, now};
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

fn main() {
    // Behave like a normal CLI under `| head` instead of panicking on EPIPE.
    unsafe { libc::signal(libc::SIGPIPE, libc::SIG_DFL) };
    if let Err(e) = real_main() {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn fomod_choices(json: Option<String>, what: &'static str) -> Result<Option<fomod::Choices>> {
    json.map(|j| serde_json::from_str::<fomod::Choices>(&j)).transpose().context(what)
}

/// `[MOD, REQ_ID]` from `--ignore`/`--unignore`.
fn pair(v: Option<Vec<String>>) -> Option<(String, String)> {
    v.map(|v| (v[0].clone(), v[1].clone()))
}

fn real_main() -> Result<()> {
    let cli = Cli::parse();
    let paths = Paths::load()?;
    let mut term = cli::Term;
    // Commands that inspect the base dir must not run inside the overlay.
    let in_ns = std::env::var_os("CPMM_NS").is_some();
    match cli.cmd {
        Cmd::Run { cmd } => return ops::launch(&paths, &cmd, &mut term),
        Cmd::Exec { cmd } => return ops::exec(&paths, &cmd),
        _ if in_ns => bail!("run cpmm management commands outside `cpmm exec`/the game session"),
        _ => {}
    }
    if let Cmd::Nxm { url: Some(_), .. } = &cli.cmd {
        // Launched by the browser without a terminal: log instead. (CPMM_GUI=1 keeps stdout,
        // for a caller that reads it; the GUI itself runs nxm links in-process now.)
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
    let act = |a: Action| ops::act(&paths, a, &mut cli::Term);
    match cli.cmd {
        Cmd::Run { .. } | Cmd::Exec { .. } => unreachable!(),
        Cmd::Init { game } => act(Action::Init { game })?,
        Cmd::List { long } => cli::list(&paths, &State::load(&paths)?, long),
        Cmd::Profile { action: None | Some(ProfileCmd::List) } => cli::profiles(&paths, State::load(&paths)?),
        Cmd::Profile { action: Some(p) } => act(Action::Profile(match p {
            ProfileCmd::List => unreachable!(),
            ProfileCmd::Create { name, from, empty, own_saves, copy_saves, switch } => {
                ProfileAction::Create { name, from, empty, own_saves, copy_saves, switch }
            }
            ProfileCmd::Switch { name } => ProfileAction::Switch { name },
            ProfileCmd::Rename { old, new } => ProfileAction::Rename { old, new },
            ProfileCmd::Saves { name, mode, copy } => ProfileAction::Saves { name, own: mode == "own", copy },
            ProfileCmd::Delete { name } => ProfileAction::Delete { name },
        }))?,
        Cmd::Info { r#mod } => cli::info(&ops::query::detail(&paths, &State::load(&paths)?, &r#mod)?),
        Cmd::Enable { mods } => act(Action::Enable { mods })?,
        Cmd::Disable { mods } => act(Action::Disable { mods })?,
        Cmd::Order { r#mod, top, bottom, before, after } => {
            let place = if top {
                Some(Place::Top)
            } else if bottom {
                Some(Place::Bottom)
            } else if let Some(b) = before {
                Some(Place::Before(b))
            } else {
                after.map(Place::After)
            };
            act(Action::Order { r#mod, place })?
        }
        Cmd::Conflicts { r#mod, no_hash, base } => {
            cli::conflicts(&ops::query::conflict_report(&paths, &State::load(&paths)?, r#mod.as_deref(), !no_hash)?, base)
        }
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
            nexus,
            keep_archive,
            fomod_choices: fc,
            fomod_defaults,
        } => {
            let fomod = fomod_choices(fc, "--fomod-choices must be JSON: {\"Step/Group\": [\"Option\"]}")?;
            let missing = if with_deps {
                OnMissing::Install
            } else if ignore_deps {
                OnMissing::Warn
            } else {
                OnMissing::Refuse
            };
            let req =
                InstallRequest { source, file, id, name, root, dest, fomod, ask: !fomod_defaults, missing, disabled, nexus, keep_archive };
            act(Action::Install(req))?
        }
        Cmd::Updates { refresh } => {
            let st = State::load(&paths)?;
            cli::updates(&st, &ops::query::check_updates(&paths, &st, refresh))
        }
        Cmd::Update { mods, all } => act(Action::Update { mods, all })?,
        Cmd::Rename { r#mod, new_id } => act(Action::Rename { r#mod, new_id })?,
        Cmd::Remove { r#mod, force } => act(Action::Remove { r#mod, force })?,
        Cmd::Deps { refresh, fix, ignore, unignore } => act(Action::Deps { refresh, fix, ignore: pair(ignore), unignore: pair(unignore) })?,
        Cmd::Hide { r#mod, paths: p } => act(Action::Hide { r#mod, paths: p })?,
        Cmd::Unhide { r#mod, paths: p } => act(Action::Unhide { r#mod, paths: p })?,
        Cmd::Prefer { r#mod, paths: p } => act(Action::Prefer { r#mod, paths: p })?,
        Cmd::Tracked { install, untrack } => act(Action::Tracked { install, untrack })?,
        Cmd::RegisterHandler { check: true, .. } => cli::handler(),
        Cmd::RegisterHandler { unregister: true, .. } => act(Action::UnregisterHandler)?,
        Cmd::RegisterHandler { .. } => act(Action::RegisterHandler)?,
        Cmd::Nxm { url, register } => act(Action::Nxm { url, register })?,
        Cmd::Verify { mods } => {
            ops::query::verify(&paths, &State::load(&paths)?, &mods, &mut |v| cli::lines(v.lines()))?;
            cli::warnings();
        }
        Cmd::Import { modlist, apply } => act(Action::Import { modlist: modlist.into(), apply })?,
        Cmd::Adopt { id, name, paths: p } => act(Action::Adopt { id, name, paths: p })?,
        Cmd::Unmanaged { modified, flat } => cli::unmanaged(&paths, &State::load(&paths)?, modified, flat)?,
        Cmd::Overwrite { action: None | Some(OwCmd::List) } => cli::overwrite(&paths, &State::load(&paths)?),
        Cmd::Overwrite { action: Some(OwCmd::Move { to, paths: p }) } => act(Action::OverwriteMove { to, paths: p })?,
        Cmd::Overwrite { action: Some(OwCmd::Clear { paths: p }) } => act(Action::OverwriteClear { paths: p })?,
        Cmd::Eject { apply } => act(Action::Eject { apply })?,
        Cmd::Api(a) => api(&paths, a)?,
        Cmd::Sort { apply } => act(Action::Sort { apply })?,
        Cmd::Rule { r#mod: Some(m), after: None, list: false, .. } => {
            State::load(&paths)?.get(&m)?;
            bail!("give --after <mod>")
        }
        Cmd::Rule { r#mod: Some(m), after: Some(after), remove, list: false } => act(Action::Rule { r#mod: m, after, remove })?,
        Cmd::Rule { .. } => cli::rules(&State::load(&paths)?),
        Cmd::Archives { all } => cli::archives(&paths, &State::load(&paths)?, all),
        Cmd::Doctor { ack, unack, all } => {
            if unack.is_some() {
                act(Action::Unack { finding: ack.or(unack).unwrap_or_default() })?
            } else if let Some(k) = ack {
                act(Action::Ack { finding: k })?
            }
            cli::doctor(&paths, &State::load(&paths)?, all)
        }
        Cmd::History { r#mod } => cli::history(&paths, &State::load(&paths)?, &r#mod)?,
        Cmd::Rollback { r#mod, to } => act(Action::Rollback { r#mod, to })?,
        Cmd::UndoUpdate { batch } => act(Action::UndoUpdate { batch })?,
        Cmd::Crashes { limit } => cli::crashes(&paths, limit),
        Cmd::Errors => act(Action::Errors)?,
        Cmd::Patch => {
            cli::patch(&paths, &State::load(&paths)?);
            cli::warnings();
        }
        Cmd::Vanilla { state } => match state.as_deref() {
            Some("on") => act(Action::Vanilla { on: true })?,
            Some("off") => act(Action::Vanilla { on: false })?,
            _ => cli::vanilla(&paths),
        },
        Cmd::Saves { snapshot, restore } => {
            if snapshot {
                act(Action::SnapshotSaves)?
            }
            if let Some(r) = restore {
                act(Action::RestoreSaves { name: r })?
            }
            cli::saves(&paths, &State::load(&paths)?)
        }
        Cmd::Changelog { r#mod, all } => {
            cli::changelog(&ops::query::changelog(&paths, &State::load(&paths)?, &r#mod)?, all);
            cli::warnings();
        }
        Cmd::Modio { ack: true } => act(Action::ModioAck)?,
        Cmd::Modio { ack: false } => cli::modio(&paths, &State::load(&paths)?),
        Cmd::Collection { action } => match action {
            CollectionCmd::Show { reference } => {
                cli::collection(&ops::query::collection(&paths, &State::load(&paths)?, &reference)?, &reference);
                cli::warnings();
            }
            CollectionCmd::Install { reference, optional, into_current, profile } => {
                act(Action::CollectionInstall { reference, optional, into_current, profile })?
            }
            CollectionCmd::List => cli::collections(&State::load(&paths)?),
        },
        Cmd::Settings { action } => match action.unwrap_or(SettingsCmd::Show { json: false }) {
            SettingsCmd::Show { json } => cli::settings(&paths, json)?,
            SettingsCmd::Set { key, value } => act(Action::SettingsSet { key, value })?,
            SettingsCmd::Reset { key } => act(Action::SettingsReset { key })?,
        },
        Cmd::Download { source, file } => act(Action::Download { source, file })?,
        Cmd::Downloads { action } => match action.unwrap_or(DownloadsCmd::List) {
            DownloadsCmd::List => cli::downloads(&paths, &State::load(&paths)?),
            DownloadsCmd::Install { key, fomod_choices: fc, fomod_defaults, with_deps } => {
                let fomod = fomod_choices(fc, "--fomod-choices must be JSON")?;
                act(Action::DownloadsInstall { key, fomod, ask: !fomod_defaults, with_deps })?
            }
            DownloadsCmd::Remove { key } => act(Action::DownloadsRemove { key })?,
            DownloadsCmd::Add { path, nexus, move_it, source, auto } => act(Action::DownloadsAdd { path, nexus, move_it, source, auto })?,
        },
        Cmd::Clean { dry_run } => act(Action::Clean { dry_run })?,
        Cmd::Status => cli::status(&paths),
        Cmd::SteamShortcut { name, exe, remove, dry_run } => act(Action::SteamShortcut { name, exe, remove, dry_run })?,
        Cmd::LaunchOptions { apply: true } => act(Action::LaunchOptionsApply)?,
        Cmd::LaunchOptions { apply: false } => cli::launch_options(&ops::query::launch_options(&term)?),
        Cmd::NexusKey { remove: true, .. } => act(Action::NexusKeyRemove)?,
        Cmd::NexusKey { key: Some(k), .. } => act(Action::NexusKeySet { key: k })?,
        Cmd::NexusKey { check, .. } => cli::nexus_key(&ops::query::nexus_key(&paths, check)?),
        Cmd::Setup => act(Action::Setup)?,
    }
    Ok(())
}

/// `cpmm api …`: one JSON value on stdout.
fn api(paths: &Paths, a: ApiCmd) -> Result<()> {
    use ops::query::Query;
    let q = match a {
        ApiCmd::State => Query::State,
        ApiCmd::Info { r#mod } => Query::Info { r#mod },
        ApiCmd::Conflicts => Query::Conflicts,
        ApiCmd::Updates { refresh } => Query::Updates { refresh },
        ApiCmd::Overwrite => Query::Overwrite,
        ApiCmd::Tracked => Query::Tracked,
        ApiCmd::Loadorder => Query::Loadorder,
        ApiCmd::Doctor => Query::Doctor,
        ApiCmd::Downloads => Query::Downloads,
        ApiCmd::Onboarding => Query::Onboarding,
        ApiCmd::Changelog { r#mod } => Query::Changelog { r#mod },
        ApiCmd::Modio => Query::Modio,
        ApiCmd::Fomod { source, file, choices } => {
            Query::Fomod { source, file, choices: choices.map(|j| serde_json::from_str(&j)).transpose()? }
        }
        ApiCmd::Collection { reference } => Query::Collection { reference },
        ApiCmd::Settings => Query::Settings,
        ApiCmd::Safety => Query::Safety,
        ApiCmd::History { r#mod } => Query::History { r#mod },
        ApiCmd::Archives => Query::Archives,
        ApiCmd::Lookup { source } => Query::Lookup { source },
    };
    let v = ops::query::view(paths, &q, &mut cli::Term)?;
    cli::warnings();
    println!("{}", serde_json::to_string(&v)?);
    Ok(())
}
