//! The text of events: what the CLI prints and the GUI's job log shows, the same for both.

use super::query::KeyStatus;
use super::{CollectionNote, Event, LaunchNote, Note, ProfileNote, SetupStep};
use crate::import::Progress;
use crate::install;
use crate::nexus::Transfer;
use crate::steam::Step;

/// One line of output. `err`: the CLI's stderr (warnings, notices). `open`: the line
/// continues (download percentages); the next line for the same stream ends it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub text: String,
    pub err: bool,
    pub open: bool,
}

/// An action's text as a job log for an adapter that shows it (the GUI): complete lines as
/// they happen (an open line waits for its end), the CLI's stdout and stderr kept apart as
/// that streams them, and the mods the action newly installed.
#[derive(Default)]
pub struct Transcript {
    /// the unfinished line of stdout, stderr
    open: [String; 2],
    stdout: Vec<String>,
    stderr: Vec<String>,
    installed: Vec<String>,
}

/// One line of a job's log, as the GUI receives it (Tauri event `cpmm-log`).
#[derive(serde::Serialize, Clone, Debug, ts_rs::TS)]
pub struct LogLine {
    pub job: u64,
    pub line: String,
    pub stderr: bool,
}

/// How an action ended. `output` is the log as one text, stdout lines then stderr lines (the
/// last line of a failed action is its error, `error: …`); `installed` the mods it newly
/// installed (what the GUI names in "Installed …").
#[derive(serde::Serialize, Clone, Debug, ts_rs::TS)]
pub struct ActResult {
    pub code: i32,
    pub output: String,
    pub installed: Vec<String>,
}

impl Transcript {
    /// Take an event; `line` hears each line it completes (text, stderr?).
    pub fn add(&mut self, e: &Event, line: &mut dyn FnMut(&str, bool)) {
        if let Some(id) = e.installed() {
            self.installed.push(id.to_string());
        }
        for l in lines(e) {
            if l.open {
                self.open[l.err as usize].push_str(&l.text);
                continue;
            }
            let text = std::mem::take(&mut self.open[l.err as usize]) + &l.text;
            line(&text, l.err);
            if l.err { &mut self.stderr } else { &mut self.stdout }.push(text);
        }
    }

    /// The end: an error is the last stderr lines, as the CLI prints it.
    pub fn finish(mut self, r: anyhow::Result<()>, line: &mut dyn FnMut(&str, bool)) -> ActResult {
        if let Err(e) = &r {
            for l in format!("error: {e:#}").lines() {
                line(l, true);
                self.stderr.push(l.to_string());
            }
        }
        let output = self.stdout.iter().chain(&self.stderr).map(|l| format!("{l}\n")).collect();
        ActResult { code: if r.is_ok() { 0 } else { 1 }, output, installed: self.installed }
    }
}

fn out(v: Vec<String>) -> Vec<Line> {
    v.into_iter().map(|text| Line { text, err: false, open: false }).collect()
}

fn err(text: String) -> Vec<Line> {
    vec![Line { text, err: true, open: false }]
}

pub fn lines(e: &Event) -> Vec<Line> {
    match e {
        Event::Install(install::Event::Transfer(Transfer::Percent(p))) => vec![Line { text: format!(" {p}%"), err: true, open: true }],
        Event::Install(install::Event::Transfer(Transfer::Finished)) => err(String::new()),
        Event::Install(e @ install::Event::Kept(_)) => install::lines(e).into_iter().flat_map(err).collect(),
        Event::Install(e) => out(install::lines(e)),
        Event::Note(n) => note(n),
    }
}

fn mb(bytes: u64) -> f64 {
    bytes as f64 / 1048576.0
}

