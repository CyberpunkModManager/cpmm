//! FOMOD installers (`fomod/ModuleConfig.xml`): parse the installer, work out the default
//! selection, evaluate conditions and turn a selection into file moves for staging.
//!
//! Supported: required files, install steps with visibility conditions, option groups of every
//! type (SelectExactlyOne / AtMostOne / AtLeastOne / All / Any), plugin types (including
//! dependency-driven types), condition flags, file conditions, conditional file installs,
//! file/folder copies with priorities, and display order. Choices are keyed
//! `"<step name>/<group name>"` -> plugin names, which stays stable across mod updates.

use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct FileOp {
    pub source: String,
    pub destination: String,
    pub folder: bool,
    pub priority: i64,
}

#[derive(Clone, Debug)]
pub enum Cond {
    Flag(String, String),
    File(String, String),
    And(Vec<Cond>),
    Or(Vec<Cond>),
    True,
}

#[derive(Clone, Debug)]
pub struct Plugin {
    pub name: String,
    pub description: String,
    pub image: Option<String>,
    pub files: Vec<FileOp>,
    pub flags: Vec<(String, String)>,
    pub kind: String,
    pub kind_patterns: Vec<(Cond, String)>,
}
#[derive(Clone, Debug)]
pub struct Group {
    pub name: String,
    pub kind: String,
    pub plugins: Vec<Plugin>,
}
#[derive(Clone, Debug)]
pub struct Step {
    pub name: String,
    pub visible: Option<Cond>,
    pub groups: Vec<Group>,
}
#[derive(Clone, Debug)]
pub struct Fomod {
    pub name: String,
    /// path prefix (inside the archive) of the folder that contains `fomod/`
    pub base: String,
    pub required: Vec<FileOp>,
    pub steps: Vec<Step>,
    pub conditional: Vec<(Cond, Vec<FileOp>)>,
}

pub type Choices = BTreeMap<String, Vec<String>>;

/// `fomod/ModuleConfig.xml` inside an extracted archive, if there is one.
pub fn detect(extracted: &[String]) -> Option<String> {
    extracted.iter().filter(|p| p.to_lowercase().ends_with("fomod/moduleconfig.xml")).min_by_key(|p| p.matches('/').count()).cloned()
}

/// XML files from Windows tools are often UTF-16 with a BOM.
fn decode(b: &[u8]) -> String {
    let s = if b.starts_with(&[0xFF, 0xFE]) || b.starts_with(&[0xFE, 0xFF]) {
        let le = b[0] == 0xFF;
        let u: Vec<u16> = b[2..]
            .chunks(2)
            .filter(|c| c.len() == 2)
            .map(|c| if le { u16::from_le_bytes([c[0], c[1]]) } else { u16::from_be_bytes([c[0], c[1]]) })
            .collect();
        String::from_utf16_lossy(&u)
    } else {
        String::from_utf8_lossy(b.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(b)).into_owned()
    };
    // roxmltree rejects encoding declarations that don't match the (already decoded) text
    let re = regex::Regex::new(r#"(?i)^\s*<\?xml[^>]*\?>"#).unwrap();
    re.replace(&s, "").into_owned()
}

fn norm(p: &str) -> String {
    p.replace('\\', "/").trim_matches('/').to_string()
}

fn el<'a>(n: roxmltree::Node<'a, 'a>, name: &str) -> Option<roxmltree::Node<'a, 'a>> {
    n.children().find(|c| c.is_element() && c.tag_name().name().eq_ignore_ascii_case(name))
}
fn els<'a>(n: roxmltree::Node<'a, 'a>, name: &'a str) -> impl Iterator<Item = roxmltree::Node<'a, 'a>> + 'a {
    n.children().filter(move |c| c.is_element() && c.tag_name().name().eq_ignore_ascii_case(name))
}
fn attr(n: roxmltree::Node, a: &str) -> String {
    n.attributes().find(|x| x.name().eq_ignore_ascii_case(a)).map(|x| x.value().to_string()).unwrap_or_default()
}

