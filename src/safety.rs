//! Safety net: version history + rollback, game sessions and crash triage, "new errors since
//! last launch", the game-patch guard, and save snapshots.

use crate::files;
use crate::state::{Mod, Paths, State};
use anyhow::{Context, Result, bail};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::UNIX_EPOCH;

pub fn now_unix() -> i64 {
    std::time::SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}
fn mtime(p: &Path) -> i64 {
    fs::metadata(p).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64).unwrap_or(0)
}
fn append_jsonl<T: Serialize>(p: &Path, v: &T) -> Result<()> {
    if let Some(d) = p.parent() {
        fs::create_dir_all(d)?;
    }
    let mut f = fs::OpenOptions::new().create(true).append(true).open(p)?;
    writeln!(f, "{}", serde_json::to_string(v)?)?;
    Ok(())
}
fn read_jsonl<T: for<'de> Deserialize<'de>>(p: &Path) -> Vec<T> {
    fs::read_to_string(p).unwrap_or_default().lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
}

/* ------------------------------------------------------------------ journal */

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct JEntry {
    pub ts: i64,
    pub action: String,
    pub mods: Vec<String>,
    pub detail: String,
}
pub fn journal(paths: &Paths, action: &str, mods: &[String], detail: &str) {
    let _ = append_jsonl(
        &paths.data.join("journal.jsonl"),
        &JEntry { ts: now_unix(), action: action.into(), mods: mods.to_vec(), detail: detail.into() },
    );
}
pub fn read_journal(paths: &Paths) -> Vec<JEntry> {
    read_jsonl(&paths.data.join("journal.jsonl"))
}

/* ------------------------------------------------------------------ history / rollback */

/// Set while `update --all` (or a multi-mod update) runs, so its replacements can be undone together.
pub static BATCH: Mutex<Option<String>> = Mutex::new(None);

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct HistMeta {
    pub ts: i64,
    pub id: String,
    pub version: String,
    pub nexus: Option<crate::state::NexusSource>,
    pub reason: String,
    #[serde(default)]
    pub batch: Option<String>,
}
#[derive(Serialize, Clone, Debug)]
pub struct HistEntry {
    #[serde(flatten)]
    pub meta: HistMeta,
    pub files: usize,
    #[serde(skip)]
    pub dir: PathBuf,
}

fn history_root(paths: &Paths, id: &str) -> PathBuf {
    paths.data.join("history").join(id)
}

/// Move the mod's current staged files into its history (instead of trash).
pub fn archive_current(paths: &Paths, m: &Mod, reason: &str) -> Result<Option<PathBuf>> {
    let cur = paths.mods().join(&m.id);
    if !cur.exists() {
        return Ok(None);
    }
    let mut ts = now_unix();
    let root = history_root(paths, &m.id);
    while root.join(ts.to_string()).exists() {
        ts += 1;
    }
    let dir = root.join(ts.to_string());
    fs::create_dir_all(&root)?;
    fs::rename(&cur, &dir)?;
    let batch = BATCH.lock().ok().and_then(|b| b.clone());
    let meta = HistMeta { ts, id: m.id.clone(), version: m.version.clone(), nexus: m.nexus.clone(), reason: reason.into(), batch };
    fs::write(dir.join("meta.json"), serde_json::to_vec_pretty(&meta)?)?;
    prune_history(paths, &m.id);
    Ok(Some(dir))
}

pub fn list_history(paths: &Paths, id: &str) -> Vec<HistEntry> {
    let mut v: Vec<HistEntry> = fs::read_dir(history_root(paths, id))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let meta: HistMeta = serde_json::from_slice(&fs::read(e.path().join("meta.json")).ok()?).ok()?;
            Some(HistEntry { files: files::list_tree(&e.path().join("files")).len(), dir: e.path(), meta })
        })
        .collect();
    v.sort_by_key(|h| std::cmp::Reverse(h.meta.ts));
    v
}

