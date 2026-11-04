//! The change protocol: one unit of work around every mutation of the mod setup.
//!
//! A command describes its edit and what it needs (a [`Change`]); [`Change::apply`] runs the
//! steps in one fixed order:
//!
//! ```text
//! lock held? → idle check → save snapshot? → edit (batch tag in scope) → journal → save
//!            → auto-sort → modlist.txt → running-session notice
//! ```
//!
//! `State::save` is private to this module, so State only reaches disk through a change, and
//! the journal only records what was saved. Long edits (installs with requirements, update
//! runs, collections) persist progress with [`Edit::save`], which journals and saves the same
//! way, so a later failure keeps what was done.

use crate::loadorder;
use crate::safety::{self, JEntry};
use crate::state::{Paths, State};
use anyhow::{Result, bail};
use std::cell::Cell;
use std::fs;

/* ------------------------------------------------------------------ lock */

thread_local! {
    /// How many cpmm locks this thread holds (the flock itself is per open file).
    static HELD: Cell<usize> = const { Cell::new(0) };
}

/// The cpmm lock: serialises commands that change state (two nxm clicks at once, etc.).
/// Commands declare whether they change state; those take it before reading State.
pub struct Lock {
    _file: fs::File,
}

impl Lock {
    pub fn acquire(paths: &Paths) -> Result<Lock> {
        use std::os::fd::AsRawFd;
        fs::create_dir_all(&paths.data)?;
        let f = fs::OpenOptions::new().create(true).truncate(false).write(true).open(paths.data.join("cpmm.lock"))?;
        if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            eprintln!("waiting for another cpmm command to finish…");
            if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX) } != 0 {
                bail!("flock: {}", std::io::Error::last_os_error());
            }
        }
        HELD.with(|h| h.set(h.get() + 1));
        Ok(Lock { _file: f })
    }
    fn held() -> bool {
        HELD.with(|h| h.get() > 0)
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        HELD.with(|h| h.set(h.get() - 1));
    }
}

/* ------------------------------------------------------------------ preconditions */

