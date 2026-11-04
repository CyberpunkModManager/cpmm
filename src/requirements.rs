//! Requirements of installed mods: the status of each one, which mods depend on a mod, and
//! which mods' stored requirements need fetching again. The one place these rules live;
//! callers (CLI, `cpmm api`, doctor) only render what this returns.
//!
//! Fetching and merging requirements from Nexus is not here (see `nexus::Catalogue::requirements`).

use crate::state::{Mod, ReqCheck, Requirement, State};

/// Where one requirement of a mod stands.
///
/// The rule: a dependency that is installed and enabled is `Ok`; otherwise a requirement
/// the user chose to do without is `Ignored`; otherwise it is disabled or missing, and
/// optional requirements get their own variants because they are never a problem.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Ok,
    Ignored,
    Disabled,
    OptionalDisabled,
    Missing,
    OptionalMissing,
}

impl Status {
    /// The name `cpmm api` uses (gui/src/lib/cpmm.ts `ReqStatus`).
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Ok => "ok",
            Status::Ignored => "ignored",
            Status::Disabled => "disabled",
            Status::OptionalDisabled => "optional-disabled",
            Status::Missing => "missing",
            Status::OptionalMissing => "optional-missing",
        }
    }

    /// An unmet hard requirement: not optional, not ignored, and missing or disabled.
    pub fn is_problem(self) -> bool {
        matches!(self, Status::Disabled | Status::Missing)
    }
}

/// One requirement with its status and the installed mod that provides it, if any.
pub struct Check<'a> {
    pub req: &'a Requirement,
    pub status: Status,
    /// Set for `Ok` and the disabled variants; `Ignored` has it when the dependency is
    /// installed but disabled.
    pub dep: Option<&'a Mod>,
}

/// The status of each requirement in `reqs`, in order, given the requirements the user
/// ignored. For a mod that isn't stored yet (install time); use [`of`] for installed mods.
pub fn check<'a>(st: &'a State, reqs: &'a [Requirement], ignored: &[u64]) -> Vec<Check<'a>> {
    reqs.iter()
        .map(|req| {
            let dep = st.by_nexus(req.mod_id);
            let status = match dep {
                Some(d) if d.enabled => Status::Ok,
                _ if ignored.contains(&req.mod_id) => Status::Ignored,
                Some(_) if req.optional => Status::OptionalDisabled,
                Some(_) => Status::Disabled,
                None if req.optional => Status::OptionalMissing,
                None => Status::Missing,
            };
            Check { req, status, dep }
        })
        .collect()
}

/// The status of each stored requirement of an installed mod.
pub fn of<'a>(st: &'a State, m: &'a Mod) -> Vec<Check<'a>> {
    check(st, &m.requires, &m.ignored_reqs)
}

/// Enabled mods that list `m` as a hard (non-optional) requirement: the "required by" list
/// shown when removing or disabling `m`. Empty for mods without a Nexus id.
pub fn dependents<'a>(st: &'a State, m: &Mod) -> Vec<&'a Mod> {
    let Some(nid) = m.nexus.as_ref().map(|n| n.mod_id) else {
        return vec![];
    };
    st.mods.iter().filter(|x| x.enabled && x.requires.iter().any(|r| r.mod_id == nid && !r.optional)).collect()
}

/// Which mods' stored requirements need fetching from Nexus.
pub struct Stale {
    /// Indices into `st.mods`.
    pub mods: Vec<usize>,
    /// Old entries were stamped as checked; the state changed and should be saved.
    pub migrated: bool,
}