fn prune_history(paths: &Paths, id: &str) {
    for h in list_history(paths, id).into_iter().skip(paths.cfg.keep_versions.max(1)) {
        let _ = fs::remove_dir_all(&h.dir);
    }
}

/// Swap the mod's current files with a history entry (newest if `ts` is None).
pub fn rollback(paths: &Paths, st: &mut State, key: &str, ts: Option<i64>) -> Result<String> {
    let i = st.get(key)?;
    let id = st.mods[i].id.clone();
    let hist = list_history(paths, &id);
    let target = match ts {
        Some(t) => hist.into_iter().find(|h| h.meta.ts == t).with_context(|| format!("no history entry {t} for {id}"))?,
        None => hist.into_iter().next().with_context(|| format!("{id} has no earlier version to roll back to"))?,
    };
    let from = st.mods[i].version.clone();
    archive_current(paths, &st.mods[i], &format!("rolled back to {}", target.meta.version))?;
    // the target may have been pruned by the archive call only if we kept >KEEP; guard anyway
    if !target.dir.exists() {
        bail!("history entry disappeared");
    }
    let dest = paths.mods().join(&id);
    fs::rename(&target.dir, &dest)?;
    let _ = fs::remove_file(dest.join("meta.json"));
    let m = &mut st.mods[i];
    m.version = target.meta.version.clone();
    m.nexus = target.meta.nexus.clone();
    m.installed_at = crate::state::now();
    let msg = format!("{id}: {from} -> {}", target.meta.version);
    journal(paths, "rollback", &[id], &msg);
    Ok(msg)
}

#[derive(Serialize, Debug)]
pub struct Batch {
    pub batch: String,
    pub ts: i64,
    pub mods: Vec<(String, String)>, // (id, version it would return to)
}
/// Update batches that can still be undone (every replaced mod still has its entry).
pub fn batches(paths: &Paths, st: &State) -> Vec<Batch> {
    let mut by: BTreeMap<String, Batch> = BTreeMap::new();
    for m in &st.mods {
        for h in list_history(paths, &m.id) {
            if let Some(b) = &h.meta.batch {
                let e = by.entry(b.clone()).or_insert(Batch { batch: b.clone(), ts: h.meta.ts, mods: vec![] });
                e.mods.push((m.id.clone(), h.meta.version.clone()));
                e.ts = e.ts.min(h.meta.ts);
            }
        }
    }
    let mut v: Vec<Batch> = by.into_values().collect();
    v.sort_by_key(|b| std::cmp::Reverse(b.ts));
    v
}
pub fn undo_batch(paths: &Paths, st: &mut State, batch: &str) -> Result<Vec<String>> {
    let mut out = vec![];
    let ids: Vec<String> = st.mods.iter().map(|m| m.id.clone()).collect();
    for id in ids {
        if let Some(h) = list_history(paths, &id).into_iter().find(|h| h.meta.batch.as_deref() == Some(batch)) {
            out.push(rollback(paths, st, &id, Some(h.meta.ts))?);
        }
    }
    if out.is_empty() {
        bail!("nothing from batch {batch} is left to undo");
    }
    Ok(out)
}

/* ------------------------------------------------------------------ sessions */

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SessMod {
    pub id: String,
    pub version: String,
    pub enabled: bool,
}
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Session {
    pub start: i64,
    pub build_id: String,
    pub vanilla: bool,
    pub mods: Vec<SessMod>,
}
pub fn record_session(paths: &Paths, st: &State, vanilla: bool) {
    let s = Session {
        start: now_unix(),
        build_id: game_build(paths).0,
        vanilla,
        mods: st.mods.iter().map(|m| SessMod { id: m.id.clone(), version: m.version.clone(), enabled: m.enabled }).collect(),
    };
    let _ = append_jsonl(&paths.data.join("sessions.jsonl"), &s);
}
pub fn sessions(paths: &Paths) -> Vec<Session> {
    read_jsonl(&paths.data.join("sessions.jsonl"))
}

