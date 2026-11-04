//! `cpmm steam-shortcut`: put the cpmm GUI in the Steam library as a non-Steam game, with the
//! capsule/hero/logo/wide artwork from `gui/art/` (embedded in the binary).
//!
//! Two routes:
//! * Steam running: through Steam's own client API (`SteamClient.Apps.*`) in the SharedJSContext
//!   page, over the CEF remote debugger (enabled by Decky, or by
//!   `<steam>/.cef-enable-remote-debugging`). The shortcut appears immediately, no restart, and
//!   Steam itself writes shortcuts.vdf and the grid files.
//! * Steam not running: edit `userdata/<user>/config/shortcuts.vdf` (binary KeyValues) and write
//!   the artwork into `config/grid/` directly. Steam reads both at startup.
//!
//! Editing shortcuts.vdf while Steam runs is refused: Steam keeps its own copy in memory and
//! overwrites the file the next time it saves.

use anyhow::{Context, Result, bail};
use std::fs;
use std::path::{Path, PathBuf};

const CAPSULE: &[u8] = include_bytes!("../gui/art/capsule.png"); // 600x900, grid/<id>p.png
const WIDE: &[u8] = include_bytes!("../gui/art/wide.png"); // 920x430, grid/<id>.png
const HERO: &[u8] = include_bytes!("../gui/art/hero.png"); // 1920x620, grid/<id>_hero.png
const LOGO: &[u8] = include_bytes!("../gui/art/logo.png"); // 1000x400, grid/<id>_logo.png
const ICON: &[u8] = include_bytes!("../gui/src-tauri/icons/icon.png"); // 512x512

/// (Steam asset type for SetCustomArtworkForApp, grid file suffix, bytes)
const ART: [(u32, &str, &[u8]); 4] = [(0, "p", CAPSULE), (1, "_hero", HERO), (2, "_logo", LOGO), (3, "", WIDE)];

const CEF: &str = "http://127.0.0.1:8080";

pub struct Opts {
    pub name: String,
    pub exe: Option<PathBuf>,
    pub remove: bool,
    pub dry_run: bool,
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").expect("HOME not set"))
}

fn steam_root() -> PathBuf {
    let link = home().join(".steam/root");
    fs::canonicalize(&link).unwrap_or_else(|_| home().join(".local/share/Steam"))
}

/// Steam accounts that have logged in on this machine (userdata/<account id>).
fn users(root: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = fs::read_dir(root.join("userdata"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let n = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            !n.is_empty() && n != "0" && n.bytes().all(|b| b.is_ascii_digit()) && p.join("config").is_dir()
        })
        .collect();
    v.sort();
    v
}

fn steam_running() -> bool {
    let Ok(pid) = fs::read_to_string(home().join(".steam/steam.pid")) else { return false };
    let comm = fs::read_to_string(format!("/proc/{}/comm", pid.trim())).unwrap_or_default();
    comm.trim() == "steam"
}

fn gui_exe(opt: Option<&PathBuf>) -> Result<PathBuf> {
    let p = match opt {
        Some(p) => p.clone(),
        None => std::env::current_exe()?.with_file_name("cpmm-gui"),
    };
    let p = fs::canonicalize(&p).with_context(|| format!("{} not found (build the GUI, or pass --exe)", p.display()))?;
    Ok(p)
}

/// The shortcut's icon: the installed desktop icon if there is one, else the embedded one
/// written next to the grid art.
fn icon_path(grid: &Path, appid: u32) -> PathBuf {
    let installed = home().join(".local/share/icons/hicolor/256x256/apps/cpmm.png");
    if installed.is_file() { installed } else { grid.join(format!("{appid}_icon.png")) }
}

// ---- binary KeyValues (shortcuts.vdf) ----

#[derive(Clone, Debug, PartialEq)]
pub enum Val {
    Map(Vec<(String, Val)>),
    Str(String),
    I32(i32),
    /// Any other scalar type, kept byte for byte: (type tag, payload).
    Raw(u8, Vec<u8>),
}