fn parse_files(n: Option<roxmltree::Node>) -> Vec<FileOp> {
    let Some(n) = n else { return vec![] };
    n.children()
        .filter(|c| c.is_element())
        .filter_map(|c| {
            let folder = c.tag_name().name().eq_ignore_ascii_case("folder");
            if !folder && !c.tag_name().name().eq_ignore_ascii_case("file") {
                return None;
            }
            // a trailing slash marks a folder destination ("archive\\pc\\mod\\"): keep it
            let raw = attr(c, "destination");
            let mut destination = norm(&raw);
            if !destination.is_empty() && (raw.ends_with('/') || raw.ends_with('\\')) {
                destination.push('/');
            }
            Some(FileOp { source: norm(&attr(c, "source")), destination, folder, priority: attr(c, "priority").parse().unwrap_or(0) })
        })
        .collect()
}

fn parse_cond(n: roxmltree::Node) -> Cond {
    let parts: Vec<Cond> = n
        .children()
        .filter(|c| c.is_element())
        .map(|c| match c.tag_name().name().to_lowercase().as_str() {
            "flagdependency" => Cond::Flag(attr(c, "flag"), attr(c, "value")),
            "filedependency" => Cond::File(norm(&attr(c, "file")), attr(c, "state")),
            "dependencies" => parse_cond(c),
            _ => Cond::True, // game / fomm / fose version checks: not applicable here
        })
        .collect();
    if attr(n, "operator").eq_ignore_ascii_case("or") { Cond::Or(parts) } else { Cond::And(parts) }
}

fn ordered<T>(mut v: Vec<T>, order: &str, name: impl Fn(&T) -> String) -> Vec<T> {
    match order.to_lowercase().as_str() {
        "explicit" => v,
        "descending" => {
            v.sort_by_key(|x| std::cmp::Reverse(name(x).to_lowercase()));
            v
        }
        _ => {
            v.sort_by_key(|x| name(x).to_lowercase()); // the FOMOD default is Ascending
            v
        }
    }
}

pub fn parse(xml_bytes: &[u8], config_path: &str) -> Result<Fomod> {
    let text = decode(xml_bytes);
    let doc = roxmltree::Document::parse(&text).context("parsing ModuleConfig.xml")?;
    let root = doc.root_element();
    let base = config_path[..config_path.to_lowercase().rfind("fomod/moduleconfig.xml").unwrap_or(0)].trim_end_matches('/').to_string();
    let name = el(root, "moduleName").and_then(|n| n.text()).unwrap_or("").trim().to_string();
    let required = parse_files(el(root, "requiredInstallFiles"));
    let mut steps = vec![];
    if let Some(is) = el(root, "installSteps") {
        let raw: Vec<Step> = els(is, "installStep")
            .map(|s| {
                let groups = el(s, "optionalFileGroups")
                    .map(|og| {
                        let gs: Vec<Group> = els(og, "group")
                            .map(|g| {
                                let plugins = el(g, "plugins")
                                    .map(|ps| {
                                        let v: Vec<Plugin> = els(ps, "plugin")
                                            .map(|p| {
                                                let td = el(p, "typeDescriptor");
                                                let (kind, kind_patterns) = match td {
                                                    Some(td) => match el(td, "type") {
                                                        Some(t) => (attr(t, "name"), vec![]),
                                                        None => {
                                                            let dt = el(td, "dependencyType");
                                                            let def = dt
                                                                .and_then(|d| el(d, "defaultType"))
                                                                .map(|d| attr(d, "name"))
                                                                .unwrap_or_else(|| "Optional".into());
                                                            let pats = dt
                                                                .and_then(|d| el(d, "patterns"))
                                                                .map(|ps| {
                                                                    els(ps, "pattern")
                                                                        .filter_map(|pt| {
                                                                            Some((
                                                                                parse_cond(el(pt, "dependencies")?),
                                                                                attr(el(pt, "type")?, "name"),
                                                                            ))
                                                                        })
                                                                        .collect()
                                                                })
                                                                .unwrap_or_default();
                                                            (def, pats)
                                                        }
                                                    },
                                                    None => ("Optional".into(), vec![]),
                                                };
                                                Plugin {
                                                    name: attr(p, "name"),
                                                    description: el(p, "description")
                                                        .and_then(|d| d.text())
                                                        .unwrap_or("")
                                                        .trim()
                                                        .to_string(),
                                                    image: el(p, "image").map(|i| norm(&attr(i, "path"))).filter(|s| !s.is_empty()),
                                                    files: parse_files(el(p, "files")),
                                                    flags: el(p, "conditionFlags")
                                                        .map(|cf| {
                                                            els(cf, "flag")
                                                                .map(|f| (attr(f, "name"), f.text().unwrap_or("").trim().to_string()))
                                                                .collect()
                                                        })
                                                        .unwrap_or_default(),
                                                    kind,
                                                    kind_patterns,
                                                }
                                            })
                                            .collect();
                                        ordered(v, &attr(ps, "order"), |p| p.name.clone())
                                    })
                                    .unwrap_or_default();
                                Group { name: attr(g, "name"), kind: attr(g, "type"), plugins }
                            })
                            .collect();
                        ordered(gs, &attr(og, "order"), |g| g.name.clone())
                    })
                    .unwrap_or_default();
                Step { name: attr(s, "name"), visible: el(s, "visible").map(parse_cond), groups }
            })
            .collect();
        steps = ordered(raw, &attr(is, "order"), |s| s.name.clone());
    }
    let conditional: Vec<(Cond, Vec<FileOp>)> = el(root, "conditionalFileInstalls")
        .and_then(|c| el(c, "patterns"))
        .map(|ps| els(ps, "pattern").filter_map(|p| Some((parse_cond(el(p, "dependencies")?), parse_files(el(p, "files"))))).collect())
        .unwrap_or_default();
    if steps.is_empty() && required.is_empty() && conditional.is_empty() {
        bail!("the FOMOD installer has nothing to install");
    }
    Ok(Fomod { name, base, required, steps, conditional })
}

