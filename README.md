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

## Nexus Mods API

cpmm uses the Nexus Mods API (v3 REST, v2 GraphQL, v1 for tracked mods) under the
[API Acceptable Use Policy](https://help.nexusmods.com/article/114-api-acceptable-use-policy):

* **Your key stays on your machine.** It's read from `~/.config/nexusmods/apikey` (keep it
  `chmod 600`) and sent only to `api.nexusmods.com`. cpmm has no server of its own.
* **Only what you ask for.** Requests happen when you run a command or press a button that
  needs Nexus data (install, update, deps, tracked, the Health view after a game patch). Nothing
  polls in the background, and `cpmm run` makes no API requests.
* **It identifies itself.** Every API request carries `Application-Name: cpmm` and
  `Application-Version: <release>`.
* **It respects rate limits.** Metadata is cached for an hour (requirements for 15 minutes,
  manifests for good). The quota headers are recorded in `~/.cache/cpmm/ratelimit.json`.
  After a 429, cpmm sends nothing until the reset time Nexus gives, rather than retrying.
* **No scraping or rehosting.** Downloads come from links Nexus issues to your own account,
  either via the API (Premium) or an `nxm://` link you click on the site.

cpmm is not yet registered with Nexus Mods, so for now it needs a **personal API key**.
Nexus tolerates personal keys only for personal use and testing. Registration, and with it
Single Sign-On, is planned before any wider release.

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
| `changelog <mod> [--all]` | the mod's Nexus changelog, versions newer than yours first (GUI: **What's new** beside each update) |
| `deps [--refresh]` | requirements of enabled mods (mod page via GraphQL + file-level deps): missing / disabled. Cached per installed version; `--refresh` re-checks all |
| `conflicts [mod] [--base]` | file overwrites (identical copies hidden) and redscript `@replaceMethod` clashes |
| `enable` / `disable` / `order <mod> --top/--bottom/--before X/--after X` | |
| `verify [mod…]` | staged files vs Nexus manifest hashes |
| `uninstall`/`remove <mod>` | moves staged files to `trash/` |
| `overwrite` | what the game wrote at runtime (and what it shadows) |
| `modio [--ack]` | mod.io watcher: subscriptions from the in-game Mods menu, what's new since you last reviewed (`--ack`), and conflicts with cpmm's mods: the same mod installed both ways, shared files, archive resources and redscript clashes |
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

Requirements are checked at every install: Nexus installs and local archives tagged with `--nexus <id>`
(the built-in browser's manual downloads) refuse to install with missing hard requirements unless
`--with-deps` (install them) or `--ignore-deps` is given, and nothing is staged before that check.
Updates warn about requirements a new version added (`cpmm deps --fix` / `--ignore`), and requirements
you ignored for a mod stay ignored. Results are stored per mod with the installed version they were
checked for (`reqs_checked` in state.json) and reused until the mod is updated or reinstalled, or until
`cpmm deps --refresh`; mods that aren't installed yet are cached for 15 minutes in `~/.cache/cpmm/reqs/`
(so a lookup followed by an install asks Nexus once).
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

### Built-in Nexus browser

The GUI's **Browse Nexus** button (sidebar, or the Install view) opens nexusmods.com in a cpmm window,
which is the easiest route in Game Mode:

* **Mod Manager Download** (`nxm://`, premium or the free "slow download") is intercepted and installed
  with its requirements as a normal job: it shows in Activity, and the result pops up in the page.
* **Manual download** archives are saved to `~/.local/share/cpmm/downloads/browser/` and installed
  with `--nexus <id>` from the page they came from, so update checks work.
* Controller: right stick / D-pad moves a cursor, A / RT / R3 clicks, left stick scrolls (clicks and
  scrolling are real GTK pointer events injected into the webview, so sites treat them as a mouse), B / LB back,
  RB forward, Y opens Steam's on-screen keyboard (also opened when you click a text field in Game Mode;
  `CPMM_OSK=0/1` overrides the detection), Select reloads, Start returns to cpmm.
