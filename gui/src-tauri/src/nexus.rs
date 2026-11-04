//! The built-in Nexus browser: nexusmods.com in a second window, where "Mod Manager Download"
//! (nxm://) links and manual downloads go straight to cpmm. Steam's browser refuses nxm:// links,
//! and a desktop browser is awkward in Game Mode, so this is the handheld-friendly route.
//!
//! Security: the page is a remote site and gets no Tauri IPC (the window isn't in any
//! capability). It reaches cpmm only through navigations that `on_navigation` intercepts:
//! `nxm://…` (install), `cpmm://close` and `cpmm://keyboard`. Everything it can trigger, a click
//! on an nxm:// link could trigger anyway. The backend drives the page with `eval` (gamepad,
//! toasts) through the `window.__cpmm` object that `nexus.js` installs.

use serde::Serialize;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use tauri::webview::{DownloadEvent, NewWindowResponse};
use tauri::{AppHandle, Emitter, Manager, Url, WebviewUrl, WebviewWindowBuilder};

pub const LABEL: &str = "nexus";
const HOME: &str = "https://www.nexusmods.com/games/cyberpunk2077/mods";
const SCRIPT: &str = include_str!("nexus.js");

fn home_dir() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default()
}

fn is_nexus(u: &Url) -> bool {
    u.scheme() == "https" && u.host_str().is_some_and(|h| h == "nexusmods.com" || h.ends_with(".nexusmods.com"))
}

/// Nexus mod id from a page URL like …/cyberpunk2077/mods/4626?tab=files.
fn mod_id_of(u: &Url) -> Option<u64> {
    let segs: Vec<&str> = u.path_segments()?.collect();
    segs.windows(2).find(|w| w[0] == "mods").and_then(|w| w[1].parse().ok())
}

/// Game Mode (gamescope) has no desktop keyboard; Steam's on-screen one has to be asked for.
fn wants_osk() -> bool {
    match std::env::var("CPMM_OSK").as_deref() {
        Ok("1") => return true,
        Ok("0") => return false,
        _ => {}
    }
    std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|d| d.eq_ignore_ascii_case("gamescope"))
        || std::env::var_os("SteamDeck").is_some()
        || std::env::var_os("GAMESCOPE_WAYLAND_DISPLAY").is_some()
}

pub fn steam_bin() -> String {
    let home = home_dir();
    [home.join(".local/share/Steam/ubuntu12_32/steam"), home.join(".steam/steam/ubuntu12_32/steam")]
        .into_iter()
        .find(|p| p.is_file())
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "steam".into())
}

fn open_keyboard() {
    if !wants_osk() {
        return;
    }
    // Hands the URL to the running Steam over its pipe and exits.
    let _ = Command::new(steam_bin()).arg("steam://open/keyboard").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
}

#[derive(Serialize, Clone)]
struct NxmEvent {
    url: String,
}

#[derive(Serialize, Clone)]
struct ArchiveEvent {
    path: String,
    mod_id: Option<u64>,
}

/// Show a toast inside the Nexus page (no-op if the browser isn't open).
pub fn toast(app: &AppHandle, kind: &str, text: &str) {
    if let Some(w) = app.get_webview_window(LABEL) {
        let js = format!("window.__cpmm && window.__cpmm.toast({}, {})", serde_json::json!(kind), serde_json::json!(text));
        let _ = w.eval(js);
    }
}

/// Forward a gamepad action / analog motion to the page.
pub fn pad(w: &tauri::WebviewWindow, call: &str) {
    let _ = w.eval(format!("window.__cpmm && window.__cpmm.{call}"));
}

fn close(app: &AppHandle) {
    let app = app.clone();
    // Not from inside the navigation callback: let it return first.
    std::thread::spawn(move || {
        if let Some(w) = app.get_webview_window(LABEL) {
            let _ = w.close();
        }
        if let Some(m) = app.get_webview_window("main") {
            let _ = m.show();
            let _ = m.set_focus();
        }
    });
}

