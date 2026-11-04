//! Queries: read-only views of the mod setup. [`Query`] names each view the GUI and `cpmm api`
//! read; [`view`] answers it with a typed value (each one's TypeScript type is generated into
//! gui/src/lib/bindings.ts). The CLI's own listings are below. None of them take the cpmm
//! lock or change State (`safety` records the game-log baseline, as `cpmm errors` does).

use crate::loadorder::{Pair, Rule, SortPlan};
use crate::nexus::{self, Catalogue};
use crate::requirements::{self, Status};
use crate::state::{Mod, NexusSource, Paths, State};
use crate::{conflicts, downloads, files, fomod, health, install, loadorder, manage, modio, safety, settings, steam};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use ts_rs::TS;

/// A read-only view, by name (`{"view": "info", "mod": "…"}` from the GUI; `cpmm api info …`).
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq, TS)]
#[serde(tag = "view", rename_all = "snake_case")]
pub enum Query {
    /// Everything the main view needs ([`AppState`])
    State,
    /// Files of one mod with conflict info ([`ModInfo`])
    Info {
        r#mod: String,
    },
    Conflicts,
    /// Newer Nexus uploads of installed files ([`UpdateRow`]s)
    Updates {
        refresh: bool,
    },
    Overwrite,
    Tracked,
    /// A Nexus mod before installing: files, requirements ([`Lookup`])
    Lookup {
        source: String,
    },
    /// Sort plan, rules, modlist ([`LoadOrder`])
    Loadorder,
    Doctor,
    /// Archive resource conflicts between owners, with the current winner
    Archives,
    /// Crashes, new log errors, patch guard, saves and undoable update runs ([`SafetyView`])
    Safety,
    History {
        r#mod: String,
    },
    Settings,
    Downloads,
    /// First-run status: game found, Nexus key, launch options, link handler
    Onboarding,
    Changelog {
        r#mod: String,
    },
    Modio,
    /// FOMOD installer options of an archive or Nexus mod (downloads it if needed), for the
    /// choices so far ([`FomodView`])
    Fomod {
        source: String,
        #[ts(optional = nullable)]
        file: Option<String>,
        #[ts(optional = nullable)]
        choices: Option<fomod::Choices>,
    },
    /// A Nexus collection's contents with install status
    Collection {
        reference: String,
    },
}

