//! cpmm GUI backend: the Tauri adapter at the core's Operations seam. It links the `cpmm`
//! library and runs operations in-process: `query` answers a typed Query with its view;
//! `act` runs a typed Action on a blocking thread under the same cpmm lock the CLI takes,
//! streaming its log as `cpmm-log` events (the CLI's text, rendered from the events).

use cpmm::ops::query::Query;
use cpmm::ops::render::{ActResult, LogLine, Transcript};
use cpmm::ops::{self, Action, Event, Ui};
use cpmm::state::Paths;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use tauri::{AppHandle, Emitter, Manager};

mod nexus;

/// The `cpmm` binary: what Steam's launch options and the nxm:// link handler run.
fn cpmm_bin() -> PathBuf {
    if let Some(p) = std::env::var_os("CPMM_BIN") {
        return p.into();
    }
    // next to this executable (both live in ~/.local/bin), else PATH
    if let Ok(exe) = std::env::current_exe() {
        let sib = exe.with_file_name("cpmm");
        if sib.is_file() {
            return sib;
        }
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let local = home.join(".local/bin/cpmm");
    if local.is_file() {
        return local;
    }
    std::env::var_os("PATH")
        .and_then(|p| std::env::split_paths(&p).map(|d| d.join("cpmm")).find(|f| f.is_file()))
        .unwrap_or_else(|| "cpmm".into())
}

/// Operations ask the GUI nothing (no terminal: FOMOD options come with the request, or the
/// installer's suggestions are used); their events become the job's log.
struct Job {
    app: AppHandle,
    job: u64,
    log: Transcript,
}

impl Job {
    fn line(app: &AppHandle, job: u64) -> impl FnMut(&str, bool) + '_ {
        move |line, stderr| {
            let _ = app.emit("cpmm-log", LogLine { job, line: line.to_string(), stderr });
        }
    }
}

impl Ui for Job {
    fn event(&mut self, e: &Event) {
        self.log.add(e, &mut Self::line(&self.app, self.job));
    }
    fn this_exe(&self) -> ops::Result<PathBuf> {
        Ok(cpmm_bin())
    }
}

/// A query from the GUI; ui only lends `this_exe` (no job: progress is not shown).
struct Quiet;

impl Ui for Quiet {
    fn event(&mut self, _: &Event) {}
    fn this_exe(&self) -> ops::Result<PathBuf> {
        Ok(cpmm_bin())
    }
}

/// A read-only view (what `cpmm api …` prints). Errors read as the CLI's did.
#[tauri::command]
async fn query(q: Query) -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let paths = Paths::load().map_err(|e| format!("error: {e:#}"))?;
        ops::query::view(&paths, &q, &mut Quiet).map_err(|e| format!("error: {e:#}"))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Run an action as job `job`: its log lines stream as `cpmm-log` events, the result says how
/// it ended. The action takes the cpmm lock itself, so it waits for (and blocks) a CLI command.
#[tauri::command]
async fn act(app: AppHandle, job: u64, action: Action) -> Result<ActResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut ui = Job { app, job, log: Transcript::default() };
        let r = Paths::load().and_then(|paths| ops::act(&paths, action, &mut ui));
        let Job { app, job, log } = ui;
        Ok(log.finish(r, &mut Job::line(&app, job)))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
fn quit(app: AppHandle) {
    app.exit(0);
}

/// Launch Cyberpunk through Steam (so its launch options, incl. `cpmm run`, apply), then
/// close cpmm: in Game Mode a second "game" launching while cpmm runs would prompt.
#[tauri::command]
fn launch_game(app: AppHandle, close: Option<bool>) -> Result<(), String> {
    let close = close.unwrap_or(true);
    let steam = nexus::steam_bin();
    // Steam's reaper kills everything a "game" leaves behind, so the delayed request must run
    // outside cpmm's process tree: hand it to the user's systemd. It fires after cpmm exits.
    let script = format!("sleep {}; exec \"{steam}\" steam://rungameid/1091500", if close { "1.5" } else { "0" });
    let via_systemd = Command::new("systemd-run")
        .args(["--user", "--quiet", "--collect", "--unit", &format!("cpmm-launch-{}", std::process::id()), "sh", "-c", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !via_systemd {
        // fallback: ask Steam right away (it may ask to close cpmm first)
        Command::new(&steam)
            .arg("steam://rungameid/1091500")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("starting steam: {e}"))?;
    }
    glog(&format!("launch_game requested (systemd: {via_systemd}, close: {close})"));
    if close {
        let a = app.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(200));
            a.exit(0);
        });
    }
    Ok(())
}

