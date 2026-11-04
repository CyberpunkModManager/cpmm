//! Mod list profiles. A profile is a mod list: which mods are enabled, their priority order
//! and the list's load-order rules. Staged files, hidden files, ignored requirements and the
//! overwrite dir are shared by all profiles (a mod is downloaded and staged once).
//!
//! The active profile is materialised in `State::mods` / `State::rules`, so every other
//! command works on it unchanged; `State::save` writes it back into its profile entry.
//! Switching saves the current list into its profile, then applies the target's. Mods the
//! target has never seen (installed while another profile was active) are added to it
//! disabled, at the top of the order.

use crate::loadorder::Rule;
use crate::state::{State, now};
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

pub const DEFAULT: &str = "Default";

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Profile {
    pub name: String,
    /// Mod ids in priority order (later wins).
    pub order: Vec<String>,
    /// Mod ids enabled in this profile.
    pub enabled: Vec<String>,
    #[serde(default)]
    pub rules: Vec<Rule>,
    #[serde(default)]
    pub created_at: String,
}

pub fn valid_name(name: &str) -> Result<String> {
    let n = name.trim();
    if n.is_empty() || n.chars().count() > 40 || n.chars().any(char::is_control) {
        bail!("profile names are 1–40 characters, no control characters");
    }
    Ok(n.to_string())
}

impl State {
    /// First run with profiles: the current list becomes "Default".
    pub fn ensure_profiles(&mut self) {
        if self.profiles.is_empty() {
            self.active_profile = DEFAULT.into();
            let p = self.snapshot(DEFAULT);
            self.profiles.push(p);
        }
        if !self.profiles.iter().any(|p| p.name == self.active_profile) {
            self.active_profile = self.profiles[0].name.clone();
        }
    }

    fn snapshot(&self, name: &str) -> Profile {
        Profile {
            name: name.into(),
            order: self.mods.iter().map(|m| m.id.clone()).collect(),
            enabled: self.mods.iter().filter(|m| m.enabled).map(|m| m.id.clone()).collect(),
            rules: self.rules.clone(),
            created_at: now(),
        }
    }

    /// Write the live list back into the active profile's entry.
    pub fn sync_active_profile(&mut self) {
        self.ensure_profiles();
        self.profiles = self.synced_profiles();
    }

