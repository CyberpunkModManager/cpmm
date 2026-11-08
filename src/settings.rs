//! User settings (`settings.json` in the data dir). Every field has a default, so older or
//! partial files keep working. The GUI renders its Settings page from [`schema`].

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, ts_rs::TS)]
#[serde(default)]
pub struct Settings {
    // Updates
    pub check_updates_on_start: bool,
    pub auto_install_updates: bool,
    pub refresh_minutes: u32,
    // Load order
    pub auto_sort: bool,
    // After installing a mod
    pub after_install_verify: bool,
    pub after_install_doctor: bool,
    pub after_install_command: String,
    // Downloads
    pub auto_install_downloads: bool,
    pub keep_archives: bool,
    // Storage
    pub staging_dir: String,
    pub download_dir: String,
    // Profiles
    pub profiles_enabled: bool,
    pub new_profiles_own_saves: bool,
    // Safety
    pub snapshot_saves: bool,
    pub keep_versions: usize,
    pub keep_save_snapshots: usize,
    // Launching
    pub close_on_launch: bool,
    // First-run
    pub onboarded: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            check_updates_on_start: true,
            auto_install_updates: false,
            refresh_minutes: 60,
            auto_sort: true,
            after_install_verify: false,
            after_install_doctor: true,
            after_install_command: String::new(),
            auto_install_downloads: false,
            keep_archives: true,
            staging_dir: String::new(),
            download_dir: String::new(),
            profiles_enabled: true,
            new_profiles_own_saves: false,
            snapshot_saves: true,
            keep_versions: 3,
            keep_save_snapshots: 10,
            close_on_launch: true,
            onboarded: false,
        }
    }
}

pub fn file(data: &Path) -> PathBuf {
    data.join("settings.json")
}