/// Answer a query. `ui` hears progress (a FOMOD query may download the archive first).
pub fn view(paths: &Paths, q: &Query, ui: &mut dyn super::Ui) -> Result<serde_json::Value> {
    let st = State::load(paths)?;
    let v = match q {
        Query::State => serde_json::to_value(state(paths, &st))?,
        Query::Info { r#mod } => serde_json::to_value(info(paths, &st, r#mod)?)?,
        Query::Conflicts => serde_json::to_value(conflicts(paths, &st))?,
        Query::Updates { refresh } => serde_json::to_value(updates(paths, &st, *refresh))?,
        Query::Overwrite => serde_json::to_value(overwrite(paths, &st))?,
        Query::Tracked => serde_json::to_value(tracked(paths, &st)?)?,
        Query::Lookup { source } => serde_json::to_value(lookup(paths, source)?)?,
        Query::Loadorder => serde_json::to_value(load_order(paths, &st))?,
        Query::Doctor => serde_json::to_value(health::check(paths, &st))?,
        Query::Archives => serde_json::to_value(archives(paths, &st))?,
        Query::Safety => serde_json::to_value(safety(paths, &st))?,
        Query::History { r#mod } => serde_json::to_value(history(paths, &st, r#mod)?)?,
        Query::Settings => serde_json::to_value(settings(paths))?,
        Query::Downloads => serde_json::to_value(downloads::list(paths, &st))?,
        Query::Onboarding => serde_json::to_value(onboarding(paths, &st, ui))?,
        Query::Changelog { r#mod } => {
            let c = changelog(paths, &st, r#mod)?;
            serde_json::to_value(ChangelogView {
                id: c.m.id.clone(),
                name: c.m.name.clone(),
                installed: c.m.version.clone(),
                entries: c.entries,
            })?
        }
        Query::Modio => serde_json::to_value(modio(paths, &st))?,
        Query::Fomod { source, file, choices } => serde_json::to_value(fomod(paths, &st, ui, source, file.as_deref(), choices.as_ref())?)?,
        Query::Collection { reference } => serde_json::to_value(collection(paths, &st, reference)?)?,
    };
    Ok(v)
}

fn nexus_url(id: u64) -> String {
    format!("https://www.nexusmods.com/cyberpunk2077/mods/{id}")
}

#[derive(Serialize, Debug, TS)]
pub struct AppState {
    pub game_dir: PathBuf,
    pub data_dir: PathBuf,
    pub game_running: bool,
    pub session: Option<SessionInfo>,
    pub mods: Vec<ModRow>,
    pub modio: Vec<ModioSub>,
    pub overwrite_count: usize,
    pub nxm_handler: bool,
    pub active_profile: String,
    pub profiles: Vec<ProfileRow>,
}

#[derive(Serialize, Debug, TS)]
pub struct SessionInfo {
    pub pid: u32,
    pub started: String,
    pub layers: usize,
}

/// One installed mod in priority order (1 = lowest).
#[derive(Serialize, Debug, TS)]
pub struct ModRow {
    pub priority: usize,
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub version: String,
    pub installed_at: String,
    pub notes: String,
    pub nexus: Option<NexusRef>,
    /// The FOMOD choices it was installed with
    #[ts(type = "Record<string, string[]> | null")]
    pub fomod: Option<serde_json::Value>,
    pub files: usize,
    pub hidden: usize,
    /// Conflicting files it wins and loses
    pub wins: usize,
    pub loses: usize,
    pub requires: Vec<ReqRow>,
    /// Unmet hard requirements (0 for a disabled mod)
    pub dep_problems: usize,
}

#[derive(Serialize, Debug, TS)]
pub struct NexusRef {
    pub mod_id: u64,
    pub file_name: String,
    pub file_id: String,
    pub version_gsid: String,
    pub uploaded_at: String,
    pub url: String,
}

#[derive(Serialize, Debug, TS)]
#[ts(rename = "Requirement")]
pub struct ReqRow {
    pub mod_id: u64,
    pub name: String,
    pub notes: String,
    pub optional: bool,
    pub status: Status,
    pub installed_as: Option<String>,
    pub url: String,
}

#[derive(Serialize, Debug, TS)]
pub struct ModioSub {
    pub id: String,
    pub name: String,
    pub author: String,
    pub version: String,
}

#[derive(Serialize, Debug, TS)]
pub struct ProfileRow {
    pub name: String,
    /// Mods enabled in it
    pub enabled: usize,
    pub active: bool,
    pub created_at: String,
    pub own_saves: bool,
    pub saves: usize,
}

pub fn state(paths: &Paths, st: &State) -> AppState {
    let own = conflicts::ownership(paths, &st.mods, true);
    let mut wins: BTreeMap<&str, usize> = BTreeMap::new();
    let mut loses: BTreeMap<&str, usize> = BTreeMap::new();
    for (_, owners) in own.values() {
        if owners.len() > 1 {
            let w = owners.last().unwrap();
            *wins.entry(w.as_str()).or_default() += 1;
            for o in &owners[..owners.len() - 1] {
                *loses.entry(o.as_str()).or_default() += 1;
            }
        }
    }
    let mods = st
        .mods
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let checks = requirements::of(st, m);
            let problems = if m.enabled { checks.iter().filter(|c| c.status.is_problem()).count() } else { 0 };
            let requires = checks
                .iter()
                .map(|c| ReqRow {
                    mod_id: c.req.mod_id,
                    name: c.req.name.clone(),
                    notes: c.req.notes.clone(),
                    optional: c.req.optional,
                    status: c.status,
                    installed_as: c.dep.map(|d| d.id.clone()),
                    url: nexus_url(c.req.mod_id),
                })
                .collect();
            ModRow {
                priority: i + 1,
                id: m.id.clone(),
                name: m.name.clone(),
                enabled: m.enabled,
                version: m.version.clone(),
                installed_at: m.installed_at.clone(),
                notes: m.notes.clone(),
                nexus: m.nexus.as_ref().map(|n| NexusRef {
                    mod_id: n.mod_id,
                    file_name: n.file_name.clone(),
                    file_id: n.file_id.clone(),
                    version_gsid: n.version_gsid.clone(),
                    uploaded_at: n.uploaded_at.clone(),
                    url: nexus_url(n.mod_id),
                }),
                fomod: m.fomod.clone(),
                files: files::list_tree(&paths.mod_files(&m.id)).len(),
                hidden: files::list_tree(&paths.mods().join(&m.id).join("hidden")).len(),
                wins: wins.get(m.id.as_str()).copied().unwrap_or(0),
                loses: loses.get(m.id.as_str()).copied().unwrap_or(0),
                requires,
                dep_problems: problems,
            }
        })
        .collect();
    AppState {
        game_dir: paths.game.clone(),
        data_dir: paths.data.clone(),
        game_running: crate::vfs::game_running(),
        session: crate::vfs::running(paths).map(|(info, pid)| SessionInfo { pid, started: info.started, layers: info.layers.len() }),
        mods,
        modio: modio::list(paths).into_iter().map(|m| ModioSub { id: m.id, name: m.name, author: m.author, version: m.version }).collect(),
        overwrite_count: manage::overwrite_entries(paths).len(),
        nxm_handler: manage::current_nxm_handler() == "cpmm-nxm.desktop",
        active_profile: st.active_profile.clone(),
        profiles: st
            .synced_profiles()
            .iter()
            .map(|p| ProfileRow {
                name: p.name.clone(),
                enabled: p.enabled.len(),
                active: p.name == st.active_profile,
                created_at: p.created_at.clone(),
                own_saves: p.own_saves,
                saves: crate::profiles::count_saves(&p.own_saves_dir(paths).unwrap_or_else(|| safety::shared_saves_dir(paths))),
            })
            .collect(),
    }
}

#[derive(Serialize, Debug, TS)]
pub struct ModInfo {
    pub id: String,
    pub name: String,
    pub files: Vec<OwnedFile>,
    pub hidden: Vec<String>,
    pub ignored_reqs: Vec<u64>,
}

/// One staged file of a mod and who else ships it.
#[derive(Serialize, Debug, TS)]
pub struct OwnedFile {
    pub path: String,
    pub others: Vec<String>,
    /// The enabled mod whose copy the game sees ("" if none is enabled)
    pub winner: String,
    pub wins: bool,
}

pub fn info(paths: &Paths, st: &State, key: &str) -> Result<ModInfo> {
    let m = &st.mods[st.get(key)?];
    let own = conflicts::ownership(paths, &st.mods, false);
    let files = files::list_tree(&paths.mod_files(&m.id))
        .into_iter()
        .map(|f| {
            let owners = own.get(&f.to_lowercase()).map(|(_, o)| o.clone()).unwrap_or_default();
            let enabled_owners: Vec<&String> = owners.iter().filter(|o| st.mods.iter().any(|x| &x.id == *o && x.enabled)).collect();
            let winner = enabled_owners.last().map(|s| s.to_string()).unwrap_or_default();
            let others = owners.iter().filter(|o| **o != m.id).cloned().collect();
            OwnedFile { path: f, others, wins: winner == m.id, winner }
        })
        .collect();
    let hidden = files::list_tree(&paths.mods().join(&m.id).join("hidden"));
    Ok(ModInfo { id: m.id.clone(), name: m.name.clone(), files, hidden, ignored_reqs: m.ignored_reqs.clone() })
}

#[derive(Serialize, Debug, TS)]
pub struct Conflicts {
    pub files: Vec<FileRow>,
    pub scripts: Vec<ScriptRow>,
    pub base_overrides: usize,
}

#[derive(Serialize, Debug, TS)]
pub struct FileRow {
    pub path: String,
    /// In priority order; the last one wins
    pub mods: Vec<String>,
    pub winner: String,
    pub identical: bool,
    pub overrides_base: bool,
}

#[derive(Serialize, Debug, TS)]
pub struct ScriptRow {
    pub kind: String,
    pub target: String,
    pub mods: Vec<String>,
    pub severe: bool,
}

pub fn conflicts(paths: &Paths, st: &State) -> Conflicts {
    let fc = conflicts::file_conflicts(paths, &st.mods, true);
    let files = fc
        .iter()
        .filter(|c| c.mods.len() > 1)
        .map(|c| FileRow {
            path: c.path.clone(),
            mods: c.mods.clone(),
            winner: c.mods.last().cloned().unwrap_or_default(),
            identical: c.identical,
            overrides_base: c.overrides_base,
        })
        .collect();
    let base_overrides = fc.iter().filter(|c| c.overrides_base).count();
    let scripts = conflicts::redscript_conflicts(paths, &st.mods)
        .into_iter()
        .map(|c| ScriptRow {
            severe: c.kind.starts_with("REPLACE") || c.kind.starts_with("duplicate"),
            kind: c.kind,
            target: c.target,
            mods: c.mods,
        })
        .collect();
    Conflicts { files, scripts, base_overrides }
}

#[derive(Serialize, Debug, TS)]
pub struct UpdateRow {
    pub id: String,
    pub status: String,
    pub update: Option<UpdateInfo>,
}

#[derive(Serialize, Debug, TS)]
pub struct UpdateInfo {
    pub version: String,
    pub uploaded_at: String,
    /// The upload's game-scoped id (`cpmm install <mod> --file <gsid>`)
    pub gsid: String,
}

pub fn updates(paths: &Paths, st: &State, refresh: bool) -> Vec<UpdateRow> {
    check_updates(paths, st, refresh)
        .into_iter()
        .map(|u| UpdateRow {
            id: u.id,
            status: u.status,
            update: u.newer.map(|v| UpdateInfo { version: v.version, uploaded_at: v.uploaded_at, gsid: v.game_scoped_id }),
        })
        .collect()
}

#[derive(Serialize, Debug, TS)]
pub struct OverwriteRow {
    pub path: String,
    pub kind: manage::OwKind,
    pub shadows: Option<String>,
}

pub fn overwrite(paths: &Paths, st: &State) -> Vec<OverwriteRow> {
    manage::overwrite_files(paths, st).into_iter().map(|e| OverwriteRow { path: e.rel, kind: e.kind, shadows: e.shadows }).collect()
}

#[derive(Serialize, Debug, TS)]
pub struct TrackedRow {
    pub mod_id: u64,
    pub name: String,
    pub installed_as: Option<String>,
    pub url: String,
}

pub fn tracked(paths: &Paths, st: &State) -> Result<Vec<TrackedRow>> {
    let nx = Catalogue::open(paths);
    let nx = nx.gate();
    Ok(nx
        .tracked()?
        .into_iter()
        .map(|gid| TrackedRow {
            mod_id: gid,
            name: nx.mod_info(gid).map(|i| i.name).unwrap_or_default(),
            installed_as: st.by_nexus(gid).map(|m| m.id.clone()),
            url: nexus_url(gid),
        })
        .collect())
}

#[derive(Serialize, Debug, TS)]
pub struct Lookup {
    pub mod_id: u64,
    pub name: String,
    pub version: String,
    pub author: String,
    pub url: String,
    pub files: Vec<LookupFile>,
    pub requires: Vec<LookupReq>,
}

#[derive(Serialize, Debug, TS)]
pub struct LookupFile {
    pub gsid: String,
    pub name: String,
    pub version: String,
    pub category: String,
    pub uploaded_at: String,
}

#[derive(Serialize, Debug, TS)]
pub struct LookupReq {
    pub mod_id: u64,
    pub name: String,
    pub notes: String,
    pub optional: bool,
    /// Outside Nexus
    pub external: bool,
}

pub fn lookup(paths: &Paths, source: &str) -> Result<Lookup> {
    let gid = nexus::catalogue::mod_id(source).context("not a Nexus mod id or URL")?;
    let nx = Catalogue::open(paths);
    let nx = nx.gate();
    let info = nx.mod_info(gid)?;
    let reqs = nx.requirements(&[gid], false)?.into_iter().next().map(|x| x.2).unwrap_or_default();
    let mut files = vec![];
    for f in nx.mod_files(&info.id)?.into_iter().filter(|f| f.is_active) {
        if let Some(v) = nx.versions(&f.id)?.into_iter().next() {
            files.push(LookupFile {
                gsid: v.game_scoped_id,
                name: f.name,
                version: v.version,
                category: v.category,
                uploaded_at: v.uploaded_at,
            });
        }
    }
    Ok(Lookup {
        mod_id: gid,
        name: info.name,
        version: info.version,
        author: info.author,
        url: nexus_url(gid),
        files,
        requires: reqs
            .iter()
            .map(|r| LookupReq {
                mod_id: r.mod_id,
                name: r.name.clone(),
                notes: r.notes.clone(),
                optional: r.optional(),
                external: r.external,
            })
            .collect(),
    })
}

#[derive(Serialize, Debug, TS)]
pub struct LoadOrder {
    pub plan: SortPlan,
    pub rules: Vec<Rule>,
    /// Archives in modlist.txt order
    pub modlist: Vec<String>,
}

pub fn load_order(paths: &Paths, st: &State) -> LoadOrder {
    LoadOrder { plan: loadorder::plan(paths, st), rules: st.rules.clone(), modlist: loadorder::modlist(paths, st) }
}

#[derive(Serialize, Debug, TS)]
pub struct Onboarding {
    pub game_dir: PathBuf,
    pub game_found: bool,
    pub api_key: bool,
    pub launch_options: Option<String>,
    pub launch_ok: bool,
    pub launch_suggested: String,
    pub steam_running: bool,
    pub steam_cef: bool,
    pub nxm_handler: bool,
    pub onboarded: bool,
    pub mods_installed: usize,
}

pub fn onboarding(paths: &Paths, st: &State, ui: &dyn super::Ui) -> Onboarding {
    let exe = super::installed_exe(ui).unwrap_or_default();
    let lo = steam::read_launch_options();
    Onboarding {
        game_dir: paths.game.clone(),
        game_found: paths.game.join("bin/x64/Cyberpunk2077.exe").is_file(),
        api_key: paths.api_key().is_ok(),
        launch_ok: lo.as_deref().is_some_and(steam::launch_options_ok),
        launch_suggested: steam::with_cpmm(lo.as_deref().unwrap_or(""), &exe),
        launch_options: lo,
        steam_running: steam::is_running(),
        steam_cef: steam::cef_available(),
        nxm_handler: manage::current_nxm_handler() == "cpmm-nxm.desktop",
        onboarded: paths.cfg.onboarded,
        mods_installed: st.mods.len(),
    }
}

/// A mod's Nexus changelog, entries newer than the installed version marked.
pub struct Changelog<'a> {
    pub m: &'a Mod,
    pub entries: Vec<nexus::ChangelogEntry>,
}

