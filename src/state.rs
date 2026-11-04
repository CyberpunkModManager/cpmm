//! Persistent state: where things live, and which mods are installed in what order.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

pub const STEAM_APP_ID: &str = "1091500";

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").expect("HOME not set"))
}

/// Filesystem layout. Everything under `data` must be on the same filesystem as the game
/// so that importing is a rename and overlayfs upper/work dirs are valid.
#[derive(Clone)]
pub struct Paths {
    pub data: PathBuf,
    pub game: PathBuf,
    /// User settings (settings.json); decides the staging and download folders.
    pub cfg: crate::settings::Settings,
}

impl Paths {
    pub fn load() -> Result<Self> {
        let data = std::env::var_os("CPMM_DATA").map(PathBuf::from).unwrap_or_else(|| home().join(".local/share/cpmm"));
        let game = match fs::read_to_string(data.join("game_dir")) {
            Ok(s) => PathBuf::from(s.trim()),
            Err(_) => home().join(".local/share/Steam/steamapps/common/Cyberpunk 2077"),
        };
        let cfg = crate::settings::load(&data);
        Ok(Self { data, game, cfg })
    }
    pub fn state_file(&self) -> PathBuf {
        self.data.join("state.json")
    }
    pub fn mods(&self) -> PathBuf {
        crate::settings::expand(&self.cfg.staging_dir).unwrap_or_else(|| self.data.join("mods"))
    }
    pub fn mod_files(&self, id: &str) -> PathBuf {
        self.mods().join(id).join("files")
    }
    pub fn upper(&self) -> PathBuf {
        self.data.join("overwrite")
    }
    pub fn work(&self) -> PathBuf {
        self.data.join("work")
    }
    pub fn tmp(&self) -> PathBuf {
        self.data.join("tmp")
    }
    pub fn trash(&self) -> PathBuf {
        self.data.join("trash")
    }
    /// A fresh, unused trash path for `name` (timestamped, suffixed if needed).
    pub fn trash_for(&self, name: &str) -> PathBuf {
        let base = format!("{name}-{}", now().replace(':', ""));
        let mut p = self.trash().join(&base);
        let mut n = 2;
        while p.exists() {
            p = self.trash().join(format!("{base}-{n}"));
            n += 1;
        }
        p
    }
    pub fn downloads(&self) -> PathBuf {
        crate::settings::expand(&self.cfg.download_dir).unwrap_or_else(|| self.data.join("downloads"))
    }
    pub fn run_file(&self) -> PathBuf {
        self.data.join("run.json")
    }
    /// Save folders of profiles that don't use the game's shared one.
    pub fn profile_saves(&self) -> PathBuf {
        self.data.join("profile-saves")
    }
    pub fn cache(&self) -> PathBuf {
        std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".cache")).join("cpmm")
    }
    pub fn api_key(&self) -> Result<String> {
        let p = crate::nexus::key_file();
        let k = fs::read_to_string(&p).with_context(|| format!("reading Nexus API key {}", p.display()))?;
        Ok(k.trim().to_string())
    }
    pub fn modio_dir(&self) -> PathBuf {
        // compatdata sits next to common/ in the same Steam library
        let lib = self.game.parent().and_then(|p| p.parent()).map(Path::to_path_buf).unwrap_or_default();
        lib.join("compatdata").join(STEAM_APP_ID).join("pfx/drive_c/users/Public/mod.io/5784")
    }
    pub fn ensure_dirs(&self) -> Result<()> {
        for d in [self.mods(), self.upper(), self.work(), self.tmp(), self.trash(), self.downloads(), self.cache()] {
            fs::create_dir_all(&d).with_context(|| format!("creating {}", d.display()))?;
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct NexusSource {
    /// Game-scoped mod id, the number in the mod page URL.
    pub mod_id: u64,
    /// v3 mod uid.
    #[serde(default)]
    pub mod_uid: String,
    /// v3 mod file id (stable across uploads of the same file).
    #[serde(default)]
    pub file_id: String,
    #[serde(default)]
    pub file_name: String,
    /// v3 mod file version id (one specific upload).
    #[serde(default)]
    pub version_id: String,
    /// Game-scoped id of that upload (used for download links).
    #[serde(default)]
    pub version_gsid: String,
    #[serde(default)]
    pub uploaded_at: String,
    #[serde(default)]
    pub manifest_url: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Requirement {
    pub mod_id: u64,
    pub name: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub optional: bool,
}

/// When a mod's requirements were last fetched from Nexus, and for which installed version.
/// They stay valid until the mod is updated/reinstalled or `cpmm deps --refresh` re-checks.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ReqCheck {
    pub at: String,
    /// `NexusSource::version_id` at check time ("" for local archives tagged with --nexus)
    pub version_id: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Mod {
    /// Short slug; also the staging directory name.
    pub id: String,
    pub name: String,
    pub enabled: bool,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub nexus: Option<NexusSource>,
    #[serde(default)]
    pub requires: Vec<Requirement>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reqs_checked: Option<ReqCheck>,
    /// lower-cased paths the user hid (kept across updates)
    #[serde(default)]
    pub hidden: Vec<String>,
    /// Requirements the user decided to do without (Nexus mod ids)
    #[serde(default)]
    pub ignored_reqs: Vec<u64>,
    /// FOMOD installer choices ("step/group" -> option names), reused when the mod updates
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fomod: Option<serde_json::Value>,
    /// The download (file name in the downloads folder) this mod was installed from
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive: Option<String>,
    #[serde(default)]
    pub installed_at: String,
    #[serde(default)]
    pub notes: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct InstalledCollection {
    pub slug: String,
    pub name: String,
    pub revision: u64,
    #[serde(default)]
    pub profile: String,
    /// cpmm mod ids that came from this collection
    pub mods: Vec<String>,
    pub installed_at: String,
}

#[derive(Serialize, Deserialize, Default, Debug)]
pub struct State {
    pub version: u32,
    /// Priority order: later entries win file conflicts (they are the upper overlay layers).
    pub mods: Vec<Mod>,
    /// Load order rules from the user ("X after Y"), enforced by `cpmm sort`.
    #[serde(default)]
    pub rules: Vec<crate::loadorder::Rule>,
    /// Nexus collections installed through cpmm.
    #[serde(default)]
    pub collections: Vec<InstalledCollection>,
    /// Doctor findings the user marked as known/accepted (by title).
    #[serde(default)]
    pub acked: Vec<String>,
    /// Mod list profiles (see profiles.rs); `mods`/`rules` above are the active one's.
    #[serde(default)]
    pub profiles: Vec<crate::profiles::Profile>,
    #[serde(default)]
    pub active_profile: String,
}

impl State {
    pub fn load(p: &Paths) -> Result<Self> {
        match fs::read(p.state_file()) {
            Ok(b) => {
                let mut st: State = serde_json::from_slice(&b).context("parsing state.json")?;
                st.ensure_profiles();
                Ok(st)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let mut st = State {
                    version: 1,
                    mods: vec![],
                    rules: vec![],
                    acked: vec![],
                    profiles: vec![],
                    active_profile: String::new(),
                    collections: vec![],
                };
                st.ensure_profiles();
                Ok(st)
            }
            Err(e) => Err(e.into()),
        }
    }
    pub fn save(&self, p: &Paths) -> Result<()> {
        fs::create_dir_all(&p.data)?;
        let tmp = p.data.join("state.json.tmp");
        let mut v = serde_json::to_value(self)?;
        v["profiles"] = serde_json::to_value(self.synced_profiles())?;
        fs::write(&tmp, serde_json::to_vec_pretty(&v)?)?;
        if p.state_file().exists() {
            let _ = fs::copy(p.state_file(), p.data.join("state.json.bak"));
        }
        fs::rename(tmp, p.state_file())?;
        Ok(())
    }
    pub fn find(&self, key: &str) -> Option<usize> {
        if let Some(i) = self.mods.iter().position(|m| m.id == key) {
            return Some(i);
        }
        if let Ok(n) = key.parse::<u64>()
            && let Some(i) = self.mods.iter().position(|m| m.nexus.as_ref().is_some_and(|x| x.mod_id == n))
        {
            return Some(i);
        }
        let lk = key.to_lowercase();
        let hits: Vec<usize> = (0..self.mods.len()).filter(|&i| self.mods[i].name.to_lowercase().contains(&lk)).collect();
        if hits.len() == 1 { Some(hits[0]) } else { None }
    }
    pub fn get(&self, key: &str) -> Result<usize> {
        match self.find(key) {
            Some(i) => Ok(i),
            None => bail!("no unique mod matches {key:?} (use the id from `cpmm list`)"),
        }
    }
    pub fn by_nexus(&self, mod_id: u64) -> Option<&Mod> {
        self.mods.iter().find(|m| m.nexus.as_ref().is_some_and(|x| x.mod_id == mod_id))
    }
    pub fn unique_id(&self, base: &str) -> String {
        let mut id = base.to_string();
        let mut n = 2;
        while self.mods.iter().any(|m| m.id == id) {
            id = format!("{base}-{n}");
            n += 1;
        }
        id
    }
}

pub fn slugify(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out = out.trim_end_matches('-');
    let out: String = out.chars().take(40).collect();
    if out.is_empty() { "mod".into() } else { out.trim_end_matches('-').to_string() }
}

pub fn now() -> String {
    // RFC3339-ish UTC timestamp without pulling in chrono.
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
    let (d, s) = (t.div_euclid(86400), t.rem_euclid(86400));
    // civil-from-days (Howard Hinnant)
    let z = d + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let dd = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{dd:02}T{:02}:{:02}:{:02}Z", s / 3600, (s / 60) % 60, s % 60)
}

/// "Highbeam - Headlights for 2.5" -> "Highbeam", "Mazda RX7 FD (Garage)" stays readable.
pub fn short_name(n: &str) -> &str {
    // "- House of Vesna - Actually Black Hair": leading dashes are decoration, and a series
    // prefix before " - " is not a useful short name when what follows names the mod.
    let n = n.trim_start_matches(['-', ' ']);
    if let Some(rest) = n.strip_prefix("House of Vesna - ") {
        return rest;
    }
    let cut = [" - ", " | ", " – ", " ("].iter().filter_map(|s| n.find(s)).min().unwrap_or(n.len());
    if cut >= 4 { &n[..cut] } else { n }
}
