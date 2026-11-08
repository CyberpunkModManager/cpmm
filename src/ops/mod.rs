//! Operations: the one interface the two adapters, the `cpmm` CLI and the Tauri GUI, drive
//! the core through.
//!
//! ```text
//! CLI (clap, printing, exit codes) ─┐                        ┌─ queries: read-only views
//!                                   ├─ Operations ───────────┤
//! Tauri (commands, Tauri events) ───┘   (this module)        └─ actions: changes, under the
//!                                                               cpmm lock, through Change
//! ```
//!
//! [`act`] runs an [`Action`]; [`query`] holds the views. Neither prints: what happens goes
//! to the adapter's [`Ui`] as [`Event`]s, and [`render::lines`] is their text (the CLI prints
//! it; the GUI's job log shows it). What only an adapter can do (ask the user something,
//! know which `cpmm` binary the outside world should run) is a [`Ui`] capability.

mod act;
#[cfg(test)]
mod bindings;
pub mod query;
pub mod render;
#[cfg(test)]
mod tests;

pub use act::{Action, CollectionDone, InstallRequest, Place, ProfileAction, act, exec, installed_exe, launch, tool_sandbox};

use crate::fomod::{Choices, Fomod};
use crate::{import, install, loadorder, nexus, safety, steam};
pub use anyhow::Result;
use std::path::PathBuf;

/// What the adapter supplies: where events go, and its interactive capabilities.
pub trait Ui {
    fn event(&mut self, e: &Event);
    /// Ask the user to pick a FOMOD installer's options. None: nobody to ask (the GUI, or the
    /// CLI off a terminal); the installer's suggested options are used.
    fn choose_fomod(&mut self, _installer: &Fomod, _active: &dyn Fn(&str) -> bool) -> Option<Result<Choices>> {
        None
    }
    /// Ask the user a question. None: nobody to ask; the default is used.
    fn ask(&mut self, _question: &str, _default: &str) -> Option<String> {
        None
    }
    /// The `cpmm` binary that the nxm:// link handler and Steam's launch options should run.
    /// The CLI is that binary; the GUI finds it next to itself.
    fn this_exe(&self) -> Result<PathBuf> {
        Ok(std::env::current_exe()?)
    }
}

/// Any closure over events is a non-interactive [`Ui`] (tests, quiet callers).
impl<F: FnMut(&Event)> Ui for F {
    fn event(&mut self, e: &Event) {
        self(e)
    }
}

/// Passes Nexus quota warnings on before each event (they are raised deep in the Nexus gate).
pub(crate) struct Warned<'u>(pub &'u mut dyn Ui);

impl Warned<'_> {
    pub fn flush(&mut self) {
        for w in nexus::take_warnings() {
            self.0.event(&Event::Note(Note::QuotaLow(w)));
        }
    }
}

impl Ui for Warned<'_> {
    fn event(&mut self, e: &Event) {
        self.flush();
        self.0.event(e)
    }
    fn choose_fomod(&mut self, installer: &Fomod, active: &dyn Fn(&str) -> bool) -> Option<Result<Choices>> {
        self.0.choose_fomod(installer, active)
    }
    fn ask(&mut self, question: &str, default: &str) -> Option<String> {
        self.0.ask(question, default)
    }
    fn this_exe(&self) -> Result<PathBuf> {
        self.0.this_exe()
    }
}

/// Quota warnings raised during a query, for adapters to show (the CLI prints them).
pub fn warnings() -> Vec<Note> {
    nexus::take_warnings().into_iter().map(Note::QuotaLow).collect()
}

/// Something that happened during an operation.
pub enum Event<'a> {
    /// A step of an install (`install::run`).
    Install(&'a install::Event<'a>),
    Note(Note),
}

impl Event<'_> {
    /// The id of a mod newly installed (not updated or replaced) by this event, if any: what
    /// the GUI names in "Installed …".
    pub fn installed(&self) -> Option<&str> {
        match self {
            Event::Install(install::Event::Staged(d)) if d.change == install::Change::Installed => Some(&d.id),
            _ => None,
        }
    }
}