    /// All profiles with the active one updated from the live list, and mods that are no
    /// longer installed dropped from every profile (what `State::save` writes).
    pub fn synced_profiles(&self) -> Vec<Profile> {
        let installed = |id: &String| self.mods.iter().any(|m| &m.id == id);
        let snap = self.snapshot(&self.active_profile);
        self.profiles
            .iter()
            .map(|p| {
                let mut p = p.clone();
                if p.name == snap.name {
                    (p.order, p.enabled, p.rules) = (snap.order.clone(), snap.enabled.clone(), snap.rules.clone());
                }
                p.order.retain(installed);
                p.enabled.retain(installed);
                p.rules.retain(|r| installed(&r.r#mod) && installed(&r.after));
                p
            })
            .collect()
    }

    pub fn profile_index(&self, name: &str) -> Result<usize> {
        if let Some(i) = self.profiles.iter().position(|p| p.name == name) {
            return Ok(i);
        }
        let hits: Vec<usize> = (0..self.profiles.len()).filter(|&i| self.profiles[i].name.eq_ignore_ascii_case(name)).collect();
        match hits.as_slice() {
            [i] => Ok(*i),
            _ => bail!("no profile {name:?} (cpmm profile list)"),
        }
    }

    /// Make `name` the active profile. Returns the mods it hadn't seen before.
    pub fn switch_profile(&mut self, name: &str) -> Result<Vec<String>> {
        self.sync_active_profile();
        let target = self.profiles[self.profile_index(name)?].clone();
        let mut pool: Vec<_> = std::mem::take(&mut self.mods);
        let mut mods = Vec::with_capacity(pool.len());
        for id in &target.order {
            if let Some(i) = pool.iter().position(|m| &m.id == id) {
                let mut m = pool.remove(i);
                m.enabled = target.enabled.contains(id);
                mods.push(m);
            }
        }
        let new: Vec<String> = pool.iter().map(|m| m.id.clone()).collect();
        for mut m in pool {
            m.enabled = false;
            mods.push(m);
        }
        self.mods = mods;
        let known = |id: &String| self.mods.iter().any(|m| &m.id == id);
        self.rules = target.rules.iter().filter(|r| known(&r.r#mod) && known(&r.after)).cloned().collect();
        self.active_profile = target.name.clone();
        self.sync_active_profile();
        Ok(new)
    }

    /// New profile: a copy of `from` (default: the active one), or an empty list (all
    /// installed mods present but disabled).
    pub fn create_profile(&mut self, name: &str, from: Option<&str>, empty: bool) -> Result<String> {
        self.sync_active_profile();
        let name = valid_name(name)?;
        if self.profiles.iter().any(|p| p.name.eq_ignore_ascii_case(&name)) {
            bail!("a profile called {name:?} already exists");
        }
        let src = self.profiles[self.profile_index(from.unwrap_or(&self.active_profile.clone()))?].clone();
        let p = Profile {
            name: name.clone(),
            order: src.order.clone(),
            enabled: if empty { vec![] } else { src.enabled.clone() },
            rules: if empty { vec![] } else { src.rules.clone() },
            created_at: now(),
        };
        self.profiles.push(p);
        Ok(name)
    }

    pub fn rename_profile(&mut self, old: &str, new: &str) -> Result<(String, String)> {
        self.sync_active_profile();
        let i = self.profile_index(old)?;
        let new = valid_name(new)?;
        if self.profiles.iter().enumerate().any(|(j, p)| j != i && p.name.eq_ignore_ascii_case(&new)) {
            bail!("a profile called {new:?} already exists");
        }
        let old = std::mem::replace(&mut self.profiles[i].name, new.clone());
        if self.active_profile == old {
            self.active_profile = new.clone();
        }
        Ok((old, new))
    }

    pub fn delete_profile(&mut self, name: &str) -> Result<String> {
        self.sync_active_profile();
        let i = self.profile_index(name)?;
        if self.profiles[i].name == self.active_profile {
            bail!("{:?} is the active profile; switch to another one first", self.profiles[i].name);
        }
        Ok(self.profiles.remove(i).name)
    }

    /// Keep every profile's references in step with a mod id rename.
    pub fn rename_mod_in_profiles(&mut self, old: &str, new: &str) {
        for p in self.profiles.iter_mut() {
            for id in p.order.iter_mut().chain(p.enabled.iter_mut()) {
                if id == old {
                    *id = new.to_string();
                }
            }
            for r in p.rules.iter_mut() {
                if r.r#mod == old {
                    r.r#mod = new.to_string();
                }
                if r.after == old {
                    r.after = new.to_string();
                }
            }
        }
    }

    /// Profiles (other than the active one) in which `id` is enabled.
    pub fn profiles_enabling(&self, id: &str) -> Vec<String> {
        self.profiles.iter().filter(|p| p.name != self.active_profile && p.enabled.iter().any(|e| e == id)).map(|p| p.name.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Mod;

    fn m(id: &str, enabled: bool) -> Mod {
        serde_json::from_value(serde_json::json!({"id": id, "name": id, "enabled": enabled})).unwrap()
    }

    fn st() -> State {
        let mut s: State = serde_json::from_value(serde_json::json!({"version": 1, "mods": []})).unwrap();
        s.mods = vec![m("a", true), m("b", false), m("c", true)];
        s.rules = vec![Rule { r#mod: "c".into(), after: "a".into() }];
        s.ensure_profiles();
        s
    }

    fn view(s: &State) -> Vec<String> {
        s.mods.iter().map(|m| format!("{}{}", m.id, if m.enabled { "+" } else { "-" })).collect()
    }

    #[test]
    fn migrates_current_list_to_default() {
        let s = st();
        assert_eq!(s.active_profile, DEFAULT);
        assert_eq!(s.profiles[0].order, ["a", "b", "c"]);
        assert_eq!(s.profiles[0].enabled, ["a", "c"]);
    }

    #[test]
    fn switching_round_trips_and_keeps_lists_apart() {
        let mut s = st();
        s.create_profile("Lean", None, true).unwrap();
        s.switch_profile("lean").unwrap(); // case-insensitive lookup
        assert_eq!(view(&s), ["a-", "b-", "c-"]);
        assert!(s.rules.is_empty());
        s.mods[1].enabled = true;
        s.mods.swap(0, 2); // reorder: c, b, a
        s.switch_profile(DEFAULT).unwrap();
        assert_eq!(view(&s), ["a+", "b-", "c+"]);
        assert_eq!(s.rules.len(), 1);
        s.switch_profile("Lean").unwrap();
        assert_eq!(view(&s), ["c-", "b+", "a-"]);
    }

    #[test]
    fn mods_installed_elsewhere_arrive_disabled_on_top() {
        let mut s = st();
        s.create_profile("Other", None, false).unwrap();
        s.mods.push(m("new", true)); // installed while Default is active
        s.switch_profile("Other").unwrap();
        assert_eq!(view(&s), ["a+", "b-", "c+", "new-"]);
        // removed mods drop out of every profile on the next switch
        s.mods.retain(|x| x.id != "b");
        s.switch_profile(DEFAULT).unwrap();
        assert_eq!(view(&s), ["a+", "c+", "new+"]);
    }

    #[test]
    fn names_and_guards() {
        let mut s = st();
        assert!(s.create_profile("default", None, false).is_err());
        assert!(s.create_profile("  ", None, false).is_err());
        assert!(s.delete_profile(DEFAULT).is_err());
        s.create_profile("X", None, false).unwrap();
        s.rename_profile(DEFAULT, "Main").unwrap();
        assert_eq!(s.active_profile, "Main");
        s.rename_mod_in_profiles("c", "c2");
        assert!(s.profiles.iter().all(|p| p.order.contains(&"c2".to_string()) && p.rules[0].r#mod == "c2"));
        assert_eq!(s.delete_profile("x").unwrap(), "X");
    }
}
