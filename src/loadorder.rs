//! Automatic load order: archive resource conflicts, a stable dependency-aware sort, and the
//! generated `archive/pc/mod/modlist.txt`.
//!
//! Semantics used throughout: a mod later in cpmm's priority list WINS. For loose files the
//! overlay already does that; for `.archive` resources the game uses `modlist.txt` (archives
//! listed first load first and take precedence; unlisted ones follow alphabetically), so cpmm
//! writes it with the highest-priority mod's archives first.

use crate::archive;
use crate::conflicts;
use crate::files;
use crate::state::{Paths, State};
use anyhow::Result;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap, HashMap};
use std::fs;
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Rule {
    /// This mod must load after (win over) `after`.
    pub r#mod: String,
    pub after: String,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(tag = "kind", content = "id", rename_all = "lowercase")]
pub enum Owner {
    Mod(String),
    Base(String),
    Modio(String),
}
impl Owner {
    pub fn label(&self) -> String {
        match self {
            Owner::Mod(id) => id.clone(),
            Owner::Base(_) => "(unmanaged)".into(),
            Owner::Modio(n) => format!("mod.io: {n}"),
        }
    }
}

pub struct Arc {
    pub owner: Owner,
    /// file name as the game sees it (modlist.txt entry) or a path for mod.io
    pub name: String,
    pub path: PathBuf,
}

const MOD_DIR: &str = "archive/pc/mod/";

/// Every archive the game will load: enabled mods (the copy that wins in the overlay),
/// leftovers in the real game dir, and mod.io subscriptions.
pub fn collect(paths: &Paths, st: &State) -> Vec<Arc> {
    let mut out = vec![];
    for (_, (rel, owners)) in conflicts::ownership(paths, &st.mods, true) {
        let l = rel.to_lowercase();
        if l.starts_with(MOD_DIR) && l.ends_with(".archive") && !l[MOD_DIR.len()..].contains('/') {
            let w = owners.last().unwrap().clone();
            out.push(Arc { name: rel[MOD_DIR.len()..].to_string(), path: paths.mod_files(&w).join(&rel), owner: Owner::Mod(w) });
        }
    }
    for f in files::list_tree(&paths.game.join(MOD_DIR)) {
        if f.to_lowercase().ends_with(".archive") && !f.contains('/') {
            out.push(Arc { owner: Owner::Base(f.clone()), name: f.clone(), path: paths.game.join(MOD_DIR).join(&f) });
        }
    }
    let names: HashMap<String, String> = crate::modio::list(paths).into_iter().map(|m| (m.id, m.name)).collect();
    let mio = paths.modio_dir().join("mods");
    if let Ok(rd) = fs::read_dir(&mio) {
        for d in rd.flatten() {
            let id = d.file_name().to_string_lossy().to_string();
            let name = names.get(&id).cloned().unwrap_or(id.clone());
            for f in files::list_tree(&d.path()) {
                if f.to_lowercase().ends_with(".archive") {
                    out.push(Arc {
                        owner: Owner::Modio(name.clone()),
                        name: f.rsplit('/').next().unwrap_or(&f).to_string(),
                        path: d.path().join(&f),
                    });
                }
            }
        }
    }
    out
}

#[derive(Serialize, Clone, Debug)]
pub struct Pair {
    pub a: Owner,
    pub b: Owner,
    pub archive_a: String,
    pub archive_b: String,
    pub shared: usize,
    pub identical: usize,
    /// resources in the smaller archive (to tell "patch" from "duplicate")
    pub smaller: usize,
    pub related: bool,
    pub winner: Option<String>,
}

