//! One-time migration of the hand-installed setup into cpmm staging.
//!
//! Sources: per-mod file lists (`installs/*.files`) and check_mods.py's Nexus cache
//! (`.cache/result.json`, `v<fileid>.json`, `mod<gid>.json`). Each mod's files move out of
//! the game directory into `mods/<id>/files` (a rename: same btrfs volume, no extra space).
//! Files claimed by several mods, and files that are part of vanilla, are reflinked instead
//! of moved, so nothing the base game needs disappears from it.

use crate::files;
use crate::state::{Mod, NexusSource, Paths, State, now, short_name, slugify};
use crate::vanilla;
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io::Write;
use std::path::Path;

struct Cand {
    id: String,
    name: String,
    nexus: Option<NexusSource>,
    version: String,
    files: Vec<String>,
    order_key: (u8, u64),
}

fn read_json(p: &Path) -> Option<Value> {
    serde_json::from_slice(&fs::read(p).ok()?).ok()
}

pub fn import(paths: &Paths, state: &mut State, modlist: &Path, apply: bool) -> Result<()> {
    if !state.mods.is_empty() && apply {
        bail!("cpmm already manages {} mods; import is meant to run once on a fresh state", state.mods.len());
    }
    if crate::game_running() {
        bail!("Cyberpunk is running; quit it first");
    }
    let cache = modlist.join(".cache");
    let result = read_json(&cache.join("result.json")).context("missing .cache/result.json; run check_mods.py first")?;
    let result = result.as_object().context("result.json")?;

    // Best matching uploaded version per Nexus mod, like check_mods.py's report.
    struct Best {
        gid: u64,
        name: String,
        fid: String,
        ver: String,
        up: String,
        paths: Vec<String>,
        /// lower-cased paths of every file version on the page
        all: BTreeSet<String>,
    }
    let mut bests = vec![];
    for (gid, r) in result {
        let vs = r["versions"].as_array().cloned().unwrap_or_default();
        let b = vs
            .iter()
            .filter(|v| v["present"].as_u64().unwrap_or(0) > 0)
            .max_by_key(|v| (v["match"].as_u64().unwrap_or(0), u64::MAX - v["n"].as_u64().unwrap_or(0)));
        if let Some(b) = b {
            bests.push(Best {
                gid: gid.parse().unwrap_or(0),
                name: r["name"].as_str().unwrap_or("").to_string(),
                fid: b["fid"].as_str().unwrap_or("").to_string(),
                ver: b["ver"].as_str().unwrap_or("").to_string(),
                up: b["up"].as_str().unwrap_or("").to_string(),
                paths: b["paths"].as_array().cloned().unwrap_or_default().iter().filter_map(|p| p.as_str().map(String::from)).collect(),
                all: vs.iter().flat_map(|v| v["paths"].as_array().cloned().unwrap_or_default()).filter_map(|p| p.as_str().map(|s| s.to_lowercase())).collect(),
            });
        }
    }
    let source_for = |b: &Best| -> NexusSource {
        let mut s = NexusSource { mod_id: b.gid, file_id: b.fid.clone(), uploaded_at: b.up.clone(), ..Default::default() };
        if let Some(m) = read_json(&cache.join(format!("mod{}.json", b.gid))) {
            s.mod_uid = m.pointer("/data/id").and_then(Value::as_str).unwrap_or("").into();
        }
        if let Some(v) = read_json(&cache.join(format!("v{}.json", b.fid))) {
            for x in v.pointer("/data/versions").and_then(Value::as_array).cloned().unwrap_or_default() {
                if x["version"].as_str() == Some(&b.ver) && x["uploaded_at"].as_str() == Some(&b.up) {
                    s.version_id = x["id"].as_str().unwrap_or("").into();
                    s.version_gsid = x["game_scoped_id"].as_str().unwrap_or("").into();
                    s.manifest_url = x["manifest_url"].as_str().unwrap_or("").into();
                    s.file_name = x.pointer("/file/name").and_then(Value::as_str).unwrap_or("").into();
                }
            }
        }
        s
    };

    // File lists -> best-overlapping Nexus mod.
    let mut lists: Vec<(String, u64, Vec<String>)> = vec![];
    for e in fs::read_dir(modlist.join("installs"))?.flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("files") {
            continue;
        }
        let stem = p.file_stem().unwrap().to_string_lossy().to_string();
        let mtime = e.metadata()?.modified()?.duration_since(std::time::UNIX_EPOCH)?.as_secs();
        let fl: Vec<String> = fs::read_to_string(&p)?.lines().map(|l| l.trim().replace('\\', "/")).filter(|l| !l.is_empty() && !l.ends_with('/')).collect();
        lists.push((stem, mtime, fl));
    }
    let mut taken: BTreeSet<u64> = BTreeSet::new();
    let mut cands: Vec<Cand> = vec![];
    // Pair greedily by overlap, biggest first.
    let mut pairs = vec![];
    for (li, (_, _, fl)) in lists.iter().enumerate() {
        let set: BTreeSet<String> = fl.iter().map(|x| x.to_lowercase()).collect();
        for (bi, b) in bests.iter().enumerate() {
            let ov = b.paths.iter().filter(|p| set.contains(&p.to_lowercase())).count();
            // a single shared helper file (e.g. VehicleManufacturerFix.reds) must not pair a list
            if ov > 0 && ov * 2 >= fl.len().min(b.paths.len()) {
                pairs.push((ov, li, bi));
            }
        }
    }
    pairs.sort_by(|a, b| b.cmp(a));
    let mut list_gid: HashMap<usize, usize> = HashMap::new();
    for (_, li, bi) in pairs {
        if list_gid.contains_key(&li) || taken.contains(&bests[bi].gid) {
            continue;
        }
        list_gid.insert(li, bi);
        taken.insert(bests[bi].gid);
    }
    for b in &bests {
        if !taken.contains(&b.gid) {
            // Framework mods installed before file lists were kept: claim the manifest paths.
            let present: Vec<String> = b.paths.iter().filter(|p| paths.game.join(p).is_file()).cloned().collect();
            if present.is_empty() {
                continue;
            }
            cands.push(Cand { id: slugify(short_name(&b.name)), name: b.name.clone(), nexus: Some(source_for(b)), version: b.ver.clone(), files: present, order_key: (0, 0) });
        }
    }
    for (li, (stem, mtime, fl)) in lists.iter().enumerate() {
        let (id, name, nexus, version) = match list_gid.get(&li) {
            Some(&bi) => (slugify(short_name(&bests[bi].name)), bests[bi].name.clone(), Some(source_for(&bests[bi])), bests[bi].ver.clone()),
            None => {
                // Extra file (e.g. an add-on) from a Nexus page already paired with another list:
                // keep the mod id for requirements, but the exact file is unknown.
                let set: BTreeSet<String> = fl.iter().map(|x| x.to_lowercase()).collect();
                let sib = bests.iter().filter(|b| set.iter().any(|p| b.all.contains(p))).max_by_key(|b| set.iter().filter(|p| b.all.contains(*p)).count());
                match sib {
                    Some(b) => (slugify(stem), format!("{} ({stem})", b.name), Some(NexusSource { mod_id: b.gid, ..Default::default() }), String::new()),
                    None => (slugify(stem), stem.clone(), None, String::new()),
                }
            }
        };
        cands.push(Cand { id, name, nexus, version, files: fl.clone(), order_key: (1, *mtime) });
    }
    cands.sort_by_key(|c| c.order_key);

    // Ownership and moves.
    let vanilla = vanilla::load(paths);
    let mut claims: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, c) in cands.iter().enumerate() {
        for f in &c.files {
            claims.entry(f.to_lowercase()).or_default().push(i);
        }
    }
    let mut ids = BTreeSet::new();
    for c in cands.iter_mut() {
        let mut id = c.id.clone();
        let mut n = 2;
        while !ids.insert(id.clone()) {
            id = format!("{}-{n}", c.id);
            n += 1;
        }
        c.id = id;
    }

    println!("vanilla manifest: {} files known", vanilla.len());
    println!("{:<34} {:>6} {:>7} {:>6} {:>6} {:>7}  {}", "id", "nexus", "files", "moved", "copied", "missing", "name");
    let mut journal = if apply {
        paths.ensure_dirs()?;
        Some(fs::OpenOptions::new().create(true).append(true).open(paths.data.join("import-journal.tsv"))?)
    } else {
        None
    };
    let mut new_mods = vec![];
    for (i, c) in cands.iter().enumerate() {
        let (mut moved, mut copied, mut missing) = (0, 0, 0);
        for f in &c.files {
            let src = paths.game.join(f);
            if !src.is_file() {
                missing += 1;
                continue;
            }
            let lf = f.to_lowercase();
            let owners = &claims[&lf];
            let keep_base = vanilla.contains_key(&lf) || vanilla::protected_prefix(&lf);
            let last_owner = *owners.iter().max().unwrap() == i;
            let do_move = !keep_base && last_owner;
            if apply {
                let dst = paths.mod_files(&c.id).join(f);
                if do_move {
                    files::move_file(&src, &dst)?;
                } else {
                    files::reflink_or_copy(&src, &dst)?;
                }
                if let Some(j) = journal.as_mut() {
                    writeln!(j, "{}\t{}\t{}", if do_move { "move" } else { "copy" }, src.display(), dst.display())?;
                }
                if do_move {
                    files::prune_empty_dirs(src.parent().unwrap(), &paths.game);
                }
            }
            if do_move { moved += 1 } else { copied += 1 }
        }
        let nid = c.nexus.as_ref().map(|n| n.mod_id.to_string()).unwrap_or("-".into());
        println!("{:<34} {:>6} {:>7} {:>6} {:>6} {:>7}  {}", c.id, nid, c.files.len(), moved, copied, missing, c.name);
        new_mods.push(Mod {
            id: c.id.clone(),
            name: c.name.clone(),
            enabled: true,
            version: c.version.clone(),
            nexus: c.nexus.clone(),
            requires: vec![],
            reqs_checked: None,
            hidden: vec![],
                ignored_reqs: vec![],
            installed_at: now(),
            notes: "imported".into(),
        });
    }
    let shared = claims.values().filter(|v| v.len() > 1).count();
    println!("\n{} mods, {} files claimed by more than one mod (each owner gets its own reflinked copy)", new_mods.len(), shared);
    if apply {
        state.mods = new_mods;
        state.save(paths)?;
        println!("imported. Base game dir now holds only vanilla + unmanaged files; launch through `cpmm run`.");
    } else {
        println!("dry run; re-run with --apply to move files");
    }
    Ok(())
}