pub fn changelog<'a>(paths: &Paths, st: &'a State, key: &str) -> Result<Changelog<'a>> {
    let m = &st.mods[st.get(key)?];
    let gid = m.nexus.as_ref().map(|n| n.mod_id).context("not a Nexus mod")?;
    let entries = nexus::changelog_entries(Catalogue::open(paths).gate().changelog(gid)?, &m.version);
    Ok(Changelog { m, entries })
}

#[derive(Serialize, Debug, TS)]
#[ts(rename = "Changelog")]
pub struct ChangelogView {
    pub id: String,
    pub name: String,
    pub installed: String,
    pub entries: Vec<nexus::ChangelogEntry>,
}

#[derive(Serialize, Debug, TS)]
#[ts(rename = "ModioData")]
pub struct ModioView {
    pub mods: Vec<ModioRow>,
    pub changes: modio::Changes,
    pub conflicts: Vec<modio::Conflict>,
}

#[derive(Serialize, Debug, TS)]
pub struct ModioRow {
    pub id: String,
    pub name: String,
    pub author: String,
    pub version: String,
    pub files: usize,
}

pub fn modio(paths: &Paths, st: &State) -> ModioView {
    ModioView {
        mods: modio::list(paths)
            .into_iter()
            .map(|m| ModioRow { files: m.files.len(), id: m.id, name: m.name, author: m.author, version: m.version })
            .collect(),
        changes: modio::changes(paths),
        conflicts: modio::conflicts(paths, st),
    }
}

