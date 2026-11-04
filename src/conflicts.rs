//! File-level overwrite analysis and redscript annotation conflicts.

use crate::files;
use crate::state::{Mod, Paths};
use regex::Regex;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;

pub struct FileConflict {
    pub path: String,
    /// Mod ids in priority order, lowest first; the last one wins.
    pub mods: Vec<String>,
    pub identical: bool,
    pub overrides_base: bool,
}

/// lower-cased relative path -> (display path, owners in priority order)
pub fn ownership(paths: &Paths, mods: &[Mod], only_enabled: bool) -> BTreeMap<String, (String, Vec<String>)> {
    let mut map: BTreeMap<String, (String, Vec<String>)> = BTreeMap::new();
    for m in mods.iter().filter(|m| m.enabled || !only_enabled) {
        for f in files::list_tree(&paths.mod_files(&m.id)) {
            let e = map.entry(f.to_lowercase()).or_insert_with(|| (f.clone(), vec![]));
            e.1.push(m.id.clone());
        }
    }
    map
}

pub fn file_conflicts(paths: &Paths, mods: &[Mod], hash_check: bool) -> Vec<FileConflict> {
    let mut out = vec![];
    for (_, (path, owners)) in ownership(paths, mods, true) {
        let base = paths.game.join(&path);
        let overrides_base = base.is_file();
        if owners.len() < 2 && !overrides_base {
            continue;
        }
        let identical = owners.len() > 1
            && hash_check
            && {
                let hs: BTreeSet<String> = owners.iter().filter_map(|o| files::sha256_file(&paths.mod_files(o).join(&path)).ok()).collect();
                hs.len() == 1
            };
        out.push(FileConflict { path, mods: owners, identical, overrides_base });
    }
    out
}

