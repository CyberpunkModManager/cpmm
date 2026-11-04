//! Health check (`cpmm doctor`): findings about things that commonly cause crashes, broken
//! saves or stutter, each with its remedies. One interface, `check`; the inputs (requirements,
//! script and archive conflicts, overwrite/, mod.io, the safety net, load order) are read here
//! and only rendered by callers.
//!
//! Order of findings: crash, warning, perf, info; within a severity, the order the checks run.
//! Findings whose title the user acknowledged (`State::acked`) keep their place as "acked".

use crate::conflicts;
use crate::loadorder::{self, Owner, Pair};
use crate::manage::{self, OwKind};
use crate::modio;
use crate::nexus::{Catalogue, Nexus};
use crate::requirements::{self, Status};
use crate::safety;
use crate::state::{Paths, State};
use serde::Serialize;

/// A one-click fix: the GUI shows `label` and runs `cpmm <args>`.
#[derive(Serialize, Debug, Clone, ts_rs::TS)]
#[ts(rename = "Remedy")]
pub struct Action {
    pub label: String,
    pub args: Vec<String>,
    /// "primary" for the recommended choice
    #[ts(type = "\"primary\" | \"secondary\"")]
    pub style: &'static str,
}
fn act(label: impl Into<String>, args: &[&str], style: &'static str) -> Action {
    Action { label: label.into(), args: args.iter().map(|s| s.to_string()).collect(), style }
}

#[derive(Serialize, Debug, ts_rs::TS)]
pub struct Finding {
    #[ts(type = "\"crash\" | \"warning\" | \"perf\" | \"info\" | \"acked\"")]
    pub severity: &'static str, // "crash" | "warning" | "perf" | "info" | "acked"
    pub title: String,
    pub detail: String,
    /// What to do when there is no in-app action (e.g. mod.io mods)
    pub fix: Option<String>,
    pub actions: Vec<Action>,
}

/// Every finding for the current setup, most severe first.
pub fn check(paths: &Paths, st: &State) -> Vec<Finding> {
    let nx = Catalogue::open(paths);
    findings(paths, st, Some(nx.gate()))
}

/// `check` with the Nexus client injected (None: the patch guard asks Nexus nothing).
fn findings(paths: &Paths, st: &State, nx: Option<&Nexus>) -> Vec<Finding> {
    let mut f = vec![];
    f.extend(imports(paths, st));
    f.extend(deps(st));
    f.extend(script_clashes(paths, st));
    f.extend(archives(paths, st));
    f.extend(overwrite(paths, st));
    let mio = modio_findings(paths, st, &f);
    f.extend(mio);
    f.extend(safety_net(paths, st, nx));
    f.extend(load_order(paths, st));
    let order = |s: &str| match s {
        "crash" => 0,
        "warning" => 1,
        "perf" => 2,
        _ => 3,
    };
    f.sort_by_key(|x| order(x.severity));
    for x in f.iter_mut() {
        if st.acked.contains(&x.title) {
            x.severity = "acked";
        }
    }
    f
}

fn name(st: &State, id: &str) -> String {
    st.mods.iter().find(|m| m.id == id).map(|m| m.name.clone()).unwrap_or(id.to_string())
}

fn disable(st: &State, id: &str) -> Action {
    act(format!("Disable {}", name(st, id)), &["disable", id], "secondary")
}

fn imports(paths: &Paths, st: &State) -> Vec<Finding> {
    conflicts::unresolved_imports(paths, &st.mods)
        .into_iter()
        .map(|(owner, imp, rel)| Finding {
            severity: "crash",
            title: format!("{owner} imports {imp}, which no installed mod provides"),
            detail: format!("{rel}: one unresolved import fails the whole redscript compile, so every script mod stops working. Install the mod that provides `{imp}` (see this mod's requirements) or disable it."),
            fix: None,
            actions: if owner.starts_with('(') { vec![] } else { vec![act(format!("Disable {}", name(st, &owner)), &["disable", &owner], "primary")] },
        })
        .collect()
}