* Nexus "Mod" links elsewhere in the GUI open here too; other sites still open in the system browser.
* Sign in with email and password: Google/Apple sign-in usually refuse embedded browsers.
* WebKitGTK crashes the page as soon as a site creates a media element if GStreamer has no
  `autoaudiosink` (package `gst-plugins-good`; missing on this Ally and on read-only SteamOS images).
  cpmm detects that and turns media off in the Nexus window (`CPMM_NEXUS_MEDIA=0/1` overrides), and
  reloads the page if its process dies anyway. `cpmm-gui --nexus [url]` opens the browser directly.

The page gets no access to the Tauri IPC (the window isn't in any capability). It reaches cpmm only
through navigations the backend intercepts and cancels: `nxm://…`, `cpmm://close` and `cpmm://keyboard`.
See `gui/src-tauri/src/nexus.rs` and `nexus.js`.

## GUI (`gui/`)

Tauri 2 + React 19 + TypeScript + Tailwind CSS 4 (TanStack Query, Lucide icons). The Tauri side
links the `cpmm` library and runs the same typed queries and actions as the CLI, in-process; an
action's log in the Activity panel is the text the CLI prints. The TypeScript types are generated
from the Rust ones (`gui/src/lib/bindings.ts`).

Built for the handheld: large touch targets, and spatial navigation with the D-pad/stick (or
arrow keys), A to select, B to go back, LB/RB to switch sections.

## Building from source

```
./scripts/install-local.sh     # builds cpmm + cpmm-gui (release) and installs both into ~/.local/bin
```

Needs Rust, Node 22+ and the Tauri system libraries (WebKitGTK 4.1). Rerun it after pulling or
editing to run the working tree; `CPMM_INSTALL_DIR` installs elsewhere. A running GUI or game
session keeps its old binary until it restarts. `./scripts/check.sh` runs what CI runs.

## Steam library shortcut

Add it to the Steam library as the non-Steam game **cpmm**, with the capsule/wide/hero/logo artwork
from `gui/art/` (embedded in the binary):

```
cpmm steam-shortcut [--dry-run] [--remove] [--name cpmm] [--exe path/to/cpmm-gui]
```

With Steam running it goes through Steam's own client API over the CEF debugger (Decky enables it;
otherwise `touch ~/.local/share/Steam/.cef-enable-remote-debugging` and restart Steam), so the entry
appears at once. With Steam closed it edits `userdata/<user>/config/shortcuts.vdf` and `config/grid/`
directly. Rerunning refreshes the artwork; the existing entry is matched by its exe path.

## Profiles

A profile is a mod list: which mods are enabled, their priority order and its load-order rules.
Installed files, hidden files, ignored requirements and `overwrite/` are shared, so a mod is
downloaded and staged once however many profiles use it, and switching is instant.

```
cpmm profile                               # list (* = active)
cpmm profile create "Story run" [--from P | --empty] [--switch]
cpmm profile switch "Story run"            # alias: use; refused while the game runs
cpmm profile rename OLD NEW / delete NAME  # delete keeps every mod installed
```

Everything else (`list`, `enable`, `order`, `sort`, `deps`, `doctor`, `run`) works on the active
profile. A mod installed while one profile is active is added to the others switched off, at the
top of their order; uninstalling removes it everywhere (cpmm names the other profiles that had it
on). Existing setups become the profile "Default" on first run. The GUI's profile button (top of the
sidebar) does the same.

### Per-profile saves

```
cpmm profile create "Story run" --own-saves [--copy-saves] [--switch]
cpmm profile saves NAME own [--copy]       # or: shared
```

A profile uses either the game's normal save folder ("shared", which Steam Cloud syncs) or its own
folder in `~/.local/share/cpmm/profile-saves/<key>`. `cpmm run` bind-mounts the profile's folder over
the game's save folder inside the game's namespace (vanilla launches too), so only the game sees the
swap: switching moves no files, and Steam, outside the namespace, keeps syncing the shared folder
only. Own saves therefore aren't in Steam Cloud. `--copy-saves` / `--copy` starts the folder with a
reflinked copy of the active profile's saves (no extra space on btrfs). Turning own saves off keeps
the folder for later; deleting a profile moves its saves to `trash/`. Save snapshots (`cpmm saves`)
record their profile, and restore only into it.

