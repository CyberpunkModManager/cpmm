//! Installing a mod: one pipeline from a [`Source`] to a staged, recorded mod.
//!
//! ```text
//! source → archive → requirements (recursion-guarded) → unpack → FOMOD or layout plan → stage
//!        → record on the mod → [re-hide → save → journal → new conflicts → after-install hooks]
//! ```
//!
//! Callers pick a source and a [`Policy`], render the [`Event`]s as they happen ([`print`] is
//! the CLI's rendering; the GUI reads its "installed X: N files" lines) and get a [`Report`]
//! back. Nexus is reached through the [`Releases`] port, so tests run the whole pipeline
//! against a fake.

mod stage;
#[cfg(test)]
mod tests;

pub use stage::extract;

use crate::conflicts::{self, FileConflict};
use crate::downloads::Download;
use crate::fomod::{self, Choices};
use crate::loadorder::{self, Finding};
use crate::nexus::{self, Manifest, ModInfo, Nexus, Version};
use crate::requirements::{self, Status};
use crate::state::{Mod, NexusSource, Paths, ReqCheck, Requirement, State, now, short_name, slugify};
use crate::{files, manage, safety};
use anyhow::{Context, Result, bail};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

/// What to install.
pub enum Source<'a> {
    /// A Nexus mod. `file` selects one of its files as `--file` does (a file or version id, or
    /// part of a name); without it, the main file.
    Nexus { mod_id: u64, file: Option<String> },
    /// A local archive or folder, optionally tagged with the Nexus mod it is from: that mod's
    /// requirements are then checked, and an install of it is replaced.
    Local { path: PathBuf, nexus: Option<u64> },
    /// An entry of the Downloads list. A Nexus download installs as its exact Nexus file
    /// (reusing the archive on disk, so update tracking stays exact); anything else installs
    /// as a local archive named after the download. The archive is always kept.
    Download(&'a Download),
}

/// What to do about a hard requirement that is not installed.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub enum OnMissing {
    /// Fail before anything is staged.
    #[default]
    Refuse,
    /// Install it from Nexus first (`--with-deps`), and its requirements in turn.
    Install,
    /// Install anyway and say how to fix it (`--ignore-deps`, updates, collections).
    Warn,
}

/// How to install. `id`, `name`, `root`, `dest` and `fomod` apply to the requested mod only,
/// not to the requirements it pulls in.
#[derive(Clone)]
pub struct Policy {
    /// Mod id (default: from the name). For a local source, an installed id is replaced.
    pub id: Option<String>,
    pub name: Option<String>,
    /// Layout overrides (`--root`, `--dest`); either one skips a FOMOD installer.
    pub root: Option<String>,
    pub dest: Option<String>,
    /// Explicit FOMOD choices. On an installed Nexus file they mean "reinstall with these".
    pub fomod: Option<Choices>,
    /// FOMOD choice fallback after explicit and saved choices: ask in a terminal when set,
    /// else (and off a terminal) the installer's suggested options.
    pub ask: bool,
    pub missing: OnMissing,
    /// New mods arrive disabled.
    pub disabled: bool,
    /// Run the after-install tasks from settings for the requested mod. Off for bulk
    /// installs, which sort once at the end.
    pub hooks: bool,
    /// Keep a downloaded Nexus archive even when settings say not to.
    pub keep_archive: bool,
}

impl Default for Policy {
    fn default() -> Self {
        Policy {
            id: None,
            name: None,
            root: None,
            dest: None,
            fomod: None,
            ask: false,
            missing: OnMissing::Refuse,
            disabled: false,
            hooks: true,
            keep_archive: false,
        }
    }
}

/// What an install did.
pub struct Report {
    /// Every mod installed, updated or found up to date, in the order they finished:
    /// requirements before the mods needing them, the requested mod last.
    pub mods: Vec<Done>,
}

impl Report {
    /// The mod that was asked for.
    pub fn requested(&self) -> &Done {
        self.mods.last().expect("a successful install records the requested mod")
    }
}

