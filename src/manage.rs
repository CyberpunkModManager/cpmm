//! Conflict management (hiding files, preferring a mod per file), overwrite management,
//! and nxm:// link handling.

use crate::conflicts;
use crate::files;
use crate::state::{Mod, Paths, State, now, slugify};
use anyhow::{Context, Result, bail};
use std::fs;
use std::os::unix::fs::FileTypeExt;
use std::path::Path;

fn matches(rel: &str, sel: &[String]) -> bool {
    let l = rel.to_lowercase();
    sel.is_empty()
        || sel.iter().any(|s| {
            let s = s.trim_matches('/').to_lowercase();
            l == s || l.starts_with(&format!("{s}/"))
        })
}

/// Hide files of a mod (they stay on disk under mods/<id>/hidden/ and are excluded from the VFS).
pub fn hide(paths: &Paths, st: &mut State, i: usize, sel: &[String]) -> Result<Vec<String>> {
    if sel.is_empty() {
        bail!("name the files or folders to hide");
    }
    let id = st.mods[i].id.clone();
    let root = paths.mod_files(&id);
    let hidden_root = paths.mods().join(&id).join("hidden");
    let mut done = vec![];
    for rel in files::list_tree(&root).into_iter().filter(|r| matches(r, sel)) {
        files::move_file(&root.join(&rel), &hidden_root.join(&rel))?;
        files::prune_empty_dirs(root.join(&rel).parent().unwrap(), &root);
        let lr = rel.to_lowercase();
        if !st.mods[i].hidden.contains(&lr) {
            st.mods[i].hidden.push(lr);
        }
        done.push(rel);
    }
    Ok(done)
}

pub fn unhide(paths: &Paths, st: &mut State, i: usize, sel: &[String]) -> Result<Vec<String>> {
    let id = st.mods[i].id.clone();
    let root = paths.mod_files(&id);
    let hidden_root = paths.mods().join(&id).join("hidden");
    let mut done = vec![];
    for rel in files::list_tree(&hidden_root).into_iter().filter(|r| matches(r, sel)) {
        let dst = root.join(&rel);
        if dst.exists() {
            fs::remove_file(&dst)?; // a newer install brought it back already
        }
        files::move_file(&hidden_root.join(&rel), &dst)?;
        files::prune_empty_dirs(hidden_root.join(&rel).parent().unwrap(), &hidden_root);
        done.push(rel);
    }
    let gone: Vec<String> = done.iter().map(|d| d.to_lowercase()).collect();
    st.mods[i].hidden.retain(|h| !gone.contains(h) && !matches(h, sel));
    Ok(done)
}

/// After (re)installing a mod, re-hide the files the user hid before.
pub fn reapply_hidden(paths: &Paths, st: &mut State, id: &str) -> Result<usize> {
    let Some(i) = st.find(id) else { return Ok(0) };
    let want = st.mods[i].hidden.clone();
    if want.is_empty() {
        return Ok(0);
    }
    let root = paths.mod_files(id);
    let present: Vec<String> = files::list_tree(&root).into_iter().filter(|r| want.contains(&r.to_lowercase())).collect();
    let n = present.len();
    if n > 0 {
        let hidden_root = paths.mods().join(id).join("hidden");
        for rel in present {
            let dst = hidden_root.join(&rel);
            if dst.exists() {
                fs::remove_file(&dst)?;
            }
            files::move_file(&root.join(&rel), &dst)?;
        }
    }
    Ok(n)
}

/// Make mod `i` win the given conflicting paths (default: all its conflicts) by hiding
/// those files in every other enabled mod, regardless of priority.
pub fn prefer(paths: &Paths, st: &mut State, i: usize, sel: &[String]) -> Result<Vec<(String, String)>> {
    let id = st.mods[i].id.clone();
    let own = conflicts::ownership(paths, &st.mods, true);
    let mut todo: Vec<(String, String)> = vec![];
    for (_, (rel, owners)) in own {
        if owners.len() > 1 && owners.contains(&id) && matches(&rel, sel) {
            for o in owners.iter().filter(|o| **o != id) {
                todo.push((o.clone(), rel.clone()));
            }
        }
    }
    for (o, rel) in &todo {
        let j = st.get(o)?;
        // hide by exact path (the other mod's own casing is the same: install canonicalises)
        hide(paths, st, j, std::slice::from_ref(rel))?;
    }
    Ok(todo)
}

