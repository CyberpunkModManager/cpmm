//! The Nexus catalogue: the questions cpmm asks of Nexus, answered above the HTTP gate.
//!
//! - which file (and upload) to install for a mod, optionally narrowed by `--file`
//! - what that file requires: the mod page's list merged with the file's own dependencies
//! - whether an installed mod has an update
//! - a file's archive, downloaded into the downloads dir (reused while its hash matches)
//! - a collection revision and its manifest (cached per revision)
//! - mod ids in what users paste (ids, page URLs) and in manual download names
//!
//! Every request still goes through [`Nexus`] (AUP headers, 429 gate, cache, retry); this is
//! the only place one is built, from the stored API key and the [`Endpoints`] transport seam.

use super::{Endpoints, Manifest, ModInfo, Nexus, Req, Version};
use crate::state::{NexusSource, Paths, Requirement, slugify};
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub struct Catalogue<'p> {
    nx: Nexus,
    paths: &'p Paths,
}

/// What the catalogue says about one installed Nexus mod.
#[derive(Debug, Clone)]
pub enum Update {
    /// The mod page isn't published (hidden, under moderation, …): its status.
    PageStatus(String),
    /// Installed without a known Nexus file, so only the page's version can be shown.
    FileUnknown {
        page_version: String,
    },
    /// The installed file has no uploads left.
    NoVersions,
    UpToDate,
    /// The installed upload is the newest, but its author has archived the file.
    Archived {
        page_version: String,
    },
    /// A newer current upload of the installed file.
    Available(Box<Version>),
}

impl<'p> Catalogue<'p> {
    /// The catalogue for this setup, with the stored API key (if any).
    pub fn open(paths: &'p Paths) -> Self {
        Self::with_key(paths, paths.api_key().ok())
    }

    /// The catalogue with a given key (to validate one before it is saved).
    pub fn with_key(paths: &'p Paths, key: Option<String>) -> Self {
        Self::over(paths, key, &paths.cache(), Endpoints::nexus())
    }

    fn over(paths: &'p Paths, key: Option<String>, cache: &Path, ends: Endpoints) -> Self {
        Catalogue { nx: Nexus::new(key, cache, ends), paths }
    }

    /// Re-ask Nexus for mod pages, files and versions instead of answering from the hour-long
    /// metadata cache (`cpmm updates --refresh`).
    pub fn uncached(mut self) -> Self {
        self.nx.ttl = Duration::ZERO;
        self
    }

    /// The gate itself, for plain lookups that need no catalogue policy (account, tracked
    /// list, changelogs, manifests, mod names).
    pub fn gate(&self) -> &Nexus {
        &self.nx
    }

    /// Which file to install for mod `gid`. `sel` (from `--file`, an nxm link or a collection)
    /// is a file id, a game-scoped version id (any upload, old ones included) or part of a file
    /// name. Without it: the only main file, else the page's primary file; several candidates
    /// is an error listing them.
    pub fn pick(&self, gid: u64, sel: Option<&str>) -> Result<(ModInfo, Version)> {
        let info = self.nx.mod_info(gid)?;
        let v = self.pick_file(&info, sel)?;
        Ok((info, v))
    }