fn note(n: &Note) -> Vec<Line> {
    let one = |s: String| out(vec![s]);
    match n {
        Note::WaitingForLock => err("waiting for another cpmm command to finish…".into()),
        Note::SessionRunning => err("note: a game session is running; changes apply on next launch".into()),
        Note::AutoSorted(n) => one(format!("  auto-sorted the load order ({n} moves)")),
        Note::QuotaLow(q) => err(format!(
            "warning: Nexus daily API quota used up (resets {}); {} requests left this hour (resets {}).",
            q.daily_reset.as_deref().unwrap_or("later"),
            q.hourly_remaining.map_or("?".into(), |n| n.to_string()),
            q.hourly_reset.as_deref().unwrap_or("on the hour")
        )),
        Note::Initialized { data, game } => out(vec![format!("data: {}", data.display()), format!("game: {}", game.display())]),
        Note::DifferentFilesystems => {
            err("warning: data dir and game dir are on different filesystems; import will copy instead of rename".into())
        }
        Note::Toggled { id, on } => one(format!("{} {id}", if *on { "enabled" } else { "disabled" })),
        Note::StillRequired(users) => err(format!("  warning: still required by {}", users.join(", "))),
        Note::RequiresMissing { mod_id, name } => err(format!("  warning: requires {mod_id} {name} (not installed)")),
        Note::RequiresDisabled(id) => err(format!("  warning: requires {id} which is disabled")),
        Note::Placed { id, priority } => one(format!("{id} now at priority {priority}")),
        Note::Renamed { old, new } => one(format!("renamed {old} -> {new}")),
        Note::AlsoInProfiles(p) => {
            one(format!("note: also removes it from profile(s) {} where it is enabled (disable it instead to keep it there)", p.join(", ")))
        }
        Note::Trashed(t) => one(format!("files moved to {} (delete when sure)", t.display())),
        Note::Removed(id) => one(format!("removed {id}")),
        Note::Ignoring { id, req, on } => {
            one(format!("{} requirement {req} for {id}", if *on { "ignoring" } else { "no longer ignoring" }))
        }
        Note::ReqCheckFailed { id, error } => one(format!("  {id}: couldn't check requirements: {error}")),
        Note::ReqsChecked { checked, cached } => one(format!("checked requirements of {checked} mod(s) on Nexus; {cached} cached")),
        Note::ReqsCached { total } => one(format!("requirements of all {total} Nexus mods are cached (re-check with --refresh)")),
        Note::Unmet(u) => {
            let mut v: Vec<String> = u
                .rows
                .iter()
                .map(|r| {
                    let s = r.disabled.as_ref().map_or("MISSING".to_string(), |d| format!("DISABLED ({d})"));
                    if !r.problem {
                        format!("  {:<34} optional {} {} {s}", r.id, r.req.mod_id, r.req.name)
                    } else {
                        let notes = if r.req.notes.is_empty() { String::new() } else { format!(" — {}", r.req.notes) };
                        format!("  {:<34} requires {} {} {s}{notes}", r.id, r.req.mod_id, r.req.name)
                    }
                })
                .collect();
            v.push(format!(
                "{} unmet hard requirements{}",
                u.problems,
                if u.ignored > 0 { format!(" ({} ignored by choice)", u.ignored) } else { String::new() }
            ));
            out(v)
        }
        Note::FixFailed { mod_id, error } => one(format!("  {mod_id}: {error}")),
        Note::Hid { id, files } => one(format!("hid {files} files of {id}")),
        Note::Unhid { id, files } => one(format!("restored {files} files of {id}")),
        Note::Preferred { id, hid } => {
            let mut v: Vec<String> = hid.iter().map(|(o, rel)| format!("  hid {rel} in {o}")).collect();
            v.push(format!("{id} now wins {} contested files (undo with `cpmm unhide <other mod>`)", hid.len()));
            out(v)
        }
        Note::NoTracked => one("no tracked Cyberpunk mods (press Track on a mod page on nexusmods.com)".into()),
        Note::Tracked { mod_id, name, installed_as: Some(id) } => one(format!("  {mod_id:<7} {name}: installed as {id}")),
        Note::Tracked { mod_id, name, installed_as: None } => one(format!("  {mod_id:<7} {name}: not installed")),
        Note::TrackedFailed { mod_id, name, error } => one(format!("  {mod_id} {name}: {error}")),
        Note::Untracked(gid) => one(format!("  untracked {gid}")),
        Note::HandlerSet { exe, desktop, replaced, log } => {
            let mut v = vec![];
            if let Some(b) = replaced {
                v.push(format!("replaced previous handler {b}"));
            }
            v.push(format!("nxm:// links now open {} nxm <url> ({})", exe.display(), desktop.display()));
            v.push(format!("works on the desktop and in Game Mode; progress goes to {}", log.display()));
            out(v)
        }
        Note::HandlerRemoved { before, now } => one(format!("unregistered (handler was {before:?}, now {now:?})")),
        Note::Registered { exe, desktop } => one(format!("registered {} as the nxm:// handler ({})", exe.display(), desktop.display())),
        Note::Fetching { name, version, bytes } => one(format!("downloading {name} {version} ({:.1} MB)", mb(*bytes))),
        Note::Downloaded { name, version, file } => {
            one(format!("downloaded {name} {version}; install it from Downloads (cpmm downloads install {file:?})"))
        }
        Note::DownloadedTo(p) => one(format!("downloaded to {} (cpmm downloads install … to install it)", p.display())),
        Note::DownloadDeleted { file, installed_as } => {
            one(format!("deleted {file}{}", installed_as.as_ref().map(|x| format!(" ({x} stays installed)")).unwrap_or_default()))
        }
        Note::DownloadAdded(file) => one(format!("added {file} to Downloads")),
        Note::NothingToUpdate => one("nothing to update".into()),
        Note::NotFromNexus(id) => one(format!("{id}: not from Nexus, skipping")),
        Note::FileUnknown { id, mod_id } => one(format!("{id}: installed file unknown; use `cpmm install {mod_id} --file …`")),
        Note::UpdateFailed { id, error } => one(format!("{id}: {error}")),
        Note::Import(p) => out(import_lines(p)),
        Note::FetchingRequirements => one("fetching Nexus requirements…".into()),
        Note::Adopted { id, files } => one(format!("adopted {files} files into {id}")),
        Note::OverwriteMoved { id, moved, skipped } => one(format!(
            "moved {moved} files into {id}{}",
            if *skipped > 0 { format!(" ({skipped} deletion markers left; `cpmm overwrite clear` drops them)") } else { String::new() }
        )),
        Note::OverwriteCleared { entries, trash } => one(format!("moved {entries} entries to {}", trash.display())),
        Note::Ejected { mods, overwrite, game, applied } => {
            let mut v = vec![format!(
                "{} {mods} mod files and {overwrite} overwrite files into {}",
                if *applied { "copied" } else { "would copy" },
                game.display()
            )];
            if *applied {
                v.push("game dir is now a classic manual install; remove `cpmm run --` from the launch options to keep it that way".into());
            }
            out(v)
        }
        Note::Cleaned { bytes, dry_run } => one(format!("{} {:.1} MB", if *dry_run { "would free" } else { "freed" }, mb(*bytes))),
        Note::SortPlan(p) => {
            let mut v = vec![];
            if p.moved.is_empty() {
                v.push(format!("load order already satisfies every constraint ({} constraints)", p.edges.len()));
            }
            for mv in &p.moved {
                v.push(format!("  {:<34} {:>3} -> {:<3} {}", mv.id, mv.from, mv.to, mv.because.join("; ")));
            }
            for c in &p.cycles {
                v.push(format!("  cycle (left in current order): {}", c.join(" -> ")));
            }
            if !p.unresolved.is_empty() {
                v.push("unrelated mods replacing the same resources (current order kept; pin with `cpmm rule`):".into());
                for u in &p.unresolved {
                    v.push(format!(
                        "  {} vs {}: {} resources, winner {}",
                        u.a.label(),
                        u.b.label(),
                        u.shared - u.identical,
                        u.winner.clone().unwrap_or("game order".into())
                    ));
                }
            }
            out(v)
        }
        Note::Sorted { archives } => one(format!("applied; {archives} archives in modlist.txt")),
        Note::SortDryRun => one("dry run: `cpmm sort --apply` to save".into()),
        Note::Ruled { a, b, removed: false } => one(format!("rule: {a} wins over {b}")),
        Note::Ruled { a, b, removed: true } => one(format!("removed rules between {a} and {b}")),
        Note::Moved(mv) => one(format!("  moved {} {} -> {}", mv.id, mv.from, mv.to)),
        Note::RulesApplied { archives } => one(format!("load order updated; {archives} archives in modlist.txt")),
        Note::Acked { title, on } => one(format!("{} {title:?}", if *on { "acknowledged" } else { "un-acknowledged" })),
        Note::RolledBack(m) => one(format!("rolled back {m}")),
        Note::RolledBackMod(m) => one(format!("  rolled back {m}")),
        Note::Undid(b) => one(format!("undid {b}")),
        Note::Errors(d) => {
            let mut v = vec![];
            if d.baseline {
                v.push(format!("first log snapshot recorded ({} sources); differences show from the next launch", d.totals.len()));
            }
            for (src, n) in &d.totals {
                let new = d.new.get(src).map(|v| v.len()).unwrap_or(0);
                let fixed = d.fixed.get(src).map(|v| v.len()).unwrap_or(0);
                v.push(format!("  {src:<14} {n:>4} total  {new:>3} new  {fixed:>3} gone"));
            }
            for (src, ls) in &d.new {
                for l in ls.iter().take(15) {
                    v.push(format!("  NEW [{src}] {l}"));
                }
            }
            out(v)
        }
        Note::Vanilla(true) => one("next launch runs without mods (one time)".into()),
        Note::Vanilla(false) => one("next launch runs with mods".into()),
        Note::Snapshot(s) => one(format!("snapshot: {}", s.clone().unwrap_or("no saves found".into()))),
        Note::Restored { files, from } => one(format!("restored {files} files from {from}")),
        Note::ModioReviewed(n) => one(format!("marked {n} mod.io subscriptions as reviewed")),
        Note::LaunchOptionsSet { options, live } => {
            let mut v = vec!["launch options set:".to_string(), format!("  {options}")];
            if *live {
                v.push("(applied live in Steam)".into());
            }
            out(v)
        }
        Note::KeySaved { name, premium } => one(format!("saved the key for {name}{}", if *premium { " (premium)" } else { "" })),
        Note::KeyRemoved => one("Nexus API key removed".into()),
        Note::Key(KeyStatus::Valid { name, premium }) => one(format!(
            "key OK: {name}{}",
            if *premium { " (premium: downloads work from cpmm)" } else { " (free: use Mod Manager Download links)" }
        )),
        Note::Key(KeyStatus::Set(f)) => one(format!("a Nexus API key is set ({}); --check validates it", f.display())),
        Note::Key(KeyStatus::Missing) => {
            one("no Nexus API key; get yours at https://www.nexusmods.com/users/myaccount?tab=api and run `cpmm nexus-key <KEY>`".into())
        }
        Note::StagingMoved { mods, to } => one(format!("moved {mods} mods to {}", to.display())),
        Note::StagingOtherDrive => {
            one("note: the new folder is on a different drive than the game; installs and imports will copy instead of move".into())
        }
        Note::SettingsSaved => one("saved".into()),
        Note::Shortcut(s) => one(shortcut(s)),
        Note::Setup(s) => out(setup(s)),
        Note::Profile(p) => out(profile(p)),
        Note::Collection(c) => out(collection(c)),
        Note::Launch(l) => err(match l {
            LaunchNote::Vanilla => "cpmm: vanilla launch (no mods mounted)".into(),
            LaunchNote::ModlistFailed(e) => format!("cpmm: modlist.txt not updated: {e}"),
            LaunchNote::Mounted(n) => format!("cpmm: overlay mounted with {n} mod layers"),
            LaunchNote::Saves(s) => format!("cpmm: saves from {}", s.display()),
        }),
    }
}