/// Frontend diagnostics into the same log as the gamepad thread.
#[tauri::command]
fn gui_log(msg: String) {
    glog(&format!("ui: {msg}"));
}

/// Controller input, read natively (evdev via gilrs). WebKitGTK's Gamepad API doesn't see
/// Steam's virtual pad in Game Mode, so the page gets `gamepad` events with an action name.
fn glog(msg: &str) {
    use std::io::Write;
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(home.join(".local/share/cpmm/gui.log")) {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let _ = writeln!(f, "{t} [{}] {msg}", std::process::id());
    }
}

/// The controller's buttons and the action each sends to the page (gamepad.ts, nexus.js):
/// the one place they are bound, read by the input thread and shown in the control map.
const BUTTONS: &[(gilrs::Button, &str, &str)] = {
    use gilrs::Button::*;
    &[
        (South, "A", "a"),
        (East, "B", "b"),
        (North, "Y", "y"),
        (LeftTrigger, "LB", "lb"),
        (RightTrigger, "RB", "rb"),
        (Start, "Start", "start"),
        (Select, "Select", "select"),
        // R3 and RT both click at the virtual cursor
        (RightThumb, "R3", "click"),
        (RightTrigger2, "RT", "click"),
    ]
};

fn button_action(b: gilrs::Button) -> Option<&'static str> {
    BUTTONS.iter().find(|(x, _, _)| *x == b).map(|(_, _, a)| *a)
}

#[derive(serde::Serialize)]
struct Binding {
    /// The control as printed on an Xbox-style pad (ROG Ally, Steam Deck in Game Mode)
    input: &'static str,
    /// The action it sends: a button's action name, or move / scroll / cursor
    action: &'static str,
}

/// What each control on the controller does, for the control map.
#[tauri::command]
fn controller_bindings() -> Vec<Binding> {
    let sticks = [("D-pad", "move"), ("Left stick", "scroll"), ("Right stick", "cursor")];
    sticks
        .into_iter()
        .chain(BUTTONS.iter().map(|(_, input, action)| (*input, *action)))
        .map(|(input, action)| Binding { input, action })
        .collect()
}