    fn pick_file(&self, info: &ModInfo, sel: Option<&str>) -> Result<Version> {
        let nx = &self.nx;
        let files = nx.mod_files(&info.id)?;
        // A numeric selection that is a version id (collections, nxm links): one lookup instead of
        // listing every file's versions.
        if let Some(s) = sel.filter(|s| s.chars().all(|c| c.is_ascii_digit()) && !files.iter().any(|f| f.id == *s))
            && let Ok(v) = nx.version_by_gsid(s)
            && files.iter().any(|f| f.id == v.file.id)
        {
            return Ok(v);
        }
        let mut cands = vec![];
        for f in files.iter().filter(|f| f.is_active) {
            if let Some(v) = nx.versions(&f.id)?.into_iter().next() {
                cands.push(v);
            }
        }
        if let Some(s) = sel {
            if let Some(c) = cands.iter().find(|c| c.file.id == s || c.game_scoped_id == s) {
                return Ok(c.clone());
            }
            if s.chars().all(|c| c.is_ascii_digit()) {
                // an older upload, addressed by its game-scoped version id
                let v = nx.version_by_gsid(s)?;
                if !files.iter().any(|f| f.id == v.file.id) {
                    bail!("version {s} does not belong to {}", info.name);
                }
                return Ok(v);
            }
            let ls = s.to_lowercase();
            let hits: Vec<&Version> = cands
                .iter()
                .filter(|c| c.file.id == s || c.file.name.to_lowercase().contains(&ls) || c.name.to_lowercase().contains(&ls))
                .collect();
            if hits.len() == 1 {
                return Ok(hits[0].clone());
            }
            bail!("--file {s:?} matched {} files", hits.len());
        }
        let mains: Vec<&Version> = cands.iter().filter(|c| c.category == "main").collect();
        let primary = info.primary_file_version.as_ref().map(|p| p.id.clone()).unwrap_or_default();
        if mains.len() == 1 {
            return Ok(mains[0].clone());
        }
        if let Some(p) = cands.iter().find(|c| c.id == primary)
            && (mains.is_empty() || mains.iter().any(|m| m.id == p.id))
        {
            return Ok(p.clone());
        }
        let mut msg = format!("{} has several files; choose one with --file <gsid>:\n", info.name);
        for c in &cands {
            msg += &format!(
                "  {:>7}  {:<9} v{:<10} {}  {}\n",
                c.game_scoped_id,
                c.category,
                c.version,
                c.uploaded_at.get(..10).unwrap_or(""),
                c.file.name
            );
        }
        bail!(msg)
    }

    /// Everything a Nexus mod requires: its mod page's list (GraphQL, cached 15 minutes) plus
    /// the file-level dependencies of upload `version_id` (v3), if known; `refresh` re-fetches
    /// the latter. Returns the external (non-Nexus) requirements for display, and the list to
    /// store on the mod: Nexus mods only, each once, never the mod itself.
    pub fn requirements(&self, gid: u64, version_id: &str, refresh: bool) -> Result<(Vec<Req>, Vec<Requirement>)> {
        let reqs = self.nx.requirements(&[gid], false)?.into_iter().next().map(|x| x.2).unwrap_or_default();
        let mut stored: Vec<Requirement> = reqs
            .iter()
            .filter(|r| !r.external && r.mod_id != 0)
            .map(|r| Requirement { mod_id: r.mod_id, name: r.name.clone(), notes: r.notes.clone(), optional: r.optional() })
            .collect();
        if !version_id.is_empty() {
            for (dgid, dname, minv) in self.nx.file_deps(version_id, refresh).unwrap_or_default() {
                if dgid != 0 && dgid != gid && !stored.iter().any(|r| r.mod_id == dgid) {
                    stored.push(Requirement {
                        mod_id: dgid,
                        name: dname,
                        notes: format!("file dependency, min version {minv}"),
                        optional: false,
                    });
                }
            }
        }
        Ok((reqs.into_iter().filter(|r| r.external).collect(), stored))
    }

    /// Is there an update for this installed Nexus file? Only the installed file is compared
    /// (a page's other files are separate mods), against its newest *current* upload.
    pub fn update(&self, n: &NexusSource) -> Result<Update> {
        let info = self.nx.mod_info(n.mod_id)?;
        if info.status != "published" && !info.status.is_empty() {
            return Ok(Update::PageStatus(info.status));
        }
        if n.file_id.is_empty() {
            return Ok(Update::FileUnknown { page_version: info.version });
        }
        let vs = self.nx.versions(&n.file_id)?;
        let Some(newest) = vs.first() else { return Ok(Update::NoVersions) };
        let ours = vs.iter().find(|v| v.id == n.version_id || (n.version_id.is_empty() && v.uploaded_at == n.uploaded_at));
        if ours.is_some_and(|o| o.id == newest.id) {
            if newest.category == "archived" {
                return Ok(Update::Archived { page_version: info.version });
            }
            return Ok(Update::UpToDate);
        }
        Ok(Update::Available(Box::new(newest.clone())))
    }

    /// [`Self::update`] for many mods, in order, on at most 6 threads.
    pub fn updates(&self, sources: &[&NexusSource]) -> Vec<Result<Update>> {
        let mut out = vec![];
        std::thread::scope(|s| {
            let hs: Vec<_> = sources
                .chunks(sources.len().div_ceil(6).max(1))
                .map(|chunk| s.spawn(move || chunk.iter().map(|n| self.update(n)).collect::<Vec<_>>()))
                .collect();
            for h in hs {
                out.extend(h.join().unwrap());
            }
        });
        out
    }