/// The result of a step, or a notice, with the data the text is made from.
#[derive(Clone, Debug)]
pub enum Note {
    /// Another cpmm (CLI or GUI) holds the lock; this waits for it.
    WaitingForLock,
    /// A change to what the game loads, while a session runs: it applies at the next launch.
    SessionRunning,
    AutoSorted(usize),
    QuotaLow(nexus::QuotaLow),
    Initialized {
        data: PathBuf,
        game: PathBuf,
    },
    DifferentFilesystems,
    Toggled {
        id: String,
        on: bool,
    },
    StillRequired(Vec<String>),
    RequiresMissing {
        mod_id: u64,
        name: String,
    },
    RequiresDisabled(String),
    Placed {
        id: String,
        priority: usize,
    },
    Renamed {
        old: String,
        new: String,
    },
    AlsoInProfiles(Vec<String>),
    Trashed(PathBuf),
    Removed(String),
    Ignoring {
        id: String,
        req: u64,
        on: bool,
    },
    ReqCheckFailed {
        id: String,
        error: String,
    },
    ReqsChecked {
        checked: usize,
        cached: usize,
    },
    ReqsCached {
        total: usize,
    },
    Unmet(query::Unmet),
    FixFailed {
        mod_id: u64,
        error: String,
    },
    Hid {
        id: String,
        files: usize,
    },
    Unhid {
        id: String,
        files: usize,
    },
    /// (other mod, path) hidden so this one wins
    Preferred {
        id: String,
        hid: Vec<(String, String)>,
    },
    NoTracked,
    Tracked {
        mod_id: u64,
        name: String,
        installed_as: Option<String>,
    },
    TrackedFailed {
        mod_id: u64,
        name: String,
        error: String,
    },
    Untracked(u64),
    HandlerSet {
        exe: PathBuf,
        desktop: PathBuf,
        replaced: Option<String>,
        log: PathBuf,
    },
    HandlerRemoved {
        before: String,
        now: String,
    },
    Registered {
        exe: PathBuf,
        desktop: PathBuf,
    },
    Fetching {
        name: String,
        version: String,
        bytes: u64,
    },
    Downloaded {
        name: String,
        version: String,
        file: String,
    },
    DownloadedTo(PathBuf),
    DownloadDeleted {
        file: String,
        installed_as: Option<String>,
    },
    DownloadAdded(String),
    NothingToUpdate,
    NotFromNexus(String),
    FileUnknown {
        id: String,
        mod_id: u64,
    },
    UpdateFailed {
        id: String,
        error: String,
    },
    Import(import::Progress),
    FetchingRequirements,
    Adopted {
        id: String,
        files: usize,
    },
    OverwriteMoved {
        id: String,
        moved: usize,
        skipped: usize,
    },
    OverwriteCleared {
        entries: usize,
        trash: PathBuf,
    },
    Ejected {
        mods: usize,
        overwrite: usize,
        game: PathBuf,
        applied: bool,
    },
    Cleaned {
        bytes: u64,
        dry_run: bool,
    },
    SortPlan(loadorder::SortPlan),
    Sorted {
        archives: usize,
    },
    SortDryRun,
    Ruled {
        a: String,
        b: String,
        removed: bool,
    },
    Moved(loadorder::Moved),
    RulesApplied {
        archives: usize,
    },
    Acked {
        title: String,
        on: bool,
    },
    RolledBack(String),
    RolledBackMod(String),
    Undid(String),
    Errors(safety::ErrDiff),
    Vanilla(bool),
    Snapshot(Option<String>),
    Restored {
        files: usize,
        from: String,
    },
    ModioReviewed(usize),
    LaunchOptionsSet {
        options: String,
        live: bool,
    },
    KeySaved {
        name: String,
        premium: bool,
    },
    KeyRemoved,
    Key(query::KeyStatus),
    StagingMoved {
        mods: usize,
        to: PathBuf,
    },
    StagingOtherDrive,
    SettingsSaved,
    Shortcut(steam::Step),
    Setup(SetupStep),
    Profile(ProfileNote),
    Collection(CollectionNote),
    Launch(LaunchNote),
    Tool(ToolNote),
}

#[derive(Clone, Debug)]
pub enum ProfileNote {
    AlreadyActive(String),
    /// `from` None: an empty profile
    Created {
        name: String,
        from: Option<String>,
    },
    SwitchedTo(String),
    /// `new`: mods this profile hadn't seen, added disabled
    Switched {
        from: String,
        to: String,
        summary: String,
        new: Vec<String>,
    },
    Renamed {
        old: String,
        new: String,
    },
    Deleted(String),
    SavesTrashed(PathBuf),
    SharedSaves {
        name: String,
        kept: usize,
        dir: PathBuf,
    },
    SavesCopied {
        saves: usize,
        files: usize,
        from: PathBuf,
    },
    OwnSaves {
        name: String,
        dir: PathBuf,
        saves: usize,
    },
}

#[derive(Clone, Debug)]
pub enum CollectionNote {
    Start { name: String, revision: u64, author: String, mods: usize },
    ProfileCreated(String),
    Into(String),
    Step { n: usize, total: usize, name: String, version: String },
    Failed(String),
    Done { name: String, optional: bool, instructions: String, done: CollectionDone },
}

#[derive(Clone, Debug)]
pub enum ToolNote {
    /// A download that is a Windows tool, installed under Tools instead of as a mod
    IsTool(String),
    Installed {
        id: String,
        name: String,
        version: String,
        actions: Vec<String>,
        replaced: bool,
    },
    Removed {
        id: String,
        trash: PathBuf,
    },
    PrefixSetup(PathBuf),
    Starting {
        label: String,
        wine: String,
        game_prefix: bool,
        live: bool,
    },
    /// A line the tool printed
    Output {
        line: String,
        stderr: bool,
    },
    Finished(Box<crate::tools::ToolRun>),
    Saved {
        run: String,
        id: String,
        rules: usize,
    },
    Discarded(String),
}

#[derive(Clone, Debug)]
pub enum LaunchNote {
    Vanilla,
    ModlistFailed(String),
    Mounted(usize),
    Saves(PathBuf),
}

#[derive(Clone, Debug)]
pub enum SetupStep {
    Start,
    Game(PathBuf),
    KeyNeeded,
    KeySet,
    /// The new launch options, or why they couldn't be set
    LaunchOptions(std::result::Result<String, String>),
    Done,
}
