//! The built-in Nexus browser: nexusmods.com in a second window, where "Mod Manager Download"
//! (nxm://) links and manual downloads go straight to cpmm. Steam's browser refuses nxm:// links,
//! and a desktop browser is awkward in Game Mode, so this is the handheld-friendly route.
//!
//! Security: the page is a remote site and gets no Tauri IPC (the window isn't in any
//! capability). It reaches cpmm only through navigations that `on_navigation` intercepts:
//! `nxm://…` (install), `cpmm://close` and `cpmm://keyboard`. Everything it can trigger, a click
//! on an nxm:// link could trigger anyway. The backend drives the page with `eval` (gamepad,
//! toasts) through the `window.__cpmm` object that `nexus.js` installs.
//!
//! It also shows tool reports (HTML files a tool run wrote, under cpmm's tool-runs folder),
//! so they get the same controller support. A report is no more trusted than the tool that
//! wrote it, and that tool already ran as the user.

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

/// A file a tool run wrote (`file://<data>/tool-runs/…`).
fn is_report(u: &Url) -> bool {
    let runs = cpmm::state::Paths::load().map(|p| cpmm::tools::runs_dir(&p)).unwrap_or_default();
    u.scheme() == "file"
        && u.to_file_path()
            .is_ok_and(|p| !runs.as_os_str().is_empty() && p.starts_with(&runs) && !p.components().any(|c| c.as_os_str() == ".."))
}

