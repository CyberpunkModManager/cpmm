# cpmm

cpmm manages Cyberpunk 2077 mods: it installs them from Nexus Mods or local archives, keeps the mod setup consistent, and checks its health before the game launches.

## Mod setup

**Mod setup**:
The installed mods of the active profile, with their order, enabled state and load-order rules: what the game will load.
_Avoid_: mod list, config, state (when you mean this)

**Change**:
One mutation of the mod setup or of cpmm's records, carried out as a single unit of work.
_Avoid_: transaction, operation (when you mean a mutation)

**Setup change**:
A change that alters what the game loads, such as enabling, installing, ordering or removing a mod.

**Records change**:
A change that only touches cpmm's own bookkeeping, such as cached requirements, ignored requirements or acknowledged findings.

**Idle requirement**:
What must not be running while a change is applied: neither a cpmm game session nor the game, only one of them, or nothing.

**Update batch**:
The versions replaced by one update run, grouped so they can be undone together.
_Avoid_: batch tag (when talking to users)

## Requirements

**Requirement**:
Another Nexus mod that a mod needs, either hard or optional.
_Avoid_: dependency (when you mean the requirement rather than the mod that satisfies it)

**Dependency**:
The installed mod that satisfies a requirement.

**Requirement status**:
Where one requirement of a mod stands: ok, ignored, disabled, optional-disabled, missing or optional-missing.

**Ignored requirement**:
A requirement the user chose to do without for one mod; it only counts while its dependency is not installed and enabled.

**Unmet hard requirement**:
A requirement that is neither optional nor ignored, whose dependency is missing or disabled.
_Avoid_: dep problem, broken dependency

**Dependents**:
The enabled mods that list a mod as a hard requirement.
_Avoid_: reverse dependencies, required-by list

**Stale requirements**:
Stored requirements that were never checked, or were checked for a different installed version of the mod.

**Stored requirements**:
A mod's requirements as cpmm keeps them: the Nexus page's list merged with the file's own dependencies, each mod once, never the mod itself, external ones left out.

## Installing

**Install source**:
What an install starts from: a Nexus file, a local archive or folder, or an entry in the Downloads list.

**Install policy**:
How an install is carried out: what to do about missing requirements, where FOMOD choices come from, whether hooks run and the archive is kept.

**FOMOD fallback**:
The order FOMOD choices come from: explicit choices, then those saved on the mod, then asking in a terminal, then the installer's defaults.

**Install report**:
The result of an install: each finished mod with what happened to it (installed, updated, replaced, unchanged), its new conflicts and its requirement outcomes.

**Upstream requirement**:
A requirement on a mod the same install is already working on; it is skipped so requirement cycles end.

## Nexus

**Nexus catalogue**:
What cpmm asks of Nexus Mods: which file to install, what it requires, whether there is an update, the file's archive, and a collection revision.

**Update status**:
What the catalogue reports for one installed Nexus file: up to date, update available, archived by its author, page not published, file unknown, or no versions.

## Health

**Finding**:
One result of the health check, with a severity, an explanation and optional remedies.
_Avoid_: issue, warning (as a noun for findings in general)

**Remedy**:
A one-click fix attached to a finding.
_Avoid_: action, fix

**Acked finding**:
A finding the user acknowledged; it is still listed, in its place, marked as acknowledged.

**Overwrite kind**:
How a file in the overwrite folder relates to installed files: a deleted file, an edited mod file, an edited base-game file, or a new file.

**Shadowed mod file**:
A file in the overwrite folder that hides the copy shipped by the enabled mod that would otherwise win.