pub fn eval(c: &Cond, flags: &BTreeMap<String, String>, file_active: &dyn Fn(&str) -> bool) -> bool {
    match c {
        Cond::True => true,
        Cond::Flag(f, v) => flags.get(f).map(String::as_str).unwrap_or("") == v,
        Cond::File(f, state) => match state.to_lowercase().as_str() {
            "missing" => !file_active(f),
            _ => file_active(f), // Active / Inactive both mean "present" for us
        },
        Cond::And(v) => v.iter().all(|x| eval(x, flags, file_active)),
        Cond::Or(v) => v.iter().any(|x| eval(x, flags, file_active)),
    }
}

fn plugin_kind(p: &Plugin, flags: &BTreeMap<String, String>, fa: &dyn Fn(&str) -> bool) -> String {
    p.kind_patterns.iter().find(|(c, _)| eval(c, flags, fa)).map(|(_, k)| k.clone()).unwrap_or_else(|| p.kind.clone())
}

fn key(s: &Step, g: &Group) -> String {
    format!("{}/{}", s.name, g.name)
}

/// The installer's own suggestion for one group.
fn default_for(g: &Group, flags: &BTreeMap<String, String>, fa: &dyn Fn(&str) -> bool) -> Vec<String> {
    let kinds: Vec<String> = g.plugins.iter().map(|p| plugin_kind(p, flags, fa).to_lowercase()).collect();
    let usable: Vec<usize> = (0..g.plugins.len()).filter(|&i| kinds[i] != "notusable").collect();
    let mut pick: Vec<usize> = (0..g.plugins.len()).filter(|&i| kinds[i] == "required" || kinds[i] == "recommended").collect();
    match g.kind.to_lowercase().as_str() {
        "selectall" => pick = usable.clone(),
        "selectexactlyone" => {
            pick.truncate(1);
            if pick.is_empty() {
                pick = usable.iter().take(1).copied().collect();
            }
        }
        "selectatleastone" if pick.is_empty() => pick = usable.iter().take(1).copied().collect(),
        "selectatmostone" => pick.truncate(1),
        _ => {}
    }
    pick.into_iter().map(|i| g.plugins[i].name.clone()).collect()
}

#[derive(Debug)]
pub struct Resolved {
    pub ops: Vec<FileOp>,
    pub choices: Choices,
}