/// Resource-level conflicts between archives of different owners.
pub fn archive_conflicts(paths: &Paths, st: &State, order: &[String]) -> Vec<Pair> {
    let arcs = collect(paths, st);
    let idx: Vec<Option<archive::Index>> = arcs.iter().map(|a| archive::read_index(&a.path).ok()).collect();
    let mut by_hash: HashMap<u64, Vec<(usize, usize)>> = HashMap::new();
    for (ai, ix) in idx.iter().enumerate() {
        if let Some(ix) = ix {
            for (ei, e) in ix.entries.iter().enumerate() {
                by_hash.entry(e.hash).or_default().push((ai, ei));
            }
        }
    }
    let mut pairs: BTreeMap<(usize, usize), (usize, usize)> = BTreeMap::new();
    for hits in by_hash.values().filter(|h| h.len() > 1) {
        for i in 0..hits.len() {
            for j in i + 1..hits.len() {
                let ((a, ea), (b, eb)) = (hits[i], hits[j]);
                if arcs[a].owner == arcs[b].owner {
                    continue;
                }
                let key = if a < b { (a, b) } else { (b, a) };
                let same = match (&idx[a], &idx[b]) {
                    (Some(ia), Some(ib)) => {
                        let fa = archive::fingerprint(&arcs[a].path, ia, &ia.entries[ea]).ok();
                        fa.is_some() && fa == archive::fingerprint(&arcs[b].path, ib, &ib.entries[eb]).ok()
                    }
                    _ => false,
                };
                let e = pairs.entry(key).or_default();
                e.0 += 1;
                e.1 += same as usize;
            }
        }
    }
    let pos: HashMap<&str, usize> = order.iter().enumerate().map(|(i, s)| (s.as_str(), i)).collect();
    let rel = related_pairs(st);
    pairs
        .into_iter()
        .map(|((a, b), (shared, identical))| {
            let (oa, ob) = (&arcs[a].owner, &arcs[b].owner);
            let winner = match (oa, ob) {
                (Owner::Mod(x), Owner::Mod(y)) => Some(if pos.get(x.as_str()) > pos.get(y.as_str()) { x.clone() } else { y.clone() }),
                (Owner::Mod(x), Owner::Base(_)) | (Owner::Base(_), Owner::Mod(x)) => Some(x.clone()),
                _ => None, // mod.io load order relative to archive/pc/mod is the game's business
            };
            let related = match (oa, ob) {
                (Owner::Mod(x), Owner::Mod(y)) => rel.contains(&(x.clone(), y.clone())) || rel.contains(&(y.clone(), x.clone())),
                _ => false,
            };
            let smaller = idx[a].as_ref().map_or(0, |i| i.entries.len()).min(idx[b].as_ref().map_or(0, |i| i.entries.len()));
            Pair {
                a: oa.clone(),
                b: ob.clone(),
                archive_a: arcs[a].name.clone(),
                archive_b: arcs[b].name.clone(),
                shared,
                identical,
                smaller,
                related,
                winner,
            }
        })
        .collect()
}

/// (dependent, dependency) pairs among installed mods, from Nexus requirements.
fn related_pairs(st: &State) -> BTreeSet<(String, String)> {
    let mut s = BTreeSet::new();
    // Files from the same Nexus page: the main file (known file id) is the base, add-ons
    // (imported without a file id, or optional files) depend on it.
    for a in &st.mods {
        for b in &st.mods {
            if let (Some(na), Some(nb)) = (&a.nexus, &b.nexus)
                && a.id != b.id
                && na.mod_id == nb.mod_id
                && na.file_id.is_empty()
                && !nb.file_id.is_empty()
            {
                s.insert((a.id.clone(), b.id.clone()));
            }
        }
    }
    for m in &st.mods {
        for r in &m.requires {
            if let Some(d) = st.by_nexus(r.mod_id)
                && d.id != m.id
            {
                s.insert((m.id.clone(), d.id.clone()));
            }
        }
    }
    s
}

#[derive(Serialize, Clone, Debug)]
pub struct Edge {
    pub before: String,
    pub after: String,
    pub why: String,
    pub strength: &'static str,
}

#[derive(Serialize, Debug)]
pub struct SortPlan {
    pub order: Vec<String>,
    pub moved: Vec<Moved>,
    pub edges: Vec<Edge>,
    pub cycles: Vec<Vec<String>>,
    pub unresolved: Vec<Pair>,
}
#[derive(Serialize, Debug)]
pub struct Moved {
    pub id: String,
    pub from: usize,
    pub to: usize,
    pub because: Vec<String>,
}

