//! Vanilla file list from Steam's cached depot manifests (steamapps/depotcache).
//!
//! Format: u32 magic 0x71F617D0, u32 length, protobuf ContentManifestPayload
//! { repeated FileMapping mappings = 1 }, FileMapping { string filename = 1; uint64 size = 2;
//! uint32 flags = 3; bytes sha_content = 5; ... }. Filenames use backslashes.

use crate::state::{Paths, STEAM_APP_ID};
use std::collections::HashMap;
use std::fs;

pub struct VanillaFile {
    pub size: u64,
    pub sha1: String,
}

/// lower-cased '/'-separated path -> file info. Empty if no manifest is cached.
pub fn load(paths: &Paths) -> HashMap<String, VanillaFile> {
    let mut out = HashMap::new();
    let lib = match paths.game.parent().and_then(|p| p.parent()) {
        Some(l) => l.to_path_buf(),
        None => return out,
    };
    let steam = lib.parent().unwrap_or(&lib).to_path_buf();
    let acf = fs::read_to_string(lib.join(format!("appmanifest_{STEAM_APP_ID}.acf"))).unwrap_or_default();
    // Parse "InstalledDepots" { "<depot>" { "manifest" "<gid>" ... } }
    let mut wanted = vec![];
    let toks: Vec<&str> = acf.split('"').enumerate().filter(|(i, _)| i % 2 == 1).map(|(_, s)| s).collect();
    for w in toks.windows(3) {
        if w[1] == "manifest" && w[0].chars().all(|c| c.is_ascii_digit()) {
            wanted.push(format!("{}_{}.manifest", w[0], w[2]));
        }
    }
    // tokens are: depotid, "manifest", gid  -> but between depotid and "manifest" there is '{'
    // which isn't quoted, so the windows above work.
    for name in wanted {
        let Ok(b) = fs::read(steam.join("depotcache").join(&name)) else { continue };
        parse(&b, &mut out);
    }
    out
}

fn varint(b: &[u8], i: &mut usize) -> Option<u64> {
    let mut v = 0u64;
    let mut shift = 0;
    loop {
        let byte = *b.get(*i)?;
        *i += 1;
        v |= ((byte & 0x7f) as u64) << shift;
        if byte & 0x80 == 0 {
            return Some(v);
        }
        shift += 7;
        if shift > 63 {
            return None;
        }
    }
}

/// Iterate (field number, wire type, value-or-slice) over a protobuf message.
fn fields(b: &[u8], mut f: impl FnMut(u64, Option<u64>, &[u8])) {
    let mut i = 0;
    while i < b.len() {
        let Some(key) = varint(b, &mut i) else { return };
        let (num, wt) = (key >> 3, key & 7);
        match wt {
            0 => match varint(b, &mut i) {
                Some(v) => f(num, Some(v), &[]),
                None => return,
            },
            1 => i += 8,
            5 => i += 4,
            2 => {
                let Some(len) = varint(b, &mut i) else { return };
                let end = i + len as usize;
                if end > b.len() {
                    return;
                }
                f(num, None, &b[i..end]);
                i = end;
            }
            _ => return,
        }
    }
}

fn parse(b: &[u8], out: &mut HashMap<String, VanillaFile>) {
    if b.len() < 8 || u32::from_le_bytes(b[0..4].try_into().unwrap()) != 0x71F6_17D0 {
        return;
    }
    let len = u32::from_le_bytes(b[4..8].try_into().unwrap()) as usize;
    let Some(payload) = b.get(8..8 + len) else { return };
    fields(payload, |num, _, sub| {
        if num != 1 {
            return;
        }
        let (mut name, mut size, mut flags, mut sha) = (String::new(), 0u64, 0u64, String::new());
        fields(sub, |n, v, s| match n {
            1 => name = String::from_utf8_lossy(s).replace('\\', "/"),
            2 => size = v.unwrap_or(0),
            3 => flags = v.unwrap_or(0),
            5 => sha = hex::encode(s),
            _ => {}
        });
        // flag 64 = directory
        if !name.is_empty() && flags & 64 == 0 {
            out.insert(name.to_lowercase(), VanillaFile { size, sha1: sha });
        }
    });
}

/// Base-game files that are neither vanilla nor managed, grouped by folder (three levels; six
/// under CET's mods folder), and with `modified`, vanilla files whose content differs from
/// Steam's copy.
pub struct Unmanaged {
    /// folder (or the file itself) -> files under it
    pub groups: std::collections::BTreeMap<String, Vec<String>>,
    pub edited: Vec<String>,
}

pub fn unmanaged(paths: &Paths, st: &crate::state::State, modified: bool) -> anyhow::Result<Unmanaged> {
    let van = load(paths);
    if van.is_empty() {
        anyhow::bail!("no Steam depot manifest cached; can't tell vanilla files apart");
    }
    let owned: std::collections::BTreeSet<String> = crate::conflicts::ownership(paths, &st.mods, false).into_keys().collect();
    let mut groups: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    let mut edited = vec![];
    for f in crate::files::list_tree(&paths.game) {
        let lf = f.to_lowercase();
        if let Some(v) = van.get(&lf) {
            if modified {
                let p = paths.game.join(&f);
                let size = fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
                let changed = size != v.size || (size < 64 << 20 && sha1_file(&p).is_some_and(|h| h != v.sha1));
                if changed {
                    edited.push(f);
                }
            }
            continue;
        }
        if owned.contains(&lf) || protected_prefix(&lf) {
            continue;
        }
        let key: String = f
            .split('/')
            .take(if lf.starts_with("bin/x64/plugins/cyber_engine_tweaks/mods/") { 6 } else { 3 })
            .collect::<Vec<_>>()
            .join("/");
        groups.entry(key).or_default().push(f);
    }
    Ok(Unmanaged { groups, edited })
}

/// Paths that belong to depots whose manifests may not be cached; never treat them as mod files.
pub fn protected_prefix(rel_lower: &str) -> bool {
    ["archive/pc/content/", "archive/pc/ep1/", "tools/redmod/", "engine/tools/"].iter().any(|p| rel_lower.starts_with(p))
}

pub fn sha1_file(p: &std::path::Path) -> Option<String> {
    use sha1::Digest;
    use std::io::Read;
    let mut f = fs::File::open(p).ok()?;
    let mut h = sha1::Sha1::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Some(hex::encode(h.finalize()))
}
