import clsx from "clsx";
import { Check, Copy, FilePlus2, Pencil, Trash2, UserRound } from "lucide-react";
import { useState } from "react";
import type { AppState } from "../lib/cpmm";
import { useJobs } from "../lib/jobs";
import { Badge, Button, Confirm, Drawer, Input } from "../components/ui";

/** Sidebar button showing the active profile; opens the profile manager. */
export function ProfileButton({ state }: { state: AppState | undefined }) {
  const [open, setOpen] = useState(false);
  return (
    <>
      <button
        onClick={() => setOpen(true)}
        disabled={!state}
        className="mb-6 flex h-12 w-full items-center gap-3 rounded-xl border border-ink-700 bg-ink-850 px-3 text-left hover:border-neon-cyan/40 cursor-pointer disabled:opacity-50"
      >
        <UserRound className="size-4.5 shrink-0 text-neon-cyan" />
        <span className="min-w-0 flex-1">
          <span className="block text-[11px] leading-3 text-ink-400">Profile</span>
          <span className="block truncate text-sm">{state?.active_profile ?? "…"}</span>
        </span>
        {state && state.profiles.length > 1 && <span className="text-xs text-ink-400">{state.profiles.length}</span>}
      </button>
      {state && <ProfilesDrawer open={open} onClose={() => setOpen(false)} state={state} />}
    </>
  );
}

function ProfilesDrawer({ open, onClose, state }: { open: boolean; onClose: () => void; state: AppState }) {
  const { run, busy } = useJobs();
  const [name, setName] = useState("");
  const [renaming, setRenaming] = useState<string | null>(null);
  const [newName, setNewName] = useState("");
  const [deleting, setDeleting] = useState<string | null>(null);
  const locked = state.game_running || !!state.session;
  const taken = (n: string) => state.profiles.some((p) => p.name.toLowerCase() === n.trim().toLowerCase());
  const nameOk = name.trim().length > 0 && !taken(name);

  const create = async (empty: boolean) => {
    const args = ["profile", "create", name.trim(), ...(empty ? ["--empty"] : []), ...(locked ? [] : ["--switch"])];
    if (await run(`New profile ${name.trim()}`, args)) setName("");
  };

  return (
    <>
      <Drawer
        open={open}
        onClose={onClose}
        width="max-w-xl"
        title={
          <div>
            <div className="text-lg font-semibold">Profiles</div>
            <div className="text-sm text-ink-400">Separate mod lists: which mods are on, their order and load-order rules. Installed files are shared, so switching is instant.</div>
          </div>
        }
      >
        <div className="space-y-6 p-6">
          {locked && <div className="rounded-xl border border-neon-amber/40 bg-neon-amber/10 px-4 py-3 text-sm text-neon-amber">The game is running. Quit it to switch profiles.</div>}
          <div className="space-y-2">
            {state.profiles.map((p) => (
              <div key={p.name} className={clsx("flex flex-wrap items-center gap-3 rounded-xl border px-4 py-3", p.active ? "border-neon-cyan/50 bg-neon-cyan/8" : "border-ink-700 bg-ink-850")}>
                {renaming === p.name ? (
                  <form
                    className="flex min-w-0 flex-1 gap-2"
                    onSubmit={async (e) => {
                      e.preventDefault();
                      if (await run(`Rename profile ${p.name}`, ["profile", "rename", p.name, newName.trim()])) setRenaming(null);
                    }}
                  >
                    <Input autoFocus value={newName} onChange={(e) => setNewName(e.target.value)} className="flex-1" aria-label="New profile name" />
                    <Button type="submit" variant="primary" disabled={busy || !newName.trim() || (taken(newName) && newName.trim().toLowerCase() !== p.name.toLowerCase())}>
                      Save
                    </Button>
                    <Button type="button" variant="ghost" onClick={() => setRenaming(null)}>
                      Cancel
                    </Button>
                  </form>
                ) : (
                  <>
                    <div className="min-w-0 flex-1">
                      <div className="flex items-center gap-2">
                        <span className="truncate font-medium">{p.name}</span>
                        {p.active && <Badge tone="cyan">active</Badge>}
                      </div>
                      <div className="text-xs text-ink-400">
                        {p.enabled} of {state.mods.length} mods enabled
                      </div>
                    </div>
                    {!p.active && (
                      <Button size="sm" variant="primary" icon={<Check className="size-3.5" />} disabled={busy || locked} onClick={() => run(`Switch to ${p.name}`, ["profile", "switch", p.name])}>
                        Switch
                      </Button>
                    )}
                    <Button
                      size="sm"
                      variant="ghost"
                      icon={<Pencil className="size-3.5" />}
                      disabled={busy}
                      onClick={() => {
                        setRenaming(p.name);
                        setNewName(p.name);
                      }}
                    >
                      Rename
                    </Button>
                    {!p.active && (
                      <Button size="sm" variant="ghost" icon={<Trash2 className="size-3.5" />} disabled={busy} onClick={() => setDeleting(p.name)}>
                        Delete
                      </Button>
                    )}
                  </>
                )}
              </div>
            ))}
          </div>

          <div className="rounded-xl border border-ink-700 p-4">
            <div className="mb-3 text-sm font-semibold">New profile</div>
            <form
              className="space-y-3"
              onSubmit={(e) => {
                e.preventDefault();
                if (nameOk) create(false);
              }}
            >
              <Input placeholder="Name, e.g. Story run or Photo mode" value={name} onChange={(e) => setName(e.target.value)} aria-label="New profile name" />
              {name.trim() && taken(name) && <div className="text-xs text-neon-red">A profile with that name exists.</div>}
              <div className="flex flex-wrap gap-2">
                <Button type="submit" variant="primary" icon={<Copy className="size-4" />} disabled={busy || !nameOk}>
                  Copy of {state.active_profile}
                </Button>
                <Button type="button" icon={<FilePlus2 className="size-4" />} disabled={busy || !nameOk} onClick={() => create(true)}>
                  Empty (all mods off)
                </Button>
              </div>
              <div className="text-xs text-ink-400">
                {locked ? "It's created now; switch to it after quitting the game." : "cpmm switches to the new profile straight away."} Mods you install later are added to other profiles switched off.
              </div>
            </form>
          </div>
        </div>
      </Drawer>
      <Confirm
        open={!!deleting}
        title={`Delete profile ${deleting}?`}
        body="Only the list goes: every mod stays installed and keeps its place in your other profiles."
        confirmLabel="Delete profile"
        danger
        onCancel={() => setDeleting(null)}
        onConfirm={async () => {
          const n = deleting!;
          setDeleting(null);
          await run(`Delete profile ${n}`, ["profile", "delete", n]);
        }}
      />
    </>
  );
}
