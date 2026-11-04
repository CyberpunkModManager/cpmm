//! The terminal side of the CLI adapter: its [`Ui`] (events printed as they happen, questions
//! asked on stdin) and the text of the read-only commands.

use cpmm::fomod::{self, Choices, Fomod};
use cpmm::ops::query::{self, ConflictReport, Detail, LaunchOptions, UpdateStatus};
use cpmm::ops::render::{self, Line};
use cpmm::ops::{self, Event, Ui};
use cpmm::requirements::Status;
use cpmm::state::{Paths, State};
use cpmm::{downloads, health, loadorder, manage, modio, profiles, safety, settings};
use std::collections::BTreeMap;
use std::io::Write;

/// The terminal: events on stdout/stderr, questions on stdin.
pub struct Term;

fn emit(l: &Line) {
    match (l.err, l.open) {
        (true, true) => eprint!("{}", l.text),
        (true, false) => eprintln!("{}", l.text),
        (false, true) => print!("{}", l.text),
        (false, false) => println!("{}", l.text),
    }
}

fn tty() -> bool {
    unsafe { libc::isatty(0) == 1 && libc::isatty(1) == 1 }
}

impl Ui for Term {
    fn event(&mut self, e: &Event) {
        for l in render::lines(e) {
            emit(&l);
        }
    }

    /// One group at a time; Enter keeps the installer's suggestion. Only on a terminal.
    fn choose_fomod(&mut self, f: &Fomod, active: &dyn Fn(&str) -> bool) -> Option<anyhow::Result<Choices>> {
        if !tty() {
            return None;
        }
        println!("\nFOMOD installer: {}", if f.name.is_empty() { "(unnamed)" } else { &f.name });
        Some(fomod::choose(f, active, &mut |g| {
            println!("\n[{}] {} ({})", g.step, g.group, g.kind);
            for (i, p) in g.options.iter().enumerate() {
                let first = p.description.lines().next().unwrap_or("");
                println!(
                    "  {}{} {}  [{}]  {}",
                    if p.default { "*" } else { " " },
                    i + 1,
                    p.name,
                    p.kind,
                    first.chars().take(90).collect::<String>()
                );
            }
            loop {
                print!("choose (numbers separated by spaces, Enter = *): ");
                std::io::stdout().flush()?;
                let mut l = String::new();
                std::io::stdin().read_line(&mut l)?;
                let l = l.trim();
                if l.is_empty() {
                    return Ok(g.options.iter().filter(|p| p.default).map(|p| p.name.to_string()).collect());
                }
                let picked: Option<Vec<String>> = l
                    .split_whitespace()
                    .map(|n| n.parse::<usize>().ok().and_then(|n| g.options.get(n.wrapping_sub(1))).map(|p| p.name.to_string()))
                    .collect();
                match picked {
                    Some(v) => return Ok(v),
                    None => println!("  numbers from 1 to {}", g.options.len()),
                }
            }
        }))
    }

    fn ask(&mut self, q: &str, default: &str) -> Option<String> {
        print!("{q}{} ", if default.is_empty() { String::new() } else { format!(" [{default}]") });
        let _ = std::io::stdout().flush();
        let mut l = String::new();
        let _ = std::io::stdin().read_line(&mut l);
        Some(l.trim().to_string())
    }
}

pub fn lines(v: Vec<String>) {
    for l in v {
        println!("{l}");
    }
}

/// Nexus quota warnings a query raised.
pub fn warnings() {
    for n in ops::warnings() {
        Term.event(&Event::Note(n));
    }
}