/// Human-readable differences between two mod snapshots (b relative to a).
pub fn diff_snapshots(a: &[SessMod], b: &[SessMod]) -> Vec<String> {
    let am: BTreeMap<&str, &SessMod> = a.iter().map(|m| (m.id.as_str(), m)).collect();
    let bm: BTreeMap<&str, &SessMod> = b.iter().map(|m| (m.id.as_str(), m)).collect();
    let mut out = vec![];
    for (id, m) in &bm {
        match am.get(id) {
            None => out.push(format!("installed {id} {}", m.version)),
            Some(o) => {
                if o.version != m.version {
                    out.push(format!("{id}: {} -> {}", o.version, m.version));
                }
                if o.enabled != m.enabled {
                    out.push(format!("{} {id}", if m.enabled { "enabled" } else { "disabled" }));
                }
            }
        }
    }
    for id in am.keys() {
        if !bm.contains_key(id) {
            out.push(format!("removed {id}"));
        }
    }
    let ao: Vec<&str> = a.iter().filter(|m| bm.contains_key(m.id.as_str())).map(|m| m.id.as_str()).collect();
    let bo: Vec<&str> = b.iter().filter(|m| am.contains_key(m.id.as_str())).map(|m| m.id.as_str()).collect();
    let moved = ao.iter().zip(&bo).filter(|(x, y)| x != y).count();
    if moved > 0 {
        out.push(format!("load order changed ({moved} positions)"));
    }
    out
}

/* ------------------------------------------------------------------ crashes */

#[derive(Serialize, Debug, Clone)]
pub struct Crash {
    pub report: String,
    pub ts: i64,
    pub message: String,
    pub module: String,
    pub offset: String,
    pub session_start: Option<i64>,
    pub seconds_in: Option<i64>,
    pub last_good: Option<i64>,
    pub changes: Vec<String>,
    /// mods that changed since the last good session and can be rolled back
    pub suspects: Vec<String>,
}

fn report_queue(paths: &Paths) -> PathBuf {
    paths.modio_dir().parent().unwrap().parent().unwrap().parent().unwrap().join("steamuser/AppData/Local/REDEngine/ReportQueue")
}

