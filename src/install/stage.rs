//! Laying out an extracted tree as a staged mod: extract, find the game root, canonicalise case,
//! move the files into `mods/<id>/files`. Pure planning plus the filesystem move; the
//! pipeline around it (sources, requirements, recording) is in `install/mod.rs`.

use crate::files::{self, CaseIndex};
use crate::state::{Paths, State};
use anyhow::{Context, Result, bail};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const KNOWN_TOPS: &[&str] = &["archive", "bin", "r6", "red4ext", "engine", "mods", "tools"];

fn ext(p: &str) -> String {
    Path::new(p).extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}

/// Documentation and images that ship next to the game folders and should not be installed.
fn is_junk(rel: &str) -> bool {
    let name = rel.rsplit('/').next().unwrap_or(rel).to_lowercase();
    matches!(ext(rel).as_str(), "txt" | "md" | "pdf" | "png" | "jpg" | "jpeg" | "webp" | "gif" | "url" | "html" | "htm" | "rtf" | "docx")
        || name.starts_with("readme")
        || name.starts_with("license")
        || name.starts_with("changelog")
        || name == ".ds_store"
        || name == "thumbs.db"
}

pub fn extract(archive: &Path, dest: &Path) -> Result<()> {
    fs::create_dir_all(dest)?;
    let out = Command::new("7z")
        .arg("x")
        .arg("-y")
        .arg("-bd")
        .arg(format!("-o{}", dest.display()))
        .arg(archive)
        .output()
        .context("running 7z (install p7zip/7zip)")?;
    if !out.status.success() {
        bail!("7z failed on {}: {}", archive.display(), String::from_utf8_lossy(&out.stderr));
    }
    Ok(())
}

#[derive(Debug)]
pub struct Plan {
    /// (path inside extracted tree, path relative to game root)
    pub moves: Vec<(String, String)>,
    pub skipped: Vec<String>,
}

/// Work out where each extracted file goes. `root` strips a prefix, `dest` prepends one.
pub fn plan(extracted: &[String], slug: &str, root: Option<&str>, dest: Option<&str>) -> Result<Plan> {
    let mut skipped = vec![];
    if root.is_none()
        && dest.is_none()
        && extracted.iter().any(|p| p.to_lowercase().starts_with("fomod/") || p.to_lowercase().contains("/fomod/"))
    {
        let tops: BTreeSet<&str> = extracted.iter().filter_map(|p| p.split('/').next()).collect();
        bail!(
            "this is a FOMOD installer; pick the option folder with --root <dir> (top-level entries: {})",
            tops.into_iter().collect::<Vec<_>>().join(", ")
        );
    }
    if root.is_some() || dest.is_some() {
        let r = root.unwrap_or("").trim_matches('/');
        let d = dest.unwrap_or("").trim_matches('/');
        let mut moves = vec![];
        for p in extracted {
            let rest = if r.is_empty() { Some(p.as_str()) } else { p.strip_prefix(r).and_then(|x| x.strip_prefix('/')) };
            match rest {
                Some(x) if !(d.is_empty() && is_junk(x) && !x.contains('/')) => {
                    moves.push((p.clone(), if d.is_empty() { x.to_string() } else { format!("{d}/{x}") }))
                }
                _ => skipped.push(p.clone()),
            }
        }
        if moves.is_empty() {
            bail!("--root {r:?} matched no files");
        }
        return Ok(Plan { moves, skipped });
    }

    // 1. Files under a known game folder, possibly behind one or more wrapper folders.
    let mut prefixes = BTreeSet::new();
    let mut unknown = vec![];
    for p in extracted {
        let comps: Vec<&str> = p.split('/').collect();
        match comps.iter().position(|c| KNOWN_TOPS.contains(&c.to_lowercase().as_str())) {
            Some(i) if i + 1 < comps.len() => {
                prefixes.insert(comps[..i].join("/"));
            }
            _ => {
                if !is_junk(p) {
                    unknown.push(p.clone())
                }
            }
        }
    }
    if !prefixes.is_empty() {
        if prefixes.len() > 1 {
            bail!("several possible game roots in archive: {:?}; choose one with --root", prefixes);
        }
        let pre = prefixes.into_iter().next().unwrap();
        let stray: Vec<&String> = unknown.iter().filter(|u| pre.is_empty() || u.starts_with(&format!("{pre}/"))).collect();
        if !stray.is_empty() {
            bail!("files outside known game folders: {:?}; use --root/--dest to place them", &stray[..stray.len().min(8)]);
        }
        let mut moves = vec![];
        for p in extracted {
            let rest = if pre.is_empty() { Some(p.as_str()) } else { p.strip_prefix(&format!("{pre}/")) };
            match rest {
                Some(r) if KNOWN_TOPS.contains(&r.split('/').next().unwrap_or("").to_lowercase().as_str()) => {
                    moves.push((p.clone(), r.to_string()))
                }
                _ => skipped.push(p.clone()),
            }
        }
        return Ok(Plan { moves, skipped });
    }

    // 2. Loose files: infer from extensions.
    let real: Vec<&String> = extracted.iter().filter(|p| !is_junk(p)).collect();
    skipped.extend(extracted.iter().filter(|p| is_junk(p)).cloned());
    if real.is_empty() {
        bail!("archive contains no installable files");
    }
    let exts: BTreeSet<String> = real.iter().map(|p| ext(p)).collect();
    let all_in = |set: &[&str]| exts.iter().all(|e| set.contains(&e.as_str()));
    let base = |p: &str| p.rsplit('/').next().unwrap_or(p).to_string();
    let strip_single_top = |p: &str| -> String {
        let tops: BTreeSet<&str> = real.iter().map(|x| x.split('/').next().unwrap_or("")).collect();
        if tops.len() == 1 && real.iter().all(|x| x.contains('/')) {
            p.split_once('/').map(|x| x.1.to_string()).unwrap_or(p.into())
        } else {
            p.into()
        }
    };
    let moves: Vec<(String, String)> = if all_in(&["archive", "xl"]) {
        real.iter().map(|p| ((*p).clone(), format!("archive/pc/mod/{}", base(p)))).collect()
    } else if all_in(&["reds"]) {
        real.iter().map(|p| ((*p).clone(), format!("r6/scripts/{slug}/{}", strip_single_top(p)))).collect()
    } else if all_in(&["yaml", "yml", "tweak"]) {
        real.iter().map(|p| ((*p).clone(), format!("r6/tweaks/{slug}/{}", strip_single_top(p)))).collect()
    } else if real.iter().any(|p| base(p) == "init.lua") {
        // CET mod folder(s): <name>/init.lua
        let mut v = vec![];
        for p in &real {
            let rel = if real.iter().all(|x| x.contains('/')) { (*p).clone() } else { format!("{slug}/{p}") };
            v.push(((*p).clone(), format!("bin/x64/plugins/cyber_engine_tweaks/mods/{rel}")));
        }
        v
    } else {
        bail!("can't tell where these files go (extensions {:?}); use --root/--dest", exts);
    };
    Ok(Plan { moves, skipped })
}