/// Walk the installer with the given choices (defaults where a group has none).
pub fn resolve(f: &Fomod, choices: &Choices, fa: &dyn Fn(&str) -> bool) -> Result<Resolved> {
    let mut flags = BTreeMap::new();
    let mut ops = f.required.clone();
    let mut used = Choices::new();
    for s in &f.steps {
        if s.visible.as_ref().is_some_and(|c| !eval(c, &flags, fa)) {
            continue;
        }
        for g in &s.groups {
            let k = key(s, g);
            let sel = choices.get(&k).cloned().unwrap_or_else(|| default_for(g, &flags, fa));
            let names: BTreeSet<&str> = g.plugins.iter().map(|p| p.name.as_str()).collect();
            if let Some(bad) = sel.iter().find(|n| !names.contains(n.as_str())) {
                bail!("{k}: no option named {bad:?}");
            }
            let n = sel.len();
            let ok = match g.kind.to_lowercase().as_str() {
                "selectexactlyone" => n == 1,
                "selectatmostone" => n <= 1,
                "selectatleastone" => n >= 1,
                _ => true,
            };
            if !ok {
                bail!("{k}: {} needs a different number of choices (got {n})", g.kind);
            }
            for p in g.plugins.iter() {
                let kind = plugin_kind(p, &flags, fa).to_lowercase();
                let chosen = sel.contains(&p.name) || kind == "required" || g.kind.eq_ignore_ascii_case("selectall");
                if chosen && kind != "notusable" {
                    ops.extend(p.files.iter().cloned());
                    for (fl, v) in &p.flags {
                        flags.insert(fl.clone(), v.clone());
                    }
                }
            }
            used.insert(k, sel);
        }
    }
    for (c, files) in &f.conditional {
        if eval(c, &flags, fa) {
            ops.extend(files.iter().cloned());
        }
    }
    ops.sort_by_key(|o| o.priority); // stable: later and higher-priority entries win
    Ok(Resolved { ops, choices: used })
}

/// File operations -> (path inside the extracted tree, game-relative destination).
pub fn moves(f: &Fomod, ops: &[FileOp], extracted: &[String]) -> Result<Vec<(String, String)>> {
    let lower: BTreeMap<String, &String> = extracted.iter().map(|p| (p.to_lowercase(), p)).collect();
    let join = |a: &str, b: &str| {
        if a.is_empty() {
            b.to_string()
        } else if b.is_empty() {
            a.to_string()
        } else {
            format!("{a}/{b}")
        }
    };
    let mut out: BTreeMap<String, (String, String)> = BTreeMap::new(); // dest(lower) -> (src, dest)
    for op in ops {
        let src = join(&f.base, &op.source).to_lowercase();
        if op.folder {
            let prefix = if src.is_empty() { String::new() } else { format!("{src}/") };
            let mut any = false;
            for (l, orig) in lower.range(prefix.clone()..) {
                if !l.starts_with(&prefix) {
                    break;
                }
                if l.contains("fomod/") {
                    continue;
                }
                let rest = &orig[prefix.len()..];
                let dst = join(&op.destination, rest);
                out.insert(dst.to_lowercase(), ((*orig).clone(), dst));
                any = true;
            }
            if !any && !src.is_empty() {
                bail!("the installer references a missing folder {:?}", op.source);
            }
        } else {
            let orig = lower.get(&src).with_context(|| format!("the installer references a missing file {:?}", op.source))?;
            let dst = if op.destination.is_empty() {
                op.source.clone()
            } else if op.destination.ends_with('/') {
                format!("{}{}", op.destination, orig.rsplit('/').next().unwrap_or(orig))
            } else {
                op.destination.clone()
            };
            out.insert(dst.to_lowercase(), ((*orig).clone(), norm(&dst)));
        }
    }
    if out.is_empty() {
        bail!("these options install no files (installers often keep the stock look by default); pick some options");
    }
    Ok(out.into_values().collect())
}