/// Exception address and the module containing it, from a Windows minidump.
fn minidump_fault(p: &Path) -> Option<(String, u64)> {
    let d = fs::read(p).ok()?;
    let u32at = |o: usize| d.get(o..o + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
    let u64at = |o: usize| d.get(o..o + 8).map(|b| u64::from_le_bytes(b.try_into().unwrap()));
    let (n, rva) = (u32at(8)? as usize, u32at(12)? as usize);
    let mut streams = BTreeMap::new();
    for i in 0..n {
        let o = rva + i * 12;
        streams.insert(u32at(o)?, u32at(o + 8)? as usize);
    }
    let er = *streams.get(&6)?; // ExceptionStream
    let addr = u64at(er + 8 + 16)?; // ThreadId, align, then EXCEPTION_RECORD {code, flags, record, address}
    let ml = *streams.get(&4)?; // ModuleListStream
    let cnt = u32at(ml)? as usize;
    for i in 0..cnt {
        let o = ml + 4 + i * 108;
        let (base, size) = (u64at(o)?, u32at(o + 8)? as u64);
        if addr >= base && addr < base + size {
            let nr = u32at(o + 20)? as usize;
            let ln = u32at(nr)? as usize;
            let raw: Vec<u16> = d.get(nr + 4..nr + 4 + ln)?.chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            let name = String::from_utf16_lossy(&raw);
            return Some((name.rsplit('\\').next().unwrap_or(&name).to_string(), addr - base));
        }
    }
    Some(("(unknown module)".into(), addr))
}

pub fn crashes(paths: &Paths, limit: usize) -> Vec<Crash> {
    let sess = sessions(paths);
    let mut reports: Vec<(i64, PathBuf)> = fs::read_dir(report_queue(paths))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("Cyberpunk2077-"))
        .map(|e| (mtime(&e.path().join("stacktrace.txt")).max(mtime(&e.path())), e.path()))
        .collect();
    reports.sort_by_key(|r| std::cmp::Reverse(r.0));
    let crash_times: Vec<i64> = reports.iter().map(|r| r.0).collect();
    let crashed = |s: &Session, next: Option<&Session>| crash_times.iter().any(|&t| t >= s.start && next.is_none_or(|n| t < n.start));
    reports
        .into_iter()
        .take(limit)
        .map(|(ts, dir)| {
            let st = fs::read_to_string(dir.join("stacktrace.txt")).unwrap_or_default().replace('\r', "");
            let message = st
                .lines()
                .filter(|l| l.starts_with("Expression:") || l.starts_with("Message:"))
                .map(|l| l.split_once(':').map(|x| x.1).unwrap_or("").trim())
                .collect::<Vec<_>>()
                .join(" — ");
            let (module, off) = minidump_fault(&dir.join("Cyberpunk2077.dmp")).unwrap_or(("?".into(), 0));
            let si = sess.iter().rposition(|s| s.start <= ts);
            let (mut changes, mut last_good, mut suspects) = (vec![], None, vec![]);
            if let Some(i) = si
                && let Some(g) = (0..i).rev().find(|&j| !crashed(&sess[j], sess.get(j + 1)) && !sess[j].vanilla)
            {
                last_good = Some(sess[g].start);
                changes = diff_snapshots(&sess[g].mods, &sess[i].mods);
                let gm: BTreeMap<&str, &SessMod> = sess[g].mods.iter().map(|m| (m.id.as_str(), m)).collect();
                for j in read_journal(paths).into_iter().filter(|j| {
                    j.ts > sess[g].start
                        && j.ts < sess[i].start
                        && !matches!(j.action.as_str(), "enable" | "disable" | "update" | "uninstall")
                }) {
                    changes.push(format!("{} {} {}", j.action, j.mods.join(", "), j.detail).trim().to_string());
                }
                suspects = sess[i]
                    .mods
                    .iter()
                    .filter(|m| m.enabled && gm.get(m.id.as_str()).is_none_or(|o| o.version != m.version || !o.enabled))
                    .map(|m| m.id.clone())
                    .collect();
            }
            Crash {
                report: dir.file_name().unwrap().to_string_lossy().into(),
                ts,
                message,
                module,
                offset: format!("+0x{off:x}"),
                session_start: si.map(|i| sess[i].start),
                seconds_in: si.map(|i| ts - sess[i].start),
                last_good,
                changes,
                suspects,
            }
        })
        .collect()
}

/* ------------------------------------------------------------------ log errors */

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct ErrSnap {
    pub ts: i64,
    pub newest_log: i64,
    pub errors: BTreeMap<String, BTreeSet<String>>,
}

fn log_files(paths: &Paths) -> Vec<(String, PathBuf)> {
    // newest file of each log family, from the overlay upper dir (where the game writes)
    let up = paths.upper();
    let newest = |dir: PathBuf, prefix: &str| -> Option<PathBuf> {
        fs::read_dir(&dir)
            .ok()?
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with(prefix) && n.to_string_lossy().ends_with(".log")))
            .max_by_key(|p| mtime(p))
    };
    let mut out = vec![];
    let fixed = [
        ("redscript", up.join("r6/logs/redscript_rCURRENT.log")),
        ("CET", up.join("bin/x64/plugins/cyber_engine_tweaks/cyber_engine_tweaks.log")),
        ("CET scripting", up.join("bin/x64/plugins/cyber_engine_tweaks/scripting.log")),
    ];
    for (n, p) in fixed {
        if p.is_file() {
            out.push((n.to_string(), p));
        }
    }
    for (n, d, pre) in [
        ("RED4ext", "red4ext/logs", "red4ext-"),
        ("TweakXL", "red4ext/plugins/TweakXL", "TweakXL-"),
        ("ArchiveXL", "red4ext/plugins/ArchiveXL", "ArchiveXL-"),
        ("Codeware", "red4ext/plugins/Codeware", "Codeware-"),
    ] {
        if let Some(p) = newest(up.join(d), pre) {
            out.push((n.to_string(), p));
        }
    }
    out
}

