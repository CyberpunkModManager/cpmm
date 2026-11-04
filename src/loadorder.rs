//! Automatic load order: archive resource conflicts, a stable dependency-aware sort, the
//! generated `archive/pc/mod/modlist.txt`, and a health check (`doctor`).
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
    let names: HashMap<String, String> = crate::modio_mods(paths).into_iter().map(|(id, name, _, _)| (id, name)).collect();
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

/// A one-click fix: the GUI shows `label` and runs `cpmm <args>`.
#[derive(Serialize, Debug, Clone)]
pub struct Action {
    pub label: String,
    pub args: Vec<String>,
    /// "primary" for the recommended choice
    pub style: &'static str,
}
fn act(label: impl Into<String>, args: &[&str], style: &'static str) -> Action {
    Action { label: label.into(), args: args.iter().map(|s| s.to_string()).collect(), style }
}

#[derive(Serialize, Debug)]
pub struct Finding {
    pub severity: &'static str, // "crash" | "warning" | "perf" | "info" | "acked"
    pub title: String,
    pub detail: String,
    /// What to do when there is no in-app action (e.g. mod.io mods)
    pub fix: Option<String>,
    pub actions: Vec<Action>,
}

/// Health check for things that commonly cause crashes, broken saves or stutter.
pub fn doctor(paths: &Paths, st: &State) -> Vec<Finding> {
    let mut f = vec![];
    let ids: Vec<String> = st.mods.iter().map(|m| m.id.clone()).collect();
    let name = |id: &str| st.mods.iter().find(|m| m.id == id).map(|m| m.name.clone()).unwrap_or(id.to_string());
    for (owner, imp, rel) in conflicts::unresolved_imports(paths, &st.mods) {
        f.push(Finding {
            severity: "crash",
            title: format!("{owner} imports {imp}, which no installed mod provides"),
            detail: format!("{rel}: one unresolved import fails the whole redscript compile, so every script mod stops working. Install the mod that provides `{imp}` (see this mod's requirements) or disable it."),
            fix: None,
            actions: if owner.starts_with('(') { vec![] } else { vec![act(format!("Disable {}", name(&owner)), &["disable", &owner], "primary")] },
        });
    }
    for m in st.mods.iter().filter(|m| m.enabled) {
        for r in &m.requires {
            if r.optional || m.ignored_reqs.contains(&r.mod_id) {
                continue;
            }
            let rid = r.mod_id.to_string();
            match st.by_nexus(r.mod_id) {
                None => f.push(Finding {
                    severity: "warning",
                    title: format!("{} is missing a requirement: {}", m.name, r.name),
                    detail: if r.notes.is_empty() {
                        "Parts of the mod may not work.".into()
                    } else {
                        format!("Author's note: {}", r.notes)
                    },
                    fix: None,
                    actions: vec![
                        act(format!("Install {}", r.name), &["install", &rid, "--with-deps"], "primary"),
                        act("Skip it (ignore)", &["deps", "--ignore", &m.id, &rid], "secondary"),
                    ],
                }),
                Some(d) if !d.enabled => f.push(Finding {
                    severity: "crash",
                    title: format!("{} requires {}, which is disabled", m.name, d.name),
                    detail: String::new(),
                    fix: None,
                    actions: vec![
                        act(format!("Enable {}", d.name), &["enable", &d.id], "primary"),
                        act(format!("Disable {}", m.name), &["disable", &m.id], "secondary"),
                    ],
                }),
                _ => {}
            }
        }
    }
    for c in conflicts::redscript_conflicts(paths, &st.mods, &crate::unmanaged_reds(paths, st)) {
        if c.kind.starts_with("REPLACE") || c.kind.starts_with("duplicate") {
            f.push(Finding {
                severity: "warning",
                title: format!("Script clash on {}", c.target),
                detail: format!("{} between {}. Only one version can exist: one mod's behaviour is silently lost, or redscript refuses to compile. Look for a compatibility patch, or keep only one.", c.kind, c.mods.join(", ")),
                fix: None,
                actions: c.mods.iter().filter(|m| !m.starts_with('(')).map(|m| act(format!("Disable {}", name(m)), &["disable", m], "secondary")).collect(),
            });
        }
    }
    let ruled = |p: &Pair| match (&p.a, &p.b) {
        (Owner::Mod(x), Owner::Mod(y)) => st.rules.iter().any(|r| (r.r#mod == *x && r.after == *y) || (r.r#mod == *y && r.after == *x)),
        _ => false,
    };
    for p in archive_conflicts(paths, st, &ids) {
        let (la, lb) = (p.a.label(), p.b.label());
        let modio = matches!(p.a, Owner::Modio(_)) || matches!(p.b, Owner::Modio(_));
        let dup_name = p.archive_a.eq_ignore_ascii_case(&p.archive_b);
        let mostly_same = p.shared >= 10 && p.shared * 2 >= p.smaller.max(1);
        let differing = p.shared - p.identical;
        let mods: Vec<String> = [&p.a, &p.b].iter().filter_map(|o| if let Owner::Mod(x) = o { Some(x.clone()) } else { None }).collect();
        if (dup_name && p.shared * 2 >= p.smaller.max(1)) || (modio && mostly_same) {
            f.push(Finding {
                severity: "crash",
                title: format!("Same mod installed twice: {la} and {lb}"),
                detail: format!(
                    "{} and {} share {} of {} resources. Two copies of one mod double-hook the game and are a classic crash cause.",
                    p.archive_a, p.archive_b, p.shared, p.smaller
                ),
                fix: modio.then(|| "Unsubscribe the mod.io copy in the in-game Mods menu (quit the game afterwards).".into()),
                actions: mods.iter().map(|m| act(format!("Disable {}", name(m)), &["disable", m], "secondary")).collect(),
            });
        } else if p.identical == p.shared && p.shared == p.smaller && p.smaller > 0 {
            f.push(Finding {
                severity: "perf",
                title: format!("{} is fully contained in {}", p.archive_a, p.archive_b),
                detail: format!("{la} and {lb} ship {} byte-identical resources; the duplicate costs load time and memory.", p.shared),
                fix: None,
                actions: vec![],
            });
        } else if differing > 0 && !p.related && !ruled(&p) {
            let minor = differing <= 2 && p.identical * 5 >= p.shared * 4;
            let winner = p.winner.clone().unwrap_or("the game's own order".into());
            let actions = if let (Owner::Mod(a), Owner::Mod(b)) = (&p.a, &p.b) {
                vec![
                    act(
                        format!("{} wins", name(a)),
                        &["rule", a, "--after", b],
                        if p.winner.as_deref() == Some(a.as_str()) { "primary" } else { "secondary" },
                    ),
                    act(
                        format!("{} wins", name(b)),
                        &["rule", b, "--after", a],
                        if p.winner.as_deref() == Some(b.as_str()) { "primary" } else { "secondary" },
                    ),
                ]
            } else {
                vec![]
            };
            f.push(Finding {
                severity: if minor { "info" } else { "warning" },
                title: format!("{la} and {lb} replace the same {} game resource{}", differing, if differing == 1 { "" } else { "s" }),
                detail: format!("{} vs {}. Currently {} wins. Mismatched meshes/textures from two mods are a common source of crashes and visual glitches; pick the one you want.", p.archive_a, p.archive_b, if p.winner.is_some() { name(&winner) } else { winner }),
                fix: modio.then(|| "cpmm can't order mod.io archives; unsubscribe one of them in the in-game Mods menu if something looks wrong.".into()),
                actions,
            });
        }
    }
    let own = conflicts::ownership(paths, &st.mods, true);
    let shadow: Vec<String> = crate::manage::overwrite_entries(paths)
        .into_iter()
        .filter(|e| !e.whiteout && own.contains_key(&e.rel.to_lowercase()) && !e.rel.ends_with(".log"))
        .map(|e| e.rel)
        .collect();
    if !shadow.is_empty() {
        let mut clear = vec!["overwrite", "clear"];
        clear.extend(shadow.iter().map(|s| s.as_str()));
        f.push(Finding {
            severity: "info",
            title: format!("{} mod files are overridden by runtime copies in overwrite/", shadow.len()),
            detail: format!("Settings the game saved (e.g. {}). After updating a mod, stale copies can keep old values. Keep them unless a mod behaves as if its update didn't apply.", shadow.iter().take(3).cloned().collect::<Vec<_>>().join(", ")),
            fix: None,
            actions: vec![act("Reset to the mods' shipped versions", &clear, "secondary")],
        });
    }
    // --- mod.io watcher ---
    let ch = crate::modio::changes(paths);
    if !ch.first_look && (!ch.added.is_empty() || !ch.updated.is_empty()) {
        f.push(Finding {
            severity: "warning",
            title: format!("mod.io subscriptions changed: {} new, {} updated", ch.added.len(), ch.updated.len()),
            detail: format!(
                "{}{}. mod.io mods load outside cpmm's control; check the conflicts below, then mark them reviewed.",
                ch.added.iter().map(|a| format!("new: {a}")).collect::<Vec<_>>().join("; "),
                if ch.updated.is_empty() { String::new() } else { format!("; updated: {}", ch.updated.join("; ")) }
            ),
            fix: Some("Unsubscribe in the in-game Mods menu if you didn't mean to add them.".into()),
            actions: vec![act("Mark as reviewed", &["modio", "--ack"], "secondary")],
        });
    }
    // archive and script clashes with mod.io mods are already reported by the general checks
    for c in crate::modio::conflicts(paths, st).into_iter().filter(|c| c.kind != "archive" && c.kind != "script") {
        let title = match c.kind {
            "same mod" => format!("Same mod installed twice: {} and mod.io: {}", c.with, c.modio),
            "script" => format!("mod.io: {} clashes with {} in a script", c.modio, c.with),
            _ => format!("mod.io: {} and {} ship the same files", c.modio, c.with),
        };
        if f.iter().any(|x| {
            x.title == title || (c.kind == "same mod" && x.title.starts_with("Same mod installed twice") && x.title.contains(&c.modio))
        }) {
            continue;
        }
        let disable = if st.find(&c.with).is_some() {
            vec![act(format!("Disable {}", name(&c.with)), &["disable", &c.with], "secondary")]
        } else {
            vec![]
        };
        f.push(Finding {
            severity: if c.kind == "same mod" {
                "crash"
            } else if c.severe {
                "warning"
            } else {
                "info"
            },
            title,
            detail: c.detail.clone(),
            fix: Some("To drop the mod.io copy, unsubscribe in the in-game Mods menu (quit the game afterwards).".into()),
            actions: disable,
        });
    }
    // --- safety net findings ---
    let sess = crate::safety::sessions(paths);
    if let Some(c) = crate::safety::crashes(paths, 1).into_iter().next() {
        // only flag the crash if it ended the latest recorded session
        if sess.last().is_some_and(|s| c.session_start == Some(s.start)) {
            let mut actions: Vec<Action> = c
                .suspects
                .iter()
                .filter(|m| !crate::safety::list_history(paths, m).is_empty())
                .take(4)
                .map(|m| act(format!("Roll back {}", name(m)), &["rollback", m], "secondary"))
                .collect();
            if let Some(b) = crate::safety::batches(paths, st).into_iter().next()
                && c.last_good.is_some_and(|g| b.ts > g)
            {
                actions.insert(0, act("Undo the last update run", &["undo-update", &b.batch], "primary"));
            }
            f.push(Finding {
                severity: "crash",
                title: format!(
                    "The last session crashed {} in ({} {})",
                    c.seconds_in.map(|s| format!("{}m{}s", s / 60, s % 60)).unwrap_or_default(),
                    c.module,
                    c.offset
                ),
                detail: {
                    let msg = c.message.trim_end_matches('.');
                    if c.changes.is_empty() {
                        format!("{msg}. No mod changes since the last clean session.")
                    } else {
                        format!("{msg}. Changed since the last clean session: {}", c.changes.join("; "))
                    }
                },
                fix: None,
                actions,
            });
        }
    }
    let ed = crate::safety::error_diff(paths);
    let new_n: usize = ed.new.values().map(|v| v.len()).sum();
    if new_n > 0 {
        let first: Vec<String> = ed.new.iter().flat_map(|(s, v)| v.iter().take(2).map(move |l| format!("[{s}] {l}"))).take(3).collect();
        f.push(Finding {
            severity: "warning",
            title: format!("{new_n} new errors in the logs since the previous launch"),
            detail: first.join(" · "),
            fix: None,
            actions: vec![],
        });
    }
    let nx = crate::nexus::Nexus::new(paths.api_key().ok(), &paths.cache());
    let ps = crate::safety::patch_status(paths, st, Some(&nx));
    if ps.patch_detected && ps.at_risk.iter().any(|x| !x.released_since_patch || x.update_available) {
        let mut actions: Vec<Action> = ps
            .at_risk
            .iter()
            .filter(|x| x.update_available)
            .map(|x| act(format!("Update {}", x.name), &["update", &x.id], "primary"))
            .collect();
        if !ps.vanilla {
            actions.push(act("Launch vanilla next time", &["vanilla", "on"], "secondary"));
        }
        f.push(Finding {
            severity: "crash",
            title: format!("Cyberpunk was updated (build {}) and {} native mods may not support it yet", ps.build_id, ps.at_risk.len()),
            detail: ps
                .at_risk
                .iter()
                .map(|x| format!("{}: {}", x.name, if x.update_available { "update available" } else { "no release since the patch" }))
                .collect::<Vec<_>>()
                .join("; "),
            fix: None,
            actions,
        });
    }
    if !ps.red4ext_errors.is_empty() {
        f.push(Finding {
            severity: "warning",
            title: "RED4ext reported errors at the last launch".into(),
            detail: ps.red4ext_errors.iter().take(3).cloned().collect::<Vec<_>>().join(" · "),
            fix: None,
            actions: vec![],
        });
    }
    for r in crate::safety::save_risks(paths, st) {
        let actions = r
            .missing
            .iter()
            .filter(|m| st.find(m).is_some())
            .map(|m| act(format!("Re-enable {}", name(m)), &["enable", m], "primary"))
            .collect();
        f.push(Finding {
            severity: "warning",
            title: format!("Save {} was made with quest mods that are now off", r.save),
            detail: format!(
                "{} — loading it without them can break quest progress (like Playing for Time). Re-enable them or load an earlier save.",
                r.missing.join(", ")
            ),
            fix: None,
            actions,
        });
    }
    let sp = plan(paths, st);
    if !sp.moved.is_empty() {
        f.push(Finding {
            severity: "warning",
            title: format!("Load order breaks {} constraints", sp.moved.len()),
            detail: sp.moved.iter().flat_map(|m| m.because.clone()).take(4).collect::<Vec<_>>().join("; "),
            fix: None,
            actions: vec![act("Sort load order", &["sort", "--apply"], "primary")],
        });
    }
    for c in &sp.cycles {
        let rules: Vec<Action> = st
            .rules
            .iter()
            .filter(|r| c.contains(&r.r#mod) && c.contains(&r.after))
            .map(|r| {
                act(
                    format!("Remove rule: {} after {}", name(&r.r#mod), name(&r.after)),
                    &["rule", &r.r#mod, "--after", &r.after, "--remove"],
                    "secondary",
                )
            })
            .collect();
        f.push(Finding {
            severity: "warning",
            title: "Circular load order constraints".into(),
            detail: c.join(" → "),
            fix: None,
            actions: rules,
        });
    }
    let order = |s: &str| match s {
        "crash" => 0,
        "warning" => 1,
        "perf" => 2,
        _ => 3,
    };
    f.sort_by_key(|x| order(x.severity));
    for x in f.iter_mut() {
        if st.acked.contains(&x.title) {
            x.severity = "acked";
        }
    }
    f
}
