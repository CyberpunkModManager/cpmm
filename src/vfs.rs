//! Virtual file system: an overlayfs mounted over the game directory inside a private
//! user+mount namespace, so only the game (and processes we explicitly put there) see mods.
//!
//! Layering (top wins): overwrite/ (upper, catches every write) > enabled mods by priority
//! > the real game directory. No root needed: since Linux 5.11 overlayfs can be mounted in
//! > a user namespace with `userxattr`. We use the new mount API so the number of layers is
//! > not limited by the 4 KiB legacy option string (`lowerdir+`, Linux 6.8+).
//!
//! Namespace dance: create userns#1 mapping our uid to 0, which gives us CAP_SYS_ADMIN over
//! a fresh mount namespace; mount the overlay; then create userns#2 mapping our uid back
//! to itself, so the game runs as the normal user (Steam/pressure-vessel expect that).

use crate::state::{Mod, Paths};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::ffi::CString;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Serialize, Deserialize)]
pub struct RunInfo {
    pub pid: u32,
    pub mnt_ns: String,
    pub layers: Vec<String>,
    pub started: String,
    pub command: Vec<String>,
    /// The profile's own save folder, bind-mounted over the game's save folder.
    #[serde(default)]
    pub saves: Option<PathBuf>,
}

/// Bind `src` over `dst` in the current mount namespace (recursive, like bwrap's --bind).
fn bind_mount(src: &Path, dst: &Path) -> Result<()> {
    let (s, d) = (CString::new(src.as_os_str().as_bytes())?, CString::new(dst.as_os_str().as_bytes())?);
    if unsafe { libc::mount(s.as_ptr(), d.as_ptr(), std::ptr::null(), libc::MS_BIND | libc::MS_REC, std::ptr::null()) } != 0 {
        return Err(cerr(&format!("bind {} over {}", src.display(), dst.display())));
    }
    Ok(())
}

pub fn layers(paths: &Paths, mods: &[Mod]) -> Vec<(String, PathBuf)> {
    // highest priority first; cpmm's generated files (modlist.txt) sit above every mod
    let generated = crate::loadorder::generated_root(paths).join("files");
    let top =
        fs::read_dir(&generated).map(|mut d| d.next().is_some()).unwrap_or(false).then(|| ("(cpmm generated)".to_string(), generated));
    top.into_iter()
        .chain(
            mods.iter()
                .rev()
                .filter(|m| m.enabled)
                .map(|m| (m.id.clone(), paths.mod_files(&m.id)))
                .filter(|(_, p)| fs::read_dir(p).map(|mut d| d.next().is_some()).unwrap_or(false)),
        )
        .collect()
}

fn cerr(what: &str) -> anyhow::Error {
    anyhow::anyhow!("{what}: {}", std::io::Error::last_os_error())
}

fn cstr(p: &Path) -> CString {
    CString::new(p.as_os_str().as_encoded_bytes()).unwrap()
}

fn write_maps(inside: u32, outside: u32, inside_g: u32, outside_g: u32) -> Result<()> {
    fs::write("/proc/self/setgroups", "deny").context("setgroups deny")?;
    fs::write("/proc/self/uid_map", format!("{inside} {outside} 1")).context("uid_map")?;
    fs::write("/proc/self/gid_map", format!("{inside_g} {outside_g} 1")).context("gid_map")?;
    Ok(())
}

fn unshare(flags: libc::c_int) -> Result<()> {
    if unsafe { libc::unshare(flags) } != 0 {
        return Err(cerr("unshare"));
    }
    Ok(())
}

// New mount API (raw syscalls; libc exposes the numbers).
const FSCONFIG_SET_FLAG: libc::c_uint = 0;
const FSCONFIG_SET_STRING: libc::c_uint = 1;
const FSCONFIG_CMD_CREATE: libc::c_uint = 6;
const FSOPEN_CLOEXEC: libc::c_uint = 1;
const FSMOUNT_CLOEXEC: libc::c_uint = 1;
const MOVE_MOUNT_F_EMPTY_PATH: libc::c_uint = 4;