pub fn current_errors(paths: &Paths) -> ErrSnap {
    let level = Regex::new(r"(?i)\[(error|warning|warn|critical)\]|\[(ERROR|WARN) - ").unwrap();
    let strip = [
        Regex::new(r"\[\d{4}-\d\d-\d\d[^\]]*\]").unwrap(),
        Regex::new(r"\[(?:ERROR|WARN|INFO) - [^\]]*\]").unwrap(),
        Regex::new(r"\[\s*\d+\s*\]").unwrap(),
        Regex::new(r"0x[0-9a-fA-F]+").unwrap(),
        Regex::new(r"\d{2}:\d{2}:\d{2}(\.\d+)?").unwrap(),
    ];
    let mut snap = ErrSnap { ts: now_unix(), ..Default::default() };
    for (name, p) in log_files(paths) {
        snap.newest_log = snap.newest_log.max(mtime(&p));
        let Ok(text) = fs::read(&p) else { continue };
        let text = String::from_utf8_lossy(&text);
        let set = snap.errors.entry(name).or_default();
        for l in text.lines().filter(|l| level.is_match(l)) {
            let mut s = l.replace('\r', "");
            for r in &strip {
                s = r.replace_all(&s, "").into_owned();
            }
            let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
            if !s.is_empty() {
                set.insert(s.chars().take(300).collect());
            }
        }
    }
    snap
}

/// Record the current logs if they changed since the last snapshot. Returns (previous, latest).
pub fn ingest_errors(paths: &Paths) -> (Option<ErrSnap>, ErrSnap) {
    let p = paths.data.join("error-snapshots.jsonl");
    let mut all: Vec<ErrSnap> = read_jsonl(&p);
    let cur = current_errors(paths);
    if cur.newest_log > 0 && all.last().is_none_or(|l| l.newest_log != cur.newest_log) {
        let _ = append_jsonl(&p, &cur);
        all.push(cur);
        if all.len() > 30 {
            let keep = &all[all.len() - 30..];
            let _ = fs::write(&p, keep.iter().map(|s| serde_json::to_string(s).unwrap_or_default()).collect::<Vec<_>>().join("\n") + "\n");
        }
    }
    let latest = all.last().cloned().unwrap_or_default();
    let prev = if all.len() >= 2 { Some(all[all.len() - 2].clone()) } else { None };
    (prev, latest)
}

#[derive(Serialize, Debug)]
pub struct ErrDiff {
    pub baseline: bool,
    pub latest_log: i64,
    pub new: BTreeMap<String, Vec<String>>,
    pub fixed: BTreeMap<String, Vec<String>>,
    pub totals: BTreeMap<String, usize>,
}
pub fn error_diff(paths: &Paths) -> ErrDiff {
    let (prev, cur) = ingest_errors(paths);
    let mut new = BTreeMap::new();
    let mut fixed = BTreeMap::new();
    let totals = cur.errors.iter().map(|(k, v)| (k.clone(), v.len())).collect();
    if let Some(p) = &prev {
        for (src, set) in &cur.errors {
            let old = p.errors.get(src).cloned().unwrap_or_default();
            let n: Vec<String> = set.difference(&old).cloned().collect();
            if !n.is_empty() {
                new.insert(src.clone(), n);
            }
        }
        for (src, old) in &p.errors {
            let now = cur.errors.get(src).cloned().unwrap_or_default();
            let f: Vec<String> = old.difference(&now).cloned().collect();
            if !f.is_empty() && cur.errors.contains_key(src) {
                fixed.insert(src.clone(), f);
            }
        }
    }
    ErrDiff { baseline: prev.is_none(), latest_log: cur.newest_log, new, fixed, totals }
}

/* ------------------------------------------------------------------ patch guard */