/// What may be running while the change happens.
pub enum Idle {
    /// Neither a cpmm game session nor the game process (the default for setup changes).
    Required,
    /// Only a cpmm session blocks: staged files are in use only through its overlay, so a game
    /// started some other way doesn't matter. The message says why.
    NoSession(&'static str),
    /// Only the game process blocks (it holds the saves or the game dir's files).
    NoGame(&'static str),
    /// Anything may run; a running session sees the change at its next launch (the change
    /// prints a notice).
    NotNeeded,
}

impl Idle {
    /// Fail if something the operation conflicts with is running. Changes check this
    /// themselves; operations outside State (saves, overwrite/, eject) call it directly.
    pub fn check(&self, paths: &Paths) -> Result<()> {
        let session = || crate::vfs::running(paths).is_some();
        match self {
            Idle::Required if session() || crate::game_running() => {
                bail!("the game (or a cpmm session) is running and has these files mounted; quit it first")
            }
            Idle::NoSession(msg) if session() => bail!("{msg}"),
            Idle::NoGame(msg) if crate::game_running() => bail!("{msg}"),
            _ => Ok(()),
        }
    }
}

/* ------------------------------------------------------------------ the change */

#[derive(Clone, Copy)]
enum Sort {
    /// If the user's `auto_sort` setting is on.
    Auto,
    /// Regardless of the setting (a collection's curator rules only matter once applied).
    Always,
    /// Never: the change places mods itself (manual order, sort, rules) or can't affect order.
    Keep,
}

/// A change to the mod setup, described by the command and applied by [`Change::apply`].
pub struct Change {
    idle: Idle,
    snapshot: Option<String>,
    batch: Option<String>,
    sort: Sort,
    /// Affects what the game loads: regenerate modlist.txt and tell a running session.
    game_facing: bool,
}

impl Change {
    /// A change to what the game loads (mods, their files, enabled state, order, profile).
    /// Defaults: nothing may be running, auto-sort per the setting, modlist.txt regenerated,
    /// a notice if a session is running.
    pub fn setup() -> Change {
        Change { idle: Idle::Required, snapshot: None, batch: None, sort: Sort::Auto, game_facing: true }
    }
    /// A change to cpmm's own records in State (acks, ignored requirements, the requirement
    /// cache, profile bookkeeping): saved and journalled, nothing else.
    pub fn records() -> Change {
        Change { idle: Idle::NotNeeded, snapshot: None, batch: None, sort: Sort::Keep, game_facing: false }
    }
    pub fn idle(mut self, idle: Idle) -> Change {
        self.idle = idle;
        self
    }
    /// Snapshot the active profile's saves first (if the user hasn't turned that off).
    pub fn snapshot(mut self, reason: impl Into<String>) -> Change {
        self.snapshot = Some(reason.into());
        self
    }
    /// Tag every mod version this change replaces, so `cpmm undo-update` can undo them together.
    pub fn batch(mut self, tag: impl Into<String>) -> Change {
        self.batch = Some(tag.into());
        self
    }
    /// Don't auto-sort afterwards.
    pub fn keep_order(mut self) -> Change {
        self.sort = Sort::Keep;
        self
    }
    /// Sort afterwards even if the auto_sort setting is off.
    pub fn always_sort(mut self) -> Change {
        self.sort = Sort::Always;
        self
    }

    /// Run the change. Fails before the edit if the cpmm lock isn't held or something that
    /// must not run is running. If the edit fails, nothing after its last [`Edit::save`] is
    /// saved or journalled.
    pub fn apply<T>(self, paths: &Paths, st: &mut State, edit: impl FnOnce(&mut Edit, &mut State) -> Result<T>) -> Result<T> {
        if !Lock::held() {
            bail!("internal error: changing the mod setup without the cpmm lock (declare the command in Cmd::changes_state)");
        }
        self.idle.check(paths)?;
        if let Some(reason) = &self.snapshot {
            safety::auto_snapshot(paths, reason);
        }
        let mut cx = Edit { paths, batch: self.batch.as_deref(), sort: self.sort, pending: vec![] };
        let out = edit(&mut cx, st)?;
        cx.save(st)?;
        cx.sort(st);
        if self.game_facing {
            let _ = loadorder::write_modlist(paths, st);
            if crate::vfs::running(paths).is_some() {
                eprintln!("note: a game session is running; changes apply on next launch");
            }
        }
        Ok(out)
    }
}

/// The edit's view of its change: where things live, the batch tag, and the journal.
pub struct Edit<'a> {
    paths: &'a Paths,
    batch: Option<&'a str>,
    sort: Sort,
    pending: Vec<JEntry>,
}

impl<'a> Edit<'a> {
    pub fn paths(&self) -> &'a Paths {
        self.paths
    }
    /// The update batch this change's replaced versions belong to.
    pub fn batch(&self) -> Option<&'a str> {
        self.batch
    }
    /// Record what happened, for crash triage ("what changed since the last good session").
    /// Written with the next save, so it never describes unsaved work.
    pub fn journal(&mut self, action: &str, mods: &[String], detail: &str) {
        self.pending.push(JEntry { ts: safety::now_unix(), action: action.into(), mods: mods.to_vec(), detail: detail.into() });
    }
    /// Persist progress now: journal what was recorded, then save State.
    pub fn save(&mut self, st: &State) -> Result<()> {
        for e in self.pending.drain(..) {
            safety::journal(self.paths, &e);
        }
        st.save(self.paths)
    }
    /// The change's sort step, now (post-install hooks want the sorted order). Saves if it
    /// moves anything.
    pub fn sort(&mut self, st: &mut State) {
        let on = match self.sort {
            Sort::Auto => self.paths.cfg.auto_sort,
            Sort::Always => true,
            Sort::Keep => false,
        };
        if !on {
            return;
        }
        let p = loadorder::plan(self.paths, st);
        if !p.moved.is_empty() {
            loadorder::apply(st, &p.order);
            if self.save(st).is_ok() {
                let _ = loadorder::write_modlist(self.paths, st);
                println!("  auto-sorted the load order ({} moves)", p.moved.len());
            }
        }
    }
}

