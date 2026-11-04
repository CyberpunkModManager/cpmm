//! The Install module through its interface: temp-dir `Paths`, small archives and folders,
//! and a fake Nexus behind the `Releases` port. The archive test needs `7z` on PATH, as
//! cpmm itself does.

use super::*;
use crate::change::{Idle, Lock};
use std::cell::RefCell;
use std::collections::BTreeMap;

fn test_paths(tag: &str) -> Paths {
    let d = std::env::temp_dir().join(format!("cpmm-install-test-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    Paths { data: d.clone(), game: d.join("game"), cfg: Default::default() }
}

fn quiet() -> Policy {
    Policy { hooks: false, ..Default::default() }
}

/// Run an install inside a change, as the CLI does, and collect what the CLI would print.
fn install(paths: &Paths, st: &mut State, nx: &dyn Releases, source: Source, policy: &Policy) -> (Result<Report>, Vec<String>) {
    let _lock = Lock::acquire(paths).unwrap();
    let mut out = vec![];
    // the install's lines on stdout (as the CLI prints them)
    let mut ui = |e: &OpEvent| {
        if let OpEvent::Install(_) = e {
            out.extend(crate::ops::render::lines(e).into_iter().filter(|l| !l.err).map(|l| l.text))
        }
    };
    let r = crate::change::Change::setup().idle(Idle::NotNeeded).apply(paths, st, &mut ui, |cx, st| run(cx, st, nx, source, policy));
    (r, out)
}

/// A folder of files to install from.
fn folder(paths: &Paths, name: &str, entries: &[(&str, &str)]) -> PathBuf {
    let d = paths.data.join("src").join(name);
    let _ = fs::remove_dir_all(&d);
    for (p, body) in entries {
        let f = d.join(p);
        fs::create_dir_all(f.parent().unwrap()).unwrap();
        fs::write(f, body).unwrap();
    }
    d
}

/// A ustar archive (one of the formats 7z extracts) written by hand: no new dependencies.
fn tar(path: &Path, entries: &[(&str, &str)]) {
    let mut out = vec![];
    for (name, body) in entries {
        let mut h = [0u8; 512];
        let mut put = |off: usize, v: &[u8]| h[off..off + v.len()].copy_from_slice(v);
        put(0, name.as_bytes());
        put(100, b"0000644\0");
        put(108, b"0000000\0");
        put(116, b"0000000\0");
        put(124, format!("{:011o}\0", body.len()).as_bytes());
        put(136, b"00000000000\0");
        put(148, b"        ");
        put(156, b"0");
        put(257, b"ustar\0");
        put(263, b"00");
        let sum: u32 = h.iter().map(|&b| b as u32).sum();
        h[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
        out.extend_from_slice(&h);
        out.extend_from_slice(body.as_bytes());
        out.resize(out.len().div_ceil(512) * 512, 0);
    }
    out.extend_from_slice(&[0u8; 1024]);
    fs::write(path, out).unwrap();
}

fn staged(paths: &Paths, id: &str) -> Vec<String> {
    files::list_tree(&paths.mod_files(id))
}

/// One Nexus mod of the fake: its current file version, requirements and archive folder.
struct FakeMod {
    name: &'static str,
    version: &'static str,
    requires: Vec<u64>,
    archive: PathBuf,
}

#[derive(Default)]
struct FakeNexus {
    mods: RefCell<BTreeMap<u64, FakeMod>>,
}

impl FakeNexus {
    fn add(&self, gid: u64, name: &'static str, version: &'static str, requires: &[u64], archive: PathBuf) {
        self.mods.borrow_mut().insert(gid, FakeMod { name, version, requires: requires.to_vec(), archive });
    }
}

impl Releases for FakeNexus {
    fn release(&self, gid: u64, _file: Option<&str>) -> Result<(ModInfo, Version)> {
        let mods = self.mods.borrow();
        let m = mods.get(&gid).context("no such mod")?;
        let info =
            serde_json::from_value(serde_json::json!({"id": format!("uid{gid}"), "game_scoped_id": gid.to_string(), "name": m.name}))?;
        let ver = serde_json::from_value(serde_json::json!({
            "id": format!("{gid}-{}", m.version), "file": {"id": format!("file{gid}"), "name": "Main"},
            "game_scoped_id": format!("{gid}0"), "version": m.version, "category": "main", "manifest_url": "https://example.invalid"
        }))?;
        Ok((info, ver))
    }

    fn requires(&self, gid: u64, _version_id: &str) -> Result<(Vec<nexus::Req>, Vec<Requirement>)> {
        let mods = self.mods.borrow();
        let reqs = mods[&gid]
            .requires
            .iter()
            .map(|r| Requirement { mod_id: *r, name: mods[r].name.into(), notes: String::new(), optional: false })
            .collect();
        Ok((vec![], reqs))
    }

    fn archive(&self, _: &ModInfo, gid: u64, _: &Version, _: &mut dyn FnMut(Fetch)) -> Result<(PathBuf, Manifest)> {
        let man = serde_json::from_value(serde_json::json!({"archive": {"size": 0, "hashes": {"SHA256": ""}}, "files": []}))?;
        Ok((self.mods.borrow()[&gid].archive.clone(), man))
    }
}

#[test]
fn local_archive_installs_end_to_end() {
    let paths = test_paths("archive");
    let mut st = State::load(&paths).unwrap();
    let arc = paths.data.join("Cool Mod.tar");
    tar(&arc, &[("Cool/r6/scripts/Cool/cool.reds", "// reds"), ("Cool/archive/pc/mod/cool.archive", "RDAR"), ("Cool/README.txt", "hi")]);

    let src = Source::Local { path: arc.clone(), nexus: None };
    let (r, out) = install(&paths, &mut st, &FakeNexus::default(), src, &quiet());
    let r = r.unwrap();

    let d = r.requested();
    assert_eq!((d.id.as_str(), d.origin, &d.change, d.files), ("cool-mod", Origin::Local, &Change::Installed, 2));
    // the line the GUI parses (gui/src/lib/nexus.ts)
    assert_eq!(out, ["installed cool-mod: 2 files"]);
    assert_eq!(staged(&paths, "cool-mod"), ["archive/pc/mod/cool.archive", "r6/scripts/Cool/cool.reds"]);
    let saved = State::load(&paths).unwrap();
    let m = &saved.mods[saved.get("cool-mod").unwrap()];
    assert!(m.enabled && m.name == "Cool Mod" && m.notes == format!("from {}", arc.display()));
    assert!(!stage::tmp_extract_dir(&paths, "cool-mod").exists(), "the tmp tree is cleaned up");
    let _ = fs::remove_dir_all(&paths.data);
}

const FOMOD: &str = r#"<config>
  <moduleName>Wheels</moduleName>
  <requiredInstallFiles><folder source="Core" destination="" /></requiredInstallFiles>
  <installSteps order="Explicit"><installStep name="Main"><optionalFileGroups order="Explicit">
    <group name="Rims" type="SelectExactlyOne"><plugins order="Explicit">
      <plugin name="Stock"><description/><files><file source="opt\stock.archive" destination="archive\pc\mod\rims.archive" /></files>
        <typeDescriptor><type name="Recommended"/></typeDescriptor></plugin>
      <plugin name="Sport"><description/><files><file source="opt\sport.archive" destination="archive\pc\mod\rims.archive" /></files>
        <typeDescriptor><type name="Optional"/></typeDescriptor></plugin>
    </plugins></group>
  </optionalFileGroups></installStep></installSteps>
</config>"#;

#[test]
fn fomod_choices_explicit_then_saved_then_defaults() {
    let paths = test_paths("fomod");
    let mut st = State::load(&paths).unwrap();
    let src = folder(
        &paths,
        "wheels",
        &[
            ("fomod/ModuleConfig.xml", FOMOD),
            ("Core/r6/scripts/Wheels/w.reds", "// reds"),
            ("opt/stock.archive", "stock"),
            ("opt/sport.archive", "sport"),
        ],
    );
    let rims = |id: &str| fs::read_to_string(paths.mod_files(id).join("archive/pc/mod/rims.archive")).unwrap();
    let local = || Source::Local { path: src.clone(), nexus: None };

    // explicit: the choices given, stored on the mod
    let sport = Choices::from([("Main/Rims".to_string(), vec!["Sport".to_string()])]);
    let policy = Policy { id: Some("wheels".into()), fomod: Some(sport.clone()), ..quiet() };
    let (r, out) = install(&paths, &mut st, &FakeNexus::default(), local(), &policy);
    assert_eq!(r.unwrap().requested().change, Change::Installed);
    assert_eq!(out, ["  FOMOD Main/Rims: Sport", "installed wheels: 2 files"]);
    assert_eq!(rims("wheels"), "sport");
    let stored = State::load(&paths).unwrap().mods[0].fomod.clone();
    assert_eq!(stored, serde_json::to_value(&sport).ok());

    // reinstalled without choices: the saved ones
    let policy = Policy { id: Some("wheels".into()), ..quiet() };
    let (r, out) = install(&paths, &mut st, &FakeNexus::default(), local(), &policy);
    assert_eq!(r.unwrap().requested().change, Change::Replaced);
    assert_eq!(out, ["  FOMOD: using the options you chose last time", "  FOMOD Main/Rims: Sport", "replaced files of wheels: 2 files"]);
    assert_eq!(rims("wheels"), "sport");

    // a new mod, not asked (not a terminal / `ask` off): the installer's defaults
    let policy = Policy { id: Some("wheels-2".into()), ..quiet() };
    let (r, out) = install(&paths, &mut st, &FakeNexus::default(), local(), &policy);
    r.unwrap();
    assert_eq!(out[0], "  FOMOD: using the installer's suggested options");
    assert_eq!(rims("wheels-2"), "stock");
    let _ = fs::remove_dir_all(&paths.data);
}

#[test]
fn replacing_an_installed_version() {
    let paths = test_paths("replace");
    let mut st = State::load(&paths).unwrap();
    let nx = FakeNexus::default();
    let v1 = folder(&paths, "v1", &[("archive/pc/mod/lib.archive", "v1"), ("archive/pc/mod/old.archive", "v1")]);
    let v2 = folder(&paths, "v2", &[("archive/pc/mod/lib.archive", "v2")]);
    let nexus = || Source::Nexus { mod_id: 7, file: None };

    nx.add(7, "Lib", "1.0", &[], v1);
    let (r, out) = install(&paths, &mut st, &nx, nexus(), &quiet());
    assert_eq!(r.unwrap().requested().change, Change::Installed);
    assert_eq!(out, ["Lib (7): file \"Main\" v1.0 [main]", "  installed lib: 2 files"]);

    // the same version again: nothing staged
    let (r, out) = install(&paths, &mut st, &nx, nexus(), &quiet());
    assert_eq!(r.unwrap().requested().change, Change::Unchanged);
    assert_eq!(out[1], "  already installed as lib");

    // a new version replaces the files of the same mod; the old ones go to history
    nx.add(7, "Lib", "2.0", &[], v2);
    let (r, out) = install(&paths, &mut st, &nx, nexus(), &quiet());
    let change = r.unwrap().requested().change.clone();
    assert_eq!(change, Change::Updated { from: "1.0".into(), to: "2.0".into() });
    assert_eq!(out[1], "  updated lib: 1.0 -> 2.0");
    assert_eq!(staged(&paths, "lib"), ["archive/pc/mod/lib.archive"]);
    let saved = State::load(&paths).unwrap();
    assert_eq!((saved.mods.len(), saved.mods[0].version.as_str()), (1, "2.0"));
    assert!(paths.data.join("history/lib").read_dir().unwrap().count() == 1);
    let _ = fs::remove_dir_all(&paths.data);
}

#[test]
fn requirement_cycle_installs_each_mod_once() {
    let paths = test_paths("cycle");
    let mut st = State::load(&paths).unwrap();
    let nx = FakeNexus::default();
    nx.add(1, "Alpha", "1", &[2], folder(&paths, "a", &[("archive/pc/mod/a.archive", "a")]));
    nx.add(2, "Beta", "1", &[1], folder(&paths, "b", &[("archive/pc/mod/b.archive", "b")]));

    let policy = Policy { missing: OnMissing::Install, ..quiet() };
    let (r, out) = install(&paths, &mut st, &nx, Source::Nexus { mod_id: 1, file: None }, &policy);
    let r = r.unwrap();
    let ids: Vec<_> = r.mods.iter().map(|d| (d.id.as_str(), d.depth)).collect();
    assert_eq!(ids, [("beta", 1), ("alpha", 0)], "the requirement finishes first");
    assert_eq!(r.mods[0].requirements[0].fix, Some(Fix::Upstream), "Beta's requirement on Alpha is the install in progress");
    assert_eq!(r.mods[1].requirements[0].fix, Some(Fix::Installed));
    assert_eq!(
        out,
        [
            "Alpha (1): file \"Main\" v1 [main]",
            "  installing requirement 2 Beta",
            "  Beta (2): file \"Main\" v1 [main]",
            "    installed beta: 1 files",
            "  installed alpha: 1 files",
        ]
    );
    assert_eq!(State::load(&paths).unwrap().mods.len(), 2);
    let _ = fs::remove_dir_all(&paths.data);
}

#[test]
fn missing_requirements_refused_or_accepted() {
    let paths = test_paths("missing");
    let mut st = State::load(&paths).unwrap();
    let nx = FakeNexus::default();
    nx.add(3, "Gamma", "1", &[4], folder(&paths, "g", &[("archive/pc/mod/g.archive", "g")]));
    nx.add(4, "Delta", "1", &[], folder(&paths, "d", &[("archive/pc/mod/d.archive", "d")]));
    let gamma = || Source::Nexus { mod_id: 3, file: None };

    let (r, _) = install(&paths, &mut st, &nx, gamma(), &quiet());
    let e = format!("{:#}", r.err().unwrap());
    assert_eq!(e, "missing hard requirements:\n  4 Delta\nre-run with --with-deps to install them, or --ignore-deps");
    assert!(st.mods.is_empty() && !paths.mods().join("gamma").exists(), "nothing staged");

    let (r, out) = install(&paths, &mut st, &nx, gamma(), &Policy { missing: OnMissing::Warn, ..quiet() });
    assert_eq!(r.unwrap().requested().requirements[0].fix, Some(Fix::Accepted));
    assert_eq!(out[1], "  WARNING: requires 4 Delta, which is NOT installed");
    assert_eq!(out[2], "           `cpmm deps --fix` installs it; `cpmm deps --ignore gamma 4` accepts it");
    assert_eq!(st.mods.len(), 1);
    let _ = fs::remove_dir_all(&paths.data);
}

#[test]
fn local_source_tagged_with_nexus_checks_its_requirements() {
    let paths = test_paths("tagged");
    let mut st = State::load(&paths).unwrap();
    let nx = FakeNexus::default();
    // the local archive is Nexus mod 5, which requires 6, which requires 5 again
    nx.add(5, "Epsilon", "1", &[6], PathBuf::new());
    nx.add(6, "Zeta", "1", &[5], folder(&paths, "z", &[("archive/pc/mod/z.archive", "z")]));
    let src = folder(&paths, "epsilon", &[("archive/pc/mod/e.archive", "e")]);

    let policy = Policy { missing: OnMissing::Install, ..quiet() };
    let (r, _) = install(&paths, &mut st, &nx, Source::Local { path: src, nexus: Some(5) }, &policy);
    let r = r.unwrap();
    let ids: Vec<_> = r.mods.iter().map(|d| d.id.as_str()).collect();
    assert_eq!(ids, ["zeta", "epsilon"]);
    let m = &st.mods[st.get("epsilon").unwrap()];
    assert_eq!((m.nexus.as_ref().map(|n| n.mod_id), m.requires.len()), (Some(5), 1));
    let _ = fs::remove_dir_all(&paths.data);
}
