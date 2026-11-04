//! Nexus Mods API gate: v3 REST for mods/files/versions/downloads, v2 GraphQL for
//! requirements and collections, v1 for the account. Every request passes the AUP gate here
//! (identification headers, the local 429 block, quota headers, cache and retry). The domain
//! questions asked of Nexus (which file, what it requires, is there an update) live one level
//! up, in [`catalogue`]; that is also the only place a [`Nexus`] is built.

pub mod catalogue;
pub use catalogue::Catalogue;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

const GAME: &str = "cyberpunk2077";
/// Sent on every API request, as the Nexus API Acceptable Use Policy requires: a name that
/// never changes between releases, and the version of the build making the request.
const APP_NAME: &str = "cpmm";
const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
const UA: &str = concat!("cpmm/", env!("CARGO_PKG_VERSION"));

/// Body of an API request.
enum Body<'a> {
    None,
    Form(&'a [(&'a str, &'a str)]),
    Json(Value),
}

/// The API's quota as of the last response (its `X-RL-*` headers), kept in the cache dir so
/// separate `cpmm` processes share it. Once Nexus answers 429 every request is refused
/// locally until the window resets: retrying a rate-limited key only burns more requests.
#[derive(Debug, Default, serde::Serialize, Deserialize, PartialEq)]
struct RateLimit {
    hourly_remaining: Option<i64>,
    hourly_reset: Option<String>,
    daily_remaining: Option<i64>,
    daily_reset: Option<String>,
    /// unix time before which no request is sent
    blocked_until: i64,
}

static LOW_QUOTA_WARNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Start of the next UTC hour: when Nexus resets the hourly quota. A 429 means the hourly
/// allowance is gone (the daily one may be too, after which hourly is all there is), so this
/// is the earliest a request can succeed; used when the response carries no reset header.
fn next_hour(now: i64) -> i64 {
    now - now.rem_euclid(3600) + 3600
}

/// Parse an `X-RL-*-Reset` value ("2026-11-04 18:00:00 +0000") to unix time.
fn parse_reset(s: &str) -> Option<i64> {
    let (date, rest) = s.trim().split_once(' ')?;
    let (time, off) = rest.split_once(' ').unwrap_or((rest, "+0000"));
    let d: Vec<i64> = date.split('-').map(|x| x.parse().ok()).collect::<Option<_>>()?;
    let t: Vec<i64> = time.split(':').map(|x| x.parse().ok()).collect::<Option<_>>()?;
    let (&[y, m, dd], &[hh, mm, ss]) = (d.as_slice(), t.as_slice()) else { return None };
    let sign = if off.starts_with('-') { -1 } else { 1 };
    let off = off.trim_start_matches(['+', '-']);
    let off_s = sign * (off.get(..2)?.parse::<i64>().ok()? * 3600 + off.get(2..4)?.parse::<i64>().ok()? * 60);
    // days from civil (Howard Hinnant)
    let y2 = if m <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + dd - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some(days * 86400 + hh * 3600 + mm * 60 + ss - off_s)
}

/// Where the API lives: the transport seam. Production uses [`Endpoints::nexus`]; tests point
/// every base URL at a local server (`fake::Server`), so v1, v3 and GraphQL requests all
/// cross the same gate whichever adapter is behind it.
#[derive(Clone, Debug)]
pub struct Endpoints {
    pub v1: String,
    pub v3: String,
    pub gql: String,
    /// Origin that relative links in responses (a collection's `downloadLink`) resolve against.
    pub site: String,
}

impl Endpoints {
    pub fn nexus() -> Self {
        let api = "https://api.nexusmods.com";
        Endpoints { v1: format!("{api}/v1"), v3: format!("{api}/v3"), gql: format!("{api}/v2/graphql"), site: api.into() }
    }
}

/// Where the Nexus API key is kept (shared with other Nexus tools that use this path).
pub fn key_file() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").expect("HOME not set")).join(".config/nexusmods/apikey")
}

