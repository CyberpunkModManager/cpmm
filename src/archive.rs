//! Reader for REDengine 4 `.archive` (RDAR) indexes, enough to know which game resources an
//! archive replaces without unpacking it.
//!
//! Layout (little endian): header { u32 magic "RDAR", u32 version, u64 index_pos, u32 index_size,
//! u64 debug_pos, u32 debug_size, u64 file_size }; at index_pos: { u32 file_table_offset,
//! u32 file_table_size, u64 crc, u32 file_count, u32 segment_count, u32 dependency_count }, then
//! file entries (56 bytes: u64 FNV-1a path hash, u64 timestamp, u32 inline_segments,
//! u32 segments_start, u32 segments_end, u32 deps_start, u32 deps_end, [u8; 20] sha1),
//! segments (16 bytes: u64 offset, u32 zsize, u32 size), dependencies (8 bytes).

use anyhow::{Context, Result, bail};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

pub const MAGIC: u32 = 0x5241_4452; // "RDAR"

#[derive(Clone, Copy, Debug)]
pub struct Entry {
    pub hash: u64,
    pub seg_start: u32,
    pub seg_end: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct Segment {
    pub offset: u64,
    pub zsize: u32,
}

pub struct Index {
    pub entries: Vec<Entry>,
    pub segments: Vec<Segment>,
}

fn u32le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn u64le(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

pub fn read_index(path: &Path) -> Result<Index> {
    let mut f = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut h = [0u8; 40];
    f.read_exact(&mut h).context("short header")?;
    if u32le(&h, 0) != MAGIC {
        bail!("{} is not an RDAR archive", path.display());
    }
    let (ipos, isize) = (u64le(&h, 8), u32le(&h, 16) as usize);
    let flen = f.metadata()?.len();
    if isize < 28 || ipos + isize as u64 > flen || isize > 256 << 20 {
        bail!("{}: corrupt index ({ipos}+{isize} of {flen})", path.display());
    }
    f.seek(SeekFrom::Start(ipos))?;
    let mut ix = vec![0u8; isize];
    f.read_exact(&mut ix)?;
    let (nfiles, nsegs) = (u32le(&ix, 16) as usize, u32le(&ix, 20) as usize);
    let need = 28 + nfiles * 56 + nsegs * 16;
    if need > ix.len() {
        bail!("{}: index too small for {nfiles} files", path.display());
    }
    let mut entries = Vec::with_capacity(nfiles);
    for i in 0..nfiles {
        let o = 28 + i * 56;
        entries.push(Entry { hash: u64le(&ix, o), seg_start: u32le(&ix, o + 20), seg_end: u32le(&ix, o + 24) });
    }
    let so = 28 + nfiles * 56;
    let segments = (0..nsegs).map(|i| Segment { offset: u64le(&ix, so + i * 16), zsize: u32le(&ix, so + i * 16 + 8) }).collect();
    Ok(Index { entries, segments })
}

/// Content fingerprint of one resource: SHA-256 over its stored (compressed) segment bytes.
/// Two archives shipping the byte-identical resource get the same fingerprint.
pub fn fingerprint(path: &Path, idx: &Index, e: &Entry) -> Result<String> {
    use sha2::{Digest, Sha256};
    let mut f = File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = Vec::new();
    for s in idx.segments.get(e.seg_start as usize..e.seg_end as usize).unwrap_or(&[]) {
        if s.zsize as usize > 512 << 20 {
            bail!("segment too large");
        }
        buf.resize(s.zsize as usize, 0);
        f.seek(SeekFrom::Start(s.offset))?;
        f.read_exact(&mut buf)?;
        h.update(&buf);
    }
    Ok(hex::encode(h.finalize()))
}

#[allow(dead_code)] // used to name resources once a path list is available
/// FNV-1a 64, the hash REDengine uses for resource paths (lower-cased, backslashes).
pub fn path_hash(p: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in p.to_lowercase().replace('/', "\\").bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

#[cfg(test)]
mod tests {
    #[test]
    fn fnv_matches_known_engine_hash() {
        // "base\\gameplay\\gui\\common\\main_colors.inkstyle" is a stock resource; any stable
        // FNV-1a implementation gives this value.
        assert_eq!(super::path_hash(""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(super::path_hash("a"), 0xaf63_dc4c_8601_ec8c);
    }
}