/// (build id, last update unix time) from Steam's app manifest.
pub fn game_build(paths: &Paths) -> (String, i64) {
    let lib = paths.game.parent().and_then(|p| p.parent()).map(Path::to_path_buf).unwrap_or_default();
    let acf = fs::read_to_string(lib.join(format!("appmanifest_{}.acf", crate::state::STEAM_APP_ID))).unwrap_or_default();
    let field = |k: &str| {
        acf.lines().find(|l| l.trim_start().starts_with(&format!("\"{k}\""))).and_then(|l| l.split('"').nth(3)).unwrap_or("").to_string()
    };
    (field("buildid"), field("LastUpdated").parse().unwrap_or(0))
}

#[derive(Serialize, Deserialize, Default, Debug, Clone)]
pub struct BuildRecord {
    pub build_id: String,
    pub previous: Option<String>,
    pub changed_at: i64,
}

#[derive(Serialize, Debug)]
pub struct Framework {
    pub id: String,
    pub name: String,
    pub version: String,
    pub latest_upload: Option<String>,
    pub update_available: bool,
    pub released_since_patch: bool,
}
#[derive(Serialize, Debug)]
pub struct PatchStatus {
    pub build_id: String,
    pub previous_build: Option<String>,
    pub patched_at: i64,
    /// the game changed build since cpmm first saw it
    pub patch_detected: bool,
    pub at_risk: Vec<Framework>,
    pub red4ext_errors: Vec<String>,
    pub vanilla: bool,
}

/// Mods with native code (DLLs / the script compiler): the ones a game patch breaks.
pub fn native_mods(paths: &Paths, st: &State) -> Vec<String> {
    st.mods
        .iter()
        .filter(|m| m.enabled)
        .filter(|m| {
            files::list_tree(&paths.mod_files(&m.id)).iter().any(|f| {
                let l = f.to_lowercase();
                l.ends_with(".dll") || l.ends_with(".asi") || l.starts_with("engine/tools/")
            })
        })
        .map(|m| m.id.clone())
        .collect()
}

pub fn vanilla_flag(paths: &Paths) -> PathBuf {
    paths.data.join("vanilla-next-launch")
}

pub fn patch_status(paths: &Paths, st: &State, nx: Option<&crate::nexus::Nexus>) -> PatchStatus {
    let (build, updated) = game_build(paths);
    let rp = paths.data.join("game-build.json");
    let mut rec: BuildRecord = fs::read(&rp).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    if rec.build_id.is_empty() {
        rec = BuildRecord { build_id: build.clone(), previous: None, changed_at: 0 };
        let _ = fs::write(&rp, serde_json::to_vec_pretty(&rec).unwrap());
    } else if rec.build_id != build && !build.is_empty() {
        rec = BuildRecord {
            previous: Some(rec.build_id.clone()),
            build_id: build.clone(),
            changed_at: if updated > 0 { updated } else { now_unix() },
        };
        let _ = fs::write(&rp, serde_json::to_vec_pretty(&rec).unwrap());
    }
    let patch_detected = rec.previous.is_some();
    let mut at_risk = vec![];
    if let (true, Some(nx)) = (patch_detected, nx) {
        let patched_iso = iso(rec.changed_at);
        for id in native_mods(paths, st) {
            let m = &st.mods[st.get(&id).unwrap()];
            let Some(n) = &m.nexus else { continue };
            if n.file_id.is_empty() {
                continue;
            }
            let newest = nx.versions(&n.file_id).ok().and_then(|v| v.into_iter().next());
            let latest = newest.as_ref().map(|v| v.uploaded_at.clone());
            let since = latest.as_deref().is_some_and(|u| u >= patched_iso.as_str());
            let update = newest.as_ref().is_some_and(|v| v.id != n.version_id);
            if !since || update {
                at_risk.push(Framework {
                    id: id.clone(),
                    name: m.name.clone(),
                    version: m.version.clone(),
                    latest_upload: latest,
                    update_available: update,
                    released_since_patch: since,
                });
            }
        }
    }
    // RED4ext states plainly when the game version isn't supported
    let red4ext_errors = log_files(paths)
        .into_iter()
        .filter(|(n, _)| n == "RED4ext")
        .flat_map(|(_, p)| {
            fs::read_to_string(p)
                .unwrap_or_default()
                .lines()
                .filter(|l| {
                    l.to_lowercase().contains("not supported")
                        || l.to_lowercase().contains("incompatible")
                        || l.contains("[error]")
                        || l.contains("[critical]")
                })
                .map(String::from)
                .collect::<Vec<_>>()
        })
        .take(10)
        .collect();
    PatchStatus {
        build_id: build,
        previous_build: rec.previous,
        patched_at: if rec.changed_at > 0 { rec.changed_at } else { updated },
        patch_detected,
        at_risk,
        red4ext_errors,
        vanilla: vanilla_flag(paths).exists(),
    }
}