/// The archive for a source: a Nexus mod (downloaded into the Downloads list if needed), a
/// path, or a name or number from the Downloads list.
fn fetch_archive(paths: &Paths, ui: &mut dyn super::Ui, source: &str, file: Option<&str>) -> Result<PathBuf> {
    match nexus::catalogue::mod_id(source) {
        Some(gid) => super::act::fetch_nexus(paths, &Catalogue::open(paths), ui, gid, file, None, "nexus"),
        None => {
            let p = files::expand_home(source);
            if p.exists() {
                return Ok(p);
            }
            let st = State::load(paths)?;
            Ok(downloads::find(paths, &st, source)?.path)
        }
    }
}

/// A FOMOD installer's options, or only `fomod: false` for an archive without one.
#[derive(Serialize, Debug, TS)]
#[ts(rename = "FomodDesc")]
pub struct FomodView {
    pub fomod: bool,
    /// The installer's name
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub name: Option<String>,
    /// The archive inspected (in the Downloads list for a Nexus mod)
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub archive: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub steps: Option<Vec<fomod::StepView>>,
}

/// A FOMOD installer's options for the given choices (see `fomod::describe`). The archive is
/// downloaded first if needed.
pub fn fomod(
    paths: &Paths,
    st: &State,
    ui: &mut dyn super::Ui,
    source: &str,
    file: Option<&str>,
    choices: Option<&fomod::Choices>,
) -> Result<FomodView> {
    let arc = fetch_archive(paths, ui, source, file)?;
    let x = paths.tmp().join("fomod-inspect");
    if x.exists() {
        fs::remove_dir_all(&x)?;
    }
    install::extract(&arc, &x)?;
    let extracted = files::list_tree(&x);
    let v = match fomod::detect(&extracted) {
        None => FomodView { fomod: false, name: None, archive: None, steps: None },
        Some(cfg) => {
            let f = fomod::parse(&fs::read(x.join(&cfg))?, &cfg)?;
            let fa = fomod::file_active(paths, st);
            let d = fomod::describe(&f, choices.unwrap_or(&Default::default()), &x, &fa);
            FomodView { fomod: true, name: Some(d.name), archive: Some(arc), steps: Some(d.steps) }
        }
    };
    let _ = fs::remove_dir_all(&x);
    Ok(v)
}