/// Allow a navigation, or swallow it and act on it.
fn on_navigation(app: &AppHandle, url: &Url) -> bool {
    match url.scheme() {
        "nxm" => {
            crate::glog(&format!("nexus: nxm link {}", url.as_str().split('?').next().unwrap_or("")));
            toast(app, "info", "Sent to cpmm, installing…");
            let _ = app.emit_to("main", "nexus-nxm", NxmEvent { url: url.to_string() });
            false
        }
        "cpmm" => {
            match url.host_str() {
                Some("close") => close(app),
                Some("keyboard") => open_keyboard(),
                Some("log") => crate::glog(&format!("nexus page: {}", url.query().unwrap_or(""))),
                _ => {}
            }
            false
        }
        // Login, Cloudflare checks and CDNs live on other hosts, so any web page is fine.
        "http" | "https" | "about" | "data" | "blob" => true,
        // mailto:, steam:, … would leave the app (or do nothing); ignore them.
        _ => false,
    }
}

fn downloads_dir() -> PathBuf {
    let data = std::env::var_os("CPMM_DATA").map(PathBuf::from).unwrap_or_else(|| home_dir().join(".local/share/cpmm"));
    data.join("downloads/browser")
}

#[tauri::command]
pub fn open_nexus(app: AppHandle, url: Option<String>) -> Result<(), String> {
    let target = url.as_deref().and_then(|u| Url::parse(u).ok()).filter(is_nexus).unwrap_or_else(|| Url::parse(HOME).unwrap());
    if let Some(w) = app.get_webview_window(LABEL) {
        if url.is_some() {
            let _ = w.navigate(target);
        }
        let _ = w.show();
        let _ = w.set_focus();
        return Ok(());
    }
    let (size, maximized) = match app.get_webview_window("main") {
        Some(m) => {
            let scale = m.scale_factor().unwrap_or(1.0);
            let s = m.inner_size().map(|s| s.to_logical::<f64>(scale)).ok();
            (s.map(|s| (s.width, s.height)), m.is_maximized().unwrap_or(false) || m.is_fullscreen().unwrap_or(false))
        }
        None => (None, false),
    };
    let (nav_app, win_app, dl_app) = (app.clone(), app.clone(), app.clone());
    let (w, h) = size.unwrap_or((1280.0, 800.0));
    // Start blank: the WebKit settings below must be in place before Nexus's code runs.
    let blank = Url::parse("about:blank").unwrap();
    let win = WebviewWindowBuilder::new(&app, LABEL, WebviewUrl::External(blank))
        .title("Nexus Mods · cpmm")
        .inner_size(w, h)
        .min_inner_size(800.0, 500.0)
        .maximized(maximized)
        .initialization_script(SCRIPT)
        .on_navigation(move |u| on_navigation(&nav_app, u))
        .on_page_load(|_, p| crate::glog(&format!("nexus: page {:?} {}", p.event(), p.url())))
        // Links that want a new tab/popup open in this window instead.
        .on_new_window(move |u, _| {
            let app = win_app.clone();
            if u.scheme() == "nxm" {
                on_navigation(&app, &u);
            } else if matches!(u.scheme(), "http" | "https") {
                std::thread::spawn(move || {
                    if let Some(w) = app.get_webview_window(LABEL) {
                        let _ = w.navigate(u);
                    }
                });
            }
            NewWindowResponse::Deny
        })
        // "Manual download": keep the archive in cpmm's downloads and install it from there.
        .on_download(move |wv, ev| {
            match ev {
                DownloadEvent::Requested { url, destination } => {
                    let dir = downloads_dir();
                    let _ = std::fs::create_dir_all(&dir);
                    let name = destination
                        .file_name()
                        .map(|n| n.to_os_string())
                        .or_else(|| url.path_segments().and_then(|mut s| s.next_back()).map(Into::into))
                        .unwrap_or_else(|| "download".into());
                    *destination = dir.join(name);
                    toast(&dl_app, "info", "Downloading…");
                }
                DownloadEvent::Finished { path, success, .. } => match (success, path) {
                    (true, Some(p)) => {
                        let mod_id = wv.url().ok().as_ref().and_then(mod_id_of);
                        crate::glog(&format!("nexus: downloaded {} (mod {mod_id:?})", p.display()));
                        let _ = dl_app.emit_to("main", "nexus-archive", ArchiveEvent { path: p.to_string_lossy().into_owned(), mod_id });
                    }
                    _ => toast(&dl_app, "error", "Download failed"),
                },
                _ => {}
            }
            true
        })
        .build()
        .map_err(|e| format!("opening the Nexus browser: {e}"))?;
    let app2 = app.clone();
    win.on_window_event(move |e| {
        if let tauri::WindowEvent::Destroyed = e {
            if let Some(m) = app2.get_webview_window("main") {
                let _ = m.set_focus();
            }
        }
    });
    #[cfg(target_os = "linux")]
    {
        let t = target.clone();
        let _ = win.with_webview(move |wv| {
            linux::harden(&wv.inner());
            use webkit2gtk::WebViewExt;
            wv.inner().load_uri(t.as_str());
        });
    }
    #[cfg(not(target_os = "linux"))]
    let _ = win.navigate(target);
    crate::glog("nexus: browser opened");
    Ok(())
}