fn deps(st: &State) -> Vec<Finding> {
    let mut f = vec![];
    for m in st.mods.iter().filter(|m| m.enabled) {
        for c in requirements::of(st, m) {
            let (r, rid) = (c.req, c.req.mod_id.to_string());
            match (c.status, c.dep) {
                (Status::Missing, _) => f.push(Finding {
                    severity: "warning",
                    title: format!("{} is missing a requirement: {}", m.name, r.name),
                    detail: if r.notes.is_empty() {
                        "Parts of the mod may not work.".into()
                    } else {
                        format!("Author's note: {}", r.notes)
                    },
                    fix: None,
                    actions: vec![
                        act(format!("Install {}", r.name), &["install", &rid, "--with-deps"], "primary"),
                        act("Skip it (ignore)", &["deps", "--ignore", &m.id, &rid], "secondary"),
                    ],
                }),
                (Status::Disabled, Some(d)) => f.push(Finding {
                    severity: "crash",
                    title: format!("{} requires {}, which is disabled", m.name, d.name),
                    detail: String::new(),
                    fix: None,
                    actions: vec![
                        act(format!("Enable {}", d.name), &["enable", &d.id], "primary"),
                        act(format!("Disable {}", m.name), &["disable", &m.id], "secondary"),
                    ],
                }),
                _ => {}
            }
        }
    }
    f
}

fn script_clashes(paths: &Paths, st: &State) -> Vec<Finding> {
    conflicts::redscript_conflicts(paths, &st.mods)
        .into_iter()
        .filter(|c| c.kind.starts_with("REPLACE") || c.kind.starts_with("duplicate"))
        .map(|c| Finding {
            severity: "warning",
            title: format!("Script clash on {}", c.target),
            detail: format!("{} between {}. Only one version can exist: one mod's behaviour is silently lost, or redscript refuses to compile. Look for a compatibility patch, or keep only one.", c.kind, c.mods.join(", ")),
            fix: None,
            actions: c.mods.iter().filter(|m| !m.starts_with('(')).map(|m| disable(st, m)).collect(),
        })
        .collect()
}