pub fn list(paths: &Paths, st: &State, long: bool) {
    if st.profiles.len() > 1 {
        println!("profile: {} (cpmm profile list)", st.active_profile);
    }
    println!("{:>3} {:<3} {:<34} {:<12} {:>6}  name", "#", "on", "id", "version", "nexus");
    for (i, r) in query::list(paths, st, long).iter().enumerate() {
        let m = r.m;
        let nid = m.nexus.as_ref().map(|n| n.mod_id.to_string()).unwrap_or("-".into());
        let extra = match r.counts {
            Some((files, won, lost)) => format!(
                "  [{files} files{}{}]",
                if won > 0 { format!(", wins {won}") } else { "".into() },
                if lost > 0 { format!(", loses {lost}") } else { "".into() }
            ),
            None => String::new(),
        };
        println!(
            "{:>3} {:<3} {:<34} {:<12} {:>6}  {}{}",
            i + 1,
            if m.enabled { "x" } else { "" },
            m.id,
            m.version.chars().take(12).collect::<String>(),
            nid,
            m.name,
            extra
        );
    }
    let mio = modio::list(paths);
    if !mio.is_empty() {
        println!("\nmod.io (managed in-game via the Mods menu, outside the game dir):");
        for m in mio {
            println!("    {:<10} {} {} by {}", m.id, m.name, m.version, m.author);
        }
    }
}

pub fn profiles(paths: &Paths, mut st: State) {
    st.sync_active_profile();
    for p in &st.profiles {
        let active = p.name == st.active_profile;
        let saves = match p.own_saves_dir(paths) {
            Some(d) => format!("own saves ({})", profiles::count_saves(&d)),
            None => "shared saves".into(),
        };
        let summary = format!("{} of {} mods enabled", p.enabled.len(), st.mods.len());
        println!("{} {:<24} {:<26} {saves}", if active { "*" } else { " " }, p.name, summary);
    }
}

pub fn info(d: &Detail) {
    let m = d.m;
    println!("{}  ({})\n  enabled: {}\n  version: {}\n  installed: {}", m.name, m.id, m.enabled, m.version, m.installed_at);
    if let Some(n) = &m.nexus {
        println!(
            "  nexus: https://www.nexusmods.com/cyberpunk2077/mods/{}  file {:?} (id {}, version id {}, uploaded {})",
            n.mod_id, n.file_name, n.file_id, n.version_id, n.uploaded_at
        );
    }
    for c in &d.requires {
        let r = c.req;
        let have = match c.status {
            Status::Ok => "ok",
            Status::Disabled | Status::OptionalDisabled => "DISABLED",
            Status::Missing | Status::OptionalMissing => "MISSING",
            Status::Ignored if c.dep.is_some() => "DISABLED",
            Status::Ignored => "MISSING",
        };
        println!(
            "  requires {} {} [{}]{}{}",
            r.mod_id,
            r.name,
            have,
            if r.optional { " (optional)" } else { "" },
            if r.notes.is_empty() { String::new() } else { format!(" — {}", r.notes) }
        );
    }
    if !d.hidden.is_empty() {
        println!("  hidden ({}): {}", d.hidden.len(), d.hidden.join(", "));
    }
    println!("  files ({}):", d.files.len());
    for f in &d.files {
        println!("    {f}");
    }
}

pub fn conflicts(r: &ConflictReport, base: bool) {
    let (mut real, mut same) = (0, 0);
    println!("== file overwrites (last listed wins)");
    for c in r.files.iter().filter(|c| c.mods.len() > 1) {
        if c.identical {
            same += 1;
            continue;
        }
        real += 1;
        println!("  {}  {}", c.path, c.mods.join(" < "));
    }
    println!("  {real} differing, {same} identical copies (harmless)");
    let based: Vec<_> = r.files.iter().filter(|c| c.overrides_base).collect();
    if base {
        println!("== overriding base-game files");
        for c in &based {
            println!("  {}  {}", c.path, c.mods.join(" < "));
        }
    } else {
        println!("  {} files override base-game files (--base to list)", based.len());
    }
    println!("== redscript");
    for c in &r.scripts {
        println!("  {} {}: {}", c.kind, c.target, c.mods.join(", "));
    }
    if r.scripts.is_empty() {
        println!("  none");
    }
}

