//! cpmm GUI backend: a thin bridge to the `cpmm` CLI, which stays the single source of truth.
//! `api` runs `cpmm api …` and returns its JSON; `run` runs an action and streams its output.

use serde::Serialize;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use tauri::{AppHandle, Emitter, Manager};

mod nexus;

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
    if local.is_file() { local } else { "cpmm".into() }
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

#[tauri::command]
async fn api(args: Vec<String>) -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let out =
            Command::new(cpmm_bin()).arg("api").args(&args).stdin(Stdio::null()).output().map_err(|e| format!("running cpmm: {e}"))?;
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
        }
        serde_json::from_slice(&out.stdout).map_err(|e| format!("bad JSON from cpmm: {e}"))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(Serialize, Clone)]
struct LogLine {
    job: u64,
    line: String,
    stderr: bool,
}

#[derive(Serialize)]
struct RunResult {
    code: i32,
    output: String,
}

/// Run `cpmm <args>`; each output line is emitted as a `cpmm-log` event tagged with `job`.
#[tauri::command]
async fn run(app: AppHandle, job: u64, args: Vec<String>) -> Result<RunResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut child = Command::new(cpmm_bin())
            .args(&args)
            .env("CPMM_GUI", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("running cpmm: {e}"))?;
        let err = child.stderr.take().unwrap();
        let app2 = app.clone();
        let t = std::thread::spawn(move || {
            let mut s = String::new();
            for line in BufReader::new(err).lines().map_while(Result::ok) {
                let _ = app2.emit("cpmm-log", LogLine { job, line: line.clone(), stderr: true });
                s.push_str(&line);
                s.push('\n');
            }
            s
        });
        let mut output = String::new();
        for line in BufReader::new(child.stdout.take().unwrap()).lines().map_while(Result::ok) {
            let _ = app.emit("cpmm-log", LogLine { job, line: line.clone(), stderr: false });
            output.push_str(&line);
            output.push('\n');
        }
        output.push_str(&t.join().unwrap_or_default());
        let code = child.wait().map_err(|e| e.to_string())?.code().unwrap_or(-1);
        Ok(RunResult { code, output })
    })
    .await
    .map_err(|e| e.to_string())?
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
                    EventType::ButtonPressed(b, _) => match b {
                        Button::South => emit("a"),
                        Button::North => emit("y"),
                        Button::East => emit("b"),
                        Button::LeftTrigger => emit("lb"),
                        Button::RightTrigger => emit("rb"),
                        Button::Start => emit("start"),
                        Button::Select => emit("select"),
                        // R3 and RT both click at the virtual cursor
                        Button::RightThumb | Button::RightTrigger2 => emit("click"),
                        Button::DPadUp => dpad[0] = true,
                        Button::DPadDown => dpad[1] = true,
                        Button::DPadLeft => dpad[2] = true,
                        Button::DPadRight => dpad[3] = true,
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
            api,
            run,
            gui_log,
            quit,
            launch_game,
            nexus::open_nexus,
            nexus::close_nexus,
            nexus::nexus_toast
        ])
        .run(tauri::generate_context!())
        .expect("error while running cpmm GUI");
}
