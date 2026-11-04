//! Nexus Mods API: v3 REST for mods/files/versions/downloads, v2 GraphQL for requirements.

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::{Value, json};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

const V3: &str = "https://api.nexusmods.com/v3";
const GQL: &str = "https://api.nexusmods.com/v2/graphql";
const GAME: &str = "cyberpunk2077";
const UA: &str = concat!("cpmm/", env!("CARGO_PKG_VERSION"));

pub struct Nexus {
    key: Option<String>,
    agent: ureq::Agent,
    cache: PathBuf,
    /// Max age for cached metadata; manifests are immutable and cached forever.
    pub ttl: Duration,
}

#[derive(Deserialize, Clone, Debug)]
#[allow(dead_code)] // kept for completeness of the API shape
pub struct ModInfo {
    pub id: String,
    pub game_scoped_id: String,
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub primary_file_version: Option<IdPair>,
}
#[derive(Deserialize, Clone, Debug)]
#[allow(dead_code)] // kept for completeness of the API shape
pub struct IdPair {
    pub id: String,
    #[serde(default)]
    pub game_scoped_id: String,
}
#[derive(Deserialize, Clone, Debug)]
#[allow(dead_code)] // kept for completeness of the API shape
pub struct ModFile {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub is_active: bool,
    #[serde(default)]
    pub last_file_uploaded_at: String,
}
#[derive(Deserialize, Clone, Debug)]
pub struct FileRef {
    pub id: String,
    #[serde(default)]
    pub name: String,
}
#[derive(Deserialize, Clone, Debug)]
pub struct Version {
    pub id: String,
    pub file: FileRef,
    /// Author's display order. Not a recency signal (see `versions`).
    #[serde(default)]
    #[allow(dead_code)]
    pub position: String,
    pub game_scoped_id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub uploaded_at: String,
    #[serde(default)]
    pub manifest_url: Option<String>,
}
impl Version {
    /// Not archived/removed/superseded: a version the author still offers as current.
    pub fn is_current(&self) -> bool {
        !matches!(self.category.as_str(), "archived" | "old_version" | "removed" | "deleted")
    }
}
#[derive(Deserialize, Clone, Debug)]
pub struct Manifest {
    pub archive: ManArchive,
    pub files: Vec<ManFile>,
}
#[derive(Deserialize, Clone, Debug)]
pub struct ManArchive {
    pub size: u64,
    pub hashes: Hashes,
}
#[derive(Deserialize, Clone, Debug)]
pub struct Hashes {
    #[serde(rename = "SHA256")]
    pub sha256: String,
}
#[derive(Deserialize, Clone, Debug)]
#[allow(dead_code)] // kept for completeness of the API shape
pub struct ManFile {
    pub file_path: String,
    #[serde(default)]
    pub file_size: u64,
    pub file_hashes: Hashes,
}
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Req {
    pub mod_id: u64,
    pub name: String,
    pub notes: String,
    pub external: bool,
    pub url: String,
}
impl Req {
    /// Nexus has no optional flag; authors write it in the notes.
    pub fn optional(&self) -> bool {
        let n = self.notes.to_lowercase();
        ["optional", "recommended", "not required", "only if", "only needed if", "if you want", "if you use", "if using", "compatibility patch", "not needed"]
            .iter()
            .any(|k| n.contains(k))
    }
}