fn fsconfig(fd: i32, cmd: libc::c_uint, key: Option<&str>, val: Option<&CString>) -> Result<()> {
    let k = key.map(|k| CString::new(k).unwrap());
    let r = unsafe {
        libc::syscall(
            libc::SYS_fsconfig,
            fd,
            cmd,
            k.as_ref().map_or(std::ptr::null(), |k| k.as_ptr()),
            val.map_or(std::ptr::null(), |v| v.as_ptr()),
            0,
        )
    };
    if r != 0 {
        return Err(cerr(&format!(
            "fsconfig {}={}",
            key.unwrap_or("create"),
            val.map(|v| v.to_string_lossy().into_owned()).unwrap_or_default()
        )));
    }
    Ok(())
}

fn mount_overlay(target: &Path, lowers: &[PathBuf], upper: &Path, work: &Path) -> Result<()> {
    let name = CString::new("overlay").unwrap();
    let fd = unsafe { libc::syscall(libc::SYS_fsopen, name.as_ptr(), FSOPEN_CLOEXEC) } as i32;
    if fd < 0 {
        return Err(cerr("fsopen overlay"));
    }
    let res = (|| {
        for l in lowers {
            fsconfig(fd, FSCONFIG_SET_STRING, Some("lowerdir+"), Some(&cstr(l)))?;
        }
        fsconfig(fd, FSCONFIG_SET_STRING, Some("upperdir"), Some(&cstr(upper)))?;
        fsconfig(fd, FSCONFIG_SET_STRING, Some("workdir"), Some(&cstr(work)))?;
        fsconfig(fd, FSCONFIG_SET_FLAG, Some("userxattr"), None)?;
        fsconfig(fd, FSCONFIG_CMD_CREATE, None, None)?;
        let mfd = unsafe { libc::syscall(libc::SYS_fsmount, fd, FSMOUNT_CLOEXEC, 0) } as i32;
        if mfd < 0 {
            return Err(cerr("fsmount"));
        }
        let empty = CString::new("").unwrap();
        let r = unsafe {
            libc::syscall(libc::SYS_move_mount, mfd, empty.as_ptr(), libc::AT_FDCWD, cstr(target).as_ptr(), MOVE_MOUNT_F_EMPTY_PATH)
        };
        unsafe { libc::close(mfd) };
        if r != 0 {
            return Err(cerr("move_mount"));
        }
        Ok(())
    })();
    unsafe { libc::close(fd) };
    res
}

fn self_mnt_ns() -> String {
    fs::read_link("/proc/self/ns/mnt").map(|p| p.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Find a live process inside the namespace recorded in run.json.
pub fn running(paths: &Paths) -> Option<(RunInfo, u32)> {
    let info: RunInfo = serde_json::from_slice(&fs::read(paths.run_file()).ok()?).ok()?;
    let mine = unsafe { libc::getuid() };
    for e in fs::read_dir("/proc").ok()?.flatten() {
        let Some(pid) = e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else { continue };
        let Ok(ns) = fs::read_link(e.path().join("ns/mnt")) else { continue };
        if ns.to_string_lossy() == info.mnt_ns {
            use std::os::unix::fs::MetadataExt;
            if fs::metadata(e.path()).map(|m| m.uid() == mine).unwrap_or(false) {
                return Some((info, pid));
            }
        }
    }
    None
}

/// Enter a fresh namespace with the overlay mounted. Must be called while single-threaded.
/// `mods: None` mounts no overlay (vanilla launch); only the profile's saves are bound.
fn enter_new(paths: &Paths, mods: Option<&[Mod]>, saves: Option<&Path>, cmd: &[String]) -> Result<Vec<String>> {
    if !paths.game.is_dir() {
        bail!("game directory {} not found", paths.game.display());
    }
    paths.ensure_dirs()?;
    let ls = mods.map(|m| layers(paths, m)).unwrap_or_default();
    if ls.len() > 450 {
        bail!("{} layers exceeds overlayfs's limit of ~500", ls.len());
    }
    let mut lowers: Vec<PathBuf> = ls.iter().map(|(_, p)| p.clone()).collect();
    lowers.push(paths.game.clone());

    // Both ends of the saves bind must exist (and be ours) before we leave our uid behind.
    let shared_saves = crate::safety::shared_saves_dir(paths);
    if let Some(s) = saves {
        fs::create_dir_all(s)?;
        fs::create_dir_all(&shared_saves)?;
    }
    let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
    unshare(libc::CLONE_NEWUSER | libc::CLONE_NEWNS)?;
    write_maps(0, uid, 0, gid)?;
    let root = CString::new("/").unwrap();
    if unsafe { libc::mount(std::ptr::null(), root.as_ptr(), std::ptr::null(), libc::MS_REC | libc::MS_PRIVATE, std::ptr::null()) } != 0 {
        return Err(cerr("make / private"));
    }
    if mods.is_some() {
        mount_overlay(&paths.game, &lowers, &paths.upper(), &paths.work())?;
    }
    if let Some(s) = saves {
        bind_mount(s, &shared_saves)?;
    }

    let info = RunInfo {
        pid: std::process::id(),
        mnt_ns: self_mnt_ns(),
        layers: ls.iter().map(|(id, _)| id.clone()).collect(),
        started: crate::state::now(),
        command: cmd.to_vec(),
        saves: saves.map(Path::to_path_buf),
    };
    // Written from inside userns#1 (root there == us outside), so it is owned by us.
    fs::write(paths.run_file(), serde_json::to_vec_pretty(&info)?)?;

    unshare(libc::CLONE_NEWUSER)?;
    write_maps(uid, 0, gid, 0)?;
    Ok(info.layers)
}

fn join(pid: u32) -> Result<()> {
    // Enter user+mount namespaces atomically through a pidfd: the kernel switches creds to
    // the target userns and checks the mount ns against those, which separate setns calls
    // can't satisfy (the mount ns is owned by the outer of our two user namespaces).
    let pfd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid as libc::c_int, 0) } as i32;
    if pfd < 0 {
        return Err(cerr("pidfd_open"));
    }
    let r = unsafe { libc::setns(pfd, libc::CLONE_NEWUSER | libc::CLONE_NEWNS) };
    unsafe { libc::close(pfd) };
    if r != 0 {
        return Err(cerr("setns user+mnt"));
    }
    Ok(())
}