impl State {
    /// Atomic write (keeps the previous file as state.json.bak). Private to this module.
    fn save(&self, p: &Paths) -> Result<()> {
        fs::create_dir_all(&p.data)?;
        let tmp = p.data.join("state.json.tmp");
        let mut v = serde_json::to_value(self)?;
        v["profiles"] = serde_json::to_value(self.synced_profiles())?;
        fs::write(&tmp, serde_json::to_vec_pretty(&v)?)?;
        if p.state_file().exists() {
            let _ = fs::copy(p.state_file(), p.data.join("state.json.bak"));
        }
        fs::rename(tmp, p.state_file())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loadorder::Rule;

    fn paths(tag: &str) -> Paths {
        let d = std::env::temp_dir().join(format!("cpmm-change-test-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(d.join("game")).unwrap();
        Paths { data: d.clone(), game: d.join("game"), cfg: Default::default() }
    }

    /// Mods c, a (c first, so lower priority) and a rule "c loads after a": a sort moves c.
    fn state() -> State {
        let m = |id: &str| serde_json::from_value(serde_json::json!({"id": id, "name": id, "enabled": true})).unwrap();
        let mut s: State = serde_json::from_value(serde_json::json!({"version": 1, "mods": []})).unwrap();
        s.mods = vec![m("c"), m("a")];
        s.rules = vec![Rule { r#mod: "c".into(), after: "a".into() }];
        s.ensure_profiles();
        s
    }

    fn order(paths: &Paths) -> Vec<String> {
        State::load(paths).unwrap().mods.iter().map(|m| m.id.clone()).collect()
    }

    fn journal(paths: &Paths) -> Vec<String> {
        safety::read_journal(paths).iter().map(|e| format!("{} {}", e.action, e.mods.join(","))).collect()
    }

    #[test]
    fn refuses_without_the_lock() {
        let p = paths("nolock");
        let mut st = state();
        let r = Change::records().apply(&p, &mut st, |_, _| Ok(()));
        assert!(r.unwrap_err().to_string().contains("without the cpmm lock"));
        assert!(!p.state_file().exists());
        let _ = fs::remove_dir_all(&p.data);
    }

    #[test]
    fn saves_and_journals_what_the_edit_did() {
        let p = paths("saves");
        let _l = Lock::acquire(&p).unwrap();
        let mut st = state();
        Change::setup()
            .idle(Idle::NotNeeded)
            .keep_order()
            .apply(&p, &mut st, |cx, st| {
                st.mods[0].enabled = false;
                cx.journal("disable", &["c".into()], "");
                Ok(())
            })
            .unwrap();
        assert!(!State::load(&p).unwrap().mods[0].enabled);
        assert_eq!(journal(&p), ["disable c"]);
        let _ = fs::remove_dir_all(&p.data);
    }

    #[test]
    fn failed_edit_keeps_only_saved_progress() {
        let p = paths("fails");
        let _l = Lock::acquire(&p).unwrap();
        let mut st = state();
        let r = Change::setup().idle(Idle::NotNeeded).apply(&p, &mut st, |cx, st| -> Result<()> {
            st.mods[0].name = "first".into();
            cx.journal("install", &["c".into()], "1.0");
            cx.save(st)?;
            st.mods[1].name = "second".into();
            cx.journal("install", &["a".into()], "1.0");
            bail!("download failed")
        });
        assert!(r.is_err());
        let saved = State::load(&p).unwrap();
        assert_eq!((saved.mods[0].name.as_str(), saved.mods[1].name.as_str()), ("first", "a"));
        assert_eq!(journal(&p), ["install c"]);
        assert_eq!(order(&p), ["c", "a"], "no sort after a failed edit");
        let _ = fs::remove_dir_all(&p.data);
    }

    #[test]
    fn sort_step_follows_the_declaration() {
        let p = paths("sort");
        let _l = Lock::acquire(&p).unwrap();
        let noop = |_: &mut Edit, _: &mut State| Ok(());
        Change::setup().idle(Idle::NotNeeded).keep_order().apply(&p, &mut state(), noop).unwrap();
        assert_eq!(order(&p), ["c", "a"]);
        Change::records().apply(&p, &mut state(), noop).unwrap();
        assert_eq!(order(&p), ["c", "a"]);
        Change::setup().idle(Idle::NotNeeded).apply(&p, &mut state(), noop).unwrap();
        assert_eq!(order(&p), ["a", "c"], "auto_sort is on by default");

        let mut off = p.clone();
        off.cfg.auto_sort = false;
        Change::setup().idle(Idle::NotNeeded).apply(&off, &mut state(), noop).unwrap();
        assert_eq!(order(&p), ["c", "a"]);
        Change::setup().idle(Idle::NotNeeded).always_sort().apply(&off, &mut state(), noop).unwrap();
        assert_eq!(order(&p), ["a", "c"]);
        let _ = fs::remove_dir_all(&p.data);
    }

    #[test]
    fn game_facing_changes_regenerate_the_modlist() {
        let p = paths("modlist");
        let _l = Lock::acquire(&p).unwrap();
        let mut st = state();
        let arc = p.mod_files("a").join("archive/pc/mod/a.archive");
        fs::create_dir_all(arc.parent().unwrap()).unwrap();
        fs::write(&arc, b"").unwrap();
        let list = loadorder::generated_root(&p).join("files/archive/pc/mod/modlist.txt");
        Change::records().apply(&p, &mut st, |_, _| Ok(())).unwrap();
        assert!(!list.exists());
        Change::setup().idle(Idle::NotNeeded).keep_order().apply(&p, &mut st, |_, _| Ok(())).unwrap();
        assert_eq!(fs::read_to_string(&list).unwrap(), "a.archive\r\n");
        let _ = fs::remove_dir_all(&p.data);
    }

    #[test]
    fn batch_tag_is_scoped_to_its_change() {
        let p = paths("batch");
        let _l = Lock::acquire(&p).unwrap();
        let mut st = state();
        fs::create_dir_all(p.mod_files("c")).unwrap();
        // what staging does when an install replaces mod c
        let replace = |cx: &mut Edit, st: &mut State| {
            safety::archive_current(cx.paths(), &st.mods[st.get("c")?], "replaced", cx.batch())?;
            fs::create_dir_all(cx.paths().mod_files("c"))?;
            Ok(())
        };
        Change::setup().idle(Idle::NotNeeded).batch("update-1").apply(&p, &mut st, replace).unwrap();
        Change::setup().idle(Idle::NotNeeded).apply(&p, &mut st, replace).unwrap();
        let tags: Vec<Option<String>> = safety::list_history(&p, "c").into_iter().map(|h| h.meta.batch).collect();
        assert_eq!(tags, [None, Some("update-1".into())]); // newest first
        let _ = fs::remove_dir_all(&p.data);
    }

    #[test]
    fn idle_check_runs_before_the_edit() {
        let p = paths("idle");
        let _l = Lock::acquire(&p).unwrap();
        // a cpmm session whose mount namespace is this test's own: vfs::running finds it
        let ns = fs::read_link("/proc/self/ns/mnt").unwrap().to_string_lossy().into_owned();
        let run = serde_json::json!({"pid": std::process::id(), "mnt_ns": ns, "layers": [], "started": "", "command": []});
        fs::write(p.run_file(), run.to_string()).unwrap();
        let mut st = state();
        let mut ran = false;
        let r = Change::setup().apply(&p, &mut st, |_, _| {
            ran = true;
            Ok(())
        });
        assert!(r.unwrap_err().to_string().contains("quit it first"));
        let r = Change::setup().idle(Idle::NoSession("session running")).apply(&p, &mut st, |_, _| Ok(()));
        assert_eq!(r.unwrap_err().to_string(), "session running");
        assert!(!ran && !p.state_file().exists());
        // allowed while running: applied (the notice goes to stderr)
        Change::setup().idle(Idle::NotNeeded).apply(&p, &mut st, |_, _| Ok(())).unwrap();
        assert!(p.state_file().exists());
        let _ = fs::remove_dir_all(&p.data);
    }
}