## First run

The GUI opens a setup flow on first launch (Settings → "Run setup again" brings it back); terminal
users get the same with `cpmm setup`:

1. **Game**: finds Cyberpunk 2077 in your Steam library, or asks where it is.
2. **Nexus Mods**: paste your personal API key; it is validated and stored with owner-only
   permissions (`cpmm nexus-key <KEY>`, `--check`, `--remove`).
3. **Steam**: puts `cpmm run --` in front of `%command%` in Cyberpunk's launch options, keeping your
   existing options (`cpmm launch-options [--apply]`). Applied live through Steam's client API when
   its debugger is on (Decky enables it), otherwise in `localconfig.vdf` while Steam is closed
   (backed up to `localconfig.vdf.cpmm-bak`).
4. **Preferences**: the main settings, and registering for Nexus "Mod Manager Download" links.

## FOMOD installers

Archives with a `fomod/ModuleConfig.xml` are installed through their installer: required files,
install steps (including steps that only appear for some choices), option groups of every type,
plugin types, condition flags, file conditions and conditional installs are all supported, and
paths are matched case-insensitively.

* **GUI**: installing opens a step-by-step chooser with the installer's previews and descriptions;
  mods installed this way get **Change installer options** in their drawer.
* **Terminal**: `cpmm install` asks group by group (Enter keeps the installer's suggestion).
  `--fomod-defaults` takes the suggestions; `--fomod-choices '{"Step/Group": ["Option"]}'` sets them
  (`cpmm api fomod <source>` lists the options as JSON).
* Your choices are saved with the mod and reused when it updates. Requirement installs, nxm links
  and downloads installed automatically use the installer's suggestions.

Several files from one Nexus page (a car and its tuning-parts installer) install as separate mods.

## Nexus Collections

```
cpmm collection show <slug | collection URL>      # contents, and what you already have
cpmm collection install <slug | URL> [--optional] [--into-current | --profile NAME]
cpmm collection list
```

A collection installs into a new profile named after it (your other profiles are untouched;
`--into-current` merges it into the active one). cpmm installs the curator's exact file versions in
the collection's phase order, applies the FOMOD options the curator chose, turns their "load after"
rules into cpmm rules and sorts. Mods you already have are reused (and set to the curator's version);
the replaced versions are kept for rollback. Optional mods are skipped unless `--optional`.
Downloading mods from the API needs a Nexus premium account; if a large collection runs into the
API's hourly limit, run the same command again to continue. Vortex-only binary patches and
non-Nexus sources are listed for manual steps. The GUI has the same on the Install page.

## Downloads

Like Vortex and MO2, cpmm keeps a **Downloads list** separate from the installed mods: every archive
it downloads (Mod Manager Download links, the built-in Nexus browser, `cpmm download`, installs)
stays in the download folder with a small record of which mod, file and version it is.

```
cpmm download <nexus id | url> [--file …]   # download only
cpmm downloads                               # the list, with installed / not installed / other version
cpmm downloads install <# | name> [--with-deps] [--fomod-choices … | --fomod-defaults]
cpmm downloads remove <# | name>             # deletes the archive; the installed mod stays
cpmm downloads add <path> [--nexus <id>] [--move]
```

Downloads only install themselves when **Settings → Downloads → Install downloads automatically**
is on (off by default). Installing a Nexus download reuses the archive on disk, keeps update
tracking exact, and opens the FOMOD chooser in the GUI when the mod has an installer.

## Tools

Some Nexus uploads are Windows programs that work *on* your mods rather than mods themselves
(VRAM RipperDoc, texture optimisers, save editors). cpmm runs them through Proton with your mods
mounted, and catches everything they write.

```
cpmm tools                                   # installed tools, their actions, the Wine that runs them
cpmm tools install <nexus id | url | download | path>
cpmm tools run <tool> [action] [--input …] [-- extra args]
cpmm tools runs                              # recent runs and what they wrote
cpmm tools save <run | last> [--name …]      # install a run's game files as a mod
cpmm tools discard <run>
cpmm tools remove <tool>
```

- **Installing:** a download that has a program and nothing under the game's mod folders is
  installed as a tool, not a mod (also from the Downloads list). Tools live in `tools/<id>/`, laid
  out as they'd sit in the game folder (`tools/<Name>/…`), and are never mounted in the game.
- **Running:** each run gets its own view of the game: the tool's files over the overwrite folder
  (read-only), your enabled mods and the game, with a scratch layer on top in
  `tool-runs/<run>/upper`. Nothing a tool writes reaches the game or the overwrite folder. Runs
  work while the game is running.
- **Wine:** the game's own Proton build (Steam's compatibility tool for Cyberpunk), with its
  `wine` run directly (`proton run` hides console output). Tools get their own prefix
  (`tool-prefix`) with the game's settings folder and the active profile's saves linked in (a
  profile with its own saves gets those, not the shared ones). An action that must see the
  running game uses the game's prefix. .NET programs run with
  `DOTNET_SYSTEM_GLOBALIZATION_USENLS=1`, since Wine has no ICU; nothing else is installed into
  the prefix.
- **Outputs:** reports (anything under `tools/`) open in the GUI, HTML in the built-in browser
  window with controller support. Game files wait until you **save the run as a mod**; disable
  or remove that mod to undo it. cpmm writes `modlist.txt`, so a tool's edit to it isn't applied:
  if the tool put its own archive first, the saved mod gets top priority and a load-order rule
  over every mod it overlaps. Deletions aren't applied either. The newest 20 runs are kept.
- **Saves:** the one thing a run changes for real, since that is a save editor's job. Before an
  action that can write saves (a save editor, or any program cpmm has no presets for), cpmm takes
  a save snapshot (unless **Settings → Safety → Snapshot saves before changes** is off). After every run it
  lists the save files that changed; `cpmm saves --restore <snapshot>` (or **Restore** on the run
  in the GUI) undoes them.
- **Presets:** cpmm knows these tools' actions:
  - [VRAM RipperDoc](https://www.nexusmods.com/cyberpunk2077/mods/36878) (texture optimiser):
    Scan my mods, Make budget fix, Measure VRAM in game. They run without questions.
  - [Save Editor (Project CyberCAT-SimpleGUI)](https://www.nexusmods.com/cyberpunk2077/mods/718)
    (save editor, `cpmm tools install 718`): Open the save editor, an interactive action. It
    opens the editor's window (a self-contained .NET program, nothing to install) on the active
    profile's saves, needs the game closed, and the run lasts until you close the window.

  Any other tool's `.exe`/`.bat` files run as they are, without arguments.

A tool runs as you, with access to your files (Wine is not a sandbox): only run tools you would
trust on Windows.

## Settings

Settings live in `~/.local/share/cpmm/settings.json` (every key has a default) and are shared by
the CLI and the GUI's **Settings** page:

```
cpmm settings                         # show all
cpmm settings set auto_sort off       # booleans take on/off
cpmm settings set staging_dir ~/Games/cpmm-mods   # moves your installed mods there
cpmm settings reset [key]
```

| Group | Keys |
|---|---|
| Updates | `check_updates_on_start`, `auto_install_updates` (never while the game runs; undoable), `refresh_minutes` |
| Load order | `auto_sort`: re-sort after installs, updates, enabling and rollbacks |
| After installing | `after_install_verify`, `after_install_doctor`, `after_install_command` (gets `CPMM_MOD_ID`, `CPMM_MOD_NAME`, `CPMM_MOD_VERSION`) |
| Downloads | `auto_install_downloads`: install a mod as soon as its download finishes (off: it waits in the Downloads list); `keep_archives` (default on): installed mods stay in the list |
| Storage | `staging_dir`, `download_dir` |
| Profiles | `profiles_enabled`, `new_profiles_own_saves` |
| Safety | `snapshot_saves`, `keep_versions`, `keep_save_snapshots` |
| Launching | `close_on_launch` |

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