pub fn updates(st: &State, res: &[UpdateStatus]) {
    let mut n = 0;
    for u in res {
        if u.status != "up to date" {
            n += u.newer.is_some() as usize;
            println!("  {:<34} {}", u.id, u.status);
        }
    }
    let manual = st.mods.iter().filter(|m| m.nexus.is_none()).count();
    println!(
        "{} checked, {n} updates, {} up to date, {manual} manual mods not checked",
        res.len(),
        res.iter().filter(|u| u.status == "up to date").count()
    );
    warnings();
}

pub fn handler() {
    let h = manage::current_nxm_handler();
    println!("nxm:// handler: {}", if h.is_empty() { "(none)" } else { &h });
}

pub fn unmanaged(paths: &Paths, st: &State, modified: bool, flat: bool) -> anyhow::Result<()> {
    if query::session(paths).is_some() {
        eprintln!("note: session running; this shows the real base dir, not the overlay");
    }
    let u = cpmm::vanilla::unmanaged(paths, st, modified)?;
    println!("== unmanaged non-vanilla files{}", if flat { "" } else { " (by folder)" });
    if flat {
        for f in u.groups.values().flatten() {
            println!("  {f}");
        }
    }
    for (k, v) in u.groups.iter().filter(|_| !flat) {
        if v.len() == 1 && v[0] == *k {
            println!("  {k}");
        } else {
            println!("  {k}/  ({} files)", v.len());
        }
    }
    if modified {
        println!("== vanilla files that differ from Steam's copy");
        for f in u.edited {
            println!("  {f}");
        }
    }
    Ok(())
}

pub fn overwrite(paths: &Paths, st: &State) {
    let ow = manage::overwrite_files(paths, st);
    for e in &ow {
        let by = e.shadows.as_ref().map(|s| format!(" ({s})")).unwrap_or_default();
        println!("  {:<16} {}{by}", e.kind.as_str(), e.rel);
    }
    println!("{} files in {}", ow.len(), paths.upper().display());
}

