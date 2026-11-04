//! `cpmm api …`: JSON views of the state for the GUI (and scripts). Read-only.

use crate::conflicts;
use crate::files;
use crate::state::{Paths, State};
use anyhow::Result;
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn nexus_url(id: u64) -> String {
    format!("https://www.nexusmods.com/cyberpunk2077/mods/{id}")
}

pub fn state(paths: &Paths, st: &State) -> Result<Value> {
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
    let mods: Vec<Value> = st
        .mods
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let reqs: Vec<Value> = m
                .requires
                .iter()
                .map(|r| {
                    let dep = st.by_nexus(r.mod_id);
                    let status = match dep {
                        Some(d) if d.enabled => "ok",
                        Some(_) => "disabled",
                        None if m.ignored_reqs.contains(&r.mod_id) => "ignored",
                        None if r.optional => "optional-missing",
                        None => "missing",
                    };
                    json!({"mod_id": r.mod_id, "name": r.name, "notes": r.notes, "optional": r.optional,
                           "status": status, "installed_as": dep.map(|d| d.id.clone()), "url": nexus_url(r.mod_id)})
                })
                .collect();
            let problems = reqs.iter().filter(|r| r["status"] == "missing" || r["status"] == "disabled").count();
            json!({
                "priority": i + 1,
                "id": m.id, "name": m.name, "enabled": m.enabled, "version": m.version,
                "installed_at": m.installed_at, "notes": m.notes,
                "nexus": m.nexus.as_ref().map(|n| json!({"mod_id": n.mod_id, "file_name": n.file_name, "file_id": n.file_id,
                    "uploaded_at": n.uploaded_at, "url": nexus_url(n.mod_id)})),
                "files": files::list_tree(&paths.mod_files(&m.id)).len(),
                "hidden": files::list_tree(&paths.mods().join(&m.id).join("hidden")).len(),
                "wins": wins.get(m.id.as_str()).copied().unwrap_or(0),
                "loses": loses.get(m.id.as_str()).copied().unwrap_or(0),
                "requires": reqs,
                "dep_problems": if m.enabled { problems } else { 0 },
            })
        })
        .collect();
    let session = crate::vfs::running(paths).map(|(info, pid)| json!({"pid": pid, "started": info.started, "layers": info.layers.len()}));
    let modio: Vec<Value> = crate::modio_mods(paths)
        .into_iter()
        .map(|(id, name, by, ver)| json!({"id": id, "name": name, "author": by, "version": ver}))
        .collect();
    let overwrite = crate::manage::overwrite_entries(paths).len();
    Ok(json!({
        "game_dir": paths.game, "data_dir": paths.data,
        "game_running": crate::game_running(),
        "session": session, "mods": mods, "modio": modio, "overwrite_count": overwrite,
        "nxm_handler": crate::manage::current_nxm_handler() == "cpmm-nxm.desktop",
        "active_profile": st.active_profile,
        "profiles": st.synced_profiles().iter().map(|p| json!({"name": p.name, "enabled": p.enabled.len(), "active": p.name == st.active_profile, "created_at": p.created_at,
            "own_saves": p.own_saves, "saves": crate::profiles::count_saves(&p.own_saves_dir(paths).unwrap_or_else(|| crate::safety::shared_saves_dir(paths)))})).collect::<Vec<_>>(),
    }))
}

pub fn info(paths: &Paths, st: &State, key: &str) -> Result<Value> {
    let i = st.get(key)?;
    let m = &st.mods[i];
    let own = conflicts::ownership(paths, &st.mods, false);
    let files: Vec<Value> = files::list_tree(&paths.mod_files(&m.id))
        .into_iter()
        .map(|f| {
            let owners = own.get(&f.to_lowercase()).map(|(_, o)| o.clone()).unwrap_or_default();
            let enabled_owners: Vec<&String> = owners.iter().filter(|o| st.mods.iter().any(|x| &x.id == *o && x.enabled)).collect();
            let winner = enabled_owners.last().map(|s| s.as_str()).unwrap_or("");
            let others: Vec<&String> = owners.iter().filter(|o| **o != m.id).collect();
            json!({"path": f, "others": others, "winner": winner, "wins": winner == m.id})
        })
        .collect();
    let hidden = files::list_tree(&paths.mods().join(&m.id).join("hidden"));
    Ok(json!({"id": m.id, "name": m.name, "files": files, "hidden": hidden, "ignored_reqs": m.ignored_reqs}))
}

