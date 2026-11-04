//! Verifying staged files against the Nexus manifest hashes of the installed file.

use crate::files;
use crate::nexus::Catalogue;
use crate::state::{Paths, State};
use anyhow::Result;

/// One mod's check.
#[derive(Clone, Debug)]
pub struct Verified {
    pub id: String,
    /// None: a manual mod, no manifest to compare with.
    pub hashes: Option<Hashes>,
}

#[derive(Clone, Debug, Default)]
pub struct Hashes {
    pub ok: usize,
    /// Staged files whose content differs from the manifest
    pub modified: Vec<String>,
    /// Staged files the manifest doesn't list
    pub extra: usize,
}

impl Verified {
    /// The CLI's text (also shown after an install when `after_install_verify` is on).
    pub fn lines(&self) -> Vec<String> {
        match &self.hashes {
            None => vec![format!("  {:<34} no manifest (manual mod)", self.id)],
            Some(h) => {
                let mut out = vec![format!("  {:<34} {} ok, {} modified, {} not in manifest", self.id, h.ok, h.modified.len(), h.extra)];
                out.extend(h.modified.iter().take(5).map(|f| format!("      modified: {f}")));
                out
            }
        }
    }
}

/// Check the mods at `sel` (indexes into `st.mods`) one after another, telling `each` as
/// each one is done; stops at the first manifest that can't be fetched.
pub fn verify(paths: &Paths, st: &State, nx: &Catalogue, sel: &[usize], each: &mut dyn FnMut(Verified)) -> Result<()> {
    for &i in sel {
        let m = &st.mods[i];
        let Some(url) = m.nexus.as_ref().map(|n| n.manifest_url.clone()).filter(|u| !u.is_empty()) else {
            each(Verified { id: m.id.clone(), hashes: None });
            continue;
        };
        let man = nx.gate().manifest(&url)?;
        let by_suffix: Vec<(String, String)> =
            man.files.iter().map(|f| (f.file_path.to_lowercase(), f.file_hashes.sha256.clone())).collect();
        let root = paths.mod_files(&m.id);
        let mut h = Hashes::default();
        for f in files::list_tree(&root) {
            let lf = f.to_lowercase();
            match by_suffix.iter().find(|(p, _)| *p == lf || p.ends_with(&format!("/{lf}")) || lf.ends_with(&format!("/{p}"))) {
                Some((_, sha)) => {
                    if files::sha256_file(&root.join(&f))? == *sha {
                        h.ok += 1
                    } else {
                        h.modified.push(f)
                    }
                }
                None => h.extra += 1,
            }
        }
        each(Verified { id: m.id.clone(), hashes: Some(h) });
    }
    Ok(())
}