fn cstr(b: &[u8], i: &mut usize) -> Result<String> {
    let end = b[*i..].iter().position(|&c| c == 0).context("vdf: unterminated string")? + *i;
    let s = String::from_utf8_lossy(&b[*i..end]).into_owned();
    *i = end + 1;
    Ok(s)
}

fn parse_map(b: &[u8], i: &mut usize) -> Result<Vec<(String, Val)>> {
    let mut out = Vec::new();
    loop {
        let t = *b.get(*i).context("vdf: truncated")?;
        *i += 1;
        if t == 0x08 {
            return Ok(out);
        }
        let k = cstr(b, i)?;
        let v = match t {
            0x00 => Val::Map(parse_map(b, i)?),
            0x01 => Val::Str(cstr(b, i)?),
            0x02 => {
                let v = i32::from_le_bytes(b.get(*i..*i + 4).context("vdf: truncated")?.try_into()?);
                *i += 4;
                Val::I32(v)
            }
            0x03 | 0x04 | 0x06 => {
                let r = b.get(*i..*i + 4).context("vdf: truncated")?.to_vec();
                *i += 4;
                Val::Raw(t, r)
            }
            0x07 | 0x0a => {
                let r = b.get(*i..*i + 8).context("vdf: truncated")?.to_vec();
                *i += 8;
                Val::Raw(t, r)
            }
            _ => bail!("vdf: unknown type 0x{t:02x} at byte {}", *i - 1),
        };
        out.push((k, v));
    }
}

pub fn parse_vdf(b: &[u8]) -> Result<Vec<(String, Val)>> {
    let mut i = 0;
    let m = parse_map(b, &mut i)?;
    if i != b.len() {
        bail!("vdf: {} trailing bytes", b.len() - i);
    }
    Ok(m)
}

fn write_map(m: &[(String, Val)], out: &mut Vec<u8>) {
    for (k, v) in m {
        let tag = match v {
            Val::Map(_) => 0x00,
            Val::Str(_) => 0x01,
            Val::I32(_) => 0x02,
            Val::Raw(t, _) => *t,
        };
        out.push(tag);
        out.extend_from_slice(k.as_bytes());
        out.push(0);
        match v {
            Val::Map(m) => write_map(m, out),
            Val::Str(s) => {
                out.extend_from_slice(s.as_bytes());
                out.push(0);
            }
            Val::I32(n) => out.extend_from_slice(&n.to_le_bytes()),
            Val::Raw(_, r) => out.extend_from_slice(r),
        }
    }
    out.push(0x08);
}

pub fn write_vdf(m: &[(String, Val)]) -> Vec<u8> {
    let mut out = Vec::new();
    write_map(m, &mut out); // the root map's own end byte closes the file
    out
}

fn get<'a>(m: &'a [(String, Val)], k: &str) -> Option<&'a Val> {
    m.iter().find(|(n, _)| n.eq_ignore_ascii_case(k)).map(|(_, v)| v)
}

fn get_str<'a>(m: &'a [(String, Val)], k: &str) -> &'a str {
    match get(m, k) {
        Some(Val::Str(s)) => s,
        _ => "",
    }
}

/// Steam's id for a shortcut added outside the client: crc32(exe + name) with the top bit set.
/// (Shortcuts added in the client get a random id; both kinds live in the `appid` field.)
pub fn shortcut_appid(exe_quoted: &str, name: &str) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &b in exe_quoted.as_bytes().iter().chain(name.as_bytes()) {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 };
        }
    }
    !crc | 0x8000_0000
}

fn same_exe(field: &str, exe: &Path) -> bool {
    Path::new(field.trim().trim_matches('"')) == exe
}

struct Found {
    index: usize,
    appid: u32,
}

/// The shortcuts list inside the root map, and our entry in it (matched by exe path).
fn find(root: &[(String, Val)], exe: &Path) -> Option<Found> {
    let Some(Val::Map(list)) = get(root, "shortcuts") else { return None };
    list.iter().enumerate().find_map(|(index, (_, v))| match v {
        Val::Map(e) if same_exe(get_str(e, "Exe"), exe) => {
            let appid = match get(e, "appid") {
                Some(Val::I32(n)) => *n as u32,
                _ => shortcut_appid(get_str(e, "Exe"), get_str(e, "AppName")),
            };
            Some(Found { index, appid })
        }
        _ => None,
    })
}