fn import_lines(p: &Progress) -> Vec<String> {
    match p {
        Progress::Vanilla(n) => vec![
            format!("vanilla manifest: {n} files known"),
            format!("{:<34} {:>6} {:>7} {:>6} {:>6} {:>7}  name", "id", "nexus", "files", "moved", "copied", "missing"),
        ],
        Progress::Row { id, nexus, files, moved, copied, missing, name } => {
            let nid = nexus.map(|n| n.to_string()).unwrap_or("-".into());
            vec![format!("{id:<34} {nid:>6} {files:>7} {moved:>6} {copied:>6} {missing:>7}  {name}")]
        }
        Progress::Summary { mods, shared } => vec![
            String::new(),
            format!("{mods} mods, {shared} files claimed by more than one mod (each owner gets its own reflinked copy)"),
        ],
        Progress::Done { applied: true } => {
            vec!["imported. Base game dir now holds only vanilla + unmanaged files; launch through `cpmm run`.".into()]
        }
        Progress::Done { applied: false } => vec!["dry run; re-run with --apply to move files".into()],
    }
}

fn shortcut(s: &Step) -> String {
    match s {
        Step::NotThere { user: Some(u), name } => format!("user {u}: no {name} shortcut"),
        Step::NotThere { user: None, name } => format!("no {name} shortcut in the Steam library"),
        Step::Removing { user: Some(u), appid } => format!("user {u}: removing shortcut {appid} and its artwork"),
        Step::Removing { user: None, appid } => format!("removing shortcut {appid} (live, through the Steam client)"),
        Step::Refreshing { user: Some(u), appid } => format!("user {u}: shortcut {appid} already there; refreshing artwork"),
        Step::Refreshing { user: None, appid } => format!("shortcut {appid} already in the library; refreshing name and artwork"),
        Step::Adding { user: Some(u), appid, name, exe } => {
            format!("user {u}: adding shortcut {} ({name} -> {})", appid.unwrap_or_default(), exe.display())
        }
        Step::Adding { user: None, name, exe, .. } => format!("adding {name} -> {} (live, through the Steam client)", exe.display()),
        Step::Written => "done; Steam picks it up the next time it starts".into(),
        Step::Added { appid } => format!("added as app {appid} with capsule, wide, hero and logo artwork"),
    }
}