pub fn iso(t: i64) -> String {
    let (d, s) = (t.div_euclid(86400), t.rem_euclid(86400));
    let z = d + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let dd = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}-{m:02}-{dd:02}T{:02}:{:02}:{:02}Z", s / 3600, (s / 60) % 60, s % 60)
}

/* ------------------------------------------------------------------ saves */

/// The game's own save folder (what Steam Cloud syncs; profiles with "shared" saves use it).
pub fn shared_saves_dir(paths: &Paths) -> PathBuf {
    report_queue(paths)
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("Saved Games/CD Projekt Red/Cyberpunk 2077")
}

/// Save folder of a profile (by name; None = the active one).
fn profile_saves_dir(paths: &Paths, st: &State, profile: Option<&str>) -> PathBuf {
    let p = profile.and_then(|n| st.profiles.iter().find(|p| p.name == n)).unwrap_or_else(|| st.active());
    p.own_saves_dir(paths).unwrap_or_else(|| shared_saves_dir(paths))
}

/// The active profile's save folder, as the game sees it.
pub fn saves_dir(paths: &Paths) -> PathBuf {
    match State::load(paths) {
        Ok(st) => profile_saves_dir(paths, &st, None),
        Err(_) => shared_saves_dir(paths),
    }
}
fn snapshots_dir(paths: &Paths) -> PathBuf {
    paths.data.join("save-snapshots")
}

#[derive(Serialize, Debug)]
pub struct Snapshot {
    pub name: String,
    pub ts: i64,
    pub reason: String,
    pub saves: usize,
    /// Profile whose saves these are ("" for snapshots from before profiles).
    pub profile: String,
}

/// Reflink-copy every save folder (no extra disk space on btrfs). Keeps the newest few.
/// Automatic snapshot before a risky change, if the user hasn't turned that off.
pub fn auto_snapshot(paths: &Paths, reason: &str) {
    if paths.cfg.snapshot_saves {
        let _ = snapshot_saves(paths, reason);
    }
}

pub fn snapshot_saves(paths: &Paths, reason: &str) -> Result<Option<String>> {
    let st = State::load(paths)?;
    let profile = st.active_profile.clone();
    let src = profile_saves_dir(paths, &st, None);
    if !src.is_dir() || crate::profiles::count_saves(&src) == 0 {
        return Ok(None);
    }
    let name = format!("{}-{}", now_unix(), crate::state::slugify(reason));
    let dst = snapshots_dir(paths).join(&name);
    for f in files::list_tree(&src) {
        files::reflink_or_copy(&src.join(&f), &dst.join(&f))?;
    }
    fs::write(dst.join(".reason"), reason)?;
    fs::write(dst.join(".profile"), &profile)?;
    let mut all = list_snapshots(paths);
    all.sort_by_key(|s| std::cmp::Reverse(s.ts));
    for s in all.into_iter().skip(paths.cfg.keep_save_snapshots.max(1)) {
        let _ = fs::remove_dir_all(snapshots_dir(paths).join(&s.name));
    }
    Ok(Some(name))
}

