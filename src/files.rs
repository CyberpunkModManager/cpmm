//! File-tree helpers: listing staged mods, hashing, reflinks, case canonicalisation.

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::os::fd::AsRawFd;
use std::path::Path;
use walkdir::WalkDir;

/// Relative, '/'-separated paths of all regular files (and symlinks) under `root`, sorted.
pub fn list_tree(root: &Path) -> Vec<String> {
    if !root.exists() {
        return vec![];
    }
    let mut v: Vec<String> = WalkDir::new(root)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| !e.file_type().is_dir())
        .filter_map(|e| e.path().strip_prefix(root).ok().map(|p| p.to_string_lossy().replace('\\', "/")))
        .collect();
    v.sort();
    v
}

pub fn sha256_file(p: &Path) -> Result<String> {
    let mut f = fs::File::open(p).with_context(|| format!("open {}", p.display()))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex::encode(h.finalize()))
}

/// Copy `src` to `dst` sharing extents (btrfs/xfs reflink). Falls back to a plain copy.
pub fn reflink_or_copy(src: &Path, dst: &Path) -> Result<()> {
    if let Some(p) = dst.parent() {
        fs::create_dir_all(p)?;
    }
    let s = fs::File::open(src)?;
    let d = fs::File::create(dst)?;
    // FICLONE = _IOW(0x94, 9, int)
    const FICLONE: libc::c_ulong = 0x40049409;
    let r = unsafe { libc::ioctl(d.as_raw_fd(), FICLONE, s.as_raw_fd()) };
    if r != 0 {
        drop(d);
        fs::copy(src, dst).with_context(|| format!("copy {} -> {}", src.display(), dst.display()))?;
    } else {
        let perm = s.metadata()?.permissions();
        let _ = fs::set_permissions(dst, perm);
    }
    Ok(())
}

/// Move a file, creating parent dirs. Uses rename (same filesystem) and falls back to copy+delete.
pub fn move_file(src: &Path, dst: &Path) -> Result<()> {
    if let Some(p) = dst.parent() {
        fs::create_dir_all(p)?;
    }
    if dst.exists() {
        bail!("refusing to overwrite {}", dst.display());
    }
    match fs::rename(src, dst) {
        Ok(()) => Ok(()),
        Err(e) if e.raw_os_error() == Some(libc::EXDEV) => {
            reflink_or_copy(src, dst)?;
            fs::remove_file(src)?;
            Ok(())
        }
        Err(e) => Err(e).with_context(|| format!("rename {} -> {}", src.display(), dst.display())),
    }
}

/// Remove now-empty directories from `start` upwards, stopping at `stop`.
pub fn prune_empty_dirs(start: &Path, stop: &Path) {
    let mut cur = start.to_path_buf();
    while cur.starts_with(stop) && cur != stop {
        if fs::remove_dir(&cur).is_err() {
            break;
        }
        if !cur.pop() {
            break;
        }
    }
}

/// Maps lower-cased directory paths to the casing already in use, so that a mod shipping
/// `Archive/PC/Mod` lands in the existing `archive/pc/mod`. Wine resolves paths
/// case-insensitively and picks only one of two differently-cased directories, which would
/// silently hide files.
#[derive(Default)]
pub struct CaseIndex {
    dirs: HashMap<String, String>,
}

impl CaseIndex {
    pub fn add_tree(&mut self, root: &Path) {
        for e in WalkDir::new(root).into_iter().filter_map(|e| e.ok()).filter(|e| e.file_type().is_dir()) {
            if let Ok(rel) = e.path().strip_prefix(root) {
                let r = rel.to_string_lossy().replace('\\', "/");
                if !r.is_empty() {
                    self.dirs.entry(r.to_lowercase()).or_insert(r);
                }
            }
        }
    }
    /// Only index the directories that matter (avoids walking 80 GB of game data).
    pub fn add_game_dirs(&mut self, game: &Path) {
        for top in ["archive", "bin", "r6", "red4ext", "mods", "engine", "tools"] {
            let p = game.join(top);
            if p.is_dir() {
                self.dirs.entry(top.into()).or_insert(top.into());
                for e in WalkDir::new(&p).max_depth(6).into_iter().filter_map(|e| e.ok()).filter(|e| e.file_type().is_dir()) {
                    if let Ok(rel) = e.path().strip_prefix(game) {
                        let r = rel.to_string_lossy().replace('\\', "/");
                        self.dirs.entry(r.to_lowercase()).or_insert(r);
                    }
                }
            }
        }
    }
    pub fn canonical(&mut self, rel: &str) -> String {
        let parts: Vec<&str> = rel.split('/').collect();
        let mut out = String::new();
        for (i, part) in parts.iter().enumerate() {
            let cand = if out.is_empty() { part.to_string() } else { format!("{out}/{part}") };
            if i + 1 == parts.len() {
                out = cand;
                break;
            }
            let key = cand.to_lowercase();
            out = self.dirs.entry(key).or_insert(cand).clone();
        }
        out
    }
}