fn setup(s: &SetupStep) -> Vec<String> {
    match s {
        SetupStep::Start => vec!["cpmm setup".into(), String::new()],
        SetupStep::Game(g) => vec![format!("game: {}", g.display())],
        SetupStep::KeyNeeded => {
            vec![String::new(), "cpmm needs your personal Nexus Mods API key (https://www.nexusmods.com/users/myaccount?tab=api).".into()]
        }
        SetupStep::KeySet => vec!["Nexus API key: set".into()],
        SetupStep::LaunchOptions(Ok(n)) => vec![format!("launch options: {n}")],
        SetupStep::LaunchOptions(Err(e)) => vec![e.clone()],
        SetupStep::Done => vec![String::new(), "All set. `cpmm install <nexus url>` installs a mod; `cpmm-gui` opens the app.".into()],
    }
}

fn profile(p: &ProfileNote) -> Vec<String> {
    match p {
        ProfileNote::AlreadyActive(n) => vec![format!("{n:?} is already the active profile")],
        ProfileNote::Created { name, from } => vec![format!(
            "created profile {name:?} ({})",
            match from {
                None => "empty: every mod disabled".to_string(),
                Some(f) => format!("copy of {f:?}"),
            }
        )],
        ProfileNote::SwitchedTo(n) => vec![format!("switched to {n:?}")],
        ProfileNote::Switched { from, to, summary, new } => {
            let mut v = vec![format!("switched {from:?} -> {to:?}: {summary}")];
            if !new.is_empty() {
                v.push(format!("{} mod(s) new to this profile, added disabled: {}", new.len(), new.join(", ")));
            }
            v.push("takes effect at the next launch; `cpmm deps` / `cpmm doctor` check this list".into());
            v
        }
        ProfileNote::Renamed { old, new } => vec![format!("renamed profile {old:?} -> {new:?}")],
        ProfileNote::Deleted(n) => vec![format!("deleted profile {n:?} (mods stay installed)")],
        ProfileNote::SavesTrashed(t) => vec![format!("its own saves moved to {} (delete when sure)", t.display())],
        ProfileNote::SharedSaves { name, kept, dir } => vec![format!(
            "{name:?} now uses the game's shared saves{}",
            if *kept > 0 { format!(" (its {kept} own saves stay in {})", dir.display()) } else { String::new() }
        )],
        ProfileNote::SavesCopied { saves, files, from } => vec![format!("copied {saves} saves ({files} files) from {}", from.display())],
        ProfileNote::OwnSaves { name, dir, saves } => {
            vec![format!("{name:?} uses its own saves in {} ({saves} saves; not synced by Steam Cloud)", dir.display())]
        }
    }
}