#[derive(Serialize, Debug, TS)]
#[ts(rename = "CollectionInfo")]
pub struct CollectionView {
    pub slug: String,
    pub revision: Option<u64>,
    pub name: String,
    pub author: String,
    pub summary: Option<String>,
    /// Bytes
    pub total_size: f64,
    /// The curator's notes (first 2000 characters)
    pub instructions: String,
    /// Load-order rules
    pub rules: usize,
    /// Optional mods
    pub optional: usize,
    /// Installed through cpmm before
    pub installed: bool,
    pub mods: Vec<CollectionMod>,
}

#[derive(Serialize, Debug, TS)]
pub struct CollectionMod {
    pub name: String,
    pub version: String,
    pub mod_id: u64,
    pub file_id: String,
    pub optional: bool,
    pub phase: u64,
    pub source: String,
    /// Comes with the curator's FOMOD choices
    pub fomod: bool,
    /// The curator patches files (Vortex-only binary patches)
    pub patches: bool,
    #[ts(type = "\"installed\" | \"missing\" | \"other version\" | \"bundled\" | \"manual\"")]
    pub status: &'static str,
}

fn coll_status(st: &State, m: &crate::collections::CollMod) -> &'static str {
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

pub fn collection(paths: &Paths, st: &State, reference: &str) -> Result<CollectionView> {
    let (slug, rv, man, _) = Catalogue::open(paths).collection(reference)?;
    let text = |p: &str| rv.pointer(p).and_then(|v| v.as_str()).map(String::from);
    Ok(CollectionView {
        revision: rv["revisionNumber"].as_u64(),
        name: text("/collection/name").unwrap_or(man.name.clone()),
        author: text("/collection/user/name").unwrap_or(man.author.clone()),
        summary: text("/collection/summary"),
        total_size: rv["totalSize"].as_str().and_then(|s| s.parse::<f64>().ok()).or(rv["totalSize"].as_f64()).unwrap_or(0.0),
        instructions: man.instructions.chars().take(2000).collect(),
        rules: man.rules.len(),
        optional: man.mods.iter().filter(|m| m.optional).count(),
        installed: st.collections.iter().any(|c| c.slug == slug),
        mods: man
            .mods
            .iter()
            .map(|m| CollectionMod {
                name: m.name.clone(),
                version: m.version.clone(),
                mod_id: m.mod_id,
                file_id: m.file_id.clone(),
                optional: m.optional,
                phase: m.phase,
                source: m.source.clone(),
                fomod: m.choices.is_some(),
                patches: m.has_patches,
                status: coll_status(st, m),
            })
            .collect(),
        slug,
    })
}