pub fn conflicts(paths: &Paths, st: &State) -> Result<Value> {
    let fc = conflicts::file_conflicts(paths, &st.mods, true);
    let files: Vec<Value> = fc
        .iter()
        .filter(|c| c.mods.len() > 1)
        .map(|c| json!({"path": c.path, "mods": c.mods, "winner": c.mods.last(), "identical": c.identical, "overrides_base": c.overrides_base}))
        .collect();
    let base = fc.iter().filter(|c| c.overrides_base).count();
    let scripts: Vec<Value> = conflicts::redscript_conflicts(paths, &st.mods, &crate::unmanaged_reds(paths, st))
        .into_iter()
        .map(|c| json!({"kind": c.kind, "target": c.target, "mods": c.mods, "severe": c.kind.starts_with("REPLACE") || c.kind.starts_with("duplicate")}))
        .collect();
    Ok(json!({"files": files, "scripts": scripts, "base_overrides": base}))
}

pub fn updates(paths: &Paths, st: &State, refresh: bool) -> Result<Value> {
    let res = crate::check_updates(paths, st, refresh);
    Ok(Value::Array(
        res.into_iter()
            .map(|u| {
                json!({"id": u.id, "status": u.status, "update": u.newer.as_ref().map(|v| json!({"version": v.version, "uploaded_at": v.uploaded_at, "gsid": v.game_scoped_id}))})
            })
            .collect(),
    ))
}

pub fn overwrite(paths: &Paths, st: &State) -> Result<Value> {
    let own = conflicts::ownership(paths, &st.mods, true);
    Ok(Value::Array(
        crate::manage::overwrite_entries(paths)
            .into_iter()
            .map(|e| {
                let shadows = own.get(&e.rel.to_lowercase()).and_then(|(_, o)| o.last().cloned());
                let kind = if e.whiteout {
                    "deleted"
                } else if shadows.is_some() {
                    "edited mod file"
                } else if paths.game.join(&e.rel).exists() {
                    "edited base file"
                } else {
                    "new"
                };
                json!({"path": e.rel, "kind": kind, "shadows": shadows})
            })
            .collect(),
    ))
}

pub fn tracked(paths: &Paths, st: &State) -> Result<Value> {
    let nx = crate::nexus(paths);
    let ids = nx.tracked()?;
    Ok(Value::Array(
        ids.into_iter()
            .map(|gid| {
                let name = nx.mod_info(gid).map(|i| i.name).unwrap_or_default();
                json!({"mod_id": gid, "name": name, "installed_as": st.by_nexus(gid).map(|m| m.id.clone()), "url": nexus_url(gid)})
            })
            .collect(),
    ))
}

pub fn search_info(paths: &Paths, gid: u64) -> Result<Value> {
    let nx = crate::nexus(paths);
    let info = nx.mod_info(gid)?;
    let reqs = nx.requirements(&[gid], false)?.into_iter().next().map(|x| x.2).unwrap_or_default();
    let mut files = vec![];
    for f in nx.mod_files(&info.id)?.into_iter().filter(|f| f.is_active) {
        if let Some(v) = nx.versions(&f.id)?.into_iter().next() {
            files.push(json!({"gsid": v.game_scoped_id, "name": f.name, "version": v.version, "category": v.category, "uploaded_at": v.uploaded_at}));
        }
    }
    Ok(json!({
        "mod_id": gid, "name": info.name, "version": info.version, "author": info.author, "url": nexus_url(gid),
        "files": files,
        "requires": reqs.iter().map(|r| json!({"mod_id": r.mod_id, "name": r.name, "notes": r.notes, "optional": r.optional(), "external": r.external})).collect::<Vec<_>>(),
    }))
}