pub fn list_snapshots(paths: &Paths) -> Vec<Snapshot> {
    let mut v: Vec<Snapshot> = fs::read_dir(snapshots_dir(paths))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            let ts = name.split('-').next().and_then(|t| t.parse().ok()).unwrap_or(0);
            let reason = fs::read_to_string(e.path().join(".reason")).unwrap_or_default();
            let profile = fs::read_to_string(e.path().join(".profile")).unwrap_or_default();
            let saves = fs::read_dir(e.path()).map(|d| d.flatten().filter(|x| x.path().is_dir()).count()).unwrap_or(0);
            Snapshot { name, ts, reason, saves, profile }
        })
        .collect();
    v.sort_by_key(|s| std::cmp::Reverse(s.ts));
    v
}

/// Put a snapshot's saves back into the folder of the profile they came from (current saves
/// are snapshotted first). Snapshots of another profile's saves need that profile active.
pub fn restore_snapshot(paths: &Paths, name: &str) -> Result<usize> {
    let snap = snapshots_dir(paths).join(name);
    if !snap.is_dir() {
        bail!("no snapshot {name}");
    }
    let st = State::load(paths)?;
    let from = fs::read_to_string(snap.join(".profile")).unwrap_or_default();
    if !from.is_empty() && from != st.active_profile && st.profiles.iter().any(|p| p.name == from) {
        bail!("snapshot {name} holds saves of profile {from:?}; switch to it first (cpmm profile switch {from:?})");
    }
    snapshot_saves(paths, &format!("before restoring {name}"))?;
    let dst = profile_saves_dir(paths, &st, None);
    let mut n = 0;
    for f in files::list_tree(&snap).into_iter().filter(|f| f != ".reason" && f != ".profile") {
        let d = dst.join(&f);
        if d.exists() {
            fs::remove_file(&d)?;
        }
        files::reflink_or_copy(&snap.join(&f), &d)?;
        n += 1;
    }
    Ok(n)
}

/// Mods that add quest content (losing one mid-playthrough can break quest progress).
pub fn quest_mods(paths: &Paths, st: &State) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for m in &st.mods {
        let root = paths.mod_files(&m.id);
        let questy = m.name.to_lowercase().contains("quest")
            || files::list_tree(&root).iter().any(|f| {
                let l = f.to_lowercase();
                (l.ends_with(".xl")
                    && fs::read_to_string(root.join(f))
                        .is_ok_and(|s| s.contains("quest:") || s.contains("questphase") || s.contains(".questphase")))
                    || l.contains("/quest/")
            });
        if questy {
            out.insert(m.id.clone());
        }
    }
    out
}

#[derive(Serialize, Debug)]
pub struct SaveRisk {
    pub save: String,
    pub saved_at: i64,
    pub missing: Vec<String>,
}
/// Recent saves made while a quest mod was active that is now disabled or uninstalled.
pub fn save_risks(paths: &Paths, st: &State) -> Vec<SaveRisk> {
    let sess = sessions(paths);
    let quests = quest_mods(paths, st);
    // quest mods known from session snapshots, even if uninstalled since
    let active_now: BTreeSet<&str> = st.mods.iter().filter(|m| m.enabled).map(|m| m.id.as_str()).collect();
    let mut saves: Vec<(i64, String)> = fs::read_dir(saves_dir(paths))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().join("sav.dat").is_file())
        .map(|e| (mtime(&e.path().join("sav.dat")), e.file_name().to_string_lossy().to_string()))
        .collect();
    saves.sort_by_key(|s| std::cmp::Reverse(s.0));
    let mut out = vec![];
    for (t, name) in saves.into_iter().take(5) {
        let Some(s) = sess.iter().rev().find(|s| s.start <= t) else { continue };
        let missing: Vec<String> = s
            .mods
            .iter()
            .filter(|m| m.enabled && !active_now.contains(m.id.as_str()) && (quests.contains(&m.id) || m.id.contains("quest")))
            .map(|m| m.id.clone())
            .collect();
        if !missing.is_empty() {
            out.push(SaveRisk { save: name, saved_at: t, missing });
        }
    }
    out
}
