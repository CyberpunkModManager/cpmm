//! The Operations module through its interface: actions and queries on temp-dir `Paths`,
//! with the text an adapter would show (the CLI's output, the GUI's job log).

use super::query::Query;
use super::render::lines;
use super::*;
use crate::change::Lock;
use crate::state::{Paths, State};
use std::fs;
use std::sync::mpsc;

fn paths(tag: &str) -> Paths {
    let d = std::env::temp_dir().join(format!("cpmm-ops-test-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(d.join("game")).unwrap();
    Paths { data: d.clone(), game: d.join("game"), cfg: Default::default() }
}

/// Mods a (needs b, Nexus 2), b (Nexus 2, disabled) and c, saved.
fn setup(p: &Paths) {
    let st = serde_json::json!({"version": 1, "mods": [
        {"id": "a", "name": "A", "enabled": true, "requires": [{"mod_id": 2, "name": "B"}]},
        {"id": "b", "name": "B", "enabled": false, "nexus": {"mod_id": 2}, "reqs_checked": {"at": "2026-01-01T00:00:00Z", "version_id": ""}},
        {"id": "c", "name": "C", "enabled": true},
    ]});
    fs::write(p.state_file(), st.to_string()).unwrap();
}

/// Run an action, collecting its text as (stderr?, line).
fn run_all(p: &Paths, a: Action) -> (Result<()>, Vec<(bool, String)>) {
    let mut out = vec![];
    let r = act(p, a, &mut |e: &Event| out.extend(lines(e).into_iter().map(|l| (l.err, l.text))));
    (r, out)
}

/// [`run_all`] without lock waits: another test spawning a process (7z) can hold a copy of
/// this test's lock file descriptor for a moment, between fork and exec.
fn run(p: &Paths, a: Action) -> (Result<()>, Vec<(bool, String)>) {
    let (r, mut out) = run_all(p, a);
    out.retain(|(_, l)| !l.starts_with("waiting for another cpmm command"));
    (r, out)
}

fn text(out: &[(bool, String)]) -> Vec<&str> {
    out.iter().map(|(_, l)| l.as_str()).collect()
}

fn ids(p: &Paths) -> Vec<String> {
    State::load(p).unwrap().mods.iter().map(|m| m.id.clone()).collect()
}

#[test]
fn actions_change_state_and_say_what_they_did() {
    let p = paths("enable");
    setup(&p);
    let (r, out) = run(&p, Action::Enable { mods: vec!["b".into()] });
    r.unwrap();
    assert_eq!(text(&out), ["enabled b"]);
    assert!(State::load(&p).unwrap().mods[1].enabled);

    let (r, out) = run(&p, Action::Disable { mods: vec!["b".into()] });
    r.unwrap();
    assert_eq!(out, [(false, "disabled b".into()), (true, "  warning: still required by a".into())]);
    assert!(!State::load(&p).unwrap().mods[1].enabled);

    let (r, _) = run(&p, Action::Enable { mods: vec![] });
    assert_eq!(r.unwrap_err().to_string(), "name at least one mod");
    let _ = fs::remove_dir_all(&p.data);
}

#[test]
fn order_and_rules_place_mods() {
    let p = paths("order");
    setup(&p);
    let (r, out) = run(&p, Action::Order { r#mod: "a".into(), place: Some(Place::Top) });
    r.unwrap();
    assert_eq!(text(&out), ["a now at priority 3"]);
    assert_eq!(ids(&p), ["b", "c", "a"]);
    // no position: fails once the mod is found, and nothing is saved
    let (r, _) = run(&p, Action::Order { r#mod: "nosuch".into(), place: None });
    assert!(r.unwrap_err().to_string().contains("nosuch"));
    let (r, _) = run(&p, Action::Order { r#mod: "a".into(), place: None });
    assert_eq!(r.unwrap_err().to_string(), "give --top, --bottom, --before or --after");
    assert_eq!(ids(&p), ["b", "c", "a"]);

    let (r, out) = run(&p, Action::Rule { r#mod: "c".into(), after: "a".into(), remove: false });
    r.unwrap();
    assert_eq!(
        text(&out),
        ["rule: c wins over a", "  moved c 2 -> 3", "  moved a 3 -> 2", "load order updated; 0 archives in modlist.txt"]
    );
    assert_eq!(ids(&p), ["b", "a", "c"]);
    let _ = fs::remove_dir_all(&p.data);
}

#[test]
fn deps_ignore_then_report() {
    let p = paths("deps");
    setup(&p);
    let (r, out) = run(&p, Action::Deps { refresh: false, fix: false, ignore: Some(("a".into(), "2".into())), unignore: None });
    r.unwrap();
    // c and a have no Nexus page, so nothing is fetched
    assert_eq!(
        text(&out),
        [
            "ignoring requirement 2 for a",
            "requirements of all 1 Nexus mods are cached (re-check with --refresh)",
            "0 unmet hard requirements (1 ignored by choice)"
        ]
    );
    assert_eq!(State::load(&p).unwrap().mods[0].ignored_reqs, [2]);
    let (r, _) = run(&p, Action::Deps { refresh: false, fix: false, ignore: Some(("a".into(), "x".into())), unignore: None });
    assert_eq!(r.unwrap_err().to_string(), "requirement must be a Nexus mod id");
    let _ = fs::remove_dir_all(&p.data);
}

#[test]
fn profiles_through_actions() {
    let p = paths("profiles");
    setup(&p);
    let create = ProfileAction::Create { name: "Lean".into(), from: None, empty: true, own_saves: false, copy_saves: false, switch: true };
    let (r, out) = run(&p, Action::Profile(create));
    r.unwrap();
    assert_eq!(text(&out), ["created profile \"Lean\" (empty: every mod disabled)", "switched to \"Lean\""]);
    let st = State::load(&p).unwrap();
    assert_eq!(st.active_profile, "Lean");
    assert!(st.mods.iter().all(|m| !m.enabled));
    let (r, out) = run(&p, Action::Profile(ProfileAction::Switch { name: "Lean".into() }));
    r.unwrap();
    assert_eq!(text(&out), ["\"Lean\" is already the active profile"]);
    let _ = fs::remove_dir_all(&p.data);
}

#[test]
fn changes_wait_for_the_lock_and_reads_dont() {
    let p = paths("lock");
    setup(&p);
    // someone else (another thread here; the CLI or the GUI in real life) holds the lock
    let (held, release) = (mpsc::channel(), mpsc::channel::<()>());
    let q = p.clone();
    let other = std::thread::spawn(move || {
        let _l = Lock::acquire(&q).unwrap();
        held.0.send(()).unwrap();
        release.1.recv().unwrap();
    });
    held.1.recv().unwrap();
    // a read-only action runs right away
    let (r, out) = run(&p, Action::Sort { apply: false });
    r.unwrap();
    assert_eq!(text(&out), ["load order already satisfies every constraint (0 constraints)"]);
    // a change waits, and says so
    let r2 = p.clone();
    let change = std::thread::spawn(move || run_all(&r2, Action::Enable { mods: vec!["b".into()] }));
    std::thread::sleep(std::time::Duration::from_millis(100));
    assert!(!State::load(&p).unwrap().mods[1].enabled, "changed while the lock was held");
    release.0.send(()).unwrap();
    other.join().unwrap();
    let (r, out) = change.join().unwrap();
    r.unwrap();
    assert_eq!(out[0], (true, "waiting for another cpmm command to finish…".into()));
    assert!(State::load(&p).unwrap().mods[1].enabled);
    let _ = fs::remove_dir_all(&p.data);
}

#[test]
fn queries_read_the_setup() {
    let p = paths("query");
    setup(&p);
    fs::create_dir_all(p.mod_files("a").join("r6")).unwrap();
    fs::write(p.mod_files("a").join("r6/x.txt"), "a").unwrap();
    let st = State::load(&p).unwrap();
    let rows = query::list(&p, &st, true);
    assert_eq!(
        rows.iter().map(|r| (r.m.id.as_str(), r.counts)).collect::<Vec<_>>(),
        [("a", Some((1, 0, 0))), ("b", Some((0, 0, 0))), ("c", Some((0, 0, 0)))]
    );
    let u = query::unmet(&st);
    assert_eq!((u.problems, u.ignored), (1, 0));
    assert_eq!((u.rows[0].id.as_str(), u.rows[0].disabled.as_deref()), ("a", Some("b")));
    let d = query::detail(&p, &st, "a").unwrap();
    assert_eq!(d.files, ["r6/x.txt"]);
    let _ = fs::remove_dir_all(&p.data);
}

#[test]
fn views_answer_queries_by_name() {
    let p = paths("views");
    setup(&p);
    let ask = |q: serde_json::Value| query::view(&p, &serde_json::from_value(q).unwrap(), &mut |_: &Event| {}).unwrap();
    let st = ask(serde_json::json!({"view": "state"}));
    let a = &st["mods"][0];
    assert_eq!((a["id"].as_str(), a["priority"].as_u64(), a["dep_problems"].as_u64()), (Some("a"), Some(1), Some(1)));
    assert_eq!(a["requires"][0]["status"], "disabled");
    assert_eq!(a["requires"][0]["installed_as"], "b");
    assert_eq!(a["nexus"], serde_json::Value::Null);
    let info = ask(serde_json::json!({"view": "info", "mod": "a"}));
    assert_eq!(info, serde_json::json!({"id": "a", "name": "A", "files": [], "hidden": [], "ignored_reqs": []}));
    // an archive without an installer is only `fomod: false`
    let src = p.data.join("plain");
    fs::create_dir_all(src.join("r6")).unwrap();
    fs::write(src.join("r6/x.txt"), "x").unwrap();
    let arc = p.data.join("plain.zip");
    let ok = std::process::Command::new("7z").arg("a").arg(&arc).arg(src.join("r6")).output();
    if ok.is_ok_and(|o| o.status.success()) {
        assert_eq!(ask(serde_json::json!({"view": "fomod", "source": arc})), serde_json::json!({"fomod": false}));
    }
    let r = query::view(&p, &Query::Info { r#mod: "nosuch".into() }, &mut |_: &Event| {});
    assert!(r.unwrap_err().to_string().contains("nosuch"));
    let _ = fs::remove_dir_all(&p.data);
}