/// One mod the install finished.
pub struct Done {
    pub id: String,
    /// 0 for the requested mod, 1 for its requirements, and so on.
    pub depth: usize,
    pub origin: Origin,
    pub change: Change,
    /// Files staged, and documentation files left out.
    pub files: usize,
    pub docs_skipped: usize,
    /// Files the user hid in an earlier version, hidden again.
    pub rehidden: usize,
    /// New conflicts: files it shares with other enabled mods, with different contents.
    pub conflicts: Vec<FileConflict>,
    /// Its files shadowed by game-written copies in overwrite/.
    pub shadowed: usize,
    /// Its requirements at install time and what was done about them.
    #[allow(dead_code)] // part of the result; the CLI prints them as they happen instead
    pub requirements: Vec<Need>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Origin {
    Nexus,
    Local,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Change {
    /// A new mod.
    Installed,
    /// An installed Nexus file moved to another version.
    Updated { from: String, to: String },
    /// An installed mod got new files from a local source.
    Replaced,
    /// The installed Nexus file is this version already; only its requirements were stored.
    Unchanged,
}

/// One requirement of a mod being installed.
pub struct Need {
    pub req: Requirement,
    pub status: Status,
    /// The installed mod that provides it, if any.
    pub dep: Option<String>,
    /// For a missing hard requirement: what the install did about it.
    pub fix: Option<Fix>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Fix {
    /// Installed from Nexus as part of this install.
    Installed,
    /// Left missing (`OnMissing::Warn`).
    Accepted,
    /// It is a mod this install is already working on further up (a requirement cycle).
    Upstream,
}

/// Progress, as it happens.
pub enum Event<'a> {
    /// The Nexus file picked for a mod.
    Release {
        depth: usize,
        name: &'a str,
        mod_id: u64,
        version: &'a Version,
    },
    /// A requirement outside Nexus (not checked).
    External {
        depth: usize,
        req: &'a nexus::Req,
    },
    /// A requirement's status; for a missing one, what is being done about it. `label` is the
    /// mod the requirement belongs to.
    Requirement {
        depth: usize,
        label: &'a str,
        need: &'a Need,
    },
    Downloading {
        depth: usize,
        bytes: u64,
    },
    Fomod {
        depth: usize,
        note: Note<'a>,
    },
    /// Files staged and the mod recorded (or found up to date).
    Staged(&'a Done),
    /// Saved, with hidden files re-hidden and new conflicts found.
    Settled(&'a Done),
    /// After-install doctor findings that mention the mod.
    Health(&'a [Finding]),
    /// The after-install command ran.
    Command(&'a std::io::Result<std::process::ExitStatus>),
}

pub enum Note<'a> {
    /// Choices saved on the mod from its last install.
    Saved,
    /// The installer's suggested options.
    Defaults,
    /// Saved choices no longer fit the installer; its suggested options are used instead.
    Stale(&'a anyhow::Error),
    /// The options in effect for one step/group.
    Chose(&'a str, &'a [String]),
}

/// The Nexus side of an install: the port the pipeline reaches Nexus through. [`Nexus`] is the
/// production adapter; tests use a fake.
pub trait Releases {
    /// The mod page and the file version to install, `file` selecting as `--file` does.
    fn release(&self, mod_id: u64, file: Option<&str>) -> Result<(ModInfo, Version)>;
    /// A mod's requirements for one file version (any version for ""): the external ones to
    /// show, and the list to store on the mod.
    fn requires(&self, mod_id: u64, version_id: &str) -> Result<(Vec<nexus::Req>, Vec<Requirement>)>;
    /// The archive of a version in the Downloads list, downloading it first if needed
    /// (`downloading` is told the size before that), and its manifest.
    fn archive(
        &self,
        paths: &Paths,
        info: &ModInfo,
        mod_id: u64,
        ver: &Version,
        manifest_url: &str,
        downloading: &mut dyn FnMut(u64),
    ) -> Result<(PathBuf, Manifest)>;
}

impl Releases for Nexus {
    fn release(&self, mod_id: u64, file: Option<&str>) -> Result<(ModInfo, Version)> {
        let info = self.mod_info(mod_id)?;
        let ver = crate::pick_file(self, &info, file)?;
        Ok((info, ver))
    }

    fn requires(&self, mod_id: u64, version_id: &str) -> Result<(Vec<nexus::Req>, Vec<Requirement>)> {
        crate::fetch_requirements(self, mod_id, version_id, false, false)
    }

    fn archive(
        &self,
        paths: &Paths,
        info: &ModInfo,
        mod_id: u64,
        ver: &Version,
        manifest_url: &str,
        downloading: &mut dyn FnMut(u64),
    ) -> Result<(PathBuf, Manifest)> {
        let man = self.manifest(manifest_url)?;
        let arc = paths.downloads().join(format!("{}-{}", ver.game_scoped_id, slugify(&ver.file.name)));
        if !arc.exists() {
            downloading(man.archive.size);
        }
        self.download(ver, &man, &arc, None)?;
        crate::record_download(&arc, &info.name, mod_id, ver, "nexus");
        Ok((arc, man))
    }
}

/// Install `source` under `policy`, telling `log` what happens as it happens.
///
/// Missing requirements are handled per `policy.missing` before anything of the mod is
/// staged; each Nexus mod is installed at most once per call, so requirement cycles end.
/// On error, mods finished before it (requirements) stay installed.
pub fn run(
    paths: &Paths,
    st: &mut State,
    nexus: &dyn Releases,
    source: Source,
    policy: &Policy,
    mut log: impl FnMut(&Event),
) -> Result<Report> {
    let mut policy = policy.clone();
    let source = match source {
        Source::Download(d) => {
            policy.keep_archive = true;
            match (d.meta.mod_id, d.meta.version_gsid.is_empty()) {
                (Some(gid), false) => Source::Nexus { mod_id: gid, file: Some(d.meta.version_gsid.clone()) },
                (gid, _) => {
                    if !d.meta.name.is_empty() {
                        policy.name.get_or_insert_with(|| d.meta.name.clone());
                    }
                    Source::Local { path: d.path.clone(), nexus: gid }
                }
            }
        }
        s => s,
    };
    let mut r = Run { paths, nexus, policy, visiting: BTreeSet::new(), log: &mut log, done: vec![] };
    match source {
        Source::Nexus { mod_id, file } => r.nexus_mod(st, mod_id, file.as_deref(), 0)?,
        Source::Local { path, nexus } => r.local(st, &path, nexus)?,
        Source::Download(_) => unreachable!("resolved above"),
    }
    Ok(Report { mods: r.done })
}

/// The CLI's rendering of an event, one or more lines on stdout.
pub fn print(e: &Event) {
    for l in lines(e) {
        println!("{l}");
    }
}

fn notes(r: &Requirement) -> String {
    if r.notes.is_empty() { String::new() } else { format!(" — {}", r.notes) }
}

fn lines(e: &Event) -> Vec<String> {
    let ind = |d: usize| "  ".repeat(d);
    match e {
        Event::Release { depth, name, mod_id, version: v } => {
            vec![format!("{}{name} ({mod_id}): file {:?} v{} [{}]", ind(*depth), v.file.name, v.version, v.category)]
        }
        Event::External { depth, req } => vec![format!("{}  external requirement: {} {}", ind(*depth), req.name, req.url)],
        Event::Requirement { depth, label, need: n } => {
            let (i, r) = (ind(*depth), &n.req);
            let dep = n.dep.as_deref().unwrap_or("?");
            match (n.status, n.fix) {
                (Status::Ignored, _) => vec![format!("{i}  requires {} {}: not installed, ignored by your choice", r.mod_id, r.name)],
                (Status::Ok, _) => vec![format!("{i}  requires {} {}: installed", r.mod_id, r.name)],
                (Status::Disabled | Status::OptionalDisabled, _) => {
                    vec![format!("{i}  requires {} {}: installed but DISABLED ({dep}; cpmm enable {dep})", r.mod_id, r.name)]
                }
                (Status::OptionalMissing, _) => vec![format!("{i}  optional {} {}: not installed{}", r.mod_id, r.name, notes(r))],
                (Status::Missing, Some(Fix::Installed)) => {
                    vec![format!("{i}  installing requirement {} {}{}", r.mod_id, r.name, notes(r))]
                }
                (Status::Missing, Some(Fix::Accepted)) => vec![
                    format!("{i}  WARNING: requires {} {}, which is NOT installed{}", r.mod_id, r.name, notes(r)),
                    format!("{i}           `cpmm deps --fix` installs it; `cpmm deps --ignore {label} {}` accepts it", r.mod_id),
                ],
                (Status::Missing, Some(Fix::Upstream) | None) => vec![],
            }
        }
        Event::Downloading { depth, bytes } => vec![format!("{}  downloading {:.1} MB", ind(*depth), *bytes as f64 / 1048576.0)],
        Event::Fomod { depth, note } => {
            let i = ind(*depth);
            vec![match note {
                Note::Saved => format!("{i}  FOMOD: using the options you chose last time"),
                Note::Defaults => format!("{i}  FOMOD: using the installer's suggested options"),
                Note::Stale(e) => format!("{i}  FOMOD: saved options no longer fit ({e:#}); using the suggested ones"),
                Note::Chose(k, v) => format!("{i}  FOMOD {k}: {}", if v.is_empty() { "(none)".into() } else { v.join(", ") }),
            }]
        }
        Event::Staged(d) => {
            let (i, id) = (ind(d.depth), &d.id);
            vec![match (&d.change, d.origin) {
                (Change::Installed, Origin::Nexus) => format!(
                    "{i}  installed {id}: {} files{}",
                    d.files,
                    if d.docs_skipped == 0 { String::new() } else { format!(" ({} docs skipped)", d.docs_skipped) }
                ),
                (Change::Installed, Origin::Local) => format!("{i}installed {id}: {} files", d.files),
                (Change::Replaced, _) => format!("{i}replaced files of {id}: {} files", d.files),
                (Change::Updated { from, to }, _) => format!("{i}  updated {id}: {from} -> {to}"),
                (Change::Unchanged, _) => format!("{i}  already installed as {id}"),
            }]
        }
        Event::Settled(d) => {
            let i = ind(d.depth);
            let mut out = vec![];
            if d.origin == Origin::Nexus && d.rehidden > 0 {
                out.push(format!("{i}  re-hid {} files you hid earlier", d.rehidden));
            }
            for c in d.conflicts.iter().take(15) {
                out.push(format!("{i}  conflict: {}  {}", c.path, c.mods.join(" < ")));
            }
            if d.conflicts.len() > 15 {
                out.push(format!("{i}  … {} conflicting files in total (cpmm conflicts {})", d.conflicts.len(), d.id));
            }
            if d.shadowed > 0 {
                out.push(format!(
                    "{i}  note: {} of its files are shadowed by game-written copies in overwrite/ (cpmm overwrite)",
                    d.shadowed
                ));
            }
            out
        }
        Event::Health([]) => vec!["  health: no new risks".into()],
        Event::Health(hits) => hits.iter().map(|f| format!("  health: [{}] {}", f.severity, f.title)).collect(),
        Event::Command(Ok(s)) if s.success() => vec![],
        Event::Command(Ok(s)) => vec![format!("  after-install command exited with {s}")],
        Event::Command(Err(e)) => vec![format!("  after-install command failed: {e}")],
    }
}

/// One call of [`run`]: the policy in effect, the recursion guard and what finished.
struct Run<'r> {
    paths: &'r Paths,
    nexus: &'r dyn Releases,
    policy: Policy,
    /// Nexus mods this run has started; a requirement on one of them is not installed again.
    visiting: BTreeSet<u64>,
    log: &'r mut dyn FnMut(&Event),
    done: Vec<Done>,
}

/// A mod's files staged from an unpacked tree.
struct Laid {
    files: usize,
    skipped: usize,
    fomod: Option<Choices>,
}

impl Run<'_> {
    fn emit(&mut self, e: &Event) {
        (self.log)(e)
    }

    fn nexus_mod(&mut self, st: &mut State, gid: u64, file: Option<&str>, depth: usize) -> Result<()> {
        if !self.visiting.insert(gid) {
            return Ok(());
        }
        let target = depth == 0;
        let (info, ver) = self.nexus.release(gid, if target { file } else { None })?;
        self.emit(&Event::Release { depth, name: &info.name, mod_id: gid, version: &ver });

        // Requirements of this exact file version: checked now, then stored on the mod.
        let (external, stored) = self.nexus.requires(gid, &ver.id)?;
        for req in &external {
            self.emit(&Event::External { depth, req });
        }
        // The same install only if it's the same Nexus *file*: a page can offer several (a car
        // and its tuning-parts FOMOD), and installing one must not replace the other.
        let existing = st
            .mods
            .iter()
            .find(|m| m.nexus.as_ref().is_some_and(|n| n.mod_id == gid && (n.file_id.is_empty() || n.file_id == ver.file.id)))
            .map(|m| m.id.clone());
        let sibling = existing.is_none() && st.by_nexus(gid).is_some();
        let ignored = existing.as_ref().and_then(|e| st.find(e)).map(|i| st.mods[i].ignored_reqs.clone()).unwrap_or_default();
        let label = existing.clone().unwrap_or_else(|| slugify(short_name(&info.name)));
        let needs = self.requirements(st, &label, &stored, &ignored, depth)?;
        let checked = ReqCheck { at: now(), version_id: ver.id.clone() };

        if let Some(e) = &existing {
            let i = st.get(e)?;
            // explicit FOMOD choices mean "reinstall with these options"
            let reconfigure = target && self.policy.fomod.is_some();
            if !reconfigure && st.mods[i].nexus.as_ref().is_some_and(|n| n.version_id == ver.id) {
                st.mods[i].requires = stored;
                st.mods[i].reqs_checked = Some(checked);
                let done = Done::new(e.clone(), depth, Origin::Nexus, Change::Unchanged, needs);
                self.emit(&Event::Staged(&done));
                return self.record(st, e, done, None);
            }
        }
        let id = if target { self.policy.id.clone() } else { None }.or(existing.clone()).unwrap_or_else(|| {
            let base = slugify(short_name(&info.name));
            st.unique_id(&if sibling { slugify(&format!("{base} {}", ver.file.name)) } else { base })
        });
        let manifest_url = ver.manifest_url.clone().context("version has no manifest")?;
        self.paths.ensure_dirs()?;
        let (paths, nexus, log) = (self.paths, self.nexus, &mut *self.log);
        let (arc, man) = nexus.archive(paths, &info, gid, &ver, &manifest_url, &mut |bytes| log(&Event::Downloading { depth, bytes }))?;

        let x = unpack(paths, &id, &arc)?;
        // per-file hash check against the manifest
        let bad = man
            .files
            .iter()
            .filter(|f| !f.file_path.ends_with('/'))
            .filter(|f| !matches!(files::sha256_file(&x.join(&f.file_path)), Ok(h) if h == f.file_hashes.sha256))
            .count();
        if bad > 0 {
            bail!("{bad} extracted files don't match the Nexus manifest");
        }
        let laid = self.lay_out(st, &id, &x, depth)?;
        let archive = if !self.policy.keep_archive && !paths.cfg.keep_archives {
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
        let (shown, change) = match existing.and_then(|e| st.find(&e)) {
            Some(i) => {
                let m = &mut st.mods[i];
                let from = std::mem::replace(&mut m.version, ver.version.clone());
                m.nexus = Some(src);
                m.requires = stored;
                m.reqs_checked = Some(checked);
                m.installed_at = now();
                m.archive = archive;
                (m.id.clone(), Change::Updated { from, to: ver.version.clone() })
            }
            None => {
                st.mods.push(Mod {
                    id: id.clone(),
                    name: self.policy.name.clone().filter(|_| target).unwrap_or(info.name.clone()),
                    enabled: !self.policy.disabled,
                    version: ver.version.clone(),
                    nexus: Some(src),
                    requires: stored,
                    reqs_checked: Some(checked),
                    hidden: vec![],
                    ignored_reqs: vec![],
                    fomod: None,
                    archive,
                    installed_at: now(),
                    notes: String::new(),
                });
                (id.clone(), Change::Installed)
            }
        };
        let done = Done { files: laid.files, docs_skipped: laid.skipped, ..Done::new(shown, depth, Origin::Nexus, change, needs) };
        self.emit(&Event::Staged(&done));
        self.record(st, &id, done, laid.fomod)
    }

    fn local(&mut self, st: &mut State, arc: &Path, nid: Option<u64>) -> Result<()> {
        let stem = arc.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or("mod".into());
        let name = self.policy.name.clone().unwrap_or(stem);
        let wanted = self.policy.id.as_deref().map(slugify);
        let existing = wanted.clone().filter(|i| st.find(i).is_some()).or_else(|| nid.and_then(|g| st.by_nexus(g)).map(|m| m.id.clone()));
        let id = existing.clone().or(wanted).unwrap_or_else(|| st.unique_id(&slugify(&name)));
        // Tagged with a Nexus mod: check its requirements before anything is staged.
        let mut checked = None;
        let mut needs = vec![];
        if let Some(gid) = nid {
            let (external, stored) = self.nexus.requires(gid, "")?;
            for req in &external {
                self.emit(&Event::External { depth: 0, req });
            }
            let ignored = existing.as_ref().and_then(|e| st.find(e)).map(|i| st.mods[i].ignored_reqs.clone()).unwrap_or_default();
            self.visiting.insert(gid);
            needs = self.requirements(st, &id, &stored, &ignored, 0)?;
            checked = Some((stored, ReqCheck { at: now(), version_id: String::new() }));
        }
        self.paths.ensure_dirs()?;
        let x = unpack(self.paths, &id, arc)?;
        let laid = self.lay_out(st, &id, &x, 0)?;
        // installed from the Downloads list? remember which download
        let from_library = arc.strip_prefix(self.paths.downloads()).ok().map(|r| r.to_string_lossy().into_owned());
        let change = match st.find(&id) {
            Some(i) => {
                st.mods[i].installed_at = now();
                st.mods[i].archive = from_library;
                if let Some((requires, c)) = checked {
                    st.mods[i].requires = requires;
                    st.mods[i].reqs_checked = Some(c);
                }
                Change::Replaced
            }
            None => {
                let (requires, reqs_checked) = checked.map_or((vec![], None), |(r, c)| (r, Some(c)));
                st.mods.push(Mod {
                    id: id.clone(),
                    name,
                    enabled: !self.policy.disabled,
                    version: String::new(),
                    nexus: nid.map(|g| NexusSource { mod_id: g, ..Default::default() }),
                    requires,
                    reqs_checked,
                    hidden: vec![],
                    ignored_reqs: vec![],
                    fomod: None,
                    archive: from_library,
                    installed_at: now(),
                    notes: format!("from {}", arc.display()),
                });
                Change::Installed
            }
        };
        let done = Done { files: laid.files, docs_skipped: laid.skipped, ..Done::new(id.clone(), 0, Origin::Local, change, needs) };
        self.emit(&Event::Staged(&done));
        self.record(st, &id, done, laid.fomod)
    }

    /// Check a mod's requirements before it is staged. Missing hard requirements are
    /// installed, accepted or refused per the policy; ones the user chose to do without for
    /// this mod (`ignored`) and ones this run is already installing are skipped.
    fn requirements(&mut self, st: &mut State, label: &str, stored: &[Requirement], ignored: &[u64], depth: usize) -> Result<Vec<Need>> {
        let mut needs: Vec<Need> = requirements::check(st, stored, ignored)
            .into_iter()
            .map(|c| Need { req: c.req.clone(), status: c.status, dep: c.dep.map(|d| d.id.clone()), fix: None })
            .collect();
        for n in &mut needs {
            if n.status == Status::Missing && self.visiting.contains(&n.req.mod_id) {
                n.fix = Some(Fix::Upstream);
            }
            if n.status != Status::Missing || n.fix.is_some() {
                (self.log)(&Event::Requirement { depth, label, need: n });
            }
        }
        let missing: Vec<usize> = (0..needs.len()).filter(|&i| needs[i].status == Status::Missing && needs[i].fix.is_none()).collect();
        if missing.is_empty() {
            return Ok(needs);
        }
        match self.policy.missing {
            OnMissing::Install => {
                for i in missing {
                    needs[i].fix = Some(Fix::Installed);
                    self.emit(&Event::Requirement { depth, label, need: &needs[i] });
                    self.nexus_mod(st, needs[i].req.mod_id, None, depth + 1)?;
                }
            }
            OnMissing::Warn => {
                for i in missing {
                    needs[i].fix = Some(Fix::Accepted);
                    self.emit(&Event::Requirement { depth, label, need: &needs[i] });
                }
            }
            OnMissing::Refuse => {
                let mut msg = String::from("missing hard requirements:\n");
                for i in missing {
                    let r = &needs[i].req;
                    msg += &format!("  {} {}{}\n", r.mod_id, r.name, notes(r));
                }
                msg += "re-run with --with-deps to install them, or --ignore-deps";
                bail!(msg);
            }
        }
        Ok(needs)
    }

    /// Plan where the unpacked files in `x` go (FOMOD installer or layout rules) and stage them
    /// as mod `id`; `x` is removed afterwards.
    fn lay_out(&mut self, st: &State, id: &str, x: &Path, depth: usize) -> Result<Laid> {
        let target = depth == 0;
        let extracted = files::list_tree(x);
        let root = self.policy.root.clone().filter(|_| target);
        let dest = self.policy.dest.clone().filter(|_| target);
        let saved = st.find(id).and_then(|i| st.mods[i].fomod.clone()).and_then(|v| serde_json::from_value(v).ok());
        let (plan, fomod_choices) = match fomod::detect(&extracted) {
            Some(cfg) if root.is_none() && dest.is_none() => {
                let explicit = self.policy.fomod.clone().filter(|_| target);
                let (p, c) = self.fomod_plan(st, x, &extracted, &cfg, explicit, saved, depth)?;
                (p, Some(c))
            }
            _ => (stage::plan(&extracted, id, root.as_deref(), dest.as_deref())?, None),
        };
        let staged = stage::stage(self.paths, st, id, x, &plan)?;
        let _ = fs::remove_dir_all(x);
        Ok(Laid { files: staged.len(), skipped: plan.skipped.len(), fomod: fomod_choices })
    }

    /// Plan an install from a FOMOD installer: explicit choices, else the mod's saved choices,
    /// else ask in a terminal (if the policy says so), else the installer's defaults.
    #[allow(clippy::too_many_arguments)]
    fn fomod_plan(
        &mut self,
        st: &State,
        x: &Path,
        extracted: &[String],
        cfg: &str,
        explicit: Option<Choices>,
        saved: Option<Choices>,
        depth: usize,
    ) -> Result<(stage::Plan, Choices)> {
        let f = fomod::parse(&fs::read(x.join(cfg))?, cfg)?;
        let fa = crate::file_active(self.paths, st);
        let tty = unsafe { libc::isatty(0) } == 1 && unsafe { libc::isatty(1) } == 1;
        let is_explicit = explicit.is_some();
        let choices = match (explicit, saved) {
            (Some(c), _) => c,
            (None, Some(s)) => {
                self.emit(&Event::Fomod { depth, note: Note::Saved });
                s
            }
            (None, None) if tty && self.policy.ask => fomod::ask(&f, &fa)?,
            _ => {
                self.emit(&Event::Fomod { depth, note: Note::Defaults });
                Choices::new()
            }
        };
        // saved choices may name options a newer installer dropped: fall back to defaults
        let r = match fomod::resolve(&f, &choices, &fa) {
            Ok(r) => r,
            Err(e) if !is_explicit => {
                self.emit(&Event::Fomod { depth, note: Note::Stale(&e) });
                fomod::resolve(&f, &Choices::new(), &fa)?
            }
            Err(e) => return Err(e),
        };
        let moves = fomod::moves(&f, &r.ops, extracted)?;
        for (k, v) in &r.choices {
            self.emit(&Event::Fomod { depth, note: Note::Chose(k, v) });
        }
        Ok((stage::Plan { moves, skipped: vec![] }, r.choices))
    }

    // ---------------------------------------------------------------------------------------
    // Recording the change: the one place this module saves state, journals and runs the
    // after-install hooks (auto-sort among them). A module owning changes to the mod setup
    // can take over this block whole.

    /// Finish mod `id` (whose record `done` describes): store its FOMOD choices, re-hide the
    /// files the user hid, save, journal a new Nexus install, find its new conflicts and, for
    /// the requested mod, run the after-install hooks.
    fn record(&mut self, st: &mut State, id: &str, mut done: Done, fomod: Option<Choices>) -> Result<()> {
        if done.change == Change::Unchanged {
            st.save(self.paths)?;
            self.done.push(done);
            return Ok(());
        }
        if (done.origin, &done.change) == (Origin::Nexus, &Change::Installed) {
            let version = st.find(id).map(|i| st.mods[i].version.clone()).unwrap_or_default();
            safety::journal(self.paths, "install", &[id.to_string()], &version);
        }
        if let (Some(c), Some(i)) = (&fomod, st.find(id)) {
            st.mods[i].fomod = serde_json::to_value(c).ok();
        }
        done.rehidden = manage::reapply_hidden(self.paths, st, id)?;
        st.save(self.paths)?;
        done.conflicts = conflicts::file_conflicts(self.paths, &st.mods, true)
            .into_iter()
            .filter(|c| c.mods.len() > 1 && !c.identical && c.mods.iter().any(|m| m == id))
            .collect();
        let up = self.paths.upper();
        done.shadowed = files::list_tree(&self.paths.mod_files(id)).into_iter().filter(|f| up.join(f).exists()).count();
        self.emit(&Event::Settled(&done));
        let hooks = done.depth == 0 && self.policy.hooks;
        self.done.push(done);
        if hooks {
            self.after_install(st, id);
        }
        Ok(())
    }

    /// Settings-driven tasks after the requested mod is installed.
    fn after_install(&mut self, st: &mut State, id: &str) {
        let paths = self.paths;
        if paths.cfg.auto_sort {
            crate::auto_sort(paths, st);
        }
        if paths.cfg.after_install_verify {
            let _ = crate::verify(paths, st, &[id.to_string()]);
        }
        if paths.cfg.after_install_doctor {
            let name = st.find(id).map(|i| st.mods[i].name.clone()).unwrap_or_default();
            let hits: Vec<Finding> = loadorder::doctor(paths, st)
                .into_iter()
                .filter(|f| {
                    matches!(f.severity, "crash" | "warning") && (f.title.contains(id) || (!name.is_empty() && f.title.contains(&name)))
                })
                .collect();
            self.emit(&Event::Health(&hits));
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
            self.emit(&Event::Command(&r));
        }
    }
}

impl Done {
    fn new(id: String, depth: usize, origin: Origin, change: Change, requirements: Vec<Need>) -> Self {
        Done { id, depth, origin, change, files: 0, docs_skipped: 0, rehidden: 0, conflicts: vec![], shadowed: 0, requirements }
    }
}

/// A fresh copy of an archive's (or folder's) contents in the tmp extract dir for `id`.
fn unpack(paths: &Paths, id: &str, src: &Path) -> Result<PathBuf> {
    let x = stage::tmp_extract_dir(paths, id);
    if x.exists() {
        fs::remove_dir_all(&x)?;
    }
    if src.is_dir() {
        for f in files::list_tree(src) {
            files::reflink_or_copy(&src.join(&f), &x.join(&f))?;
        }
    } else {
        extract(src, &x)?;
    }
    Ok(x)
}