fn spawn_gamepad(app: AppHandle) {
    use gilrs::{Axis, Button, EventType, Gilrs};
    use std::time::{Duration, Instant};
    std::thread::spawn(move || {
        let mut gil = match Gilrs::new() {
            Ok(g) => g,
            Err(e) => {
                glog(&format!("gilrs init failed: {e}"));
                return;
            }
        };
        let pads: Vec<String> = gil.gamepads().map(|(_, p)| p.name().to_string()).collect();
        glog(&format!("gamepads: {pads:?}"));
        let mut last_focus = None;
        let mut presses = 0u32;
        // held direction -> next repeat time
        let mut held: [Option<Instant>; 4] = [None; 4];
        let names = ["up", "down", "left", "right"];
        let (mut sx, mut sy) = (0.0f32, 0.0f32); // left stick: scrolling
        let (mut rx, mut ry) = (0.0f32, 0.0f32); // right stick: virtual cursor
        let mut last_analog = Instant::now();
        let mut dpad = [false; 4];
        loop {
            // Input goes to the window in front: the Nexus browser when it's open and focused
            // (or when focus is unknown, as under some gamescope setups), else the main window.
            let nexus = app.get_webview_window(nexus::LABEL).filter(|w| w.is_visible().unwrap_or(true));
            let nexus_focused = nexus.as_ref().and_then(|w| w.is_focused().ok()).unwrap_or(false);
            let main_focused = app.get_webview_window("main").and_then(|w| w.is_focused().ok()).unwrap_or(nexus.is_none());
            let nexus = nexus.filter(|_| nexus_focused || !main_focused);
            let focused = main_focused && nexus.is_none();
            let state = (focused, nexus.is_some());
            if last_focus != Some(state) {
                glog(&format!("input target: main {focused}, nexus {}", nexus.is_some()));
                last_focus = Some(state);
            }
            let emit = |a: &str| {
                if let Some(w) = &nexus {
                    nexus::action(w, a);
                } else if focused {
                    let _ = app.emit_to("main", "gamepad", a.to_string());
                }
            };
            while let Some(ev) = gil.next_event() {
                if let EventType::ButtonPressed(b, _) = ev.event {
                    presses += 1;
                    if presses <= 400 {
                        glog(&format!("press {b:?} from {} (focused {focused})", gil.gamepad(ev.id).name()));
                    }
                }
                if let EventType::Connected = ev.event {
                    glog(&format!("connected: {}", gil.gamepad(ev.id).name()));
                }
                match ev.event {
                    EventType::ButtonPressed(b, _) => match (b, button_action(b)) {
                        (_, Some(a)) => emit(a),
                        (Button::DPadUp, _) => dpad[0] = true,
                        (Button::DPadDown, _) => dpad[1] = true,
                        (Button::DPadLeft, _) => dpad[2] = true,
                        (Button::DPadRight, _) => dpad[3] = true,
                        _ => {}
                    },
                    EventType::ButtonReleased(b, _) => match b {
                        Button::DPadUp => dpad[0] = false,
                        Button::DPadDown => dpad[1] = false,
                        Button::DPadLeft => dpad[2] = false,
                        Button::DPadRight => dpad[3] = false,
                        _ => {}
                    },
                    EventType::AxisChanged(Axis::LeftStickX, v, _) => sx = v,
                    EventType::AxisChanged(Axis::LeftStickY, v, _) => sy = v,
                    EventType::AxisChanged(Axis::RightStickX, v, _) => rx = v,
                    EventType::AxisChanged(Axis::RightStickY, v, _) => ry = v,
                    // some pads report the d-pad as hat axes
                    EventType::AxisChanged(Axis::DPadX, v, _) => {
                        dpad[2] = v < -0.5;
                        dpad[3] = v > 0.5;
                    }
                    EventType::AxisChanged(Axis::DPadY, v, _) => {
                        dpad[0] = v > 0.5;
                        dpad[1] = v < -0.5;
                    }
                    _ => {}
                }
            }
            // gilrs: stick Y is positive up
            // Analog sticks at ~60 Hz: response curve (squared) with a dead zone.
            let now0 = Instant::now();
            if now0.duration_since(last_analog) >= Duration::from_millis(16) {
                last_analog = now0;
                let curve = |v: f32| {
                    let a = v.abs();
                    if a < 0.15 { 0.0 } else { ((a - 0.15) / 0.85).powi(2) * v.signum() }
                };
                let (scx, scy) = (curve(sx), curve(sy));
                // gilrs Y is positive up; the page scrolls down when the stick goes down
                if scx != 0.0 || scy != 0.0 {
                    if let Some(w) = &nexus {
                        nexus::scroll(w, scx, -scy);
                    } else if focused {
                        let _ = app.emit_to("main", "scroll", serde_json::json!({ "dx": scx, "dy": -scy }));
                    }
                }
                let (cx, cy) = (curve(rx), curve(ry));
                if cx != 0.0 || cy != 0.0 {
                    if let Some(w) = &nexus {
                        nexus::pointer(w, cx, -cy);
                    } else if focused {
                        let _ = app.emit_to("main", "pointer", serde_json::json!({ "dx": cx, "dy": -cy }));
                    }
                }
            }
            let want = [dpad[0], dpad[1], dpad[2], dpad[3]];
            let now = Instant::now();
            for i in 0..4 {
                match (want[i], held[i]) {
                    (true, None) => {
                        emit(names[i]);
                        held[i] = Some(now + Duration::from_millis(350));
                    }
                    (true, Some(t)) if now >= t => {
                        emit(names[i]);
                        held[i] = Some(now + Duration::from_millis(90));
                    }
                    (false, _) => held[i] = None,
                    _ => {}
                }
            }
            std::thread::sleep(Duration::from_millis(8));
        }
    });
}

fn main() {
    // WebKitGTK's DMA-BUF renderer misbehaves under gamescope/Xwayland on some GPUs.
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        unsafe { std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1") };
    }
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            spawn_gamepad(app.handle().clone());
            // `cpmm-gui --nexus [url]`: open the Nexus browser right away.
            let args: Vec<String> = std::env::args().collect();
            if args.iter().any(|a| a == "--nexus-selftest") {
                nexus::selftest(app.handle().clone());
            }
            if let Some(i) = args.iter().position(|a| a == "--nexus" || a == "--nexus-selftest") {
                let url = args.get(i + 1).cloned();
                let h = app.handle().clone();
                std::thread::spawn(move || {
                    if let Err(e) = nexus::open_nexus(h, url) {
                        glog(&format!("nexus: {e}"));
                    }
                });
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            query,
            act,
            gui_log,
            quit,
            launch_game,
            nexus::open_nexus,
            nexus::close_nexus,
            nexus::nexus_toast,
            nexus::open_report,
            controller_bindings
        ])
        .run(tauri::generate_context!())
        .expect("error while running cpmm GUI");
}