/// `import X` statements (in enabled mods' scripts) that no installed module provides. One of
/// these fails the entire redscript compile, which disables every script mod.
pub fn unresolved_imports(paths: &Paths, mods: &[Mod]) -> Vec<(String, String, String)> {
    let comments = Regex::new(r"(?s)//[^\n]*|/\*.*?\*/").unwrap();
    let module_re = Regex::new(r"(?m)^\s*module\s+([\w.]+)").unwrap();
    let import_re = Regex::new(r#"(?m)^\s*(?:@if\(\s*(!?)\s*ModuleExists\(\s*"([\w.]+)"\s*\)\s*\)\s*)?import\s+([\w.]+)(\.\*)?"#).unwrap();
    let mut srcs: Vec<(String, String, String)> = vec![];
    for (_, (rel, owners)) in ownership(paths, mods, true) {
        if rel.to_lowercase().ends_with(".reds") {
            let w = owners.last().unwrap();
            if let Ok(s) = fs::read_to_string(paths.mod_files(w).join(&rel)) {
                srcs.push((w.clone(), rel, comments.replace_all(&s, "").into_owned()));
            }
        }
    }
    // unmanaged scripts in the real game dir count as providers too
    for f in files::list_tree(&paths.game.join("r6/scripts")).into_iter().filter(|f| f.to_lowercase().ends_with(".reds")) {
        if let Ok(s) = fs::read_to_string(paths.game.join("r6/scripts").join(&f)) {
            srcs.push(("(unmanaged)".into(), format!("r6/scripts/{f}"), comments.replace_all(&s, "").into_owned()));
        }
    }
    let modules: BTreeSet<String> = srcs.iter().flat_map(|(_, _, s)| module_re.captures_iter(s).map(|c| c[1].to_string()).collect::<Vec<_>>()).collect();
    let provided = |m: &str| modules.iter().any(|x| x == m || x.starts_with(&format!("{m}.")) || m.starts_with(&format!("{x}.")));
    let mut out = vec![];
    let mut seen = BTreeSet::new();
    for (owner, rel, s) in &srcs {
        for c in import_re.captures_iter(s) {
            if let Some(g) = c.get(2) {
                if provided(g.as_str()) == c.get(1).is_some_and(|n| n.as_str() == "!") {
                    continue; // guarded import that isn't compiled
                }
            }
            let imp = c[3].trim_end_matches('.').to_string();
            if !provided(&imp) && seen.insert((owner.clone(), imp.clone())) {
                out.push((owner.clone(), imp, rel.clone()));
            }
        }
    }
    out
}

pub struct ScriptConflict {
    pub kind: String,
    pub target: String,
    pub mods: Vec<String>,
}

/// Port of redscript_conflicts.py: two mods @replaceMethod-ing the same method means only
/// one wins (the compiler errors out or silently picks one depending on version).
pub fn redscript_conflicts(paths: &Paths, mods: &[Mod], unmanaged: &[(String, String)]) -> Vec<ScriptConflict> {
    // Optional `@if(!?ModuleExists("X"))` guard in front of the annotation: authors use it to
    // step aside when a known-conflicting mod is installed, so it must be evaluated.
    let ann = Regex::new(
        r#"(?:@if\(\s*(!?)\s*ModuleExists\(\s*"([\w.]+)"\s*\)\s*\)\s*)?@(replaceMethod|wrapMethod|replaceGlobal|wrapGlobal|addMethod|addField)\(\s*([\w.]*)\s*\)\s*(?:(?:public|protected|private|final|static|native|cb|exec|quest|const|persistent|let)\s+)*(?:func\s+)?(\w+)"#,
    )
    .unwrap();
    let comments = Regex::new(r"(?s)//[^\n]*|/\*.*?\*/").unwrap();
    let module_re = Regex::new(r"(?m)^\s*module\s+([\w.]+)").unwrap();
    let mut sources: Vec<(String, String)> = vec![];
    let mut hooks: BTreeMap<(String, String, String), BTreeSet<String>> = BTreeMap::new();
    let mut scan = |owner: &str, src: &str| sources.push((owner.to_string(), comments.replace_all(src, "").into_owned()));
    // Only the copy that wins in the overlay gets compiled.
    for (_, (rel, owners)) in ownership(paths, mods, true) {
        if !rel.to_lowercase().ends_with(".reds") {
            continue;
        }
        let w = owners.last().unwrap();
        if let Ok(s) = fs::read_to_string(paths.mod_files(w).join(&rel)) {
            scan(w, &s);
        }
    }
    for (owner, rel) in unmanaged {
        if let Ok(s) = fs::read_to_string(paths.game.join(rel)) {
            scan(&format!("(unmanaged) {owner}"), &s);
        }
    }
    let modules: BTreeSet<String> = sources.iter().flat_map(|(_, s)| module_re.captures_iter(s).map(|c| c[1].to_string()).collect::<Vec<_>>()).collect();
    for (owner, src) in &sources {
        for c in ann.captures_iter(src) {
            if let Some(m) = c.get(2) {
                let present = modules.contains(m.as_str()) || modules.iter().any(|x| x.starts_with(&format!("{}.", m.as_str())));
                let negated = c.get(1).is_some_and(|n| n.as_str() == "!");
                if present == negated {
                    continue; // guard is false: this code isn't compiled
                }
            }
            hooks.entry((c[4].to_string(), c[5].to_string(), c[3].to_string())).or_default().insert(owner.clone());
        }
    }
    let mut out = vec![];
    for ((cls, name, kind), ms) in &hooks {
        let target = if cls.is_empty() { name.clone() } else { format!("{cls}.{name}") };
        if ms.len() > 1 && matches!(kind.as_str(), "replaceMethod" | "replaceGlobal") {
            out.push(ScriptConflict { kind: "REPLACE CONFLICT".into(), target: target.clone(), mods: ms.iter().cloned().collect() });
        }
        if ms.len() > 1 && matches!(kind.as_str(), "addField" | "addMethod") {
            out.push(ScriptConflict { kind: format!("duplicate @{kind}"), target: target.clone(), mods: ms.iter().cloned().collect() });
        }
        if kind == "replaceMethod" {
            if let Some(w) = hooks.get(&(cls.clone(), name.clone(), "wrapMethod".into())) {
                let w: Vec<String> = w.difference(ms).cloned().collect();
                if !w.is_empty() {
                    let mut all: Vec<String> = ms.iter().map(|m| format!("replace:{m}")).collect();
                    all.extend(w.into_iter().map(|m| format!("wrap:{m}")));
                    out.push(ScriptConflict { kind: "replace+wrap (usually fine)".into(), target, mods: all });
                }
            }
        }
    }
    out
}