#[derive(Serialize, Debug, TS)]
#[ts(rename = "SettingsData")]
pub struct SettingsView {
    pub values: settings::Settings,
    pub schema: Vec<settings::Field>,
    pub resolved: Folders,
    pub api_key: bool,
}

/// The folders in effect (settings resolved against the defaults).
#[derive(Serialize, Debug, TS)]
pub struct Folders {
    pub staging_dir: PathBuf,
    pub download_dir: PathBuf,
    pub data_dir: PathBuf,
    pub game_dir: PathBuf,
}

pub fn settings(paths: &Paths) -> SettingsView {
    SettingsView {
        values: paths.cfg.clone(),
        schema: settings::schema(),
        resolved: Folders {
            staging_dir: paths.mods(),
            download_dir: paths.downloads(),
            data_dir: paths.data.clone(),
            game_dir: paths.game.clone(),
        },
        api_key: paths.api_key().is_ok(),
    }
}

#[derive(Serialize, Debug, TS)]
#[ts(rename = "Safety")]
pub struct SafetyView {
    pub crashes: Vec<safety::Crash>,
    pub errors: safety::ErrDiff,
    pub patch: safety::PatchStatus,
    pub snapshots: Vec<safety::Snapshot>,
    pub save_risks: Vec<safety::SaveRisk>,
    pub batches: Vec<safety::Batch>,
    /// Game sessions recorded
    pub sessions: usize,
}