pub struct OwEntry {
    pub rel: String,
    pub whiteout: bool,
}

pub fn overwrite_entries(paths: &Paths) -> Vec<OwEntry> {
    let up = paths.upper();
    walkdir::WalkDir::new(&up)
        .into_iter()
        .flatten()
        .filter(|e| !e.file_type().is_dir())
        .filter_map(|e| {
            let rel = e.path().strip_prefix(&up).ok()?.to_string_lossy().to_string();
            Some(OwEntry { whiteout: e.file_type().is_char_device(), rel })
        })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, ts_rs::TS)]
pub enum OwKind {
    /// a deletion marker (whiteout)
    #[serde(rename = "deleted")]
    Deleted,
    /// shadows a file an enabled mod ships
    #[serde(rename = "edited mod file")]
    EditedMod,
    /// shadows a file of the real game dir
    #[serde(rename = "edited base file")]
    EditedBase,
    #[serde(rename = "new")]
    New,
}
impl OwKind {
    pub fn as_str(self) -> &'static str {
        match self {
            OwKind::Deleted => "deleted",
            OwKind::EditedMod => "edited mod file",
            OwKind::EditedBase => "edited base file",
            OwKind::New => "new",
        }
    }
}

pub struct OwFile {
    pub rel: String,
    pub kind: OwKind,
    /// the enabled mod whose file this shadows (the one that wins in the overlay)
    pub shadows: Option<String>,
}

/// What each entry in overwrite/ is relative to the enabled mods and the game dir.
pub fn overwrite_files(paths: &Paths, st: &State) -> Vec<OwFile> {
    let own = conflicts::ownership(paths, &st.mods, true);
    overwrite_entries(paths)
        .into_iter()
        .map(|e| {
            let shadows = own.get(&e.rel.to_lowercase()).and_then(|(_, o)| o.last().cloned());
            let kind = if e.whiteout {
                OwKind::Deleted
            } else if shadows.is_some() {
                OwKind::EditedMod
            } else if paths.game.join(&e.rel).exists() {
                OwKind::EditedBase
            } else {
                OwKind::New
            };
            OwFile { rel: e.rel, kind, shadows }
        })
        .collect()
}

/// Move runtime-written files from overwrite/ into a mod (created as a manual mod if new).
/// Runs inside a change that needs the game idle.
pub fn overwrite_move(paths: &Paths, st: &mut State, to: &str, sel: &[String]) -> Result<(String, usize, usize)> {
    let id = match st.find(to) {
        Some(i) => st.mods[i].id.clone(),
        None => {
            let id = st.unique_id(&slugify(to));
            st.mods.push(Mod {
                id: id.clone(),
                name: to.to_string(),
                enabled: true,
                version: String::new(),
                nexus: None,
                requires: vec![],
                reqs_checked: None,
                hidden: vec![],
                ignored_reqs: vec![],
                fomod: None,
                archive: None,
                installed_at: now(),
                notes: "created from overwrite".into(),
            });
            id
        }
    };
    let up = paths.upper();
    let dst_root = paths.mod_files(&id);
    let (mut moved, mut skipped) = (0, 0);
    for e in overwrite_entries(paths).into_iter().filter(|e| matches(&e.rel, sel)) {
        if e.whiteout {
            skipped += 1; // a deletion can't live in a mod; leave it (or `overwrite clear` it)
            continue;
        }
        let dst = dst_root.join(&e.rel);
        if dst.exists() {
            fs::remove_file(&dst)?;
        }
        files::move_file(&up.join(&e.rel), &dst)?;
        files::prune_empty_dirs(up.join(&e.rel).parent().unwrap(), &up);
        moved += 1;
    }
    Ok((id, moved, skipped))
}

/// Move overwrite entries (including deletion markers) to trash.
pub fn overwrite_clear(paths: &Paths, sel: &[String]) -> Result<(usize, std::path::PathBuf)> {
    crate::change::Idle::Required.check(paths)?;
    let up = paths.upper();
    let t = paths.trash_for("overwrite");
    let mut n = 0;
    for e in overwrite_entries(paths).into_iter().filter(|e| matches(&e.rel, sel)) {
        let src = up.join(&e.rel);
        let dst = t.join(&e.rel);
        fs::create_dir_all(dst.parent().unwrap())?;
        fs::rename(&src, &dst).with_context(|| format!("moving {}", src.display()))?;
        files::prune_empty_dirs(src.parent().unwrap(), &up);
        n += 1;
    }
    Ok((n, t))
}