#[cfg(target_os = "linux")]
mod linux {
    use std::cell::Cell;
    use std::path::PathBuf;
    use webkit2gtk::{SettingsExt, WebView, WebViewExt};

    /// WebKitGTK aborts the page's web process when a page creates a media element and
    /// GStreamer has no audio sink (`autoaudiosink`, from gst-plugins-good). Nexus does that
    /// within seconds (video ads and previews), so on systems without the plugin (this Ally;
    /// read-only SteamOS images) media stays off. `CPMM_NEXUS_MEDIA=0/1` overrides.
    pub fn media_ok() -> bool {
        match std::env::var("CPMM_NEXUS_MEDIA").as_deref() {
            Ok("1") => return true,
            Ok("0") => return false,
            _ => {}
        }
        let mut dirs: Vec<PathBuf> = ["GST_PLUGIN_PATH", "GST_PLUGIN_PATH_1_0", "GST_PLUGIN_SYSTEM_PATH", "GST_PLUGIN_SYSTEM_PATH_1_0"]
            .iter()
            .filter_map(|v| std::env::var_os(v))
            .flat_map(|v| std::env::split_paths(&v).collect::<Vec<_>>())
            .collect();
        dirs.extend(["/usr/lib/gstreamer-1.0", "/usr/lib64/gstreamer-1.0", "/usr/lib/x86_64-linux-gnu/gstreamer-1.0"].map(PathBuf::from));
        dirs.iter().any(|d| d.join("libgstautodetect.so").is_file())
    }

    pub fn harden(v: &WebView) {
        if !media_ok() {
            if let Some(s) = WebViewExt::settings(v) {
                s.set_enable_media(false);
                s.set_enable_mediasource(false);
                s.set_enable_webaudio(false);
            }
            crate::glog("nexus: GStreamer has no autoaudiosink (gst-plugins-good); media disabled to keep WebKit from crashing");
        }
        // If the page's process dies anyway, reload instead of leaving a frozen page behind.
        let crashes = Cell::new(0u32);
        v.connect_web_process_terminated(move |v, reason| {
            crashes.set(crashes.get() + 1);
            crate::glog(&format!("nexus: web process terminated ({reason:?}), crash {}", crashes.get()));
            if crashes.get() <= 3 {
                v.reload();
            }
        });
    }
}

#[tauri::command]
pub fn close_nexus(app: AppHandle) {
    close(&app);
}

#[tauri::command]
pub fn nexus_toast(app: AppHandle, kind: String, text: String) {
    toast(&app, &kind, &text);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_nexus_pages_are_opened_by_url() {
        assert!(is_nexus(&Url::parse("https://www.nexusmods.com/cyberpunk2077/mods/4626").unwrap()));
        assert!(is_nexus(&Url::parse("https://nexusmods.com/").unwrap()));
        assert!(!is_nexus(&Url::parse("https://evil-nexusmods.com/").unwrap()));
        assert!(!is_nexus(&Url::parse("http://www.nexusmods.com/").unwrap()));
    }

    #[test]
    fn mod_id_from_page_url() {
        assert_eq!(mod_id_of(&Url::parse("https://www.nexusmods.com/cyberpunk2077/mods/4626?tab=files&file_id=1").unwrap()), Some(4626));
        assert_eq!(mod_id_of(&Url::parse("https://www.nexusmods.com/games/cyberpunk2077/mods").unwrap()), None);
    }
}