/// Nexus mods whose stored requirements are stale: never checked, checked for a different
/// installed version, or all of them with `refresh`.
///
/// Requirements stored before check stamps existed were fetched at install time, so those
/// are first stamped as checked at `installed_at` (not stale) and `migrated` is set.
pub fn stale(st: &mut State, refresh: bool) -> Stale {
    let mut migrated = false;
    for m in st.mods.iter_mut() {
        if let (None, Some(n)) = (&m.reqs_checked, &m.nexus)
            && !m.requires.is_empty()
        {
            m.reqs_checked = Some(ReqCheck { at: m.installed_at.clone(), version_id: n.version_id.clone() });
            migrated = true;
        }
    }
    let mods = st
        .mods
        .iter()
        .enumerate()
        .filter_map(|(i, m)| {
            let n = m.nexus.as_ref()?;
            (refresh || m.reqs_checked.as_ref().is_none_or(|c| c.version_id != n.version_id)).then_some(i)
        })
        .collect();
    Stale { mods, migrated }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A Nexus mod (`id`, Nexus id `nid`) checked for version "v1".
    fn m(id: &str, nid: u64, enabled: bool) -> Mod {
        serde_json::from_value(json!({"id": id, "name": id, "enabled": enabled,
            "nexus": {"mod_id": nid, "version_id": "v1"}, "reqs_checked": {"at": "t", "version_id": "v1"}}))
        .unwrap()
    }

    fn req(nid: u64, optional: bool) -> Requirement {
        Requirement { mod_id: nid, name: format!("#{nid}"), notes: String::new(), optional }
    }

    fn state(mods: Vec<Mod>) -> State {
        State { version: 1, mods, ..Default::default() }
    }

    fn statuses(st: &State, id: &str) -> Vec<Status> {
        of(st, &st.mods[st.get(id).unwrap()]).iter().map(|c| c.status).collect()
    }

    /// `user` requires 10 (enabled), 11 (disabled), 12 (not installed), the optional 13
    /// (disabled) and 14 (not installed); `ignore` lists the ones the user ignored.
    fn fixture(ignore: &[u64]) -> State {
        let mut user = m("user", 1, true);
        user.requires = vec![req(10, false), req(11, false), req(12, false), req(13, true), req(14, true)];
        user.ignored_reqs = ignore.to_vec();
        state(vec![user, m("on", 10, true), m("off", 11, false), m("opt-off", 13, false)])
    }

    #[test]
    fn status_of_each_requirement() {
        use Status::*;
        let st = fixture(&[]);
        assert_eq!(statuses(&st, "user"), [Ok, Disabled, Missing, OptionalDisabled, OptionalMissing]);
        let checks = of(&st, &st.mods[0]);
        let deps: Vec<_> = checks.iter().map(|c| c.dep.map(|d| d.id.as_str())).collect();
        assert_eq!(deps, [Some("on"), Some("off"), None, Some("opt-off"), None]);
    }

    #[test]
    fn ignored_counts_when_missing_or_disabled_but_not_when_met() {
        use Status::*;
        let st = fixture(&[10, 11, 12, 13, 14]);
        // the old GUI rule showed 11 and 13 as "disabled" (and counted them as problems)
        assert_eq!(statuses(&st, "user"), [Ok, Ignored, Ignored, Ignored, Ignored]);
        assert_eq!(of(&st, &st.mods[0])[1].dep.map(|d| d.id.as_str()), Some("off"));
    }

    #[test]
    fn only_hard_unmet_requirements_are_problems() {
        let problems = |st: &State| of(st, &st.mods[0]).iter().filter(|c| c.status.is_problem()).count();
        // 11 disabled + 12 missing; a disabled optional (13) is not a problem
        assert_eq!(problems(&fixture(&[])), 2);
        assert_eq!(problems(&fixture(&[11])), 1);
        assert_eq!(problems(&fixture(&[11, 12])), 0);
    }

    #[test]
    fn status_names_match_the_gui() {
        let names: Vec<_> =
            [Status::Ok, Status::Ignored, Status::Disabled, Status::OptionalDisabled, Status::Missing, Status::OptionalMissing]
                .map(Status::as_str)
                .to_vec();
        assert_eq!(names, ["ok", "ignored", "disabled", "optional-disabled", "missing", "optional-missing"]);
    }

    #[test]
    fn check_works_on_requirements_not_yet_stored() {
        let st = fixture(&[]);
        let reqs = [req(11, false), req(12, false)];
        let s: Vec<_> = check(&st, &reqs, &[12]).iter().map(|c| c.status).collect();
        assert_eq!(s, [Status::Disabled, Status::Ignored]);
    }

    #[test]
    fn dependents_are_enabled_mods_with_a_hard_requirement() {
        let mut a = m("a", 1, true);
        a.requires = vec![req(9, false)];
        let mut b = m("b", 2, false); // disabled: doesn't count
        b.requires = vec![req(9, false)];
        let mut c = m("c", 3, true); // optional: doesn't count
        c.requires = vec![req(9, true)];
        let mut d = m("d", 4, true);
        d.requires = vec![req(5, false), req(9, false)];
        let lib = m("lib", 9, true);
        let st = state(vec![a, b, c, d, lib]);
        let ids: Vec<_> = dependents(&st, &st.mods[4]).iter().map(|x| x.id.as_str()).collect();
        assert_eq!(ids, ["a", "d"]);
        assert!(dependents(&st, &st.mods[0]).is_empty());
        let mut local = m("local", 0, true);
        local.nexus = None;
        assert!(dependents(&st, &local).is_empty());
    }

    #[test]
    fn stale_requirements() {
        let checked = m("checked", 1, true);
        let mut updated = m("updated", 2, true);
        updated.nexus.as_mut().unwrap().version_id = "v2".into();
        let mut never = m("never", 3, true);
        never.reqs_checked = None;
        let mut local = m("local", 0, true);
        local.nexus = None;
        local.reqs_checked = None;
        let mut st = state(vec![checked, updated, never, local]);
        let s = stale(&mut st, false);
        assert_eq!(s.mods, [1, 2]);
        assert!(!s.migrated);
        assert_eq!(stale(&mut st, true).mods, [0, 1, 2]);
    }

    #[test]
    fn requirements_stored_before_check_stamps_are_adopted() {
        let mut old = m("old", 1, true);
        old.reqs_checked = None;
        old.installed_at = "2025-01-01T00:00:00Z".into();
        old.requires = vec![req(5, false)];
        let mut st = state(vec![old]);
        let s = stale(&mut st, false);
        assert!(s.migrated && s.mods.is_empty());
        assert_eq!(st.mods[0].reqs_checked, Some(ReqCheck { at: "2025-01-01T00:00:00Z".into(), version_id: "v1".into() }));
        assert!(!stale(&mut st, false).migrated);
    }
}
