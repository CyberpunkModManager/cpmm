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
}