fn read_shortcuts(user: &Path) -> Result<Vec<(String, Val)>> {
    let f = user.join("config/shortcuts.vdf");
    match fs::read(&f) {
        Ok(b) if !b.is_empty() => parse_vdf(&b).with_context(|| format!("parsing {}", f.display())),
        _ => Ok(vec![("shortcuts".into(), Val::Map(vec![]))]),
    }
}

fn new_entry(appid: u32, name: &str, exe_q: &str, start_q: &str, icon: &str) -> Val {
    let s = |k: &str, v: &str| (k.to_string(), Val::Str(v.to_string()));
    let n = |k: &str, v: i32| (k.to_string(), Val::I32(v));
    Val::Map(vec![
        n("appid", appid as i32),
        s("AppName", name),
        s("Exe", exe_q),
        s("StartDir", start_q),
        s("icon", icon),
        s("ShortcutPath", ""),
        s("LaunchOptions", ""),
        n("IsHidden", 0),
        n("AllowDesktopConfig", 1),
        n("AllowOverlay", 1),
        n("OpenVR", 0),
        n("Devkit", 0),
        s("DevkitGameID", ""),
        n("DevkitOverrideAppID", 0),
        n("LastPlayTime", 0),
        s("FlatpakAppID", ""),
        s("sortas", ""),
        ("tags".into(), Val::Map(vec![])),
    ])
}

/// Re-number the list entries "0", "1", … as Steam does.
fn renumber(list: &mut [(String, Val)]) {
    for (i, (k, _)) in list.iter_mut().enumerate() {
        *k = i.to_string();
    }
}

fn save_shortcuts(user: &Path, root: &[(String, Val)]) -> Result<()> {
    let f = user.join("config/shortcuts.vdf");
    if f.exists() {
        fs::copy(&f, f.with_extension("vdf.cpmm-bak"))?;
    }
    let tmp = f.with_extension("vdf.cpmm-tmp");
    fs::write(&tmp, write_vdf(root))?;
    fs::rename(&tmp, &f)?;
    Ok(())
}

fn art_files(grid: &Path, appid: u32) -> Vec<PathBuf> {
    ART.iter()
        .map(|(_, suf, _)| grid.join(format!("{appid}{suf}.png")))
        .chain([grid.join(format!("{appid}_icon.png"))])
        .collect()
}

fn offline(opts: &Opts, exe: &Path, users: &[PathBuf]) -> Result<()> {
    let exe_q = format!("\"{}\"", exe.display());
    let start = exe.parent().map(Path::to_path_buf).unwrap_or_else(home);
    let start_q = format!("\"{}\"", start.display());
    for user in users {
        let mut root = read_shortcuts(user)?;
        let grid = user.join("config/grid");
        let found = find(&root, exe);
        let uid = user.file_name().unwrap().to_string_lossy();
        let Some(Val::Map(list)) = root.iter_mut().find(|(k, _)| k.eq_ignore_ascii_case("shortcuts")).map(|(_, v)| v) else {
            bail!("{}: no shortcuts map", user.display());
        };
        if opts.remove {
            let Some(f) = found else {
                println!("user {uid}: no {} shortcut", opts.name);
                continue;
            };
            println!("user {uid}: removing shortcut {} and its artwork", f.appid);
            if opts.dry_run {
                continue;
            }
            list.remove(f.index);
            renumber(list);
            save_shortcuts(user, &root)?;
            for p in art_files(&grid, f.appid) {
                let _ = fs::remove_file(p);
            }
            continue;
        }
        let appid = found.as_ref().map(|f| f.appid).unwrap_or_else(|| shortcut_appid(&exe_q, &opts.name));
        let icon = icon_path(&grid, appid);
        match &found {
            Some(_) => println!("user {uid}: shortcut {appid} already there; refreshing artwork"),
            None => println!("user {uid}: adding shortcut {appid} ({} -> {})", opts.name, exe.display()),
        }
        if opts.dry_run {
            continue;
        }
        if found.is_none() {
            list.push((String::new(), new_entry(appid, &opts.name, &exe_q, &start_q, &icon.to_string_lossy())));
            renumber(list);
            save_shortcuts(user, &root)?;
        }
        fs::create_dir_all(&grid)?;
        for (_, suf, bytes) in ART {
            fs::write(grid.join(format!("{appid}{suf}.png")), bytes)?;
        }
        if found.is_none() && icon.starts_with(&grid) {
            fs::write(&icon, ICON)?;
        }
    }
    if !opts.dry_run {
        println!("done; Steam picks it up the next time it starts");
    }
    Ok(())
}