pub fn load(data: &Path) -> Settings {
    fs::read(file(data)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

pub fn save(data: &Path, s: &Settings) -> Result<()> {
    fs::create_dir_all(data)?;
    let tmp = data.join("settings.json.tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(s)?)?;
    fs::rename(tmp, file(data))?;
    Ok(())
}

/// `~` expansion for user-entered paths; empty means "use the default".
pub fn expand(p: &str) -> Option<PathBuf> {
    let p = p.trim();
    if p.is_empty() {
        return None;
    }
    Some(match p.strip_prefix("~/") {
        Some(r) => PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(r),
        None => PathBuf::from(p),
    })
}

/// Set one field from a string, validated through serde (so types and ranges stay right).
pub fn set(s: &Settings, key: &str, raw: &str) -> Result<Settings> {
    let mut v = serde_json::to_value(s)?;
    let obj = v.as_object_mut().unwrap();
    let cur = obj.get(key).with_context(|| format!("unknown setting {key:?} (see `cpmm settings`)"))?;
    let new = match cur {
        Value::Bool(_) => match raw.to_lowercase().as_str() {
            "true" | "on" | "yes" | "1" => Value::Bool(true),
            "false" | "off" | "no" | "0" => Value::Bool(false),
            _ => bail!("{key} is on/off"),
        },
        Value::Number(_) => {
            let n: u64 = raw.parse().with_context(|| format!("{key} is a number"))?;
            json!(n)
        }
        _ => Value::String(raw.to_string()),
    };
    obj.insert(key.into(), new);
    let out: Settings = serde_json::from_value(v)?;
    if out.refresh_minutes == 0 {
        bail!("refresh_minutes must be at least 1");
    }
    if out.keep_versions == 0 || out.keep_save_snapshots == 0 {
        bail!("keep at least 1");
    }
    Ok(out)
}

/// One setting as the GUI's Settings page shows it.
#[derive(Serialize, Clone, Debug, ts_rs::TS)]
#[ts(rename = "SettingField")]
pub struct Field {
    #[ts(type = "keyof Settings")]
    pub key: &'static str,
    pub group: &'static str,
    pub label: &'static str,
    pub help: &'static str,
    #[ts(type = "\"bool\" | \"number\" | \"text\" | \"path\"")]
    pub kind: &'static str,
}

/// Field metadata for the GUI's Settings page and the onboarding flow.
pub fn schema() -> Vec<Field> {
    let f = |key, group, label, help, kind| Field { key, group, label, help, kind };
    vec![
        f(
            "check_updates_on_start",
            "Updates",
            "Check for updates on start",
            "Ask Nexus for newer uploads of your installed files when cpmm opens.",
            "bool",
        ),
        f(
            "auto_install_updates",
            "Updates",
            "Install updates automatically",
            "Install them right away (never while the game runs). Every run can be undone from Updates.",
            "bool",
        ),
        f(
            "refresh_minutes",
            "Updates",
            "Refresh every (minutes)",
            "How often cpmm re-checks Nexus and the download folder while open.",
            "number",
        ),
        f(
            "auto_sort",
            "Load order",
            "Sort automatically",
            "Re-sort the load order after installs, updates, enabling and rollbacks.",
            "bool",
        ),
        f("after_install_verify", "After installing", "Verify files", "Check the staged files against the Nexus manifest hashes.", "bool"),
        f(
            "after_install_doctor",
            "After installing",
            "Run the health check",
            "Report new crash risks or conflicts the mod introduced.",
            "bool",
        ),
        f(
            "after_install_command",
            "After installing",
            "Run a command",
            "Shell command run after each install; gets CPMM_MOD_ID, CPMM_MOD_NAME, CPMM_MOD_VERSION.",
            "text",
        ),
        f(
            "auto_install_downloads",
            "Downloads",
            "Install downloads automatically",
            "Install a mod as soon as its download finishes (Mod Manager Download links, the built-in browser). Off: it waits in Downloads until you install it.",
            "bool",
        ),
        f(
            "keep_archives",
            "Downloads",
            "Keep archives after installing",
            "Installed mods stay in the Downloads list, so you can reinstall or switch FOMOD options without downloading again. Off saves disk space.",
            "bool",
        ),
        f(
            "staging_dir",
            "Storage",
            "Mod staging directory",
            "Where installed mods live. Default: ~/.local/share/cpmm/mods. Best on the same drive as the game. Changing it moves your mods.",
            "path",
        ),
        f(
            "download_dir",
            "Storage",
            "Download directory",
            "Where cpmm keeps archives while installing. Default: ~/.local/share/cpmm/downloads",
            "path",
        ),
        f("profiles_enabled", "Profiles", "Enable profiles", "Separate mod lists you can switch between (shown in the sidebar).", "bool"),
        f(
            "new_profiles_own_saves",
            "Profiles",
            "New profiles get their own saves",
            "Each new profile starts with a separate save folder (not synced by Steam Cloud).",
            "bool",
        ),
        f(
            "snapshot_saves",
            "Safety",
            "Snapshot saves before changes",
            "Before disabling, uninstalling, updating or rolling back mods, and before a tool that can edit saves runs. Copy-on-write, so free on btrfs.",
            "bool",
        ),
        f("keep_versions", "Safety", "Earlier versions to keep per mod", "For rollback.", "number"),
        f("keep_save_snapshots", "Safety", "Save snapshots to keep", "Oldest are removed first.", "number"),
        f(
            "close_on_launch",
            "Launching",
            "Close cpmm when launching the game",
            "Recommended in Steam Game Mode, which allows one running game at a time.",
            "bool",
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_files_and_typed_set() {
        let s: Settings = serde_json::from_str(r#"{"auto_sort": false}"#).unwrap();
        assert!(!s.auto_sort && s.check_updates_on_start && s.keep_versions == 3);
        let s = set(&s, "keep_versions", "5").unwrap();
        assert_eq!(s.keep_versions, 5);
        let s = set(&s, "auto_install_updates", "on").unwrap();
        assert!(s.auto_install_updates);
        assert!(set(&s, "keep_versions", "0").is_err());
        assert!(set(&s, "nope", "1").is_err());
        assert!(set(&s, "auto_sort", "maybe").is_err());
    }

    #[test]
    fn schema_covers_every_field() {
        let keys: Vec<String> = schema().iter().map(|f| f.key.to_string()).collect();
        let fields = serde_json::to_value(Settings::default()).unwrap();
        for k in fields.as_object().unwrap().keys().filter(|k| *k != "onboarded") {
            assert!(keys.contains(k), "{k} missing from schema");
        }
    }
}