pub fn safety(paths: &Paths, st: &State) -> SafetyView {
    let nx = Catalogue::open(paths);
    SafetyView {
        crashes: safety::crashes(paths, 5),
        errors: safety::error_diff(paths),
        patch: safety::patch_status(paths, st, Some(nx.gate())),
        snapshots: safety::list_snapshots(paths),
        save_risks: safety::save_risks(paths, st),
        batches: safety::batches(paths, st),
        sessions: safety::sessions(paths).len(),
    }
}

pub fn history(paths: &Paths, st: &State, key: &str) -> Result<Vec<safety::HistEntry>> {
    let i = st.get(key)?;
    Ok(safety::list_history(paths, &st.mods[i].id))
}

pub fn archives(paths: &Paths, st: &State) -> Vec<Pair> {
    let ids: Vec<String> = st.mods.iter().map(|m| m.id.clone()).collect();
    loadorder::archive_conflicts(paths, st, &ids)
}

/* ------------------------------------------------------------------ the CLI's listings */

/// One row of `cpmm list`.
pub struct ListRow<'a> {
    pub m: &'a Mod,
    /// With `long`: (files, conflicts won, conflicts lost)
    pub counts: Option<(usize, usize, usize)>,
}

/// The managed mods in priority order; with `long`, file and conflict counts (slower).
pub fn list<'a>(paths: &Paths, st: &'a State, long: bool) -> Vec<ListRow<'a>> {
    let owners = if long { Some(conflicts::ownership(paths, &st.mods, true)) } else { None };
    st.mods
        .iter()
        .map(|m| {
            let counts = owners.as_ref().map(|o| {
                let mine: Vec<&Vec<String>> = o.values().map(|(_, v)| v).filter(|v| v.contains(&m.id)).collect();
                let lost = mine.iter().filter(|v| v.last() != Some(&m.id)).count();
                let won = mine.iter().filter(|v| v.len() > 1 && v.last() == Some(&m.id)).count();
                (files::list_tree(&paths.mod_files(&m.id)).len(), won, lost)
            });
            ListRow { m, counts }
        })
        .collect()
}

/// `cpmm info`: one mod with its requirements, hidden and staged files.
pub struct Detail<'a> {
    pub m: &'a Mod,
    pub requires: Vec<requirements::Check<'a>>,
    pub hidden: Vec<String>,
    pub files: Vec<String>,
}

pub fn detail<'a>(paths: &Paths, st: &'a State, key: &str) -> Result<Detail<'a>> {
    let m = &st.mods[st.get(key)?];
    Ok(Detail {
        m,
        requires: requirements::of(st, m),
        hidden: files::list_tree(&paths.mods().join(&m.id).join("hidden")),
        files: files::list_tree(&paths.mod_files(&m.id)),
    })
}

/// `cpmm conflicts`: file overwrites between enabled mods (last listed wins) and against the
/// base game, and redscript conflicts; with `only`, those involving that mod.
pub struct ConflictReport {
    pub files: Vec<conflicts::FileConflict>,
    pub scripts: Vec<conflicts::ScriptConflict>,
}

pub fn conflict_report(paths: &Paths, st: &State, only: Option<&str>, hash: bool) -> Result<ConflictReport> {
    let only = only.map(|k| st.get(k).map(|i| st.mods[i].id.clone())).transpose()?;
    let files =
        conflicts::file_conflicts(paths, &st.mods, hash).into_iter().filter(|c| only.as_ref().is_none_or(|o| c.mods.contains(o))).collect();
    let scripts = conflicts::redscript_conflicts(paths, &st.mods)
        .into_iter()
        .filter(|c| only.as_ref().is_none_or(|o| c.mods.iter().any(|m| m.ends_with(o.as_str()))))
        .collect();
    Ok(ConflictReport { files, scripts })
}

/// One installed Nexus file's update check.
pub struct UpdateStatus {
    pub id: String,
    pub status: String,
    pub newer: Option<nexus::Version>,
}

