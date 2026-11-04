//! mod.io mods (installed by the game's in-game Mods menu, outside the game folder): what is
//! subscribed, what changed since the user last reviewed it, and how it collides with cpmm's mods.

use crate::conflicts;
use crate::files;
use crate::state::{Paths, State};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;

#[derive(Serialize, Clone, Debug)]
pub struct ModioMod {
    pub id: String,
    pub name: String,
    pub author: String,
    pub version: String,
    pub date_updated: i64,
    pub dir: PathBuf,
    /// game-relative paths (mod.io's `archive/X` is the game's `archive/pc/mod/X`)
    pub files: Vec<String>,
}

/// Map a path inside a mod.io mod folder to where the game sees it.
pub fn game_path(rel: &str) -> Option<String> {
    let l = rel.to_lowercase();
    if l == "about.json" || l.starts_with("metadata/") {
        return None;
    }
    match rel.split_once('/') {
        Some((top, rest)) if top.eq_ignore_ascii_case("archive") && !rest.contains('/') => Some(format!("archive/pc/mod/{rest}")),
        Some(_) => Some(rel.to_string()),
        None if l.ends_with(".archive") || l.ends_with(".xl") => Some(format!("archive/pc/mod/{rel}")),
        None => None,
    }
}

pub fn list(paths: &Paths) -> Vec<ModioMod> {
    let root = paths.modio_dir();
    let Ok(b) = fs::read(root.join("metadata/state.json")) else { return vec![] };
    let Ok(v): Result<Value, _> = serde_json::from_slice(&b) else { return vec![] };
    v["Mods"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|m| {
            let p = &m["Profile"];
            let id = m["ID"].as_u64().map(|n| n.to_string()).unwrap_or_else(|| m["ID"].to_string());
            let dir = root.join("mods").join(&id);
            ModioMod {
                name: p["name"].as_str().unwrap_or("?").trim().to_string(),
                author: p.pointer("/submitted_by/username").and_then(Value::as_str).unwrap_or("").to_string(),
                version: p.pointer("/modfile/version").and_then(Value::as_str).unwrap_or("").to_string(),
                date_updated: p["date_updated"].as_i64().unwrap_or(0),
                files: files::list_tree(&dir).iter().filter_map(|f| game_path(f)).collect(),
                dir,
                id,
            }
        })
        .collect()
}

/// mod.io scripts as (owner label, absolute path), for the redscript conflict scanner.
pub fn scripts(paths: &Paths) -> Vec<(String, PathBuf)> {
    list(paths)
        .into_iter()
        .flat_map(|m| {
            let label = format!("mod.io: {}", m.name);
            files::list_tree(&m.dir)
                .into_iter()
                .filter(|f| f.to_lowercase().ends_with(".reds"))
                .map(move |f| (label.clone(), m.dir.join(f)))
        })
        .collect()
}

/* ------------------------------------------------------------------ watcher */

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Seen {
    /// id -> (name, version, date_updated) at the last review
    pub mods: BTreeMap<String, (String, String, i64)>,
    pub reviewed_at: i64,
}

#[derive(Serialize, Debug, Default, ts_rs::TS)]
#[ts(rename = "ModioChanges")]
pub struct Changes {
    pub first_look: bool,
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub updated: Vec<String>,
}

fn seen_file(paths: &Paths) -> PathBuf {
    paths.data.join("modio-seen.json")
}

pub fn changes(paths: &Paths) -> Changes {
    let cur = list(paths);
    let Some(seen) = fs::read(seen_file(paths)).ok().and_then(|b| serde_json::from_slice::<Seen>(&b).ok()) else {
        return Changes { first_look: true, added: cur.iter().map(|m| m.name.clone()).collect(), ..Default::default() };
    };
    let now: BTreeMap<&str, &ModioMod> = cur.iter().map(|m| (m.id.as_str(), m)).collect();
    let mut c = Changes::default();
    for (id, m) in &now {
        match seen.mods.get(*id) {
            None => c.added.push(m.name.clone()),
            Some((_, v, d)) if *v != m.version || *d != m.date_updated => c.updated.push(format!("{} {} -> {}", m.name, v, m.version)),
            _ => {}
        }
    }
    for (id, (name, _, _)) in &seen.mods {
        if !now.contains_key(id.as_str()) {
            c.removed.push(name.clone());
        }
    }
    c
}

/// Mark the current subscriptions as reviewed.
pub fn ack(paths: &Paths) -> Result<usize> {
    let cur = list(paths);
    let s = Seen {
        mods: cur.iter().map(|m| (m.id.clone(), (m.name.clone(), m.version.clone(), m.date_updated))).collect(),
        reviewed_at: crate::safety::now_unix(),
    };
    fs::write(seen_file(paths), serde_json::to_vec_pretty(&s)?)?;
    Ok(cur.len())
}

/* ------------------------------------------------------------------ conflicts */