/// Build `mods/<id>/files` from an extracted tree; replaces any existing staged files
/// (old ones go to trash, or to history for an installed mod). Returns the installed relative
/// paths, and whether the installed version was kept in history.
/// `batch` tags the replaced version's history entry (change::Edit::batch).
pub fn stage(
    paths: &Paths,
    state: &State,
    id: &str,
    extracted_root: &Path,
    plan: &Plan,
    batch: Option<&str>,
) -> Result<(Vec<String>, bool)> {
    let mut kept = false;
    if paths.mods().join(id).exists() && crate::vfs::running(paths).is_some() {
        bail!("{id} is mounted by the running game session; quit the game before replacing its files");
    }
    let mut ci = CaseIndex::default();
    ci.add_game_dirs(&paths.game);
    for m in &state.mods {
        if m.id != id {
            ci.add_tree(&paths.mod_files(&m.id));
        }
    }
    let new_root = paths.tmp().join(format!("{id}.new"));
    if new_root.exists() {
        fs::remove_dir_all(&new_root)?;
    }
    let mut seen = BTreeSet::new();
    let mut out = vec![];
    for (src, dst) in &plan.moves {
        let canon = ci.canonical(dst);
        if !seen.insert(canon.to_lowercase()) {
            bail!("archive maps two files to {canon} (case-insensitive)");
        }
        files::move_file(&extracted_root.join(src), &new_root.join("files").join(&canon))?;
        out.push(canon);
    }
    let final_dir = paths.mods().join(id);
    if final_dir.exists() {
        match state.find(id) {
            Some(i) => {
                let reason = format!("replaced by an install/update (was {})", state.mods[i].version);
                crate::safety::archive_current(paths, &state.mods[i], &reason, batch)?;
                kept = true;
            }
            None => {
                let t = paths.trash_for(id);
                fs::rename(&final_dir, &t)?;
            }
        }
    }
    fs::rename(&new_root, &final_dir)?;
    out.sort();
    Ok((out, kept))
}

pub fn tmp_extract_dir(paths: &Paths, id: &str) -> PathBuf {
    paths.tmp().join(format!("{id}.x"))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn p(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }
    fn dests(pl: &Plan) -> Vec<&str> {
        pl.moves.iter().map(|(_, d)| d.as_str()).collect()
    }

    #[test]
    fn strips_wrapper_folder_and_skips_docs() {
        let pl = plan(&p(&["Wrap/r6/scripts/X/a.reds", "Wrap/archive/pc/mod/x.archive", "Wrap/README.txt"]), "x", None, None).unwrap();
        assert_eq!(dests(&pl), ["r6/scripts/X/a.reds", "archive/pc/mod/x.archive"]);
        assert_eq!(pl.skipped, ["Wrap/README.txt"]);
    }

    #[test]
    fn loose_files_by_extension() {
        assert_eq!(
            dests(&plan(&p(&["x.archive", "x.archive.xl"]), "m", None, None).unwrap()),
            ["archive/pc/mod/x.archive", "archive/pc/mod/x.archive.xl"]
        );
        assert_eq!(dests(&plan(&p(&["Foo/a.reds"]), "m", None, None).unwrap()), ["r6/scripts/m/a.reds"]);
        assert_eq!(
            dests(&plan(&p(&["MyMod/init.lua"]), "m", None, None).unwrap()),
            ["bin/x64/plugins/cyber_engine_tweaks/mods/MyMod/init.lua"]
        );
    }

    #[test]
    fn refuses_ambiguous_and_fomod() {
        assert!(plan(&p(&["A/r6/scripts/a.reds", "B/r6/scripts/b.reds"]), "m", None, None).is_err());
        assert!(plan(&p(&["fomod/ModuleConfig.xml", "Opt1/r6/scripts/a.reds"]), "m", None, None).is_err());
        let pl = plan(&p(&["fomod/ModuleConfig.xml", "Opt1/r6/scripts/a.reds"]), "m", Some("Opt1"), None).unwrap();
        assert_eq!(dests(&pl), ["r6/scripts/a.reds"]);
    }

    #[test]
    fn case_index_reuses_existing_casing() {
        let mut ci = CaseIndex::default();
        assert_eq!(ci.canonical("archive/pc/mod/a.archive"), "archive/pc/mod/a.archive");
        assert_eq!(ci.canonical("Archive/PC/Mod/B.archive"), "archive/pc/mod/B.archive");
    }
}