// ---- live: Steam's client API over the CEF debugger ----

fn shared_js_ws() -> Result<String> {
    let targets: serde_json::Value = ureq::get(&format!("{CEF}/json"))
        .timeout(std::time::Duration::from_secs(3))
        .call()?
        .into_json()?;
    targets
        .as_array()
        .into_iter()
        .flatten()
        .find(|t| t["title"] == "SharedJSContext")
        .and_then(|t| t["webSocketDebuggerUrl"].as_str())
        .map(str::to_string)
        .context("Steam's SharedJSContext page isn't listed by the CEF debugger")
}

/// Evaluate an async JS expression in Steam's SharedJSContext and return its JSON value.
fn eval(ws_url: &str, expr: &str) -> Result<serde_json::Value> {
    use tungstenite::Message;
    let (mut ws, _) = tungstenite::connect(ws_url).context("connecting to Steam's CEF debugger")?;
    let req = serde_json::json!({
        "id": 1,
        "method": "Runtime.evaluate",
        "params": { "expression": expr, "awaitPromise": true, "returnByValue": true },
    });
    ws.send(Message::text(req.to_string()))?;
    loop {
        let msg = ws.read()?;
        let Message::Text(t) = msg else { continue };
        let v: serde_json::Value = serde_json::from_str(&t)?;
        if v["id"] != 1 {
            continue;
        }
        let _ = ws.close(None);
        if let Some(ex) = v["result"].get("exceptionDetails") {
            let text = ex["exception"]["description"].as_str().or(ex["text"].as_str()).unwrap_or("?");
            bail!("Steam client error: {text}");
        }
        if let Some(err) = v.get("error") {
            bail!("CEF error: {err}");
        }
        return Ok(v["result"]["result"]["value"].clone());
    }
}

fn b64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            s.push(if i <= c.len() { T[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' });
        }
    }
    s
}

fn js(s: &str) -> String {
    serde_json::Value::from(s).to_string()
}

const NEEDED: &str = "['AddShortcut','RemoveShortcut','SetShortcutName','SetCustomArtworkForApp']";