/// Options for the GUI: visible steps for the given choices, defaults filled in, images inline.
pub fn describe(f: &Fomod, choices: &Choices, root: &Path, fa: &dyn Fn(&str) -> bool) -> Value {
    let mut flags = BTreeMap::new();
    let mut steps = vec![];
    for s in &f.steps {
        let visible = s.visible.as_ref().is_none_or(|c| eval(c, &flags, fa));
        let mut groups = vec![];
        for g in &s.groups {
            let k = key(s, g);
            let sel = choices.get(&k).cloned().unwrap_or_else(|| default_for(g, &flags, fa));
            let plugins: Vec<Value> = g
                .plugins
                .iter()
                .map(|p| {
                    let kind = plugin_kind(p, &flags, fa);
                    json!({
                        "name": p.name,
                        "description": p.description,
                        "kind": kind,
                        "selected": sel.contains(&p.name) || kind.eq_ignore_ascii_case("required"),
                        "image": p.image.as_ref().and_then(|i| image_data(root, &f.base, i)),
                    })
                })
                .collect();
            if visible {
                for p in g.plugins.iter().filter(|p| sel.contains(&p.name)) {
                    for (fl, v) in &p.flags {
                        flags.insert(fl.clone(), v.clone());
                    }
                }
            }
            groups.push(json!({"key": k, "name": g.name, "kind": g.kind, "plugins": plugins}));
        }
        steps.push(json!({"name": s.name, "visible": visible, "groups": groups}));
    }
    json!({"name": f.name, "steps": steps})
}

/// Small preview images as data: URIs (the webview can't read arbitrary local files).
fn image_data(root: &Path, base: &str, rel: &str) -> Option<String> {
    let want = if base.is_empty() { rel.to_lowercase() } else { format!("{base}/{rel}").to_lowercase() };
    let path = crate::files::list_tree(root).into_iter().find(|p| p.to_lowercase() == want)?;
    let b = std::fs::read(root.join(&path)).ok()?;
    if b.len() > 700 << 10 {
        return None;
    }
    let mime = match path.rsplit('.').next()?.to_lowercase().as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        _ => return None,
    };
    use std::fmt::Write;
    // base64 by hand: no extra dependency for a few images
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(b.len() * 4 / 3 + 32);
    let _ = write!(s, "data:{mime};base64,");
    for c in b.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                s.push(T[(n >> (18 - 6 * i)) as usize & 63] as char);
            } else {
                s.push('=');
            }
        }
    }
    Some(s)
}