#[derive(Serialize, Debug, Clone, ts_rs::TS)]
#[ts(rename = "ModioConflict")]
pub struct Conflict {
    pub modio: String,
    #[ts(type = "\"same mod\" | \"files\" | \"archive\" | \"script\"")]
    pub kind: &'static str, // "same mod" | "files" | "archive" | "script"
    pub with: String,
    pub detail: String,
    pub severe: bool,
}

/// Lower-case alphanumerics, minus filler words: "Night City Interactions - Core" -> "nightcityinteractions".
pub fn name_key(s: &str) -> String {
    let filler = ["the", "mod", "core", "redscript", "archivexl", "fix", "fixes", "for", "and", "edition", "v"];
    s.to_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty() && !filler.contains(w) && !w.chars().all(|c| c.is_ascii_digit()))
        .collect::<Vec<_>>()
        .join("")
}

pub fn conflicts(paths: &Paths, st: &State) -> Vec<Conflict> {
    let mio = list(paths);
    if mio.is_empty() {
        return vec![];
    }
    let mut out = vec![];
    // 1. same mod both ways, by name
    for m in &mio {
        let k = name_key(&m.name);
        if k.len() < 6 {
            continue;
        }
        for c in st.mods.iter().filter(|c| c.enabled) {
            let ck = name_key(&c.name);
            if ck.len() >= 6 && (ck == k || ck.starts_with(&k) || k.starts_with(&ck)) {
                out.push(Conflict {
                    modio: m.name.clone(),
                    kind: "same mod",
                    with: c.id.clone(),
                    detail: format!(
                        "\"{}\" is installed from mod.io and from cpmm (\"{}\"). Two copies of one mod are a classic crash cause.",
                        m.name, c.name
                    ),
                    severe: true,
                });
            }
        }
    }
    // 2. identical game paths (tweaks, scripts, configs, archives by name)
    let own = conflicts::ownership(paths, &st.mods, true);
    for m in &mio {
        let mut by_mod: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for f in &m.files {
            if let Some((_, owners)) = own.get(&f.to_lowercase()) {
                by_mod.entry(owners.last().unwrap().clone()).or_default().push(f.clone());
            }
        }
        for (with, fs_) in by_mod {
            if out.iter().any(|c| c.modio == m.name && c.with == with) {
                continue; // already reported as the same mod
            }
            out.push(Conflict {
                modio: m.name.clone(),
                kind: "files",
                with,
                detail: format!(
                    "both ship {} ({} file{})",
                    fs_.iter().take(3).cloned().collect::<Vec<_>>().join(", "),
                    fs_.len(),
                    if fs_.len() == 1 { "" } else { "s" }
                ),
                severe: fs_.iter().any(|f| f.ends_with(".reds") || f.ends_with(".archive")),
            });
        }
    }
    // 3. archive resources (computed by the load order code, which already sees mod.io archives)
    let ids: Vec<String> = st.mods.iter().map(|m| m.id.clone()).collect();
    for p in crate::loadorder::archive_conflicts(paths, st, &ids) {
        let (mio_name, other) = match (&p.a, &p.b) {
            (crate::loadorder::Owner::Modio(n), o) | (o, crate::loadorder::Owner::Modio(n)) => (n.clone(), o.label()),
            _ => continue,
        };
        let differing = p.shared - p.identical;
        if differing == 0 || out.iter().any(|c| c.modio == mio_name && c.with == other && c.kind == "same mod") {
            continue;
        }
        out.push(Conflict {
            modio: mio_name,
            kind: "archive",
            with: other,
            detail: format!(
                "{} vs {}: {differing} game resources replaced by both; cpmm can't order mod.io archives",
                p.archive_a, p.archive_b
            ),
            severe: differing > 2,
        });
    }
    // 4. scripts replacing the same methods
    for c in conflicts::redscript_conflicts(paths, &st.mods) {
        if !(c.kind.starts_with("REPLACE") || c.kind.starts_with("duplicate")) {
            continue;
        }
        let mios: BTreeSet<&String> = c.mods.iter().filter(|m| m.contains("mod.io: ")).collect();
        for mi in &mios {
            let others: Vec<&String> = c.mods.iter().filter(|m| !mios.contains(m)).collect();
            out.push(Conflict {
                modio: mi.strip_prefix("mod.io: ").unwrap_or(mi).to_string(),
                kind: "script",
                with: others.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "),
                detail: format!("{} {}", c.kind, c.target),
                severe: true,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modio_layout_maps_to_game_paths() {
        assert_eq!(game_path("archive/NCI.archive").as_deref(), Some("archive/pc/mod/NCI.archive"));
        assert_eq!(game_path("r6/tweaks/x.yaml").as_deref(), Some("r6/tweaks/x.yaml"));
        assert_eq!(game_path("about.json"), None);
        assert_eq!(game_path("Thing.archive").as_deref(), Some("archive/pc/mod/Thing.archive"));
    }

    #[test]
    fn names_match_across_sources() {
        assert_eq!(name_key("Night City Interactions - Core"), name_key("Night City Interactions"));
        assert_ne!(name_key("Immersive Rippers"), name_key("Immersive Timeskip"));
    }
}