/// Move arbitrary game-dir files/dirs into a new manual mod.
pub fn adopt(paths: &Paths, state: &mut State, id: &str, name: &str, rels: &[String]) -> Result<Vec<String>> {
    if crate::game_running() {
        bail!("Cyberpunk is running; quit it first");
    }
    if state.find(id).is_some() {
        bail!("mod {id} already exists");
    }
    let vanilla = vanilla::load(paths);
    let mut out = vec![];
    for r in rels {
        let r = r.trim_matches('/');
        let src = paths.game.join(r);
        let list: Vec<String> = if src.is_dir() { files::list_tree(&src).into_iter().map(|f| format!("{r}/{f}")).collect() } else { vec![r.to_string()] };
        for f in list {
            let s = paths.game.join(&f);
            let d = paths.mod_files(id).join(&f);
            if vanilla.contains_key(&f.to_lowercase()) || vanilla::protected_prefix(&f.to_lowercase()) {
                files::reflink_or_copy(&s, &d)?;
            } else {
                files::move_file(&s, &d)?;
                files::prune_empty_dirs(s.parent().unwrap(), &paths.game);
            }
            out.push(f);
        }
    }
    state.mods.push(Mod {
        id: id.into(),
        name: name.into(),
        enabled: true,
        version: String::new(),
        nexus: None,
        requires: vec![],
        reqs_checked: None,
        hidden: vec![],
                ignored_reqs: vec![],
        installed_at: now(),
        notes: "adopted".into(),
    });
    state.save(paths)?;
    Ok(out)
}
