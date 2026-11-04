//! Nexus Collections: read a revision's manifest (`collection.json`), and turn its mods, FOMOD
//! choices and the curator's load-order rules into cpmm installs and rules.

use crate::fomod::Choices;
use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;

/// `iszwwe`, `https://www.nexusmods.com/games/cyberpunk2077/collections/iszwwe`,
/// `…/collections/iszwwe/revisions/483` -> (slug, revision).
pub fn parse_ref(s: &str) -> Result<(String, Option<u64>)> {
    let s = s.trim();
    let after = match s.find("collections/") {
        Some(i) => &s[i + "collections/".len()..],
        None => s,
    };
    let mut parts = after.split(['/', '?', '#']);
    let slug = parts.next().unwrap_or("").to_string();
    if slug.is_empty() || !slug.chars().all(|c| c.is_ascii_alphanumeric()) {
        bail!("not a collection slug or URL: {s:?}");
    }
    let rest: Vec<&str> = parts.collect();
    let rev = rest.iter().position(|p| *p == "revisions").and_then(|i| rest.get(i + 1)).and_then(|r| r.parse().ok());
    Ok((slug, rev))
}

#[derive(Serialize, Clone, Debug)]
pub struct CollMod {
    pub name: String,
    pub version: String,
    pub optional: bool,
    pub phase: u64,
    /// "nexus", "bundle", "browse", "direct", …
    pub source: String,
    pub mod_id: u64,
    /// the file's game-scoped (legacy) id, which cpmm downloads by
    pub file_id: String,
    pub logical: String,
    pub bundle: String,
    pub choices: Option<Choices>,
    pub has_patches: bool,
    pub instructions: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct CollRule {
    pub kind: String,
    pub source: (u64, String),
    pub reference: (u64, String),
}

#[derive(Serialize, Clone, Debug)]
pub struct Manifest {
    pub name: String,
    pub author: String,
    pub instructions: String,
    pub mods: Vec<CollMod>,
    pub rules: Vec<CollRule>,
}

/// FOMOD choices in a collection: options[step].groups[group].choices[name]
fn choices_from(v: &Value) -> Option<Choices> {
    if v["type"].as_str() != Some("fomod") {
        return None;
    }
    let mut out = Choices::new();
    for step in v["options"].as_array()? {
        let sname = step["name"].as_str().unwrap_or("");
        for g in step["groups"].as_array().cloned().unwrap_or_default() {
            let names =
                g["choices"].as_array().cloned().unwrap_or_default().iter().filter_map(|c| c["name"].as_str().map(String::from)).collect();
            out.insert(format!("{sname}/{}", g["name"].as_str().unwrap_or("")), names);
        }
    }
    Some(out)
}

/// Mod id from a rule's `fileExpression` ("Name-1712-4-3-0-1775269435" or "Name 21865 4.0 …").
fn expr_mod_id(expr: &str, known: &[u64]) -> u64 {
    expr.split(|c: char| !c.is_ascii_digit()).filter_map(|n| n.parse::<u64>().ok()).find(|n| known.contains(n)).unwrap_or(0)
}

pub fn parse_manifest(b: &[u8]) -> Result<Manifest> {
    let v: Value = serde_json::from_slice(b).context("collection.json")?;
    let mods: Vec<CollMod> = v["mods"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|m| {
            let src = &m["source"];
            CollMod {
                name: m["name"].as_str().unwrap_or("?").to_string(),
                version: m["version"].as_str().unwrap_or("").to_string(),
                optional: m["optional"].as_bool().unwrap_or(false),
                phase: m["phase"].as_u64().unwrap_or(0),
                source: src["type"].as_str().unwrap_or("nexus").to_string(),
                mod_id: src["modId"].as_u64().or_else(|| src["modId"].as_str().and_then(|s| s.parse().ok())).unwrap_or(0),
                file_id: src["fileId"]
                    .as_u64()
                    .map(|n| n.to_string())
                    .or_else(|| src["fileId"].as_str().map(String::from))
                    .unwrap_or_default(),
                logical: src["logicalFilename"].as_str().unwrap_or("").to_string(),
                bundle: src["fileExpression"].as_str().unwrap_or("").to_string(),
                choices: m.get("choices").and_then(choices_from),
                has_patches: m.get("patches").is_some_and(|p| p.as_object().is_some_and(|o| !o.is_empty())),
                instructions: m["instructions"].as_str().unwrap_or("").to_string(),
            }
        })
        .collect();
    let known: Vec<u64> = mods.iter().map(|m| m.mod_id).filter(|&n| n > 0).collect();
    let by_logical: BTreeMap<String, u64> = mods.iter().map(|m| (m.logical.to_lowercase(), m.mod_id)).collect();
    let side = |r: &Value| {
        let logical = r["logicalFileName"].as_str().unwrap_or("").to_string();
        let id = by_logical
            .get(&logical.to_lowercase())
            .copied()
            .filter(|&n| n > 0)
            .unwrap_or_else(|| expr_mod_id(r["fileExpression"].as_str().unwrap_or(""), &known));
        (id, logical)
    };
    let rules = v["modRules"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter(|r| matches!(r["type"].as_str(), Some("after" | "before")))
        .map(|r| CollRule { kind: r["type"].as_str().unwrap().to_string(), source: side(&r["source"]), reference: side(&r["reference"]) })
        .collect();
    Ok(Manifest {
        name: v.pointer("/info/name").and_then(Value::as_str).unwrap_or("").to_string(),
        author: v.pointer("/info/author").and_then(Value::as_str).unwrap_or("").to_string(),
        instructions: v.pointer("/info/installInstructions").and_then(Value::as_str).unwrap_or("").to_string(),
        mods,
        rules,
    })
}

/// Read collection.json from an extracted collection archive.
pub fn read_manifest(dir: &Path) -> Result<Manifest> {
    let f = crate::files::list_tree(dir)
        .into_iter()
        .find(|p| p.eq_ignore_ascii_case("collection.json"))
        .context("the collection archive has no collection.json")?;
    parse_manifest(&std::fs::read(dir.join(f))?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refs() {
        assert_eq!(parse_ref("iszwwe").unwrap(), ("iszwwe".into(), None));
        assert_eq!(
            parse_ref("https://www.nexusmods.com/games/cyberpunk2077/collections/iszwwe/revisions/483").unwrap(),
            ("iszwwe".into(), Some(483))
        );
        assert_eq!(parse_ref("https://next.nexusmods.com/cyberpunk2077/collections/abc123?tab=mods").unwrap(), ("abc123".into(), None));
        assert!(parse_ref("not a slug!").is_err());
    }

    #[test]
    fn manifest_mods_choices_and_rules() {
        let j = r#"{"info":{"name":"WTNC","author":"z9r"},
          "mods":[
            {"name":"Damage Scaling","version":"4.3.0","optional":false,"phase":0,
             "source":{"type":"nexus","modId":1712,"fileId":160001,"logicalFilename":"Damage Scaling"}},
            {"name":"DSB Extended","version":"4.0","optional":true,"phase":1,
             "source":{"type":"nexus","modId":21865,"fileId":"160002","logicalFilename":"Damage Scaling and Balance - Extended"},
             "choices":{"type":"fomod","options":[{"name":"Introduction","groups":[{"name":"Intro","choices":[{"name":" Spicy","idx":0}]}]}]}}],
          "modRules":[{"type":"after",
             "reference":{"fileExpression":"Damage Scaling-1712-4-3-0-1775269435","logicalFileName":"Damage Scaling"},
             "source":{"fileExpression":"Damage Scaling and Balance - Extended 21865 4.0 2026","logicalFileName":"Damage Scaling and Balance - Extended"}},
            {"type":"requires","reference":{},"source":{}}]}"#;
        let m = parse_manifest(j.as_bytes()).unwrap();
        assert_eq!((m.name.as_str(), m.mods.len(), m.rules.len()), ("WTNC", 2, 1));
        assert_eq!(m.mods[1].file_id, "160002");
        assert_eq!(m.mods[1].choices.as_ref().unwrap()["Introduction/Intro"], vec![" Spicy".to_string()]);
        assert_eq!((m.rules[0].source.0, m.rules[0].reference.0), (21865, 1712));
    }
}