impl Nexus {
    pub fn new(key: Option<String>, cache: &Path) -> Self {
        let agent = ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(20)).timeout_read(Duration::from_secs(120)).build();
        let _ = fs::create_dir_all(cache.join("http"));
        Nexus { key, agent, cache: cache.to_path_buf(), ttl: Duration::from_secs(3600) }
    }

    fn cache_path(&self, url: &str) -> PathBuf {
        use sha2::Digest;
        let h = hex::encode(sha2::Sha256::digest(url.as_bytes()));
        self.cache.join("http").join(&h[..32])
    }

    fn get_json(&self, url: &str, auth: bool, ttl: Option<Duration>) -> Result<Value> {
        let cp = self.cache_path(url);
        if let Ok(md) = fs::metadata(&cp) {
            let fresh = match ttl {
                None => true,
                Some(t) => md.modified().ok().and_then(|m| SystemTime::now().duration_since(m).ok()).is_some_and(|age| age < t),
            };
            if fresh {
                if let Ok(v) = serde_json::from_slice(&fs::read(&cp)?) {
                    return Ok(v);
                }
            }
        }
        let mut last = None;
        for attempt in 0..4 {
            let mut req = self.agent.get(url).set("User-Agent", UA).set("Application-Name", "cpmm").set("Application-Version", env!("CARGO_PKG_VERSION"));
            if auth {
                if let Some(k) = &self.key {
                    req = req.set("apikey", k);
                }
            }
            match req.call() {
                Ok(r) => {
                    let v: Value = r.into_json()?;
                    let _ = fs::write(&cp, serde_json::to_vec(&v)?);
                    return Ok(v);
                }
                Err(ureq::Error::Status(code, r)) if code == 429 || code >= 500 => {
                    last = Some(format!("HTTP {code} {}", r.into_string().unwrap_or_default().chars().take(200).collect::<String>()));
                }
                Err(ureq::Error::Status(code, r)) => {
                    bail!("GET {url}: HTTP {code} {}", r.into_string().unwrap_or_default().chars().take(300).collect::<String>())
                }
                Err(e) => last = Some(e.to_string()),
            }
            std::thread::sleep(Duration::from_secs(2u64.pow(attempt)));
        }
        bail!("GET {url}: {}", last.unwrap_or_default())
    }

    fn data<T: for<'de> Deserialize<'de>>(&self, url: &str, ptr: &str, ttl: Option<Duration>) -> Result<T> {
        let v = self.get_json(url, true, ttl)?;
        let d = v.pointer(ptr).cloned().with_context(|| format!("{url}: missing {ptr}"))?;
        serde_json::from_value(d).with_context(|| format!("decoding {url}"))
    }

    pub fn mod_info(&self, gsid: u64) -> Result<ModInfo> {
        self.data(&format!("{V3}/games/{GAME}/mods/{gsid}"), "/data", Some(self.ttl))
    }
    pub fn mod_files(&self, uid: &str) -> Result<Vec<ModFile>> {
        self.data(&format!("{V3}/mods/{uid}/files"), "/data/mod_files", Some(self.ttl))
    }
    pub fn versions(&self, file_id: &str) -> Result<Vec<Version>> {
        let mut v: Vec<Version> = self.data(&format!("{V3}/mod-files/{file_id}/versions"), "/data/versions", Some(self.ttl))?;
        // Newest *current* upload first. `position` is the author's display order and can put
        // an archived test upload on top (Curbside "1awaw"), so it must not decide recency.
        v.sort_by(|a, b| (b.is_current(), &b.uploaded_at).cmp(&(a.is_current(), &a.uploaded_at)));
        Ok(v)
    }
    pub fn version_by_gsid(&self, gsid: &str) -> Result<Version> {
        self.data(&format!("{V3}/games/{GAME}/mod-file-versions/{gsid}"), "/data", None)
    }
    pub fn manifest(&self, url: &str) -> Result<Manifest> {
        let v = self.get_json(url, false, None)?;
        serde_json::from_value(v).context("decoding manifest")
    }
    /// Raw v3 file-level dependency definitions (hard deps with version ranges); rarely authored.
    pub fn file_deps(&self, version_id: &str, force: bool) -> Result<Vec<(u64, String, String)>> {
        let ttl = if force { Duration::ZERO } else { self.ttl };
        let v = self.get_json(&format!("{V3}/mod-file-versions/{version_id}/dependencies"), true, Some(ttl))?;
        let mut out = vec![];
        for def in v.pointer("/dependency_definitions").and_then(Value::as_array).cloned().unwrap_or_default() {
            for r in def["ranges"].as_array().cloned().unwrap_or_default() {
                let t = &r["target_mod_file"];
                let gsid = t.pointer("/mod/game_scoped_id").and_then(Value::as_str).and_then(|s| s.parse().ok()).unwrap_or(0);
                let name = t.pointer("/mod/name").and_then(Value::as_str).unwrap_or("").to_string();
                let minv = r.pointer("/min_version/version").and_then(Value::as_str).unwrap_or("").to_string();
                out.push((gsid, name, minv));
            }
        }
        Ok(out)
    }

    fn v1(&self, method: &str, path: &str, form: Option<&[(&str, &str)]>) -> Result<Value> {
        let key = self.key.as_deref().context("Nexus API key required")?;
        let base = std::env::var("CPMM_NEXUS_V1").unwrap_or_else(|_| "https://api.nexusmods.com/v1".into()); // override for tests
        let req = self.agent.request(method, &format!("{base}/{path}")).set("User-Agent", UA).set("Application-Name", "cpmm").set("apikey", key);
        let r = match form {
            Some(f) => req.send_form(f),
            None => req.call(),
        }
        .map_err(|e| anyhow::anyhow!("{method} {path}: {e}"))?;
        Ok(r.into_json().unwrap_or(Value::Null))
    }
    /// Cyberpunk mods on the account's "Tracked" list (the Track button on a mod page).
    pub fn tracked(&self) -> Result<Vec<u64>> {
        let v = self.v1("GET", "user/tracked_mods.json", None)?;
        Ok(v.as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter(|x| x["domain_name"].as_str() == Some(GAME))
            .filter_map(|x| x["mod_id"].as_u64())
            .collect())
    }
    pub fn untrack(&self, mod_id: u64) -> Result<()> {
        let id = mod_id.to_string();
        self.v1("DELETE", &format!("user/tracked_mods.json?domain_name={GAME}"), Some(&[("mod_id", id.as_str())]))?;
        Ok(())
    }

    /// Mod-page requirements via GraphQL, for many mods at once. Each answer is cached per mod
    /// for 15 minutes (a lookup followed by an install asks once); `force` skips the cache.
    /// Installed mods keep theirs in state.json (`Mod::reqs_checked`), so this is mostly hit
    /// for mods that aren't installed yet.
    pub fn requirements(&self, ids: &[u64], force: bool) -> Result<Vec<(u64, String, Vec<Req>)>> {
        const TTL: Duration = Duration::from_secs(15 * 60);
        let dir = self.cache.join("reqs");
        let _ = fs::create_dir_all(&dir);
        let mut out = vec![];
        let mut need = vec![];
        for &id in ids {
            let f = dir.join(format!("{id}.json"));
            let fresh = !force
                && fs::metadata(&f).ok().and_then(|m| m.modified().ok()).and_then(|m| SystemTime::now().duration_since(m).ok()).is_some_and(|age| age < TTL);
            match fresh.then(|| fs::read(&f).ok().and_then(|b| serde_json::from_slice::<(u64, String, Vec<Req>)>(&b).ok())).flatten() {
                Some(hit) => out.push(hit),
                None => need.push(id),
            }
        }
        for chunk in need.chunks(50) {
            let q = "query($ids:[CompositeDomainWithIdInput!]!){ legacyModsByDomain(ids:$ids){ nodes { modId name modRequirements { nexusRequirements { nodes { modId modName url notes externalRequirement } } } } } }";
            let vars = json!({"ids": chunk.iter().map(|i| json!({"gameDomain": GAME, "modId": i})).collect::<Vec<_>>()});
            let body = json!({"query": q, "variables": vars});
            let r: Value = self
                .agent
                .post(GQL)
                .set("User-Agent", UA)
                .set("Content-Type", "application/json")
                .send_json(body)
                .map_err(|e| anyhow::anyhow!("GraphQL: {e}"))?
                .into_json()?;
            if let Some(e) = r.get("errors") {
                bail!("GraphQL errors: {}", e.to_string().chars().take(500).collect::<String>());
            }
            for n in r.pointer("/data/legacyModsByDomain/nodes").and_then(Value::as_array).cloned().unwrap_or_default() {
                let id = n["modId"].as_u64().unwrap_or(0);
                let name = n["name"].as_str().unwrap_or("").to_string();
                let reqs = n
                    .pointer("/modRequirements/nexusRequirements/nodes")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|x| Req {
                        mod_id: x["modId"].as_str().and_then(|s| s.parse().ok()).or(x["modId"].as_u64()).unwrap_or(0),
                        name: x["modName"].as_str().unwrap_or("").to_string(),
                        notes: x["notes"].as_str().unwrap_or("").to_string(),
                        external: x["externalRequirement"].as_bool().unwrap_or(false),
                        url: x["url"].as_str().unwrap_or("").to_string(),
                    })
                    .collect();
                let entry = (id, name, reqs);
                if let Ok(b) = serde_json::to_vec(&entry) {
                    let _ = fs::write(dir.join(format!("{id}.json")), b);
                }
                out.push(entry);
            }
        }
        Ok(out)
    }

    /// Download one file version to `dest`, verifying the archive SHA-256 from its manifest.
    pub fn download(&self, v: &Version, man: &Manifest, dest: &Path, nxm: Option<(&str, &str)>) -> Result<()> {
        if dest.exists() && crate::files::sha256_file(dest)? == man.archive.hashes.sha256 {
            return Ok(()); // cached
        }
        let q = nxm.map(|(k, e)| format!("?key={k}&expires={e}")).unwrap_or_default();
        let links = self.get_json(&format!("{V3}/games/{GAME}/mod-file-versions/{}/download-link{q}", v.game_scoped_id), true, Some(Duration::ZERO))?;
        let url = links.pointer("/data/links/0/url").and_then(Value::as_str).context("no download link (premium required for API downloads)")?;
        let url = url.replace(' ', "%20");
        let r = self.agent.get(&url).set("User-Agent", UA).call().map_err(|e| anyhow::anyhow!("download: {e}"))?;
        let part = dest.with_extension("part");
        let mut f = fs::File::create(&part)?;
        let mut rd = r.into_reader();
        let total = man.archive.size.max(1);
        let mut buf = vec![0u8; 1 << 20];
        let (mut got, mut last_pct) = (0u64, 0u64);
        loop {
            let n = std::io::Read::read(&mut rd, &mut buf)?;
            if n == 0 {
                break;
            }
            f.write_all(&buf[..n])?;
            got += n as u64;
            let pct = got * 100 / total;
            if total > 20 << 20 && pct >= last_pct + 10 {
                eprint!(" {pct}%");
                last_pct = pct;
            }
        }
        if last_pct > 0 {
            eprintln!();
        }
        drop(f);
        let h = crate::files::sha256_file(&part)?;
        if h != man.archive.hashes.sha256 {
            bail!("archive hash mismatch for {} (got {h})", v.name);
        }
        fs::rename(part, dest)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn v(ver: &str, cat: &str, up: &str, pos: &str) -> Version {
        Version {
            id: ver.into(),
            file: FileRef { id: "f".into(), name: String::new() },
            position: pos.into(),
            game_scoped_id: ver.into(),
            name: String::new(),
            version: ver.into(),
            category: cat.into(),
            uploaded_at: up.into(),
            manifest_url: None,
        }
    }

    /// Regression: Curbside's archived test upload "1awaw" had the highest `position` and was
    /// installed by `update --all`, crashing the game on car summon.
    #[test]
    fn newest_is_latest_current_upload_not_position() {
        let mut vs = vec![
            v("1awaw", "archived", "2026-10-26T03:18:20Z", "9"),
            v("1.0.1", "main", "2026-10-27T14:01:38Z", "3"),
            v("1.0.0", "old_version", "2026-10-26T05:04:58Z", "2"),
        ];
        vs.sort_by(|a, b| (b.is_current(), &b.uploaded_at).cmp(&(a.is_current(), &a.uploaded_at)));
        assert_eq!(vs[0].version, "1.0.1");
    }
}