/// Stable topological sort: constraints (requirements, user rules, patch heuristics on real
/// conflicts) are enforced; everything else keeps its current relative order.
pub fn plan(paths: &Paths, st: &State) -> SortPlan {
    let ids: Vec<String> = st.mods.iter().map(|m| m.id.clone()).collect();
    let at: HashMap<&str, usize> = ids.iter().enumerate().map(|(i, s)| (s.as_str(), i)).collect();
    // Order only matters where two mods overlap (same loose file or same archive resource),
    // so constraints are only enforced between overlapping pairs: minimal, meaningful moves.
    let pairs = archive_conflicts(paths, st, &ids);
    let mut overlaps: BTreeSet<(String, String)> = BTreeSet::new();
    for p in &pairs {
        if let (Owner::Mod(a), Owner::Mod(b)) = (&p.a, &p.b)
            && p.identical < p.shared
        {
            overlaps.insert((a.clone(), b.clone()));
        }
    }
    for c in conflicts::file_conflicts(paths, &st.mods, true).into_iter().filter(|c| !c.identical && c.mods.len() >= 2) {
        for i in 0..c.mods.len() {
            for j in i + 1..c.mods.len() {
                overlaps.insert((c.mods[i].clone(), c.mods[j].clone()));
            }
        }
    }
    let overlap =
        |a: &str, b: &str| overlaps.contains(&(a.to_string(), b.to_string())) || overlaps.contains(&(b.to_string(), a.to_string()));
    let mut edges: Vec<Edge> = vec![];
    for (m, d) in related_pairs(st) {
        if !overlap(&m, &d) {
            continue;
        }
        let opt = st.mods[at[m.as_str()]].requires.iter().any(|r| st.by_nexus(r.mod_id).is_some_and(|x| x.id == d) && r.optional);
        edges.push(Edge {
            before: d.clone(),
            after: m.clone(),
            why: format!("{m} {} {d} and overrides some of its files", if opt { "optionally uses" } else { "requires" }),
            strength: "requirement",
        });
    }
    for r in &st.rules {
        if at.contains_key(r.r#mod.as_str()) && at.contains_key(r.after.as_str()) {
            edges.push(Edge {
                before: r.after.clone(),
                after: r.r#mod.clone(),
                why: format!("your rule: {} after {}", r.r#mod, r.after),
                strength: "rule",
            });
        }
    }
    // Heuristic: a "patch/fix/compat/addon" mod that really overrides another mod's
    // resources or files should win over it.
    let patchy = Regex::new(r"(?i)\b(patch|patches|fix|fixes|compat|compatibility|add-?on|override|overhaul for)\b").unwrap();
    let constrained: BTreeSet<(String, String)> =
        edges.iter().flat_map(|e| [(e.before.clone(), e.after.clone()), (e.after.clone(), e.before.clone())]).collect();
    for (a, b) in &overlaps {
        if constrained.contains(&(a.clone(), b.clone())) {
            continue;
        }
        let (na, nb) = (&st.mods[at[a.as_str()]].name, &st.mods[at[b.as_str()]].name);
        match (patchy.is_match(na), patchy.is_match(nb)) {
            (true, false) => edges.push(Edge {
                before: b.clone(),
                after: a.clone(),
                why: format!("{na} looks like a patch and overrides {nb}"),
                strength: "heuristic",
            }),
            (false, true) => edges.push(Edge {
                before: a.clone(),
                after: b.clone(),
                why: format!("{nb} looks like a patch and overrides {na}"),
                strength: "heuristic",
            }),
            _ => {}
        }
    }

    // Kahn's algorithm, always taking the ready mod that is earliest in the current order.
    let n = ids.len();
    let mut indeg = vec![0usize; n];
    let mut out_e: Vec<Vec<usize>> = vec![vec![]; n];
    let mut seen = BTreeSet::new();
    for e in &edges {
        let (b, a) = (at[e.before.as_str()], at[e.after.as_str()]);
        if b != a && seen.insert((b, a)) {
            out_e[b].push(a);
            indeg[a] += 1;
        }
    }
    let mut heap: BinaryHeap<Reverse<usize>> = (0..n).filter(|&i| indeg[i] == 0).map(Reverse).collect();
    let mut order = vec![];
    let mut done = vec![false; n];
    while let Some(Reverse(i)) = heap.pop() {
        order.push(i);
        done[i] = true;
        for &j in &out_e[i] {
            indeg[j] -= 1;
            if indeg[j] == 0 {
                heap.push(Reverse(j));
            }
        }
    }
    let mut cycles = vec![];
    if order.len() < n {
        let rest: Vec<usize> = (0..n).filter(|&i| !done[i]).collect();
        cycles.push(rest.iter().map(|&i| ids[i].clone()).collect());
        order.extend(rest); // keep their current order
    }
    let order: Vec<String> = order.into_iter().map(|i| ids[i].clone()).collect();
    let newpos: HashMap<&str, usize> = order.iter().enumerate().map(|(i, s)| (s.as_str(), i)).collect();
    let moved = ids
        .iter()
        .enumerate()
        .filter(|(i, id)| newpos[id.as_str()] != *i)
        .map(|(i, id)| Moved {
            id: id.clone(),
            from: i + 1,
            to: newpos[id.as_str()] + 1,
            because: edges
                .iter()
                .filter(|e| (e.after == *id || e.before == *id) && at[e.before.as_str()] > at[e.after.as_str()])
                .map(|e| e.why.clone())
                .collect(),
        })
        .filter(|m| !m.because.is_empty())
        .collect();
    let unresolved = archive_conflicts(paths, st, &order)
        .into_iter()
        .filter(|p| p.identical < p.shared && !p.related && !edges.iter().any(|e| matches!((&p.a, &p.b), (Owner::Mod(x), Owner::Mod(y)) if (e.before == *x && e.after == *y) || (e.before == *y && e.after == *x))))
        .collect();
    let _ = pairs;
    SortPlan { order, moved, edges, cycles, unresolved }
}

pub fn apply(st: &mut State, order: &[String]) {
    let mut by: HashMap<String, crate::state::Mod> = st.mods.drain(..).map(|m| (m.id.clone(), m)).collect();
    for id in order {
        if let Some(m) = by.remove(id) {
            st.mods.push(m);
        }
    }
    st.mods.extend(by.into_values()); // anything not in the plan stays, at the end
}

pub fn generated_root(paths: &Paths) -> PathBuf {
    paths.data.join("generated")
}

/// Archive names in load order (first = highest precedence) for modlist.txt.
pub fn modlist(paths: &Paths, st: &State) -> Vec<String> {
    let arcs = collect(paths, st);
    let mut out = vec![];
    for m in st.mods.iter().rev().filter(|m| m.enabled) {
        let mut mine: Vec<&str> = arcs.iter().filter(|a| a.owner == Owner::Mod(m.id.clone())).map(|a| a.name.as_str()).collect();
        mine.sort_by_key(|s| s.to_lowercase());
        out.extend(mine.into_iter().map(String::from));
    }
    let mut base: Vec<String> = arcs.iter().filter(|a| matches!(a.owner, Owner::Base(_))).map(|a| a.name.clone()).collect();
    base.sort_by_key(|s| s.to_lowercase());
    out.extend(base);
    out
}

/// Write `generated/archive/pc/mod/modlist.txt` (mounted as the top mod layer). Cheap: no
/// archive parsing. Returns the number of archives listed.
pub fn write_modlist(paths: &Paths, st: &State) -> Result<usize> {
    let list = modlist(paths, st);
    let p = generated_root(paths).join("files").join(MOD_DIR).join("modlist.txt");
    if list.is_empty() {
        let _ = fs::remove_file(&p);
        return Ok(0);
    }
    fs::create_dir_all(p.parent().unwrap())?;
    let body = list.join("\r\n") + "\r\n";
    if fs::read_to_string(&p).ok().as_deref() != Some(body.as_str()) {
        fs::write(&p, body)?;
    }
    Ok(list.len())
}