    /// The manifest of an upload: archive size and hash, and each file's hash.
    pub fn manifest(&self, v: &Version) -> Result<Manifest> {
        self.nx.manifest(v.manifest_url.as_deref().context("version has no manifest")?)
    }

    /// Where an upload's archive lives in the downloads dir: `<version gsid>-<file name slug>`.
    pub fn archive_path(&self, v: &Version) -> PathBuf {
        self.paths.downloads().join(format!("{}-{}", v.game_scoped_id, slugify(&v.file.name)))
    }

    /// Download an upload to [`Self::archive_path`]; an archive already there with the
    /// manifest's hash is reused. `nxm` is the key/expires of a "Mod Manager Download" link
    /// (lets free accounts download).
    pub fn fetch_archive(&self, v: &Version, man: &Manifest, nxm: Option<(&str, &str)>) -> Result<PathBuf> {
        let arc = self.archive_path(v);
        if let Some(dir) = arc.parent() {
            fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        self.nx.download(v, man, &arc, nxm)?;
        Ok(arc)
    }

    /// A collection (slug or URL, optionally with a revision): its revision info from Nexus,
    /// and its manifest from the revision's archive, which is downloaded and extracted once
    /// per revision. Returns (slug, revision, manifest, extracted dir).
    pub fn collection(&self, reference: &str) -> Result<(String, Value, crate::collections::Manifest, PathBuf)> {
        let (slug, rev) = crate::collections::parse_ref(reference)?;
        let rv = self.nx.collection_revision(&slug, rev)?;
        let n = rv["revisionNumber"].as_u64().unwrap_or(0);
        let dir = self.nx.cache.join("collections").join(format!("{slug}-{n}"));
        if !dir.join("collection.json").is_file() {
            fs::create_dir_all(&dir)?;
            let arc = dir.join("collection.7z");
            self.nx.download_collection(rv["downloadLink"].as_str().context("no download link for this collection")?, &arc)?;
            crate::install::extract(&arc, &dir)?;
            let _ = fs::remove_file(&arc);
        }
        let man = crate::collections::read_manifest(&dir)?;
        Ok((slug, rv, man, dir))
    }
}

/// The Nexus mod id in what a user gives: the id itself or a mod page URL.
pub fn mod_id(s: &str) -> Option<u64> {
    if let Ok(n) = s.parse() {
        return Some(n);
    }
    let i = s.find("cyberpunk2077/mods/")?;
    s[i + 19..].split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()
}

/// Nexus names manual downloads `<Name>-<modid>-<version>-<unix time>.<ext>`.
fn download_re() -> regex::Regex {
    regex::Regex::new(r"^(.+?)-(\d+)-(?:[\w.]+-)*?(\d{9,11})$").unwrap()
}

/// The mod id in a manual download's file stem.
pub fn mod_id_from_download(stem: &str) -> Option<u64> {
    download_re().captures(stem).and_then(|c| c[2].parse().ok())
}

/// The mod's name in a download's file stem: the name part of a Nexus manual download, else
/// the whole stem; underscores read as spaces.
pub fn download_name(stem: &str) -> String {
    match download_re().captures(stem) {
        Some(c) => c[1].replace('_', " "),
        None => stem.replace('_', " "),
    }
}

#[cfg(test)]
mod tests {
    use super::super::fake;
    use super::*;
    use serde_json::json;