fn archives(paths: &Paths, st: &State) -> Vec<Finding> {
    let ids: Vec<String> = st.mods.iter().map(|m| m.id.clone()).collect();
    let ruled = |p: &Pair| match (&p.a, &p.b) {
        (Owner::Mod(x), Owner::Mod(y)) => st.rules.iter().any(|r| (r.r#mod == *x && r.after == *y) || (r.r#mod == *y && r.after == *x)),
        _ => false,
    };
    let mut f = vec![];
    for p in loadorder::archive_conflicts(paths, st, &ids) {
        let (la, lb) = (p.a.label(), p.b.label());
        let modio = matches!(p.a, Owner::Modio(_)) || matches!(p.b, Owner::Modio(_));
        let dup_name = p.archive_a.eq_ignore_ascii_case(&p.archive_b);
        let mostly_same = p.shared >= 10 && p.shared * 2 >= p.smaller.max(1);
        let differing = p.shared - p.identical;
        let mods: Vec<String> = [&p.a, &p.b].iter().filter_map(|o| if let Owner::Mod(x) = o { Some(x.clone()) } else { None }).collect();
        if (dup_name && p.shared * 2 >= p.smaller.max(1)) || (modio && mostly_same) {
            f.push(Finding {
                severity: "crash",
                title: format!("Same mod installed twice: {la} and {lb}"),
                detail: format!(
                    "{} and {} share {} of {} resources. Two copies of one mod double-hook the game and are a classic crash cause.",
                    p.archive_a, p.archive_b, p.shared, p.smaller
                ),
                fix: modio.then(|| "Unsubscribe the mod.io copy in the in-game Mods menu (quit the game afterwards).".into()),
                actions: mods.iter().map(|m| disable(st, m)).collect(),
            });
        } else if p.identical == p.shared && p.shared == p.smaller && p.smaller > 0 {
            f.push(Finding {
                severity: "perf",
                title: format!("{} is fully contained in {}", p.archive_a, p.archive_b),
                detail: format!("{la} and {lb} ship {} byte-identical resources; the duplicate costs load time and memory.", p.shared),
                fix: None,
                actions: vec![],
            });
        } else if differing > 0 && !p.related && !ruled(&p) {
            let minor = differing <= 2 && p.identical * 5 >= p.shared * 4;
            let winner = p.winner.clone().unwrap_or("the game's own order".into());
            let actions = if let (Owner::Mod(a), Owner::Mod(b)) = (&p.a, &p.b) {
                let style = |x: &str| if p.winner.as_deref() == Some(x) { "primary" } else { "secondary" };
                vec![
                    act(format!("{} wins", name(st, a)), &["rule", a, "--after", b], style(a)),
                    act(format!("{} wins", name(st, b)), &["rule", b, "--after", a], style(b)),
                ]
            } else {
                vec![]
            };
            f.push(Finding {
                severity: if minor { "info" } else { "warning" },
                title: format!("{la} and {lb} replace the same {} game resource{}", differing, if differing == 1 { "" } else { "s" }),
                detail: format!("{} vs {}. Currently {} wins. Mismatched meshes/textures from two mods are a common source of crashes and visual glitches; pick the one you want.", p.archive_a, p.archive_b, if p.winner.is_some() { name(st, &winner) } else { winner }),
                fix: modio.then(|| "cpmm can't order mod.io archives; unsubscribe one of them in the in-game Mods menu if something looks wrong.".into()),
                actions,
            });
        }
    }
    f
}

fn overwrite(paths: &Paths, st: &State) -> Option<Finding> {
    let shadow: Vec<String> = manage::overwrite_files(paths, st)
        .into_iter()
        .filter(|e| e.kind == OwKind::EditedMod && !e.rel.ends_with(".log"))
        .map(|e| e.rel)
        .collect();
    if shadow.is_empty() {
        return None;
    }
    let mut clear = vec!["overwrite", "clear"];
    clear.extend(shadow.iter().map(|s| s.as_str()));
    Some(Finding {
        severity: "info",
        title: format!("{} mod files are overridden by runtime copies in overwrite/", shadow.len()),
        detail: format!(
            "Settings the game saved (e.g. {}). After updating a mod, stale copies can keep old values. Keep them unless a mod behaves as if its update didn't apply.",
            shadow.iter().take(3).cloned().collect::<Vec<_>>().join(", ")
        ),
        fix: None,
        actions: vec![act("Reset to the mods' shipped versions", &clear, "secondary")],
    })
}

/// mod.io watcher and conflicts; `earlier` findings already cover some clashes.
fn modio_findings(paths: &Paths, st: &State, earlier: &[Finding]) -> Vec<Finding> {
    let mut f = vec![];
    let ch = modio::changes(paths);
    if !ch.first_look && (!ch.added.is_empty() || !ch.updated.is_empty()) {
        f.push(Finding {
            severity: "warning",
            title: format!("mod.io subscriptions changed: {} new, {} updated", ch.added.len(), ch.updated.len()),
            detail: format!(
                "{}{}. mod.io mods load outside cpmm's control; check the conflicts below, then mark them reviewed.",
                ch.added.iter().map(|a| format!("new: {a}")).collect::<Vec<_>>().join("; "),
                if ch.updated.is_empty() { String::new() } else { format!("; updated: {}", ch.updated.join("; ")) }
            ),
            fix: Some("Unsubscribe in the in-game Mods menu if you didn't mean to add them.".into()),
            actions: vec![act("Mark as reviewed", &["modio", "--ack"], "secondary")],
        });
    }
    // archive and script clashes with mod.io mods are already reported by the general checks
    for c in modio::conflicts(paths, st).into_iter().filter(|c| c.kind != "archive" && c.kind != "script") {
        let title = match c.kind {
            "same mod" => format!("Same mod installed twice: {} and mod.io: {}", c.with, c.modio),
            "script" => format!("mod.io: {} clashes with {} in a script", c.modio, c.with),
            _ => format!("mod.io: {} and {} ship the same files", c.modio, c.with),
        };
        if earlier.iter().chain(&f).any(|x| {
            x.title == title || (c.kind == "same mod" && x.title.starts_with("Same mod installed twice") && x.title.contains(&c.modio))
        }) {
            continue;
        }
        let actions = if st.find(&c.with).is_some() { vec![disable(st, &c.with)] } else { vec![] };
        f.push(Finding {
            severity: if c.kind == "same mod" {
                "crash"
            } else if c.severe {
                "warning"
            } else {
                "info"
            },
            title,
            detail: c.detail.clone(),
            fix: Some("To drop the mod.io copy, unsubscribe in the in-game Mods menu (quit the game afterwards).".into()),
            actions,
        });
    }
    f
}

/// Crashes, new log errors, the game-patch guard and save risks.
fn safety_net(paths: &Paths, st: &State, nx: Option<&Nexus>) -> Vec<Finding> {
    let mut f = vec![];
    let sess = safety::sessions(paths);
    if let Some(c) = safety::crashes(paths, 1).into_iter().next()
        // only flag the crash if it ended the latest recorded session
        && sess.last().is_some_and(|s| c.session_start == Some(s.start))
    {
        let mut actions: Vec<Action> = c
            .suspects
            .iter()
            .filter(|m| !safety::list_history(paths, m).is_empty())
            .take(4)
            .map(|m| act(format!("Roll back {}", name(st, m)), &["rollback", m], "secondary"))
            .collect();
        if let Some(b) = safety::batches(paths, st).into_iter().next()
            && c.last_good.is_some_and(|g| b.ts > g)
        {
            actions.insert(0, act("Undo the last update run", &["undo-update", &b.batch], "primary"));
        }
        let msg = c.message.trim_end_matches('.');
        f.push(Finding {
            severity: "crash",
            title: format!(
                "The last session crashed {} in ({} {})",
                c.seconds_in.map(|s| format!("{}m{}s", s / 60, s % 60)).unwrap_or_default(),
                c.module,
                c.offset
            ),
            detail: if c.changes.is_empty() {
                format!("{msg}. No mod changes since the last clean session.")
            } else {
                format!("{msg}. Changed since the last clean session: {}", c.changes.join("; "))
            },
            fix: None,
            actions,
        });
    }
    let ed = safety::error_diff(paths);
    let new_n: usize = ed.new.values().map(|v| v.len()).sum();
    if new_n > 0 {
        let first: Vec<String> = ed.new.iter().flat_map(|(s, v)| v.iter().take(2).map(move |l| format!("[{s}] {l}"))).take(3).collect();
        f.push(Finding {
            severity: "warning",
            title: format!("{new_n} new errors in the logs since the previous launch"),
            detail: first.join(" · "),
            fix: None,
            actions: vec![],
        });
    }
    let ps = safety::patch_status(paths, st, nx);
    if ps.patch_detected && ps.at_risk.iter().any(|x| !x.released_since_patch || x.update_available) {
        let mut actions: Vec<Action> = ps
            .at_risk
            .iter()
            .filter(|x| x.update_available)
            .map(|x| act(format!("Update {}", x.name), &["update", &x.id], "primary"))
            .collect();
        if !ps.vanilla {
            actions.push(act("Launch vanilla next time", &["vanilla", "on"], "secondary"));
        }
        f.push(Finding {
            severity: "crash",
            title: format!("Cyberpunk was updated (build {}) and {} native mods may not support it yet", ps.build_id, ps.at_risk.len()),
            detail: ps
                .at_risk
                .iter()
                .map(|x| format!("{}: {}", x.name, if x.update_available { "update available" } else { "no release since the patch" }))
                .collect::<Vec<_>>()
                .join("; "),
            fix: None,
            actions,
        });
    }
    if !ps.red4ext_errors.is_empty() {
        f.push(Finding {
            severity: "warning",
            title: "RED4ext reported errors at the last launch".into(),
            detail: ps.red4ext_errors.iter().take(3).cloned().collect::<Vec<_>>().join(" · "),
            fix: None,
            actions: vec![],
        });
    }
    for r in safety::save_risks(paths, st) {
        let actions = r
            .missing
            .iter()
            .filter(|m| st.find(m).is_some())
            .map(|m| act(format!("Re-enable {}", name(st, m)), &["enable", m], "primary"))
            .collect();
        f.push(Finding {
            severity: "warning",
            title: format!("Save {} was made with quest mods that are now off", r.save),
            detail: format!(
                "{} — loading it without them can break quest progress (like Playing for Time). Re-enable them or load an earlier save.",
                r.missing.join(", ")
            ),
            fix: None,
            actions,
        });
    }
    f
}

/// Load order is one input: constraints the current order breaks, and rule cycles.
fn load_order(paths: &Paths, st: &State) -> Vec<Finding> {
    let mut f = vec![];
    let sp = loadorder::plan(paths, st);
    if !sp.moved.is_empty() {
        f.push(Finding {
            severity: "warning",
            title: format!("Load order breaks {} constraints", sp.moved.len()),
            detail: sp.moved.iter().flat_map(|m| m.because.clone()).take(4).collect::<Vec<_>>().join("; "),
            fix: None,
            actions: vec![act("Sort load order", &["sort", "--apply"], "primary")],
        });
    }
    for c in &sp.cycles {
        let rules: Vec<Action> = st
            .rules
            .iter()
            .filter(|r| c.contains(&r.r#mod) && c.contains(&r.after))
            .map(|r| {
                act(
                    format!("Remove rule: {} after {}", name(st, &r.r#mod), name(st, &r.after)),
                    &["rule", &r.r#mod, "--after", &r.after, "--remove"],
                    "secondary",
                )
            })
            .collect();
        f.push(Finding {
            severity: "warning",
            title: "Circular load order constraints".into(),
            detail: c.join(" → "),
            fix: None,
            actions: rules,
        });
    }
    f
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;

    /// A fresh data dir with the game inside a Steam library, so mod.io, crash reports and
    /// saves (all found relative to the game dir) stay inside it too.
    fn test_paths(tag: &str) -> Paths {
        let d = std::env::temp_dir().join(format!("cpmm-health-test-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        let game = d.join("lib/steamapps/common/Cyberpunk 2077");
        fs::create_dir_all(&game).unwrap();
        Paths { data: d.join("data"), game, cfg: Default::default() }
    }

    fn cleanup(paths: &Paths) {
        let _ = fs::remove_dir_all(paths.data.parent().unwrap());
    }

    fn write(p: &std::path::Path, body: &str) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, body).unwrap();
    }

    fn state(v: serde_json::Value) -> State {
        serde_json::from_value(v).unwrap()
    }

    fn titles(f: &[Finding]) -> Vec<(&str, &str)> {
        f.iter().map(|x| (x.severity, x.title.as_str())).collect()
    }

    fn argv(f: &Finding) -> Vec<String> {
        f.actions.iter().map(|a| a.args.join(" ")).collect()
    }

    /// Remedies are the CLI argv the GUI runs; crash risks sort first; an acknowledged
    /// finding keeps its place.
    #[test]
    fn requirement_findings_and_their_remedies() {
        let paths = test_paths("reqs");
        let mut st = state(json!({"version": 1, "mods": [
            {"id": "alpha", "name": "Alpha", "enabled": true,
             "requires": [{"mod_id": 999, "name": "Missing Lib", "notes": "needed"}, {"mod_id": 50, "name": "Beta"}]},
            {"id": "beta", "name": "Beta", "enabled": false, "nexus": {"mod_id": 50}}]}));
        let f = findings(&paths, &st, None);
        assert_eq!(
            titles(&f),
            [("crash", "Alpha requires Beta, which is disabled"), ("warning", "Alpha is missing a requirement: Missing Lib")]
        );
        assert_eq!(argv(&f[0]), ["enable beta", "disable alpha"]);
        assert_eq!(f[0].actions[0].style, "primary");
        assert_eq!(f[1].detail, "Author's note: needed");
        assert_eq!(argv(&f[1]), ["install 999 --with-deps", "deps --ignore alpha 999"]);

        st.acked = vec!["Alpha requires Beta, which is disabled".into()];
        let f = findings(&paths, &st, None);
        assert_eq!(titles(&f)[0], ("acked", "Alpha requires Beta, which is disabled"));
        cleanup(&paths);
    }

    #[test]
    fn script_findings() {
        let paths = test_paths("scripts");
        let st = state(json!({"version": 1, "mods": [
            {"id": "gamma", "name": "Gamma", "enabled": true}, {"id": "delta", "name": "Delta", "enabled": true}]}));
        write(&paths.mod_files("gamma").join("r6/scripts/g.reds"), "module G\n@replaceMethod(PlayerPuppet)\nfunc OnAction() {}\n");
        write(
            &paths.mod_files("delta").join("r6/scripts/d.reds"),
            "module D\nimport Missing.Module\n@replaceMethod(PlayerPuppet)\nfunc OnAction() {}\n",
        );
        let f = findings(&paths, &st, None);
        assert_eq!(
            titles(&f),
            [
                ("crash", "delta imports Missing.Module, which no installed mod provides"),
                ("warning", "Script clash on PlayerPuppet.OnAction")
            ]
        );
        assert_eq!(argv(&f[0]), ["disable delta"]);
        assert_eq!(f[1].actions.iter().map(|a| a.label.as_str()).collect::<Vec<_>>(), ["Disable Delta", "Disable Gamma"]);
        cleanup(&paths);
    }

    /// Runtime copies of mod files in overwrite/ (not logs, not new files) can be reset.
    #[test]
    fn overwrite_shadows() {
        let paths = test_paths("overwrite");
        let st = state(json!({"version": 1, "mods": [{"id": "gamma", "name": "Gamma", "enabled": true}]}));
        for rel in ["r6/config/settings.json", "r6/logs/x.log"] {
            write(&paths.mod_files("gamma").join(rel), "shipped");
            write(&paths.upper().join(rel), "edited");
        }
        write(&paths.upper().join("r6/new.txt"), "new");
        let f = findings(&paths, &st, None);
        assert_eq!(titles(&f), [("info", "1 mod files are overridden by runtime copies in overwrite/")]);
        assert_eq!(argv(&f[0]), ["overwrite clear r6/config/settings.json"]);
        cleanup(&paths);
    }

    /// A crash report from the latest session, with what changed since the last clean one.
    #[test]
    fn crash_ending_the_last_session() {
        let paths = test_paths("crash");
        let st = state(json!({"version": 1, "mods": [{"id": "alpha", "name": "Alpha", "enabled": true, "version": "2"}]}));
        let now = safety::now_unix();
        let sess = |start: i64, v: &str| {
            json!({"start": start, "build_id": "", "vanilla": false, "mods": [{"id": "alpha", "version": v, "enabled": true}]}).to_string()
        };
        write(&paths.data.join("sessions.jsonl"), &format!("{}\n{}\n", sess(now - 1000, "1"), sess(now - 500, "2")));
        let rq = paths.modio_dir().join("../../../steamuser/AppData/Local/REDEngine/ReportQueue/Cyberpunk2077-1");
        write(&rq.join("stacktrace.txt"), "Expression: boom\r\nMessage: bad thing.\r\n");
        let f = findings(&paths, &st, None);
        assert_eq!(f.len(), 1, "{:?}", titles(&f));
        assert_eq!(f[0].severity, "crash");
        assert!(f[0].title.starts_with("The last session crashed 8m") && f[0].title.ends_with(" in (? +0x0)"), "{}", f[0].title);
        assert_eq!(f[0].detail, "boom — bad thing. Changed since the last clean session: alpha: 1 -> 2");
        // a crash before the latest session is old news
        write(&paths.data.join("sessions.jsonl"), &format!("{}\n{}\n", sess(now - 1000, "1"), sess(now + 10, "2")));
        assert!(findings(&paths, &st, None).is_empty());
        cleanup(&paths);
    }

    /// Load order is one input: a rule cycle is reported with a remedy per rule.
    #[test]
    fn rule_cycle() {
        let paths = test_paths("cycle");
        let st = state(json!({"version": 1,
            "mods": [{"id": "a", "name": "A", "enabled": true}, {"id": "b", "name": "B", "enabled": true}],
            "rules": [{"mod": "a", "after": "b"}, {"mod": "b", "after": "a"}]}));
        let f = findings(&paths, &st, None);
        assert_eq!(titles(&f), [("warning", "Circular load order constraints")]);
        assert_eq!(f[0].detail, "a → b");
        assert_eq!(argv(&f[0]), ["rule a --after b --remove", "rule b --after a --remove"]);
        cleanup(&paths);
    }
}