/// The HTTP gate. Built only by [`Catalogue`].
pub struct Nexus {
    key: Option<String>,
    agent: ureq::Agent,
    cache: PathBuf,
    ends: Endpoints,
    /// Max age for cached metadata; manifests are immutable and cached forever.
    ttl: Duration,
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
/// Changelog text from Nexus is HTML-ish: split on <br>, drop tags, decode the common entities.
pub fn clean_changelog(s: &str) -> Vec<String> {
    let br = regex::Regex::new(r"(?i)<br\s*/?>|</p>|</li>").unwrap();
    let tag = regex::Regex::new(r"<[^>]+>").unwrap();
    br.split(s)
        .map(|l| {
            tag.replace_all(l, "")
                .replace("&amp;", "&")
                .replace("&quot;", "\"")
                .replace("&#39;", "'")
                .replace("&lt;", "<")
                .replace("&gt;", ">")
                .replace("&nbsp;", " ")
                .trim()
                .to_string()
        })
        .filter(|l| !l.is_empty())
        .collect()
}

/// One changelog entry, placed relative to the installed version.
#[derive(Serialize, Debug, PartialEq)]
pub struct ChangelogEntry {
    pub version: String,
    pub changes: Vec<String>,
    /// later than the installed version (never, when that version is unknown)
    pub newer: bool,
    /// the installed version
    pub installed: bool,
}

/// Mark a mod page's changelog (newest first, as `Nexus::changelog` returns it) against the
/// installed version.
pub fn changelog_entries(log: Vec<(String, Vec<String>)>, installed: &str) -> Vec<ChangelogEntry> {
    use std::cmp::Ordering;
    log.into_iter()
        .map(|(version, changes)| ChangelogEntry {
            newer: !installed.is_empty() && version_cmp(&version, installed) == Ordering::Greater,
            installed: version_cmp(&version, installed) == Ordering::Equal,
            version,
            changes,
        })
        .collect()
}

/// Natural version order: numeric runs compare as numbers ("1.10" > "1.9"), a leading "v" is
/// ignored, and a plain release sorts after its pre-releases ("2.0" > "2.0-beta").
pub fn version_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    fn parts(s: &str) -> Vec<(bool, String)> {
        let s = s.trim().trim_start_matches(['v', 'V']);
        let mut out: Vec<(bool, String)> = vec![];
        for c in s.chars() {
            let d = c.is_ascii_digit();
            if !c.is_ascii_alphanumeric() {
                out.push((false, String::new()));
                continue;
            }
            match out.last_mut() {
                Some((isd, cur)) if *isd == d && !cur.is_empty() => cur.push(c),
                _ => out.push((d, c.to_string())),
            }
        }
        out.into_iter().filter(|(_, x)| !x.is_empty()).collect()
    }
    let (pa, pb) = (parts(a), parts(b));
    for i in 0..pa.len().max(pb.len()) {
        match (pa.get(i), pb.get(i)) {
            (Some((true, x)), Some((true, y))) => {
                let o = x
                    .trim_start_matches('0')
                    .len()
                    .cmp(&y.trim_start_matches('0').len())
                    .then_with(|| x.trim_start_matches('0').cmp(y.trim_start_matches('0')));
                if o != Ordering::Equal {
                    return o;
                }
            }
            (Some((_, x)), Some((_, y))) => {
                let o = x.to_lowercase().cmp(&y.to_lowercase());
                if o != Ordering::Equal {
                    return o;
                }
            }
            // "2.0" vs "2.0-beta": the extra text is a pre-release; "2.0" vs "2.0.1": newer
            (None, Some((true, _))) => return Ordering::Less,
            (Some((true, _)), None) => return Ordering::Greater,
            (None, Some(_)) => return Ordering::Greater,
            (Some(_), None) => return Ordering::Less,
            (None, None) => break,
        }
    }
    Ordering::Equal
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
        [
            "optional",
            "recommended",
            "not required",
            "only if",
            "only needed if",
            "if you want",
            "if you use",
            "if using",
            "compatibility patch",
            "not needed",
        ]
        .iter()
        .any(|k| n.contains(k))
    }
}