/// Terminal prompts, one group at a time (Enter keeps the installer's suggestion).
pub fn ask(f: &Fomod, fa: &dyn Fn(&str) -> bool) -> Result<Choices> {
    use std::io::Write;
    let mut choices = Choices::new();
    let mut flags = BTreeMap::new();
    println!("\nFOMOD installer: {}", if f.name.is_empty() { "(unnamed)" } else { &f.name });
    for s in &f.steps {
        if s.visible.as_ref().is_some_and(|c| !eval(c, &flags, fa)) {
            continue;
        }
        for g in &s.groups {
            let def = default_for(g, &flags, fa);
            println!("\n[{}] {} ({})", s.name, g.name, g.kind);
            for (i, p) in g.plugins.iter().enumerate() {
                let kind = plugin_kind(p, &flags, fa);
                let first = p.description.lines().next().unwrap_or("");
                println!(
                    "  {}{} {}  [{}]  {}",
                    if def.contains(&p.name) { "*" } else { " " },
                    i + 1,
                    p.name,
                    kind,
                    first.chars().take(90).collect::<String>()
                );
            }
            let sel = loop {
                print!("choose (numbers separated by spaces, Enter = *): ");
                std::io::stdout().flush()?;
                let mut l = String::new();
                std::io::stdin().read_line(&mut l)?;
                let l = l.trim();
                if l.is_empty() {
                    break def.clone();
                }
                let picked: Option<Vec<String>> = l
                    .split_whitespace()
                    .map(|n| n.parse::<usize>().ok().and_then(|n| g.plugins.get(n.wrapping_sub(1))).map(|p| p.name.clone()))
                    .collect();
                match picked {
                    Some(v) => break v,
                    None => println!("  numbers from 1 to {}", g.plugins.len()),
                }
            };
            for p in g.plugins.iter().filter(|p| sel.contains(&p.name)) {
                for (fl, v) in &p.flags {
                    flags.insert(fl.clone(), v.clone());
                }
            }
            choices.insert(key(s, g), sel);
        }
    }
    Ok(choices)
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = r#"<?xml version="1.0" encoding="UTF-16"?>
<config xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">
  <moduleName>Car Pack</moduleName>
  <requiredInstallFiles><folder source="Core" destination="" /></requiredInstallFiles>
  <installSteps order="Explicit">
    <installStep name="Main">
      <optionalFileGroups order="Explicit">
        <group name="Wheels" type="SelectExactlyOne">
          <plugins order="Explicit">
            <plugin name="Stock"><description>Original</description>
              <files><file source="opt\stock.archive" destination="archive\pc\mod\wheels.archive" /></files>
              <typeDescriptor><type name="Recommended"/></typeDescriptor></plugin>
            <plugin name="Sport"><description>Sport rims</description>
              <files><file source="opt\sport.archive" destination="archive\pc\mod\wheels.archive" /></files>
              <conditionFlags><flag name="sport">on</flag></conditionFlags>
              <typeDescriptor><type name="Optional"/></typeDescriptor></plugin>
          </plugins>
        </group>
      </optionalFileGroups>
    </installStep>
    <installStep name="Extras">
      <visible><flagDependency flag="sport" value="on"/></visible>
      <optionalFileGroups><group name="Spoiler" type="SelectAny"><plugins>
        <plugin name="Wing"><description/><files><file source="opt\wing.archive" destination="archive\pc\mod\" /></files>
          <typeDescriptor><type name="Optional"/></typeDescriptor></plugin>
      </plugins></group></optionalFileGroups>
    </installStep>
  </installSteps>
  <conditionalFileInstalls><patterns><pattern>
    <dependencies operator="And"><flagDependency flag="sport" value="on"/></dependencies>
    <files><file source="opt\sport.xl" destination="archive\pc\mod\sport.xl"/></files>
  </pattern></patterns></conditionalFileInstalls>
</config>"#;

    fn tree() -> Vec<String> {
        [
            "Pack/fomod/ModuleConfig.xml",
            "Pack/Core/r6/scripts/Car/car.reds",
            "Pack/opt/Stock.archive",
            "Pack/opt/sport.archive",
            "Pack/opt/wing.archive",
            "Pack/opt/sport.xl",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    fn utf16(s: &str) -> Vec<u8> {
        let mut b = vec![0xFF, 0xFE];
        for u in s.encode_utf16() {
            b.extend_from_slice(&u.to_le_bytes());
        }
        b
    }

    #[test]
    fn defaults_follow_the_installer() {
        let cfg = detect(&tree()).unwrap();
        let f = parse(&utf16(XML), &cfg).unwrap();
        assert_eq!((f.name.as_str(), f.base.as_str(), f.steps.len()), ("Car Pack", "Pack", 2));
        let r = resolve(&f, &Choices::new(), &|_| false).unwrap();
        let mv = moves(&f, &r.ops, &tree()).unwrap();
        // required folder + the recommended "Stock" wheels (source matched case-insensitively)
        assert!(mv.contains(&("Pack/Core/r6/scripts/Car/car.reds".into(), "r6/scripts/Car/car.reds".into())));
        assert!(mv.contains(&("Pack/opt/Stock.archive".into(), "archive/pc/mod/wheels.archive".into())));
        assert_eq!(mv.len(), 2, "extras step is hidden and the conditional file needs the flag: {mv:?}");
    }

    #[test]
    fn choices_set_flags_steps_and_conditionals() {
        let f = parse(XML.as_bytes(), "Pack/fomod/ModuleConfig.xml").unwrap();
        let mut c = Choices::new();
        c.insert("Main/Wheels".into(), vec!["Sport".into()]);
        c.insert("Extras/Spoiler".into(), vec!["Wing".into()]);
        let r = resolve(&f, &c, &|_| false).unwrap();
        let mv = moves(&f, &r.ops, &tree()).unwrap();
        let dsts: Vec<&str> = mv.iter().map(|(_, d)| d.as_str()).collect();
        assert!(
            dsts.contains(&"archive/pc/mod/wheels.archive")
                && dsts.contains(&"archive/pc/mod/wing.archive")
                && dsts.contains(&"archive/pc/mod/sport.xl")
        );
        assert!(mv.iter().any(|(s, d)| s == "Pack/opt/sport.archive" && d == "archive/pc/mod/wheels.archive"));
        // validation
        let mut bad = Choices::new();
        bad.insert("Main/Wheels".into(), vec![]);
        assert!(resolve(&f, &bad, &|_| false).is_err());
        bad.insert("Main/Wheels".into(), vec!["Nope".into()]);
        assert!(resolve(&f, &bad, &|_| false).is_err());
    }
}