/// Nexus mod id from a page URL like …/cyberpunk2077/mods/4626?tab=files (the core's parser).
fn mod_id_of(u: &Url) -> Option<u64> {
    cpmm::nexus::catalogue::mod_id(u.as_str())
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

fn page(w: &tauri::WebviewWindow, call: &str) {
    let _ = w.eval(format!("window.__cpmm && window.__cpmm.{call}"));
}

/// A gamepad button in the Nexus window. Clicks and the D-pad cursor nudges are real pointer
/// input (see `input`); navigation buttons are handled by the page script.
pub fn action(w: &tauri::WebviewWindow, a: &str) {
    match a {
        "a" | "click" => {
            page(w, "padClick()");
            input::click(w);
        }
        "up" => input::move_by(w, 0.0, -28.0),
        "down" => input::move_by(w, 0.0, 28.0),
        "left" => input::move_by(w, -28.0, 0.0),
        "right" => input::move_by(w, 28.0, 0.0),
        _ => page(w, &format!("pad({})", serde_json::json!(a))),
    }
}

/// Right stick: move the cursor (stick deflection after the response curve, -1..1).
pub fn pointer(w: &tauri::WebviewWindow, dx: f32, dy: f32) {
    input::move_by(w, dx as f64 * 22.0, dy as f64 * 22.0);
}

/// Left stick: scroll whatever is under the cursor.
pub fn scroll(w: &tauri::WebviewWindow, dx: f32, dy: f32) {
    input::scroll(w, dx as f64 * 0.8, dy as f64 * 0.8);
}

/// Real pointer input for the Nexus webview: GDK events built here and dispatched by GTK, so
/// WebKit treats them like a mouse (trusted events, user activation, :hover). Synthetic DOM
/// events from the page script aren't enough: sites ignore them for popups, nxm:// downloads
/// and many modal buttons.
#[cfg(target_os = "linux")]
mod input {
    use std::sync::{Mutex, OnceLock};
    use std::time::Instant;
    use webkit2gtk::WebView;

    static CURSOR: Mutex<Option<(f64, f64)>> = Mutex::new(None);

    enum Kind {
        Motion,
        Press,
        Release,
        Scroll(f64, f64),
    }

    fn now_ms() -> u32 {
        static T0: OnceLock<Instant> = OnceLock::new();
        T0.get_or_init(Instant::now).elapsed().as_millis() as u32
    }

    /// Move the cursor by (dx, dy) px, clamped to the view; returns the new position.
    fn step(v: &WebView, dx: f64, dy: f64) -> (f64, f64) {
        use gtk::prelude::WidgetExt;
        let (w, h) = (v.allocated_width().max(1) as f64, v.allocated_height().max(1) as f64);
        let mut c = CURSOR.lock().unwrap();
        let (x, y) = c.unwrap_or((w / 2.0, h / 2.0));
        let p = ((x + dx).clamp(0.0, w - 1.0), (y + dy).clamp(0.0, h - 1.0));
        *c = Some(p);
        p
    }

    unsafe fn send(v: &WebView, kind: Kind, x: f64, y: f64) {
        use glib::translate::ToGlibPtr;
        use gtk::prelude::*;
        let widget: &gtk::Widget = v.upcast_ref();
        let Some(win) = widget.window() else { return };
        let (ox, oy) = if widget.has_window() {
            (0.0, 0.0)
        } else {
            let a = widget.allocation();
            (a.x() as f64, a.y() as f64)
        };
        let wptr: *mut gdk_sys::GdkWindow = win.to_glib_none().0;
        unsafe {
            let (mut rx, mut ry) = (0, 0);
            gdk_sys::gdk_window_get_origin(wptr, &mut rx, &mut ry);
            let seat = gdk_sys::gdk_display_get_default_seat(gdk_sys::gdk_window_get_display(wptr));
            let dev = if seat.is_null() { std::ptr::null_mut() } else { gdk_sys::gdk_seat_get_pointer(seat) };
            if dev.is_null() {
                return;
            }
            let ty = match kind {
                Kind::Motion => gdk_sys::GDK_MOTION_NOTIFY,
                Kind::Press => gdk_sys::GDK_BUTTON_PRESS,
                Kind::Release => gdk_sys::GDK_BUTTON_RELEASE,
                Kind::Scroll(..) => gdk_sys::GDK_SCROLL,
            };
            let ev = gdk_sys::gdk_event_new(ty);
            let (ex, ey) = (x + ox, y + oy);
            let (xr, yr) = (rx as f64 + ex, ry as f64 + ey);
            // the event owns a reference to its window (gdk_event_free drops it)
            let wref = gobject_sys::g_object_ref(wptr as *mut _) as *mut gdk_sys::GdkWindow;
            match kind {
                Kind::Motion => {
                    let e = &mut (*ev).motion;
                    (e.window, e.send_event, e.time, e.x, e.y, e.axes, e.state, e.is_hint, e.device, e.x_root, e.y_root) =
                        (wref, 0, now_ms(), ex, ey, std::ptr::null_mut(), 0, 0, dev, xr, yr);
                }
                Kind::Press | Kind::Release => {
                    let state = if matches!(kind, Kind::Release) { gdk_sys::GDK_BUTTON1_MASK } else { 0 };
                    let e = &mut (*ev).button;
                    (e.window, e.send_event, e.time, e.x, e.y, e.axes, e.state, e.button, e.device, e.x_root, e.y_root) =
                        (wref, 0, now_ms(), ex, ey, std::ptr::null_mut(), state, 1, dev, xr, yr);
                }
                Kind::Scroll(dx, dy) => {
                    let e = &mut (*ev).scroll;
                    (e.window, e.send_event, e.time, e.x, e.y, e.state, e.direction, e.device, e.x_root, e.y_root) =
                        (wref, 0, now_ms(), ex, ey, 0, gdk_sys::GDK_SCROLL_SMOOTH, dev, xr, yr);
                    (e.delta_x, e.delta_y, e.is_stop) = (dx, dy, 0);
                }
            }
            gdk_sys::gdk_event_set_device(ev, dev);
            gdk_sys::gdk_event_set_source_device(ev, dev);
            gtk_sys::gtk_main_do_event(ev);
            gdk_sys::gdk_event_free(ev);
        }
    }

    pub fn move_by(w: &tauri::WebviewWindow, dx: f64, dy: f64) {
        let w2 = w.clone();
        let _ = w.with_webview(move |wv| {
            let v = wv.inner();
            let (x, y) = step(&v, dx, dy);
            unsafe { send(&v, Kind::Motion, x, y) };
            let _ = w2.eval(format!("window.__cpmm && window.__cpmm.cursorAt({x}, {y})"));
        });
    }

    pub fn click(w: &tauri::WebviewWindow) {
        let w2 = w.clone();
        let _ = w.with_webview(move |wv| {
            let v = wv.inner();
            let (x, y) = step(&v, 0.0, 0.0);
            unsafe {
                send(&v, Kind::Motion, x, y);
                send(&v, Kind::Press, x, y);
                send(&v, Kind::Release, x, y);
            }
            let _ = w2.eval(format!("window.__cpmm && window.__cpmm.cursorAt({x}, {y})"));
        });
    }

    pub fn scroll(w: &tauri::WebviewWindow, dx: f64, dy: f64) {
        let _ = w.with_webview(move |wv| {
            let v = wv.inner();
            let (x, y) = step(&v, 0.0, 0.0);
            unsafe { send(&v, Kind::Scroll(dx, dy), x, y) };
        });
    }
}

#[cfg(not(target_os = "linux"))]
mod input {
    pub fn move_by(_: &tauri::WebviewWindow, _: f64, _: f64) {}
    pub fn click(_: &tauri::WebviewWindow) {}
    pub fn scroll(_: &tauri::WebviewWindow, _: f64, _: f64) {}
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
            toast(app, "info", "Sent to cpmm…");
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
        "file" => is_report(url),
        // mailto:, steam:, … would leave the app (or do nothing); ignore them.
        _ => false,
    }
}