impl Nexus {
    fn new(key: Option<String>, cache: &Path, ends: Endpoints) -> Self {
        let agent = ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(20)).timeout_read(Duration::from_secs(120)).build();
        let _ = fs::create_dir_all(cache.join("http"));
        Nexus { key, agent, cache: cache.to_path_buf(), ends, ttl: Duration::from_secs(3600) }
    }

    fn cache_path(&self, url: &str) -> PathBuf {
        use sha2::Digest;
        let h = hex::encode(sha2::Sha256::digest(url.as_bytes()));
        self.cache.join("http").join(&h[..32])
    }

    fn rate_path(&self) -> PathBuf {
        self.cache.join("ratelimit.json")
    }
    fn rate(&self) -> RateLimit {
        fs::read(self.rate_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    /// Refuse locally while a 429 is in force.
    fn rate_gate(&self) -> Result<()> {
        let rl = self.rate();
        let now = crate::safety::now_unix();
        if rl.blocked_until > now {
            bail!("Nexus API rate limit reached; cpmm will not send API requests until {} UTC", crate::safety::iso(rl.blocked_until));
        }
        Ok(())
    }

    /// Record the quota headers of any API response, and a block after a 429.
    fn note_rate(&self, r: &ureq::Response) {
        let num = |h: &str| r.header(h).and_then(|v| v.trim().parse::<i64>().ok());
        let txt = |h: &str| r.header(h).map(str::to_string);
        let mut rl = self.rate();
        let now = crate::safety::now_unix();
        if rl.blocked_until <= now {
            rl.blocked_until = 0;
        }
        if let Some(v) = num("x-rl-hourly-remaining") {
            rl.hourly_remaining = Some(v);
            rl.hourly_reset = txt("x-rl-hourly-reset");
        }
        if let Some(v) = num("x-rl-daily-remaining") {
            rl.daily_remaining = Some(v);
            rl.daily_reset = txt("x-rl-daily-reset");
        }
        if r.status() == 429 {
            rl.blocked_until = rl.hourly_reset.as_deref().and_then(parse_reset).filter(|&t| t > now).unwrap_or_else(|| next_hour(now));
        }
        // Daily quota spent: the 500/hour allowance is all that's left, so say so (once).
        if rl.daily_remaining == Some(0) && r.status() != 429 && !LOW_QUOTA_WARNED.swap(true, std::sync::atomic::Ordering::Relaxed) {
            eprintln!(
                "warning: Nexus daily API quota used up (resets {}); {} requests left this hour (resets {}).",
                rl.daily_reset.as_deref().unwrap_or("later"),
                rl.hourly_remaining.map_or("?".into(), |n| n.to_string()),
                rl.hourly_reset.as_deref().unwrap_or("on the hour")
            );
        }
        let _ = fs::write(self.rate_path(), serde_json::to_vec(&rl).unwrap_or_default());
    }

    /// Every api.nexusmods.com request goes through here, so the identifying headers are
    /// always sent, the local rate-limit gate always applies, and quota headers are recorded.
    /// The outer `Result` is the local gate (nothing was sent); the inner one is the HTTP outcome.
    fn api_call(&self, method: &str, url: &str, auth: bool, body: Body) -> Result<std::result::Result<ureq::Response, ureq::Error>> {
        self.rate_gate()?;
        let mut req =
            self.agent.request(method, url).set("User-Agent", UA).set("Application-Name", APP_NAME).set("Application-Version", APP_VERSION);
        if auth && let Some(k) = &self.key {
            req = req.set("apikey", k);
        }
        let res = match body {
            Body::None => req.call(),
            Body::Form(f) => req.send_form(f),
            Body::Json(v) => req.send_json(v),
        };
        match &res {
            Ok(r) | Err(ureq::Error::Status(_, r)) => self.note_rate(r),
            Err(_) => {}
        }
        Ok(res)
    }

    fn get_json(&self, url: &str, auth: bool, ttl: Option<Duration>) -> Result<Value> {
        let cp = self.cache_path(url);
        if let Ok(md) = fs::metadata(&cp) {
            let fresh = match ttl {
                None => true,
                Some(t) => md.modified().ok().and_then(|m| SystemTime::now().duration_since(m).ok()).is_some_and(|age| age < t),
            };
            if fresh && let Ok(v) = serde_json::from_slice(&fs::read(&cp)?) {
                return Ok(v);
            }
        }
        let mut last = None;
        for attempt in 0..4 {
            match self.api_call("GET", url, auth, Body::None)? {
                Ok(r) => {
                    let v: Value = r.into_json()?;
                    let _ = fs::write(&cp, serde_json::to_vec(&v)?);
                    return Ok(v);
                }
                // Rate limited: retrying can't succeed before the window resets and only burns
                // more of the quota, so stop here (api_call has recorded the block).
                Err(ureq::Error::Status(429, _)) => {
                    self.rate_gate()?;
                    bail!("GET {url}: HTTP 429 (Nexus API rate limit)")
                }
                Err(ureq::Error::Status(code, r)) if code >= 500 => {
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
        self.data(&format!("{}/games/{GAME}/mods/{gsid}", self.ends.v3), "/data", Some(self.ttl))
    }
    pub fn mod_files(&self, uid: &str) -> Result<Vec<ModFile>> {
        self.data(&format!("{}/mods/{uid}/files", self.ends.v3), "/data/mod_files", Some(self.ttl))
    }
    pub fn versions(&self, file_id: &str) -> Result<Vec<Version>> {
        let mut v: Vec<Version> = self.data(&format!("{}/mod-files/{file_id}/versions", self.ends.v3), "/data/versions", Some(self.ttl))?;
        // Newest *current* upload first. `position` is the author's display order and can put
        // an archived test upload on top (Curbside "1awaw"), so it must not decide recency.
        v.sort_by(|a, b| (b.is_current(), &b.uploaded_at).cmp(&(a.is_current(), &a.uploaded_at)));
        Ok(v)
    }
    pub fn version_by_gsid(&self, gsid: &str) -> Result<Version> {
        self.data(&format!("{}/games/{GAME}/mod-file-versions/{gsid}", self.ends.v3), "/data", None)
    }
    pub fn manifest(&self, url: &str) -> Result<Manifest> {
        let v = self.get_json(url, false, None)?;
        serde_json::from_value(v).context("decoding manifest")
    }
    /// Raw v3 file-level dependency definitions (hard deps with version ranges); rarely authored.
    pub fn file_deps(&self, version_id: &str, force: bool) -> Result<Vec<(u64, String, String)>> {
        let ttl = if force { Duration::ZERO } else { self.ttl };
        let v = self.get_json(&format!("{}/mod-file-versions/{version_id}/dependencies", self.ends.v3), true, Some(ttl))?;
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
        self.key.as_deref().context("Nexus API key required")?;
        let body = form.map_or(Body::None, Body::Form);
        let r =
            self.api_call(method, &format!("{}/{path}", self.ends.v1), true, body)?.map_err(|e| anyhow::anyhow!("{method} {path}: {e}"))?;
        Ok(r.into_json().unwrap_or(Value::Null))
    }
    /// The mod page's changelog, newest version first. (v3 can only add changelogs; reading
    /// them is a v1 endpoint.)
    pub fn changelog(&self, gsid: u64) -> Result<Vec<(String, Vec<String>)>> {
        let v = self.get_json(&format!("{}/games/{GAME}/mods/{gsid}/changelogs.json", self.ends.v1), true, Some(self.ttl))?;
        let mut out: Vec<(String, Vec<String>)> = v
            .as_object()
            .map(|o| {
                o.iter()
                    .map(|(ver, lines)| {
                        let l = lines
                            .as_array()
                            .map(|a| a.iter().filter_map(|x| x.as_str()).flat_map(clean_changelog).collect())
                            .unwrap_or_default();
                        (ver.clone(), l)
                    })
                    .collect()
            })
            .unwrap_or_default();
        out.sort_by(|a, b| version_cmp(&b.0, &a.0));
        Ok(out)
    }

    /// One GraphQL request through the same rate gate as everything else.
    pub fn gql(&self, query: &str, vars: Value) -> Result<Value> {
        let body = json!({"query": query, "variables": vars});
        let r: Value =
            self.api_call("POST", &self.ends.gql, true, Body::Json(body))?.map_err(|e| anyhow::anyhow!("GraphQL: {e}"))?.into_json()?;
        if let Some(e) = r.get("errors") {
            bail!("GraphQL errors: {}", e.to_string().chars().take(500).collect::<String>());
        }
        Ok(r["data"].clone())
    }

    /// A collection revision (the latest published one when `revision` is None).
    pub fn collection_revision(&self, slug: &str, revision: Option<u64>) -> Result<Value> {
        let q = "query($slug:String!,$rev:Int){ collectionRevision(slug:$slug, revision:$rev, domainName:\"cyberpunk2077\", viewAdultContent:true){ revisionNumber downloadLink totalSize modCount gameVersions { reference } collection { name slug summary user { name } } } }";
        let d = self.gql(q, json!({"slug": slug, "rev": revision}))?;
        let r = d["collectionRevision"].clone();
        if r.is_null() {
            bail!("no collection {slug:?} for Cyberpunk 2077");
        }
        Ok(r)
    }

    /// Download a collection's own archive (collection.json + bundled files).
    pub fn download_collection(&self, download_link: &str, dest: &Path) -> Result<()> {
        let url = if download_link.starts_with("http") { download_link.to_string() } else { format!("{}{download_link}", self.ends.site) };
        let links: Value =
            self.api_call("GET", &url, true, Body::None)?.map_err(|e| anyhow::anyhow!("collection download link: {e}"))?.into_json()?;
        let uri =
            links.pointer("/download_links/0/URI").and_then(Value::as_str).context("Nexus returned no download link for the collection")?;
        let r = self.agent.get(uri).set("User-Agent", UA).call().map_err(|e| anyhow::anyhow!("downloading the collection: {e}"))?;
        let mut f = fs::File::create(dest)?;
        std::io::copy(&mut r.into_reader(), &mut f)?;
        Ok(())
    }

    /// Check the API key; returns the account name.
    pub fn validate(&self) -> Result<(String, bool)> {
        let v = self.v1("GET", "users/validate.json", None)?;
        let name = v["name"].as_str().unwrap_or("").to_string();
        if name.is_empty() {
            bail!("Nexus didn't accept the key");
        }
        Ok((name, v["is_premium"].as_bool().unwrap_or(false)))
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
                && fs::metadata(&f)
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|m| SystemTime::now().duration_since(m).ok())
                    .is_some_and(|age| age < TTL);
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
                .api_call("POST", &self.ends.gql, false, Body::Json(body))?
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
        let links = self.get_json(
            &format!("{}/games/{GAME}/mod-file-versions/{}/download-link{q}", self.ends.v3, v.game_scoped_id),
            true,
            Some(Duration::ZERO),
        )?;
        let url =
            links.pointer("/data/links/0/url").and_then(Value::as_str).context("no download link (premium required for API downloads)")?;
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
mod version_tests {
    use super::version_cmp;
    use std::cmp::Ordering::*;

    #[test]
    fn natural_version_order() {
        assert_eq!(version_cmp("1.10", "1.9"), Greater);
        assert_eq!(version_cmp("v2.0.1", "2.0"), Greater);
        assert_eq!(version_cmp("2.0", "2.0-beta"), Greater);
        assert_eq!(version_cmp("1.0.0", "1.0.0"), Equal);
        assert_eq!(version_cmp("0.5.32", "0.5.4"), Greater);
        assert_eq!(version_cmp("ncpdf-1.7", "ncpdf-1.6"), Greater);
    }

    #[test]
    fn changelog_marks_newer_and_installed() {
        let log = |vs: &[&str]| vs.iter().map(|v| (v.to_string(), vec![format!("in {v}")])).collect::<Vec<_>>();
        let marks = |installed: &str| {
            super::changelog_entries(log(&["2.0", "1.10", "v1.9", "1.8"]), installed)
                .into_iter()
                .map(|e| (e.newer, e.installed))
                .collect::<Vec<_>>()
        };
        assert_eq!(marks("1.9"), [(true, false), (true, false), (false, true), (false, false)]);
        // no installed version known: nothing counts as newer
        assert_eq!(marks(""), [(false, false); 4]);
        let e = &super::changelog_entries(log(&["1.0"]), "1.0")[0];
        assert_eq!((e.version.as_str(), e.changes.as_slice()), ("1.0", ["in 1.0".to_string()].as_slice()));
    }

    #[test]
    fn changelog_html_is_cleaned() {
        let l = super::clean_changelog("Fixes<br />Fix crash &amp; <b>stutter</b><br /><br />Date: 1/2/2022");
        assert_eq!(l, vec!["Fixes", "Fix crash & stutter", "Date: 1/2/2022"]);
    }
}

/// The test adapter at the transport seam: a local HTTP server standing in for Nexus. It
/// answers from a fixed route table and records every request that reached it.
#[cfg(test)]
pub mod fake {
    use super::Endpoints;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Debug)]
    pub struct Request {
        /// "GET /v3/mods/u1/files"
        pub line: String,
        /// header lines, lowercased
        pub head: String,
        pub body: String,
    }

    struct Route {
        /// "METHOD /path" (the query string included), or "" for any request
        req: String,
        /// for POSTs: a substring the body must contain (which GraphQL query)
        body_has: String,
        status: u16,
        headers: String,
        body: Vec<u8>,
    }

    #[derive(Default)]
    pub struct Server {
        routes: Vec<Route>,
    }

    pub struct Running {
        pub base: String,
        log: Arc<Mutex<Vec<Request>>>,
    }

    impl Server {
        pub fn new() -> Self {
            Self::default()
        }
        pub fn reply(mut self, req: &str, status: u16, headers: &str, body: impl Into<Vec<u8>>) -> Self {
            let (req, body_has) = req.split_once(" ~").map_or((req, ""), |(r, b)| (r, b.trim()));
            self.routes.push(Route { req: req.into(), body_has: body_has.into(), status, headers: headers.into(), body: body.into() });
            self
        }
        /// 200 with a JSON body. `req` is "GET /path", or "POST /v2/graphql ~needle" to pick a
        /// GraphQL query by a substring of its body.
        pub fn json(self, req: &str, v: serde_json::Value) -> Self {
            self.reply(req, 200, "", v.to_string())
        }
        pub fn start(self) -> Running {
            let srv = TcpListener::bind("127.0.0.1:0").unwrap();
            let base = format!("http://127.0.0.1:{}", srv.local_addr().unwrap().port());
            let log = Arc::new(Mutex::new(vec![]));
            let seen = log.clone();
            std::thread::spawn(move || {
                for c in srv.incoming() {
                    let mut c = c.unwrap();
                    let mut rd = BufReader::new(c.try_clone().unwrap());
                    let mut line = String::new();
                    let _ = rd.read_line(&mut line);
                    let mut head = String::new();
                    loop {
                        let mut l = String::new();
                        if rd.read_line(&mut l).unwrap_or(0) == 0 || l == "\r\n" {
                            break;
                        }
                        head.push_str(&l.to_lowercase());
                    }
                    let len =
                        head.lines().find_map(|l| l.strip_prefix("content-length:")).and_then(|n| n.trim().parse().ok()).unwrap_or(0usize);
                    let mut body = vec![0; len];
                    let _ = rd.read_exact(&mut body);
                    let line = line.trim().trim_end_matches(" HTTP/1.1").to_string();
                    let body = String::from_utf8_lossy(&body).to_string();
                    let route = self.routes.iter().find(|r| (r.req.is_empty() || r.req == line) && body.contains(&r.body_has));
                    seen.lock().unwrap().push(Request { line, head, body });
                    let (status, headers, payload) = route.map_or((404, "", &[][..]), |r| (r.status, r.headers.as_str(), &r.body[..]));
                    let _ = write!(
                        c,
                        "HTTP/1.1 {status} X\r\n{headers}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        payload.len()
                    );
                    let _ = c.write_all(payload);
                }
            });
            Running { base, log }
        }
    }

    impl Running {
        /// Every base URL on this server: v1, v3 and GraphQL alike.
        pub fn endpoints(&self) -> Endpoints {
            let b = &self.base;
            Endpoints { v1: format!("{b}/v1"), v3: format!("{b}/v3"), gql: format!("{b}/v2/graphql"), site: b.clone() }
        }
        pub fn requests(&self) -> Vec<Request> {
            self.log.lock().unwrap().clone()
        }
    }

    /// A fresh, empty directory for one test's cache.
    pub fn temp_dir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("cpmm-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// AUP: every request names the app and its version; a 429 is not retried, and later
    /// calls are refused locally until the window resets (no request reaches the server),
    /// whichever API they are for: v1, v3 and GraphQL share the one gate.
    #[test]
    fn identifies_itself_and_stops_on_429() {
        let srv = fake::Server::new().reply("", 429, "X-RL-Hourly-Remaining: 0\r\nX-RL-Daily-Remaining: 0\r\n", "").start();
        let dir = fake::temp_dir("rl");
        let nx = Nexus::new(Some("k".into()), &dir, srv.endpoints());
        let e1 = nx.v1("GET", "users/validate.json", None).unwrap_err().to_string();
        assert!(e1.contains("429"), "{e1}");
        let e2 = nx.v1("GET", "users/validate.json", None).unwrap_err().to_string();
        assert!(e2.contains("rate limit"), "{e2}");
        let e3 = nx.mod_info(107).unwrap_err().to_string();
        assert!(e3.contains("rate limit"), "{e3}");
        let e4 = nx.requirements(&[107], true).unwrap_err().to_string();
        assert!(e4.contains("rate limit"), "{e4}");
        let reqs = srv.requests();
        assert_eq!(reqs.len(), 1, "429 must not be retried, and the gate must stop every later call");
        assert_eq!(reqs[0].line, "GET /v1/users/validate.json");
        let h = &reqs[0].head;
        assert!(h.contains("application-name: cpmm"), "{h}");
        assert!(h.contains(&format!("application-version: {APP_VERSION}")), "{h}");
        assert!(h.contains("apikey: k"), "{h}");
        assert!(nx.rate().blocked_until > crate::safety::now_unix());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn next_hour_is_top_of_following_hour() {
        assert_eq!(next_hour(7200), 10800);
        assert_eq!(next_hour(7201), 10800);
        assert_eq!(next_hour(10799), 10800);
    }

    #[test]
    fn parses_reset_header() {
        assert_eq!(parse_reset("2026-11-04 18:00:00 +0000"), Some(1793815200));
        assert_eq!(parse_reset("1970-01-01 01:00:00 +0100"), Some(0));
        assert_eq!(parse_reset("garbage"), None);
    }
}