/// Copy every enabled mod's winning files, and overwrite/'s files, into the real game dir
/// (`apply`; else only count them). Returns (mod files, overwrite entries).
pub fn eject(paths: &Paths, st: &State, apply: bool) -> Result<(usize, usize)> {
    crate::change::Idle::NoSession("game session running; quit first").check(paths)?;
    let own = conflicts::ownership(paths, &st.mods, true);
    let mut n = 0;
    for (rel, owners) in own.values() {
        let winner = owners.last().unwrap();
        if apply {
            let dst = paths.game.join(rel);
            if dst.exists() {
                fs::remove_file(&dst)?;
            }
            files::reflink_or_copy(&paths.mod_files(winner).join(rel), &dst)?;
        }
        n += 1;
    }
    let up = files::list_tree(&paths.upper());
    if apply {
        for rel in &up {
            let src = paths.upper().join(rel);
            if fs::symlink_metadata(&src).map(|m| m.file_type().is_file()).unwrap_or(false) {
                let dst = paths.game.join(rel);
                if dst.exists() {
                    fs::remove_file(&dst)?;
                }
                files::reflink_or_copy(&src, &dst)?;
            }
        }
    }
    Ok((n, up.len()))
}

/// Empty trash/, tmp/ and the downloads folder (or, `dry_run`, only measure them). Returns
/// the bytes they held.
pub fn clean(paths: &Paths, dry_run: bool) -> Result<u64> {
    let mut total = 0u64;
    for d in [paths.trash(), paths.tmp(), paths.downloads()] {
        let size: u64 = walkdir::WalkDir::new(&d)
            .into_iter()
            .flatten()
            .filter_map(|e| e.metadata().ok())
            .filter(|m| m.is_file())
            .map(|m| m.len())
            .sum();
        total += size;
        if !dry_run && d.exists() {
            fs::remove_dir_all(&d)?;
            fs::create_dir_all(&d)?;
        }
    }
    Ok(total)
}

#[derive(Debug, PartialEq)]
pub struct NxmLink {
    pub mod_id: u64,
    pub file_gsid: String,
    pub key: Option<String>,
    pub expires: Option<String>,
}

/// nxm://cyberpunk2077/mods/<mod>/files/<file>?key=…&expires=…&user_id=…
pub fn parse_nxm(url: &str) -> Result<NxmLink> {
    let rest = url.strip_prefix("nxm://").context("not an nxm:// link")?;
    let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
    let parts: Vec<&str> = path.trim_matches('/').split('/').collect();
    if parts.first().map(|g| g.to_lowercase()) != Some("cyberpunk2077".into()) {
        bail!("link is for game {:?}, not cyberpunk2077", parts.first().unwrap_or(&""));
    }
    if parts.get(1) == Some(&"collections") {
        bail!("collection links aren't supported");
    }
    if parts.len() < 5 || parts[1] != "mods" || parts[3] != "files" {
        bail!("unrecognised nxm link layout: {path}");
    }
    let mod_id = parts[2].parse().context("mod id")?;
    let file_gsid = parts[4].to_string();
    let mut key = None;
    let mut expires = None;
    for kv in query.split('&') {
        match kv.split_once('=') {
            Some(("key", v)) => key = Some(v.to_string()),
            Some(("expires", v)) => expires = Some(v.to_string()),
            _ => {}
        }
    }
    Ok(NxmLink { mod_id, file_gsid, key, expires })
}

/// Register cpmm as the desktop handler for nxm:// links (the site's "Mod Manager Download").
pub fn register_nxm(exe: &Path) -> Result<std::path::PathBuf> {
    let apps = std::path::PathBuf::from(std::env::var("HOME")?).join(".local/share/applications");
    fs::create_dir_all(&apps)?;
    let desk = apps.join("cpmm-nxm.desktop");
    fs::write(
        &desk,
        format!(
            "[Desktop Entry]\nType=Application\nName=cpmm (Nexus Mods links)\nComment=Install Cyberpunk 2077 mods from Nexus \"Mod Manager Download\" buttons\nExec={} nxm %u\nTerminal=false\nNoDisplay=true\nMimeType=x-scheme-handler/nxm;\n",
            exe.display()
        ),
    )?;
    let _ = std::process::Command::new("update-desktop-database").arg(&apps).status();
    let st = std::process::Command::new("xdg-mime")
        .args(["default", "cpmm-nxm.desktop", "x-scheme-handler/nxm"])
        .status()
        .context("running xdg-mime")?;
    if !st.success() {
        bail!("xdg-mime failed");
    }
    Ok(desk)
}