pub fn check_updates(paths: &Paths, st: &State, refresh: bool) -> Vec<UpdateStatus> {
    use nexus::catalogue::Update;
    let nx = Catalogue::open(paths);
    let nx = if refresh { nx.uncached() } else { nx };
    let work: Vec<&Mod> = st.mods.iter().filter(|m| m.nexus.is_some()).collect();
    let sources: Vec<&NexusSource> = work.iter().filter_map(|m| m.nexus.as_ref()).collect();
    work.iter()
        .zip(nx.updates(&sources))
        .map(|(m, r)| {
            let mod_id = m.nexus.as_ref().map_or(0, |n| n.mod_id);
            let (status, newer) = match r {
                Ok(Update::PageStatus(s)) => (format!("mod page status: {s}"), None),
                Ok(Update::FileUnknown { page_version }) => (format!("file unknown; page version {page_version}"), None),
                Ok(Update::NoVersions) => ("no versions".into(), None),
                Ok(Update::Archived { page_version }) => {
                    (format!("file archived by author; page version {page_version} — check `cpmm install {mod_id} --file`"), None)
                }
                Ok(Update::UpToDate) => ("up to date".into(), None),
                Ok(Update::Available(v)) => {
                    (format!("UPDATE {} -> {} ({})", m.version, v.version, v.uploaded_at.get(..10).unwrap_or("")), Some(*v))
                }
                Err(e) => (format!("error: {e}"), None),
            };
            UpdateStatus { id: m.id.clone(), status, newer }
        })
        .collect()
}

/// Compare staged files to the Nexus manifest (all mods, or the named ones), one at a time.
pub fn verify(paths: &Paths, st: &State, keys: &[String], each: &mut dyn FnMut(crate::verify::Verified)) -> Result<()> {
    let sel: Vec<usize> =
        if keys.is_empty() { (0..st.mods.len()).collect() } else { keys.iter().map(|k| st.get(k)).collect::<Result<_>>()? };
    crate::verify::verify(paths, st, &Catalogue::open(paths), &sel, each)
}

/// Unmet requirements of the enabled mods, as `cpmm deps` lists them.
#[derive(Clone, Debug, Default)]
pub struct Unmet {
    pub rows: Vec<UnmetRow>,
    /// Unmet hard requirements
    pub problems: usize,
    /// Requirements the user chose to do without
    pub ignored: usize,
}

#[derive(Clone, Debug)]
pub struct UnmetRow {
    pub id: String,
    pub req: crate::state::Requirement,
    /// The installed but disabled mod that provides it
    pub disabled: Option<String>,
    /// A hard requirement (else an optional one, listed for information)
    pub problem: bool,
}

pub fn unmet(st: &State) -> Unmet {
    let mut u = Unmet::default();
    for m in st.mods.iter().filter(|m| m.enabled) {
        for c in requirements::of(st, m) {
            let disabled = match (c.status, c.dep) {
                (Status::Ok, _) => continue,
                (Status::Ignored, _) => {
                    u.ignored += 1;
                    continue;
                }
                (Status::Disabled | Status::OptionalDisabled, Some(d)) => Some(d.id.clone()),
                _ => None,
            };
            let problem = c.status.is_problem();
            u.problems += problem as usize;
            u.rows.push(UnmetRow { id: m.id.clone(), req: c.req.clone(), disabled, problem });
        }
    }
    u
}

/// Is a cpmm game session running (with when it started, its pid and layers)?
pub fn session(paths: &Paths) -> Option<(crate::vfs::RunInfo, u32)> {
    crate::vfs::running(paths)
}

/// The Nexus API key: none, set, or (with `check`) validated against Nexus.
#[derive(Clone, Debug)]
pub enum KeyStatus {
    Missing,
    Set(PathBuf),
    Valid { name: String, premium: bool },
}

pub fn nexus_key(paths: &Paths, check: bool) -> Result<KeyStatus> {
    Ok(match paths.api_key() {
        Ok(k) if check => {
            let (name, premium) = Catalogue::with_key(paths, Some(k)).gate().validate()?;
            KeyStatus::Valid { name, premium }
        }
        Ok(_) => KeyStatus::Set(nexus::key_file()),
        Err(_) => KeyStatus::Missing,
    })
}

/// Steam's launch options for the game and what they'd be with cpmm in them.
pub struct LaunchOptions {
    pub current: Option<String>,
    pub ok: bool,
    pub with_cpmm: String,
}

pub fn launch_options(ui: &dyn super::Ui) -> Result<LaunchOptions> {
    let exe = super::installed_exe(ui)?;
    let current = steam::read_launch_options();
    let ok = current.as_deref().is_some_and(steam::launch_options_ok);
    let with_cpmm = steam::with_cpmm(current.as_deref().unwrap_or(""), &exe);
    Ok(LaunchOptions { current, ok, with_cpmm })
}