/// Where the browser saves manual downloads (in the download folder the settings give).
fn downloads_dir() -> PathBuf {
    cpmm::state::Paths::load()
        .map(|p| cpmm::downloads::incoming(&p))
        .unwrap_or_else(|_| home_dir().join(".local/share/cpmm/downloads/.incoming"))
}

#[tauri::command]
pub fn open_nexus(app: AppHandle, url: Option<String>) -> Result<(), String> {
    let target = url
        .as_deref()
        .and_then(|u| Url::parse(u).ok())
        .filter(|u| is_nexus(u) || is_report(u))
        .unwrap_or_else(|| Url::parse(HOME).unwrap());
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
        if let tauri::WindowEvent::Destroyed = e
            && let Some(m) = app2.get_webview_window("main")
        {
            let _ = m.set_focus();
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
            .filter_map(std::env::var_os)
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

/// `cpmm-gui --nexus-selftest`: drive the cursor without a controller (for headless checks):
/// wait for the page, then click in the middle of it and again a bit lower.
pub fn selftest(app: AppHandle) {
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(8));
        for (dx, dy) in [(0.0, 0.0), (0.0, 120.0)] {
            if let Some(w) = app.get_webview_window(LABEL) {
                crate::glog(&format!("nexus selftest: click at centre + ({dx}, {dy})"));
                input::move_by(&w, dx, dy);
                std::thread::sleep(std::time::Duration::from_millis(300));
                action(&w, "a");
            }
            std::thread::sleep(std::time::Duration::from_secs(6));
        }
    });
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

/// Show a tool run's HTML report in the built-in browser window.
#[tauri::command]
pub fn open_report(app: AppHandle, path: PathBuf) -> Result<(), String> {
    let url = Url::from_file_path(&path).map_err(|_| format!("not a file path: {}", path.display()))?;
    if !is_report(&url) {
        return Err(format!("{} isn't a tool report", path.display()));
    }
    std::thread::spawn(move || {
        if let Err(e) = open_nexus(app, Some(url.to_string())) {
            crate::glog(&format!("report: {e}"));
        }
    });
    Ok(())
}