/// Desktop notification, fire-and-forget: in Steam's Game Mode (gamescope) there is no
/// notification daemon and notify-send would block for ~25 s waiting for one.
pub fn notify(summary: &str, body: &str) {
    use std::process::{Command, Stdio};
    let _ = Command::new("notify-send")
        .args(["-a", "cpmm", summary, body])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

pub fn current_nxm_handler() -> String {
    std::process::Command::new("xdg-mime")
        .args(["query", "default", "x-scheme-handler/nxm"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

/// Remove our desktop entry and its mimeapps.list association.
pub fn unregister_nxm() -> Result<()> {
    let home = std::path::PathBuf::from(std::env::var("HOME")?);
    let desk = home.join(".local/share/applications/cpmm-nxm.desktop");
    if desk.exists() {
        fs::remove_file(&desk)?;
    }
    let mimeapps = home.join(".config/mimeapps.list");
    if let Ok(s) = fs::read_to_string(&mimeapps) {
        let kept: Vec<&str> = s.lines().filter(|l| !(l.starts_with("x-scheme-handler/nxm=") && l.contains("cpmm-nxm.desktop"))).collect();
        fs::write(&mimeapps, kept.join("\n") + "\n")?;
    }
    let _ = std::process::Command::new("update-desktop-database").arg(home.join(".local/share/applications")).status();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nxm_links() {
        let l = parse_nxm("nxm://cyberpunk2077/mods/107/files/168615?key=abc&expires=1790000000&user_id=1").unwrap();
        assert_eq!(l, NxmLink { mod_id: 107, file_gsid: "168615".into(), key: Some("abc".into()), expires: Some("1790000000".into()) });
        assert!(parse_nxm("nxm://skyrimspecialedition/mods/1/files/2").is_err());
        assert!(parse_nxm("nxm://cyberpunk2077/collections/abc/revisions/3").is_err());
    }

    #[test]
    fn selection_matches_folders_and_files() {
        assert!(matches("r6/scripts/A/a.reds", &["r6/scripts/a".into()]));
        assert!(matches("r6/scripts/A/a.reds", &[]));
        assert!(!matches("r6/scripts/AB/a.reds", &["r6/scripts/a".into()]));
    }

    /// Each overwrite/ entry against the winning enabled mod and the game dir.
    #[test]
    fn overwrite_classification() {
        let d = std::env::temp_dir().join(format!("cpmm-manage-test-ow-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        let paths = Paths { data: d.clone(), game: d.join("game"), cfg: Default::default() };
        let st: State = serde_json::from_value(serde_json::json!({"version": 1, "mods": [
            {"id": "low", "name": "Low", "enabled": true}, {"id": "high", "name": "High", "enabled": true},
            {"id": "off", "name": "Off", "enabled": false}]}))
        .unwrap();
        let put = |p: std::path::PathBuf| {
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, "x").unwrap();
        };
        for rel in ["r6/Shared.json", "r6/off.json", "r6/base.txt", "r6/new.txt"] {
            put(paths.upper().join(rel));
        }
        put(paths.mod_files("low").join("r6/shared.json")); // matched case-insensitively
        put(paths.mod_files("high").join("r6/shared.json"));
        put(paths.mod_files("off").join("r6/off.json")); // a disabled mod's file is not shadowed
        put(paths.game.join("r6/base.txt"));
        let mut got: Vec<(String, &str, Option<String>)> =
            overwrite_files(&paths, &st).into_iter().map(|e| (e.rel, e.kind.as_str(), e.shadows)).collect();
        got.sort();
        let want: Vec<(String, &str, Option<String>)> = vec![
            ("r6/Shared.json".into(), "edited mod file", Some("high".into())),
            ("r6/base.txt".into(), "edited base file", None),
            ("r6/new.txt".into(), "new", None),
            ("r6/off.json".into(), "new", None),
        ];
        assert_eq!(got, want);
        let _ = fs::remove_dir_all(&d);
    }
}