pub fn rules(st: &State) {
    if st.rules.is_empty() {
        println!("no rules");
    }
    for r in &st.rules {
        println!("  {} after {}", r.r#mod, r.after);
    }
}

pub fn archives(paths: &Paths, st: &State, all: bool) {
    let ids: Vec<String> = st.mods.iter().map(|m| m.id.clone()).collect();
    let ps = loadorder::archive_conflicts(paths, st, &ids);
    let mut n = 0;
    for p in ps.iter().filter(|p| all || p.identical < p.shared) {
        n += 1;
        println!(
            "  {} ({}) vs {} ({}): {} shared, {} identical{}  winner: {}",
            p.a.label(),
            p.archive_a,
            p.b.label(),
            p.archive_b,
            p.shared,
            p.identical,
            if p.related { ", related by requirements" } else { "" },
            p.winner.clone().unwrap_or("game order".into())
        );
    }
    println!("{n} conflicting archive pairs ({} total overlaps)", ps.len());
}

pub fn doctor(paths: &Paths, st: &State, all: bool) {
    let fs_: Vec<health::Finding> = health::check(paths, st).into_iter().filter(|f| all || f.severity != "acked").collect();
    for f in &fs_ {
        let tag = match f.severity {
            "crash" => "CRASH RISK",
            "warning" => "WARNING",
            "perf" => "PERF",
            "acked" => "acked",
            _ => "info",
        };
        println!("[{tag}] {}", f.title);
        if !f.detail.is_empty() {
            println!("    {}", f.detail);
        }
        if let Some(x) = &f.fix {
            println!("    fix: {x}");
        }
        for a in &f.actions {
            println!(
                "    {}: cpmm {}",
                a.label,
                a.args.iter().map(|x| if x.contains(' ') { format!("{x:?}") } else { x.clone() }).collect::<Vec<_>>().join(" ")
            );
        }
    }
    println!("{} findings ({} crash risks)", fs_.len(), fs_.iter().filter(|f| f.severity == "crash").count());
}

pub fn history(paths: &Paths, st: &State, key: &str) -> anyhow::Result<()> {
    let i = st.get(key)?;
    let h = safety::list_history(paths, &st.mods[i].id);
    if h.is_empty() {
        println!("no earlier versions of {} kept", st.mods[i].id);
    }
    for e in h {
        println!(
            "  {}  {}  v{}  {} files  {}{}",
            e.meta.ts,
            safety::iso(e.meta.ts),
            e.meta.version,
            e.files,
            e.meta.reason,
            e.meta.batch.map(|b| format!("  [{b}]")).unwrap_or_default()
        );
    }
    Ok(())
}

pub fn crashes(paths: &Paths, limit: usize) {
    let cs = safety::crashes(paths, limit);
    if cs.is_empty() {
        println!("no crash reports");
    }
    for c in cs {
        println!("{}  {}  in {} {}", safety::iso(c.ts), c.message, c.module, c.offset);
        match (c.seconds_in, c.last_good) {
            (Some(s), Some(g)) => {
                println!("    {}m{}s into the session; changes since the last good session ({}):", s / 60, s % 60, safety::iso(g))
            }
            (Some(s), None) => println!("    {}m{}s into the session (no earlier clean session recorded)", s / 60, s % 60),
            _ => println!("    before cpmm started recording sessions"),
        }
        for ch in &c.changes {
            println!("      {ch}");
        }
    }
}

pub fn patch(paths: &Paths, st: &State) {
    let p = safety::patch_status(paths, st, Some(cpmm::nexus::Catalogue::open(paths).gate()));
    println!("game build {} (last updated {})", p.build_id, if p.patched_at > 0 { safety::iso(p.patched_at) } else { "unknown".into() });
    if p.patch_detected {
        println!("the game updated since cpmm last saw it (was build {})", p.previous_build.unwrap_or_default());
        for f in &p.at_risk {
            println!(
                "  {:<30} {}{}",
                f.name,
                if f.released_since_patch { "" } else { "no release since the patch" },
                if f.update_available { " - update available" } else { "" }
            );
        }
    } else {
        println!("no game update since cpmm started tracking");
    }
    for e in &p.red4ext_errors {
        println!("  RED4ext: {e}");
    }
    if p.vanilla {
        println!("next launch: VANILLA (cpmm vanilla off to cancel)");
    }
}

pub fn vanilla(paths: &Paths) {
    println!("next launch: {}", if safety::vanilla_flag(paths).exists() { "vanilla" } else { "modded" });
}

pub fn saves(paths: &Paths, st: &State) {
    for s in safety::list_snapshots(paths) {
        let prof = if s.profile.is_empty() || st.profiles.len() < 2 { String::new() } else { format!("  [{}]", s.profile) };
        println!("  {}  {}  {} saves  {}{prof}", s.name, safety::iso(s.ts), s.saves, s.reason);
    }
}

pub fn changelog(c: &query::Changelog, all: bool) {
    if c.entries.is_empty() {
        println!("{} has no changelog on Nexus", c.m.name);
    }
    for (shown, e) in c.entries.iter().enumerate() {
        if !all && !e.newer && shown > 0 {
            break;
        }
        let tag = if e.installed {
            "  (installed)"
        } else if e.newer {
            "  (new)"
        } else {
            ""
        };
        println!("{}{tag}", e.version);
        for l in &e.changes {
            println!("  - {l}");
        }
        if !all && !e.newer {
            break; // the installed/older entry gives context; stop there
        }
    }
}

pub fn modio(paths: &Paths, st: &State) {
    let mods = modio::list(paths);
    if mods.is_empty() {
        println!("no mod.io subscriptions (the in-game Mods menu manages them)");
    }
    let cs = modio::conflicts(paths, st);
    for m in &mods {
        let mine: Vec<&modio::Conflict> = cs.iter().filter(|c| c.modio == m.name).collect();
        println!(
            "  {:<10} {} {} by {}{}",
            m.id,
            m.name,
            m.version,
            m.author,
            if mine.is_empty() { String::new() } else { format!("  [{} conflicts]", mine.len()) }
        );
        for c in mine {
            println!("      {}{} with {}: {}", if c.severe { "! " } else { "" }, c.kind, c.with, c.detail);
        }
    }
    let ch = modio::changes(paths);
    if ch.first_look && !mods.is_empty() {
        println!("first look: `cpmm modio --ack` marks these as reviewed; cpmm then reports new subscriptions");
    } else {
        for a in &ch.added {
            println!("  NEW since last review: {a}");
        }
        for u in &ch.updated {
            println!("  updated: {u}");
        }
        for r in &ch.removed {
            println!("  unsubscribed: {r}");
        }
    }
}

pub fn collection(v: &query::CollectionView, reference: &str) {
    let rev = v.revision.map_or("null".into(), |r| r.to_string());
    println!("{} (revision {rev}) by {}", v.name, v.author);
    println!("{} mods ({} optional), {:.1} GB, {} load-order rules", v.mods.len(), v.optional, v.total_size / 1e9, v.rules);
    let mut by: BTreeMap<&str, usize> = BTreeMap::new();
    for m in &v.mods {
        *by.entry(m.status).or_default() += 1;
    }
    println!("status: {}", by.iter().map(|(k, n)| format!("{n} {k}")).collect::<Vec<_>>().join(", "));
    println!("install with: cpmm collection install {reference}");
}

pub fn collections(st: &State) {
    if st.collections.is_empty() {
        println!("no collections installed through cpmm");
    }
    for c in &st.collections {
        println!("  {}  {} (revision {}), {} mods, profile {:?}, {}", c.slug, c.name, c.revision, c.mods.len(), c.profile, c.installed_at);
    }
}

pub fn settings(paths: &Paths, json: bool) -> anyhow::Result<()> {
    let cur = &paths.cfg;
    if json {
        println!("{}", serde_json::to_string_pretty(cur)?);
    } else {
        let v = serde_json::to_value(cur)?;
        for f in settings::schema() {
            println!("  {:<24} {:<10} {}", f.key, v[f.key].to_string().trim_matches('"'), f.label);
        }
        println!("staging: {}\ndownloads: {}", paths.mods().display(), paths.downloads().display());
    }
    Ok(())
}

pub fn downloads(paths: &Paths, st: &State) {
    let all = downloads::list(paths, st);
    if all.is_empty() {
        println!("no downloads in {}", paths.downloads().display());
    }
    for (i, d) in all.iter().enumerate() {
        println!(
            "{:>3}  {:<16} {:<42} {:<10} {:>8.1} MB  {}{}",
            i + 1,
            d.status,
            d.meta.name.chars().take(42).collect::<String>(),
            d.meta.version.chars().take(10).collect::<String>(),
            d.size as f64 / 1048576.0,
            d.meta.source,
            d.installed_as.as_ref().map(|x| format!("  ({x})")).unwrap_or_default()
        );
    }
}

pub fn status(paths: &Paths) {
    match query::session(paths) {
        Some((info, pid)) => {
            println!("session running since {} (pid {pid}), {} layers:", info.started, info.layers.len());
            if let Some(s) = &info.saves {
                println!("  saves: {}", s.display());
            }
            for l in info.layers {
                println!("  {l}");
            }
        }
        None => println!("no session running"),
    }
}

pub fn launch_options(lo: &LaunchOptions) {
    match &lo.current {
        Some(c) if lo.ok => println!("already launching through cpmm:\n  {c}"),
        Some(c) => println!("current:  {c}\nwith cpmm: {}\n(`cpmm launch-options --apply` sets it)", lo.with_cpmm),
        None => println!("Cyberpunk isn't in Steam's local config yet; start it once from Steam. Use:\n  {}", lo.with_cpmm),
    }
}
