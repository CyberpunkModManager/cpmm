//! The downloads library: every archive cpmm downloads (or is handed) is kept in the download
//! folder with a small record next to it, listed separately from installed mods, and installed
//! only when the user asks (or right away when `auto_install_downloads` is on).

use crate::nexus::catalogue::{download_name, mod_id_from_download};
use crate::state::{Paths, State};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

const META_EXT: &str = "cpmm.json";

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct DlMeta {
    /// display name (mod name)
    pub name: String,
    pub mod_id: Option<u64>,
    /// Nexus file (stable across its uploads) and the exact upload
    pub file_id: String,
    pub version_gsid: String,
    pub version: String,
    pub file_name: String,
    pub uploaded_at: String,
    /// where it came from: "nexus" (cpmm download / install), "nxm" (link), "browser", "local"
    pub source: String,
    pub downloaded_at: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct Download {
    /// archive file name inside the download folder (the key for install/remove)
    pub file: String,
    pub path: PathBuf,
    pub size: u64,
    #[serde(flatten)]
    pub meta: DlMeta,
    /// "installed", "other version installed", "not installed"
    pub status: &'static str,
    pub installed_as: Option<String>,
}

fn meta_path(arc: &Path) -> PathBuf {
    let mut s = arc.as_os_str().to_owned();
    s.push(format!(".{META_EXT}"));
    PathBuf::from(s)
}

pub fn read_meta(arc: &Path) -> Option<DlMeta> {
    serde_json::from_slice(&fs::read(meta_path(arc)).ok()?).ok()
}

pub fn write_meta(arc: &Path, m: &DlMeta) -> Result<()> {
    fs::write(meta_path(arc), serde_json::to_vec_pretty(m)?)?;
    Ok(())
}

fn is_archive_file(p: &Path) -> bool {
    let n = p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    p.is_file() && !n.ends_with(&format!(".{META_EXT}")) && !n.ends_with(".part") && !n.ends_with(".tmp") && !n.starts_with('.')
}

/// Everything in the library, newest first, with install status.
pub fn list(paths: &Paths, st: &State) -> Vec<Download> {
    let dir = paths.downloads();
    // older cpmm versions put browser downloads in a subfolder
    let mut files: Vec<PathBuf> = [dir.clone(), dir.join("browser")]
        .iter()
        .flat_map(|d| fs::read_dir(d).into_iter().flatten().flatten().map(|e| e.path()))
        .filter(|p| is_archive_file(p))
        .collect();
    files.sort_by_key(|p| std::cmp::Reverse(fs::metadata(p).and_then(|m| m.modified()).ok()));
    files
        .into_iter()
        .map(|p| {
            let rel = p.strip_prefix(&dir).unwrap_or(&p).to_string_lossy().to_string();
            let stem = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            let meta = read_meta(&p).unwrap_or_else(|| DlMeta {
                name: download_name(&stem),
                mod_id: mod_id_from_download(&stem),
                source: "local".into(),
                ..Default::default()
            });
            let (status, installed_as) = status(st, &rel, &meta);
            Download { size: fs::metadata(&p).map(|m| m.len()).unwrap_or(0), file: rel, path: p, meta, status, installed_as }
        })
        .collect()
}

fn status(st: &State, file: &str, m: &DlMeta) -> (&'static str, Option<String>) {
    for x in &st.mods {
        if x.archive.as_deref() == Some(file) {
            return ("installed", Some(x.id.clone()));
        }
        if let (Some(id), Some(n)) = (m.mod_id, &x.nexus)
            && n.mod_id == id
            && !m.version_gsid.is_empty()
            && n.version_gsid == m.version_gsid
        {
            return ("installed", Some(x.id.clone()));
        }
    }
    // same Nexus file installed at another upload
    if let Some(x) = st
        .mods
        .iter()
        .find(|x| x.nexus.as_ref().is_some_and(|n| Some(n.mod_id) == m.mod_id && (m.file_id.is_empty() || n.file_id == m.file_id)))
    {
        return ("other version installed", Some(x.id.clone()));
    }
    ("not installed", None)
}

/// Find a download by file name, list number (1-based, as `cpmm downloads` prints) or name part.
pub fn find(paths: &Paths, st: &State, key: &str) -> Result<Download> {
    let all = list(paths, st);
    if let Ok(n) = key.parse::<usize>()
        && n >= 1
        && n <= all.len()
        && !all.iter().any(|d| d.file == key)
    {
        return Ok(all[n - 1].clone());
    }
    if let Some(d) = all.iter().find(|d| d.file == key || d.path == Path::new(key)) {
        return Ok(d.clone());
    }
    let lk = key.to_lowercase();
    let hits: Vec<&Download> =
        all.iter().filter(|d| d.file.to_lowercase().contains(&lk) || d.meta.name.to_lowercase().contains(&lk)).collect();
    match hits.as_slice() {
        [one] => Ok((*one).clone()),
        [] => bail!("no download matches {key:?} (see `cpmm downloads`)"),
        _ => bail!("{key:?} matches {} downloads; use the number or file name from `cpmm downloads`", hits.len()),
    }
}

/// Bring an archive from elsewhere into the library (moved if it's already on the same drive
/// and `move_it`, otherwise copied), with a record of where it came from.
pub fn add(paths: &Paths, src: &Path, mod_id: Option<u64>, source: &str, move_it: bool) -> Result<PathBuf> {
    if !src.is_file() {
        bail!("{} is not a file", src.display());
    }
    let dir = paths.downloads();
    fs::create_dir_all(&dir)?;
    let name = src.file_name().context("no file name")?;
    let mut dst = dir.join(name);
    if dst != src {
        let mut n = 2;
        while dst.exists() {
            dst = dir.join(format!("{n}-{}", name.to_string_lossy()));
            n += 1;
        }
        if !(move_it && fs::rename(src, &dst).is_ok()) {
            crate::files::reflink_or_copy(src, &dst)?;
            if move_it {
                let _ = fs::remove_file(src);
            }
        }
    }
    let stem = dst.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let mut m = read_meta(&dst).unwrap_or_default();
    m.name = if m.name.is_empty() { download_name(&stem) } else { m.name };
    m.mod_id = mod_id.or(m.mod_id).or_else(|| mod_id_from_download(&stem));
    m.source = source.into();
    m.downloaded_at = crate::state::now();
    write_meta(&dst, &m)?;
    Ok(dst)
}

/// Write the Downloads-list record for a Nexus archive, keeping when and how it first arrived
/// (an nxm download stays "nxm" after installing).
pub fn record_nexus(arc: &Path, name: &str, gid: u64, ver: &crate::nexus::Version, source: &str) {
    let prev = read_meta(arc);
    let keep = prev.as_ref().map(|m| m.downloaded_at.clone()).filter(|d| !d.is_empty());
    let source = prev.map(|m| m.source).filter(|s| !s.is_empty()).unwrap_or_else(|| source.to_string());
    let _ = write_meta(
        arc,
        &DlMeta {
            name: name.to_string(),
            mod_id: Some(gid),
            file_id: ver.file.id.clone(),
            version_gsid: ver.game_scoped_id.clone(),
            version: ver.version.clone(),
            file_name: ver.file.name.clone(),
            uploaded_at: ver.uploaded_at.clone(),
            source,
            downloaded_at: keep.unwrap_or_else(crate::state::now),
        },
    );
}

/// Where the GUI's Nexus browser saves manual downloads before they join the list.
pub fn incoming(paths: &Paths) -> PathBuf {
    paths.downloads().join(".incoming")
}

pub fn remove(d: &Download) -> Result<()> {
    fs::remove_file(&d.path).with_context(|| format!("removing {}", d.path.display()))?;
    let _ = fs::remove_file(meta_path(&d.path));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_sits_next_to_the_archive() {
        let d = std::env::temp_dir().join(format!("cpmm-dl-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        let a = d.join("168851-highbeam-1-0");
        fs::write(&a, b"x").unwrap();
        write_meta(&a, &DlMeta { name: "Highbeam".into(), mod_id: Some(36088), ..Default::default() }).unwrap();
        assert_eq!(read_meta(&a).unwrap().mod_id, Some(36088));
        assert!(is_archive_file(&a) && !is_archive_file(&meta_path(&a)));
        let _ = fs::remove_dir_all(&d);
    }
}
