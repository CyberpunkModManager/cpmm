# cpmm

Cyberpunk 2077 mod manager for Linux/Proton. Mods are staged per mod and shown to the
game through an **overlayfs VFS**: no root, no FUSE, no files copied into the game dir.

```
overwrite/            upper layer: everything the game/tools write at runtime
mods/<highest>/files  ┐
…                     ├ enabled mods, priority order (later in `cpmm list` wins)
mods/<lowest>/files   ┘
<game dir>            the real install, kept vanilla (+ runtime leftovers)
```

`cpmm run -- %command%` (the Steam launch wrapper) creates a private user and mount namespace,
mounts the overlay over the game dir, drops back to your uid in a nested user namespace, and
execs Steam's command. Only processes inside that namespace see mods; Steam updates and
"verify files" see the real dir. When the game exits, the namespace and mount disappear.

Requirements: Linux ≥ 6.8 (`lowerdir+`; unprivileged overlayfs since 5.11), unprivileged
user namespaces, data dir on the same filesystem as the game, `7z` for extraction.

## Launch options

```
cpmm launch-options     # prints your line, i.e. … game-performance /home/…/.local/bin/cpmm run -- %command% …
```

## Commands

| | |
|---|---|
| `list [-l]` | mods in priority order (+ conflict counts), plus read-only mod.io mods |
| `info <mod>` | details, requirements, files |
| `install <nexus id/url/archive> [--file] [--with-deps] [--root DIR]` | download (premium API), verify SHA-256 against the Nexus manifest, detect layout, stage |
| `updates [--refresh]` / `update <mod>… / --all` | newer uploads of the same Nexus file |
| `deps [--refresh]` | Nexus requirements (GraphQL) of enabled mods: missing / disabled |
| `conflicts [mod] [--base]` | file overwrites (identical copies hidden) and redscript `@replaceMethod` clashes |
| `enable` / `disable` / `order <mod> --top/--bottom/--before X/--after X` | |
| `verify [mod…]` | staged files vs Nexus manifest hashes |
| `uninstall`/`remove <mod>` | moves staged files to `trash/` |
| `overwrite` | what the game wrote at runtime (and what it shadows) |
| `unmanaged [--modified] [--flat]` | non-vanilla files in the base dir that no mod owns; edited vanilla files (from Steam's depot manifest) |
| `steam-shortcut [--remove] [--dry-run]` | add the GUI to the Steam library with artwork (live via Steam's CEF debugger, or via shortcuts.vdf when Steam is closed) |
| `register-handler [--check \| --unregister]` | make cpmm the default for Nexus "Mod Manager Download" (`nxm://`) links; installs with requirements, logs to `~/.local/share/cpmm/nxm.log` |
| `nxm <url>` | what the handler runs |
| `tracked [--install] [--untrack]` | install mods you pressed **Track** on at nexusmods.com: works from any browser, including Steam's |
| `deps --ignore <mod> <id>` / `--unignore` | accept a missing requirement on purpose |
| `hide` / `unhide` / `prefer <mod> [paths]` | resolve conflicts per file without reordering (hidden files survive updates) |
| `deps --fix` | install missing hard requirements, re-enable disabled ones |
| `overwrite move <mod> [paths]` / `overwrite clear [paths]` | keep runtime writes as a mod, or drop them |
| `clean` | empty trash/tmp/downloads |
| `exec -- cmd` | run a tool inside the game's view (joins the running session if any) |
| `status` | running session and its layers |
| `import [--apply]` / `adopt <id> <paths…>` | migrate the hand-made install |
| `eject [--apply]` | escape hatch: reflink every enabled mod back into the game dir |

Requirement notes containing "optional" or "recommended" count as optional; Nexus has no flag for it.
Directory casing is normalised on install (`Archive/PC/Mod` → `archive/pc/mod`) because Wine
only uses one of two differently-cased folders.

Data lives in `~/.local/share/cpmm` (`CPMM_DATA` overrides). HTTP cache: `~/.cache/cpmm`.

## Nexus links in Game Mode

The handler is a plain `mimeapps.list` association, so it doesn't need a desktop session:
in gamescope (`XDG_CURRENT_DESKTOP=gamescope`) `xdg-open` falls back to its generic path and
still finds it. Gamescope has no notification daemon, so notifications are fire-and-forget
and the log file is the record. Updating a mod that is mounted by a running game is refused;
new mods are fine and appear on the next launch.

**Steam's built-in browser refuses `nxm://` links** (`SteamUI: WARNING: Refusing to navigate to
non-HTTP URL` in `~/.local/share/Steam/logs/console_log.txt`), so the button does nothing there.
Use Firefox (as a non-Steam game, or on the desktop), or press Track in Steam's browser and run
`cpmm tracked --install`.

## GUI (`gui/`)

Tauri 2 + React 19 + TypeScript + Tailwind CSS 4 (TanStack Query, Lucide icons). The Rust side is a
thin bridge: reads go through `cpmm api …` (JSON), actions run the normal CLI commands and stream
their output into the Activity panel, so the CLI stays the single source of truth.

Built for the handheld: large touch targets, and spatial navigation with the D-pad/stick (or
arrow keys), A to select, B to go back, LB/RB to switch sections.

```
cd gui && npm install && npx tauri build --no-bundle
cp src-tauri/target/release/cpmm-gui ~/.local/bin/     # finds ~/.local/bin/cpmm next to itself
```

Add it to the Steam library as the non-Steam game **cpmm**, with the capsule/wide/hero/logo artwork
from `gui/art/` (embedded in the binary):

```
cpmm steam-shortcut [--dry-run] [--remove] [--name cpmm] [--exe path/to/cpmm-gui]
```

With Steam running it goes through Steam's own client API over the CEF debugger (Decky enables it;
otherwise `touch ~/.local/share/Steam/.cef-enable-remote-debugging` and restart Steam), so the entry
appears at once. With Steam closed it edits `userdata/<user>/config/shortcuts.vdf` and `config/grid/`
directly. Rerunning refreshes the artwork; the existing entry is matched by its exe path.

## Load order and health

* `cpmm sort [--apply]`: stable dependency-aware sort. Constraints are only enforced between mods that
  actually overlap (same loose file, or the same resource inside `.archive` files): a mod wins over
  the mods it requires, "patch/fix/compat/addon" mods win over what they override, add-on files win
  over the main file from the same Nexus page, and your rules are enforced. Everything else keeps
  its place. Cycles are reported and left as they are.
* `cpmm rule <mod> --after <other>` / `--remove` / `--list`: pin a winner between unrelated mods.
* `cpmm archives [--all]`: resource-level conflicts, read straight from RDAR indexes (byte-identical
  overlaps are told apart from real ones).
* `modlist.txt` is generated at every `cpmm run`/`exec` into `generated/` (the top overlay layer),
  highest priority first: the game loads archives in that order and the first loaded wins.
* `cpmm doctor [--ack <finding>] [--all]`: crash/stutter risks: unresolved redscript imports (fail the
  whole compile), duplicate installs (incl. mod.io vs cpmm), real `@replaceMethod` clashes (evaluating
  `@if(ModuleExists(..))` guards), unrelated archive overrides, missing requirements, stale overwrite
  copies and order violations.

## Safety net

* **Rollback**: replacing a mod (update/reinstall) keeps the previous version in `history/<mod>/`
  (last 3). `cpmm history <mod>`, `cpmm rollback <mod> [--to <ts>]`; every update run is tagged so
  `cpmm undo-update [batch]` reverts all of it. `clean` never touches history.
* **Crash triage**: each `cpmm run` records the session (mods, versions, order). `cpmm crashes` matches
  REDEngine crash reports to sessions, reads the faulting module from the minidump, and lists what
  changed since the last session that didn't crash (with rollback suggestions).
* **New log errors**: `cpmm errors` snapshots error/warning lines from redscript, RED4ext, TweakXL,
  ArchiveXL, Codeware and CET logs (timestamps stripped) at each launch and shows new vs. gone.
* **Patch guard**: `cpmm patch` notices a new game build and checks every native mod (DLLs, script
  compiler) for releases since; `cpmm vanilla on` makes the next launch mod-free (one time).
* **Save snapshots**: reflinked before disable/uninstall/update/rollback (last 10, no extra space);
  `cpmm saves [--snapshot | --restore <name>]`. The doctor warns when a recent save was made with a
  quest mod that is now off.