fn collection(c: &CollectionNote) -> Vec<String> {
    match c {
        CollectionNote::Start { name, revision, author, mods } => vec![format!("{name} (revision {revision}) by {author}: {mods} mods")],
        CollectionNote::ProfileCreated(p) => vec![format!("created profile {p:?} (your other profiles are untouched)")],
        CollectionNote::Into(p) => vec![format!("installing into profile {p:?}")],
        CollectionNote::Step { n, total, name, version } => vec![format!("[{n}/{total}] {name} {version}")],
        CollectionNote::Failed(e) => vec![format!("  failed: {e}")],
        CollectionNote::Done { name, optional, instructions, done } => {
            let mut v = vec![
                String::new(),
                format!(
                    "{name}: {} installed, {} optional skipped{}, {} load-order rules",
                    done.installed,
                    done.skipped,
                    if *optional { "" } else { " (--optional adds them)" },
                    done.rules
                ),
            ];
            if !done.failed.is_empty() {
                v.push(format!("{} failed (run the same command again to retry just those):", done.failed.len()));
                v.extend(done.failed.iter().map(|f| format!("  {f}")));
            }
            v.extend(done.manual.iter().map(|m| format!("  manual: {m}")));
            if !instructions.trim().is_empty() {
                v.push(String::new());
                v.push(format!("Curator's notes: {}", instructions.chars().take(400).collect::<String>().replace('\n', " ")));
            }
            v
        }
    }
}