    fn setup(name: &str) -> (Paths, PathBuf) {
        let dir = fake::temp_dir(name);
        let paths = Paths { data: dir.join("data"), game: dir.join("game"), cfg: crate::settings::Settings::default() };
        (paths, dir)
    }
    fn cat<'p>(paths: &'p Paths, dir: &Path, srv: &fake::Running) -> Catalogue<'p> {
        Catalogue::over(paths, Some("k".into()), &dir.join("cache"), srv.endpoints())
    }
    fn page(gsid: u64, status: &str, primary: &str) -> Value {
        json!({"data": {"id": format!("u{gsid}"), "game_scoped_id": gsid.to_string(), "name": "Curbside", "version": "1.0.1",
            "status": status, "primary_file_version": {"id": primary}}})
    }
    fn ver(id: &str, file: &str, category: &str, version: &str, at: &str) -> Value {
        json!({"id": id, "file": {"id": file, "name": format!("{file} file")}, "game_scoped_id": format!("9{id}"), "name": "",
            "version": version, "category": category, "uploaded_at": at, "manifest_url": format!("/manifests/{id}")})
    }
    fn versions(vs: Vec<Value>) -> Value {
        json!({"data": {"versions": vs}})
    }
    fn files(fs: &[(&str, &str, bool)]) -> Value {
        json!({"data": {"mod_files": fs.iter().map(|(id, name, a)| json!({"id": id, "name": name, "is_active": a})).collect::<Vec<_>>()}})
    }

    /// One main file among optional ones: that's the install, without asking.
    #[test]
    fn picks_the_only_main_file() {
        let srv = fake::Server::new()
            .json("GET /v3/games/cyberpunk2077/mods/35922", page(35922, "published", "x"))
            .json("GET /v3/mods/u35922/files", files(&[("f1", "Car", true), ("f2", "Tuning", true), ("f3", "Old", false)]))
            .json("GET /v3/mod-files/f1/versions", versions(vec![ver("a", "f1", "main", "1.0.1", "2026-10-27")]))
            .json("GET /v3/mod-files/f2/versions", versions(vec![ver("b", "f2", "optional", "1.0", "2026-10-27")]))
            .start();
        let (paths, dir) = setup("pick-main");
        let (info, v) = cat(&paths, &dir, &srv).pick(35922, None).unwrap();
        assert_eq!((info.name.as_str(), v.id.as_str()), ("Curbside", "a"));
        assert!(!srv.requests().iter().any(|r| r.line.contains("/f3/")), "inactive files are never listed");
    }

    /// Two main files: the page's primary one; with no primary among them, the error lists
    /// the candidates for `--file`. A name fragment (any case) picks one.
    #[test]
    fn several_main_files_need_the_primary_or_a_choice() {
        let mk = |primary: &str| {
            fake::Server::new()
                .json("GET /v3/games/cyberpunk2077/mods/7", page(7, "published", primary))
                .json("GET /v3/mods/u7/files", files(&[("f1", "Lite", true), ("f2", "Full", true)]))
                .json("GET /v3/mod-files/f1/versions", versions(vec![ver("a", "f1", "main", "1", "2026-10-01")]))
                .json("GET /v3/mod-files/f2/versions", versions(vec![ver("b", "f2", "main", "1", "2026-10-02")]))
                .start()
        };
        let srv = mk("b");
        let (paths, dir) = setup("pick-primary");
        assert_eq!(cat(&paths, &dir, &srv).pick(7, None).unwrap().1.id, "b");
        let srv = mk("none");
        let (paths, dir) = setup("pick-ambiguous");
        let c = cat(&paths, &dir, &srv);
        let e = c.pick(7, None).unwrap_err().to_string();
        assert!(e.contains("several files") && e.contains("9a") && e.contains("9b"), "{e}");
        assert_eq!(c.pick(7, Some("F2 FILE")).unwrap().1.id, "b");
        let e = c.pick(7, Some("file")).unwrap_err().to_string();
        assert!(e.contains("matched 2 files"), "{e}");
    }

    /// A numeric `--file` that is an upload's id (collections, nxm links) is looked up
    /// directly: two requests, not one per file. An upload from another mod is refused.
    #[test]
    fn version_id_selection_is_one_lookup() {
        let srv = fake::Server::new()
            .json("GET /v3/games/cyberpunk2077/mods/7", page(7, "published", ""))
            .json("GET /v3/mods/u7/files", files(&[("f1", "Lite", true), ("f2", "Full", true)]))
            .json("GET /v3/games/cyberpunk2077/mod-file-versions/123", json!({"data": ver("old", "f2", "old_version", "0.9", "2025")}))
            .json("GET /v3/games/cyberpunk2077/mod-file-versions/456", json!({"data": ver("x", "other", "main", "1", "2025")}))
            .json("GET /v3/mod-files/f1/versions", versions(vec![ver("a", "f1", "main", "1", "2026")]))
            .json("GET /v3/mod-files/f2/versions", versions(vec![ver("b", "f2", "main", "1", "2026")]))
            .start();
        let (paths, dir) = setup("pick-gsid");
        let c = cat(&paths, &dir, &srv);
        assert_eq!(c.pick(7, Some("123")).unwrap().1.id, "old");
        assert_eq!(srv.requests().len(), 3, "page, file list, one version lookup");
        let e = c.pick(7, Some("456")).unwrap_err().to_string();
        assert!(e.contains("does not belong to Curbside"), "{e}");
    }

    /// Stored requirements: the page's Nexus requirements (external ones and id 0 left out)
    /// merged with the file's dependencies (each mod once, never the mod itself).
    #[test]
    fn requirements_merge_page_and_file_dependencies() {
        let page_reqs = json!({"data": {"legacyModsByDomain": {"nodes": [{"modId": 7, "name": "Curbside", "modRequirements": {"nexusRequirements": {"nodes": [
            {"modId": "107", "modName": "Cyber Engine Tweaks", "url": "", "notes": "", "externalRequirement": false},
            {"modId": "1511", "modName": "Codeware", "url": "", "notes": "optional, for the menu", "externalRequirement": false},
            {"modId": "0", "modName": "Red4ext", "url": "https://github.com/x", "notes": "", "externalRequirement": true},
            {"modId": "0", "modName": "nameless", "url": "", "notes": "", "externalRequirement": false},
        ]}}}]}}});
        let target = |gsid: u64, name: &str, min: &str| json!({"target_mod_file": {"mod": {"game_scoped_id": gsid.to_string(), "name": name}}, "min_version": {"version": min}});
        let deps = json!({"dependency_definitions": [{"ranges": [target(107, "CET", "1.35"), target(7, "Curbside", "1"), target(2380, "ArchiveXL", "1.5")]}]});
        let srv = fake::Server::new()
            .json("POST /v2/graphql ~legacyModsByDomain", page_reqs)
            .json("GET /v3/mod-file-versions/v1/dependencies", deps)
            .start();
        let (paths, dir) = setup("reqs");
        let c = cat(&paths, &dir, &srv);
        let (external, stored) = c.requirements(7, "v1", false).unwrap();
        assert_eq!(external.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["Red4ext"]);
        let got: Vec<(u64, bool, &str)> = stored.iter().map(|r| (r.mod_id, r.optional, r.notes.as_str())).collect();
        assert_eq!(got, [(107, false, ""), (1511, true, "optional, for the menu"), (2380, false, "file dependency, min version 1.5")]);
        // no upload known (a local archive tagged with a mod): the page's list only
        let (_, stored) = c.requirements(7, "", false).unwrap();
        assert_eq!(stored.len(), 2);
        let posts: Vec<_> = srv.requests().into_iter().filter(|r| r.line.starts_with("POST")).collect();
        assert_eq!(posts.len(), 1, "the page list is cached");
        assert!(posts[0].body.contains(r#""gameDomain":"cyberpunk2077","modId":7"#), "{}", posts[0].body);
    }

    fn installed(file: &str, version: &str) -> NexusSource {
        NexusSource { mod_id: 7, file_id: file.into(), version_id: version.into(), ..Default::default() }
    }

    /// Update detection compares the installed upload with the file's newest current one.
    /// Regression: Curbside's archived test upload "1awaw" had the highest `position` and was
    /// installed by `update --all`, crashing the game on car summon.
    #[test]
    fn detects_updates_of_the_installed_file() {
        let mut archived = ver("1awaw", "f1", "archived", "1awaw", "2026-10-28");
        archived["position"] = json!("9");
        let srv = fake::Server::new()
            .json("GET /v3/games/cyberpunk2077/mods/7", page(7, "published", ""))
            .json(
                "GET /v3/mod-files/f1/versions",
                versions(vec![
                    archived,
                    ver("new", "f1", "main", "1.0.1", "2026-10-27"),
                    ver("old", "f1", "old_version", "1.0.0", "2026-10-26"),
                ]),
            )
            .json("GET /v3/mod-files/f2/versions", versions(vec![ver("gone", "f2", "archived", "2", "2026-01-01")]))
            .json("GET /v3/mod-files/f3/versions", versions(vec![]))
            .start();
        let (paths, dir) = setup("updates");
        let c = cat(&paths, &dir, &srv);
        let (old, new, gone, empty, unknown) =
            (installed("f1", "old"), installed("f1", "new"), installed("f2", "gone"), installed("f3", "x"), installed("", ""));
        let r = c.updates(&[&old, &new, &gone, &empty, &unknown]);
        assert!(matches!(&r[0], Ok(Update::Available(v)) if v.id == "new"), "{:?}", r[0]);
        assert!(matches!(r[1], Ok(Update::UpToDate)), "{:?}", r[1]);
        assert!(matches!(&r[2], Ok(Update::Archived { page_version }) if page_version == "1.0.1"), "{:?}", r[2]);
        assert!(matches!(r[3], Ok(Update::NoVersions)), "{:?}", r[3]);
        assert!(matches!(&r[4], Ok(Update::FileUnknown { page_version }) if page_version == "1.0.1"), "{:?}", r[4]);
    }

    /// A hidden page is reported as such; `uncached` asks Nexus again instead of using the
    /// hour-long metadata cache.
    #[test]
    fn hidden_pages_and_uncached_checks() {
        let srv = fake::Server::new().json("GET /v3/games/cyberpunk2077/mods/7", page(7, "hidden", "")).start();
        let (paths, dir) = setup("hidden");
        let n = installed("f1", "a");
        assert!(matches!(cat(&paths, &dir, &srv).update(&n), Ok(Update::PageStatus(s)) if s == "hidden"));
        cat(&paths, &dir, &srv).update(&n).unwrap();
        assert_eq!(srv.requests().len(), 1, "answered from the cache");
        cat(&paths, &dir, &srv).uncached().update(&n).unwrap();
        assert_eq!(srv.requests().len(), 2);
    }

    /// The archive lands in the downloads dir under its upload id and file name, and is reused
    /// while its hash matches the manifest (no download link is requested again).
    #[test]
    fn fetches_archives_into_downloads_once() {
        use sha2::Digest;
        let body = b"PK fake archive".to_vec();
        let sha = hex::encode(sha2::Sha256::digest(&body));
        let srv0 = fake::Server::new().reply("GET /cdn/a.zip", 200, "", body.clone()).start();
        let link = json!({"data": {"links": [{"url": format!("{}/cdn/a.zip", srv0.base)}]}});
        let man = json!({"archive": {"size": body.len(), "hashes": {"SHA256": sha}}, "files": []});
        let srv = fake::Server::new()
            .json("GET /manifests/a", man)
            .json("GET /v3/games/cyberpunk2077/mod-file-versions/9a/download-link", link)
            .start();
        let (paths, dir) = setup("fetch");
        let c = cat(&paths, &dir, &srv);
        let mut v: Version = serde_json::from_value(ver("a", "f1", "main", "1", "2026")).unwrap();
        v.file.name = "Curbside Main".into();
        v.manifest_url = Some(format!("{}/manifests/a", srv.base));
        let m = c.manifest(&v).unwrap();
        assert!(!c.archive_path(&v).exists());
        let arc = c.fetch_archive(&v, &m, None).unwrap();
        assert_eq!(arc, paths.downloads().join("9a-curbside-main"));
        assert_eq!(c.archive_path(&v), arc);
        assert_eq!(fs::read(&arc).unwrap(), body);
        c.fetch_archive(&v, &m, None).unwrap();
        assert_eq!(srv.requests().len(), 2, "manifest and one download link");
        assert_eq!(srv0.requests().len(), 1);
    }

    #[test]
    fn mod_ids_from_ids_urls_and_download_names() {
        assert_eq!(mod_id("107"), Some(107));
        assert_eq!(mod_id("https://www.nexusmods.com/cyberpunk2077/mods/35922?tab=files"), Some(35922));
        assert_eq!(mod_id("~/mods/thing.zip"), None);
        assert_eq!(mod_id_from_download("Curbside-35922-1-0-1-1761517298"), Some(35922));
        assert_eq!(mod_id_from_download("Cyber Engine Tweaks-107-1-38-1-1761442138"), Some(107));
        assert_eq!(mod_id_from_download("my_custom_mod"), None);
        assert_eq!(download_name("Cyber_Engine_Tweaks-107-1-38-1-1761442138"), "Cyber Engine Tweaks");
        assert_eq!(download_name("random"), "random");
        assert_eq!(download_name("my_custom_mod"), "my custom mod");
    }
}