fn live(opts: &Opts, exe: &Path, users: &[PathBuf], ws: &str) -> Result<()> {
    let missing = eval(ws, &format!("{NEEDED}.filter(f => typeof SteamClient.Apps[f] !== 'function')"))?;
    if missing.as_array().is_some_and(|a| !a.is_empty()) {
        bail!("this Steam client lacks SteamClient.Apps functions {missing}; quit Steam and rerun to edit shortcuts.vdf instead");
    }
    // Steam keeps shortcuts.vdf in sync with its library, so it tells us what is already there;
    // double-check against the live library in case the file is stale.
    let mut existing = None;
    for u in users {
        if let Some(f) = find(&read_shortcuts(u)?, exe) {
            let known = eval(ws, &format!("!!appStore.GetAppOverviewByAppID({})", f.appid))?;
            if known == true {
                existing = Some(f.appid);
                break;
            }
        }
    }
    if opts.remove {
        let Some(id) = existing else {
            println!("no {} shortcut in the Steam library", opts.name);
            return Ok(());
        };
        println!("removing shortcut {id} (live, through the Steam client)");
        if !opts.dry_run {
            eval(ws, &format!("SteamClient.Apps.RemoveShortcut({id}), true"))?;
            // Steam leaves the custom artwork behind.
            for u in users {
                for p in art_files(&u.join("config/grid"), id) {
                    let _ = fs::remove_file(p);
                }
            }
        }
        return Ok(());
    }
    match existing {
        Some(id) => println!("shortcut {id} already in the library; refreshing name and artwork"),
        None => println!("adding {} -> {} (live, through the Steam client)", opts.name, exe.display()),
    }
    if opts.dry_run {
        return Ok(());
    }
    let start = exe.parent().map(Path::to_path_buf).unwrap_or_else(home);
    let id = match existing {
        Some(id) => id,
        None => {
            let v = eval(
                ws,
                &format!(
                    "SteamClient.Apps.AddShortcut({}, {}, {}, '')",
                    js(&opts.name),
                    js(&exe.to_string_lossy()),
                    js(&start.to_string_lossy())
                ),
            )?;
            v.as_u64().with_context(|| format!("AddShortcut returned {v}"))? as u32
        }
    };
    let grid = users.first().map(|u| u.join("config/grid")).unwrap_or_default();
    let icon = icon_path(&grid, id);
    if icon.starts_with(&grid) {
        fs::create_dir_all(&grid)?;
        fs::write(&icon, ICON)?;
    }
    eval(
        ws,
        &format!(
            "(() => {{ const A = SteamClient.Apps; A.SetShortcutName({id}, {name}); \
             if (typeof A.SetShortcutIcon === 'function') A.SetShortcutIcon({id}, {icon}); return true }})()",
            name = js(&opts.name),
            icon = js(&icon.to_string_lossy()),
        ),
    )?;
    for (kind, _, bytes) in ART {
        eval(ws, &format!("SteamClient.Apps.SetCustomArtworkForApp({id}, '{}', 'png', {kind}).then(() => true)", b64(bytes)))?;
    }
    println!("added as app {id} with capsule, wide, hero and logo artwork");
    Ok(())
}

pub fn run(opts: Opts) -> Result<()> {
    let root = steam_root();
    let users = users(&root);
    if users.is_empty() {
        bail!("no Steam users under {}/userdata (log in to Steam once first)", root.display());
    }
    // For --remove, a missing GUI binary is fine: match on the path it would have had.
    let exe = match gui_exe(opts.exe.as_ref()) {
        Ok(p) => p,
        Err(_) if opts.remove => opts.exe.clone().unwrap_or(std::env::current_exe()?.with_file_name("cpmm-gui")),
        Err(e) => return Err(e),
    };
    if !steam_running() {
        return offline(&opts, &exe, &users);
    }
    match shared_js_ws() {
        Ok(ws) => live(&opts, &exe, &users, &ws),
        Err(e) => bail!(
            "Steam is running, but its CEF debugger isn't reachable ({e:#}).\n\
             Either enable it (Decky does; or `touch {}/.cef-enable-remote-debugging` and restart Steam),\n\
             or quit Steam and rerun: shortcuts.vdf is only read at startup and Steam overwrites it while running.",
            root.display()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vdf_round_trips_steam_bytes() {
        // A Steam-written shortcuts.vdf with one entry (trimmed field set).
        let mut b = Vec::new();
        b.extend_from_slice(b"\x00shortcuts\x00\x000\x00\x02appid\x00\x1f\x26\xf8\x9f\x01AppName\x00cpmm\x00");
        b.extend_from_slice(b"\x01Exe\x00\"/home/u/.local/bin/cpmm-gui\"\x00\x02LastPlayTime\x00\xb3\x5d\xeb\x6a");
        b.extend_from_slice(b"\x00tags\x00\x08\x08\x08\x08");
        let m = parse_vdf(&b).unwrap();
        assert_eq!(write_vdf(&m), b);
        let f = find(&m, Path::new("/home/u/.local/bin/cpmm-gui")).unwrap();
        assert_eq!(f.appid, 2683840031);
    }

    #[test]
    fn appid_matches_steam_formula() {
        // zlib.crc32(b'"/usr/bin/firefox"Firefox') | 0x80000000
        assert_eq!(shortcut_appid("\"/usr/bin/firefox\"", "Firefox"), 2910723666);
    }

    #[test]
    fn base64() {
        assert_eq!(b64(b"Ma"), "TWE=");
        assert_eq!(b64(b"Man"), "TWFu");
        assert_eq!(b64(b"M"), "TQ==");
    }
}