fn exec(cmd: &[String]) -> Result<()> {
    // Entering a mount namespace resets the cwd to its root; re-resolve the caller's cwd so
    // relative paths (and a cwd inside the game dir) see the overlay.
    if let Some(cwd) = std::env::var_os("PWD") {
        let _ = std::env::set_current_dir(cwd);
    }
    if cmd.is_empty() {
        bail!("no command given (usage: cpmm run -- %command%)");
    }
    let err = Command::new(&cmd[0]).args(&cmd[1..]).env("CPMM_NS", "1").exec();
    Err(anyhow::anyhow!("exec {}: {err}", cmd[0]))
}

/// `cpmm run -- %command%`: the Steam launch wrapper. `mounted` hears how many mod layers
/// (None for a vanilla launch) are mounted, just before the command replaces this process.
pub fn run(
    paths: &Paths,
    mods: Option<&[Mod]>,
    saves: Option<&Path>,
    cmd: &[String],
    mounted: &mut dyn FnMut(Option<usize>),
) -> Result<()> {
    if std::env::var_os("CPMM_NS").is_some() {
        return exec(cmd); // already inside (e.g. nested wrappers)
    }
    if let Some((_, pid)) = running(paths) {
        bail!("a cpmm session is already running (pid {pid}); two overlays sharing one upper dir would corrupt it");
    }
    let ls = enter_new(paths, mods, saves, cmd)?;
    mounted(mods.map(|_| ls.len()));
    exec(cmd)
}

/// True if a process whose program (argv[0]) is Cyberpunk2077.exe exists.
pub fn game_running() -> bool {
    fs::read_dir("/proc")
        .map(|d| {
            d.flatten().any(|e| {
                fs::read(e.path().join("cmdline"))
                    .map(|c| {
                        let a0 = c.split(|b| *b == 0).next().unwrap_or(&[]);
                        String::from_utf8_lossy(a0).to_lowercase().ends_with("cyberpunk2077.exe")
                    })
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

/// `cpmm exec -- cmd`: run a tool inside the game's view (joins a running session if any).
pub fn exec_in(paths: &Paths, mods: &[Mod], saves: Option<&Path>, cmd: &[String]) -> Result<()> {
    if std::env::var_os("CPMM_NS").is_some() {
        return exec(cmd);
    }
    if let Some((_, pid)) = running(paths) {
        join(pid)?;
    } else {
        enter_new(paths, Some(mods), saves, cmd)?;
    }
    exec(cmd)
}
