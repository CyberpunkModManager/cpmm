import { useQuery } from "@tanstack/react-query";
import clsx from "clsx";
import {
  ArrowDown,
  ArrowUp,
  ChevronsDown,
  ChevronsUp,
  CircleAlert,
  Crown,
  ExternalLink,
  Eye,
  EyeOff,
  Layers,
  PackageSearch,
  Search,
  Swords,
  Trash2,
} from "lucide-react";
import { useMemo, useState } from "react";
import { Badge, Button, Confirm, Drawer, Empty, ErrorBox, IconButton, Input, PageHeader, Spinner, Switch } from "../components/ui";
import { type AppState, api, type ModRow, type Place, timeAgo } from "../lib/cpmm";
import { useJobs } from "../lib/jobs";
import { openLink } from "../lib/nexus";
import { FomodDialog } from "./Fomod";
import { ModHistory } from "./Safety";

type Filter = "all" | "enabled" | "disabled" | "problems" | "conflicts" | "manual";

export function ModsView({ state }: { state: AppState }) {
  const { run, busy } = useJobs();
  const [q, setQ] = useState("");
  const [filter, setFilter] = useState<Filter>("all");
  const [open, setOpen] = useState<string | null>(null);

  const mods = useMemo(() => {
    const lq = q.toLowerCase();
    return state.mods.filter((m) => {
      if (lq && !`${m.name} ${m.id} ${m.nexus?.mod_id ?? ""}`.toLowerCase().includes(lq)) return false;
      switch (filter) {
        case "enabled":
          return m.enabled;
        case "disabled":
          return !m.enabled;
        case "problems":
          return m.dep_problems > 0;
        case "conflicts":
          return m.wins + m.loses > 0;
        case "manual":
          return !m.nexus;
      }
      return true;
    });
  }, [state.mods, q, filter]);

  const counts: Record<Filter, number> = {
    all: state.mods.length,
    enabled: state.mods.filter((m) => m.enabled).length,
    disabled: state.mods.filter((m) => !m.enabled).length,
    problems: state.mods.filter((m) => m.dep_problems > 0).length,
    conflicts: state.mods.filter((m) => m.wins + m.loses > 0).length,
    manual: state.mods.filter((m) => !m.nexus).length,
  };

  const order = (m: ModRow, how: "up" | "down" | "top" | "bottom") => {
    const i = state.mods.findIndex((x) => x.id === m.id);
    const place: Place =
      how === "top" ? "top" : how === "bottom" ? "bottom" : how === "up" ? { before: state.mods[i - 1]?.id ?? m.id } : { after: state.mods[i + 1]?.id ?? m.id };
    run(`Reorder ${m.name}`, { op: "order", mod: m.id, place });
  };

  const current = state.mods.find((m) => m.id === open) ?? null;

  return (
    <>
      <PageHeader
        title="Mods"
        subtitle={
          <>
            Load order: <span className="text-ink-300">mods further down win file conflicts</span>. Changes apply on the next launch.
          </>
        }
      />
      <div className="mb-4 flex flex-wrap items-center gap-3">
        <Input
          icon={<Search className="size-4" />}
          placeholder="Search name, id or Nexus id…"
          value={q}
          onChange={(e) => setQ(e.target.value)}
          className="w-80"
        />
        <div className="flex flex-wrap gap-1.5">
          {(Object.keys(counts) as Filter[]).map((f) => (
            <button
              type="button"
              key={f}
              onClick={() => setFilter(f)}
              className={clsx(
                "h-9 rounded-lg border px-3 text-sm capitalize cursor-pointer",
                filter === f ? "border-neon-cyan/50 bg-neon-cyan/12 text-neon-cyan" : "border-ink-700 text-ink-300 hover:bg-ink-800",
              )}
            >
              {f} <span className="ml-1 text-ink-400 tabular-nums">{counts[f]}</span>
            </button>
          ))}
        </div>
      </div>

      {mods.length === 0 ? (
        <Empty icon={<PackageSearch className="size-6" />} title="No mods match">
          Try another filter, or install something from the Install tab.
        </Empty>
      ) : (
        <div className="glass overflow-hidden rounded-2xl">
          {mods.map((m, idx) => (
            <div
              key={m.id}
              tabIndex={0}
              role="button"
              onClick={() => setOpen(m.id)}
              onKeyDown={(e) => e.key === "Enter" && e.target === e.currentTarget && setOpen(m.id)}
              className={clsx(
                "group flex cursor-pointer items-center gap-4 px-4 py-3 hover:bg-ink-800/60",
                idx > 0 && "border-t border-ink-800",
                !m.enabled && "opacity-55",
              )}
            >
              <div className="w-8 text-right font-mono text-xs text-ink-400 tabular-nums">{m.priority}</div>
              <Switch
                checked={m.enabled}
                label={m.enabled ? `Disable ${m.name}` : `Enable ${m.name}`}
                disabled={busy}
                onChange={(v) => run(`${v ? "Enable" : "Disable"} ${m.name}`, { op: v ? "enable" : "disable", mods: [m.id] })}
              />
              <div className="min-w-0 flex-1">
                <div className="truncate font-medium">{m.name}</div>
                <div className="truncate font-mono text-xs text-ink-400">
                  {m.id}
                  {m.nexus && <span className="ml-2">#{m.nexus.mod_id}</span>}
                </div>
              </div>
              <div className="hidden items-center gap-1.5 md:flex">
                {m.dep_problems > 0 && (
                  <Badge tone="red" title="Missing or disabled requirements">
                    <CircleAlert className="size-3" /> {m.dep_problems} req
                  </Badge>
                )}
                {m.wins > 0 && (
                  <Badge tone="lime" title="Files this mod wins">
                    <Crown className="size-3" /> {m.wins}
                  </Badge>
                )}
                {m.loses > 0 && (
                  <Badge tone="amber" title="Files overridden by later mods">
                    <Swords className="size-3" /> {m.loses}
                  </Badge>
                )}
                {m.hidden > 0 && (
                  <Badge tone="violet" title="Hidden files">
                    <EyeOff className="size-3" /> {m.hidden}
                  </Badge>
                )}
                {!m.nexus && <Badge>manual</Badge>}
              </div>
              <div className="w-24 truncate text-right font-mono text-sm text-ink-300">{m.version || "—"}</div>
              <div className="flex opacity-60 group-hover:opacity-100 group-focus-within:opacity-100" onClick={(e) => e.stopPropagation()}>
                <IconButton label="Move up (loses more)" disabled={busy || m.priority === 1} onClick={() => order(m, "up")}>
                  <ArrowUp className="size-4" />
                </IconButton>
                <IconButton label="Move down (wins more)" disabled={busy || m.priority === state.mods.length} onClick={() => order(m, "down")}>
                  <ArrowDown className="size-4" />
                </IconButton>
              </div>
            </div>
          ))}
        </div>
      )}

      <ModDrawer mod={current} state={state} onClose={() => setOpen(null)} onOrder={order} />
    </>
  );
}

function ModDrawer({
  mod,
  state,
  onClose,
  onOrder,
}: {
  mod: ModRow | null;
  state: AppState;
  onClose: () => void;
  onOrder: (m: ModRow, how: "up" | "down" | "top" | "bottom") => void;
}) {
  const { run, busy } = useJobs();
  const [confirm, setConfirm] = useState(false);
  const [reconf, setReconf] = useState(false);
  const info = useQuery({ queryKey: ["info", mod?.id], queryFn: () => api.info(mod!.id), enabled: !!mod });
  const locked = state.game_running || !!state.session;

  if (!mod) return null;
  const conflicting = info.data?.files.filter((f) => f.others.length > 0) ?? [];

  return (
    <Drawer
      open
      onClose={onClose}
      title={
        <div>
          <div className="truncate text-lg font-semibold">{mod.name}</div>
          <div className="font-mono text-xs text-ink-400">
            {mod.id} · priority {mod.priority} · {mod.version || "no version"} {mod.installed_at && `· installed ${timeAgo(mod.installed_at)}`}
          </div>
        </div>
      }
    >
      <div className="flex flex-wrap gap-2">
        {mod.nexus && (
          <Button icon={<ExternalLink className="size-4" />} onClick={() => openLink(mod.nexus!.url)}>
            Nexus page
          </Button>
        )}
        {mod.nexus && (
          <Button
            disabled={busy || locked}
            onClick={() => run(`Update ${mod.name}`, { op: "update", mods: [mod.id], all: false })}
            title={locked ? "Quit the game first" : undefined}
          >
            Update
          </Button>
        )}
        {conflicting.length > 0 && (
          <Button
            icon={<Crown className="size-4" />}
            disabled={busy || locked}
            onClick={() => run(`Prefer ${mod.name}`, { op: "prefer", mod: mod.id, paths: [] })}
          >
            Win all conflicts
          </Button>
        )}
        <Button icon={<ChevronsUp className="size-4" />} variant="ghost" disabled={busy} onClick={() => onOrder(mod, "bottom")}>
          First
        </Button>
        <Button icon={<ChevronsDown className="size-4" />} variant="ghost" disabled={busy} onClick={() => onOrder(mod, "top")}>
          Last (wins)
        </Button>
        {mod.fomod && mod.nexus && (
          <Button disabled={busy || locked} onClick={() => setReconf(true)}>
            Change installer options
          </Button>
        )}
        <Button variant="danger" icon={<Trash2 className="size-4" />} disabled={busy || locked} onClick={() => setConfirm(true)}>
          Uninstall
        </Button>
      </div>
      {locked && <p className="mt-3 text-xs text-neon-amber">The game is running: file changes (update, hide, uninstall) are disabled until you quit.</p>}

      <h3 className="mt-6 mb-2 text-sm font-semibold uppercase tracking-wider text-ink-400">Requirements</h3>
      {mod.requires.length === 0 ? (
        <p className="text-sm text-ink-400">None listed on Nexus.</p>
      ) : (
        <div className="space-y-2">
          {mod.requires.map((r) => (
            <div key={r.mod_id} className="flex items-center gap-3 rounded-xl border border-ink-700 bg-ink-850 px-3 py-2">
              <ReqBadge status={r.status} />
              <div className="min-w-0 flex-1">
                <div className="truncate text-sm">
                  {r.name} <span className="font-mono text-xs text-ink-400">#{r.mod_id}</span>
                </div>
                {r.notes && <div className="truncate text-xs text-ink-400">{r.notes}</div>}
              </div>
              {(r.status === "missing" || r.status === "optional-missing") && (
                <Button
                  size="sm"
                  variant={r.status === "missing" ? "primary" : "secondary"}
                  disabled={busy}
                  onClick={() => run(`Install ${r.name}`, { op: "install", source: String(r.mod_id), missing: "install" })}
                >
                  Install
                </Button>
              )}
              {(r.status === "disabled" || r.status === "optional-disabled") && r.installed_as && (
                <Button size="sm" disabled={busy} onClick={() => run(`Enable ${r.name}`, { op: "enable", mods: [r.installed_as!] })}>
                  Enable
                </Button>
              )}
              {r.status === "missing" && (
                <Button
                  size="sm"
                  variant="ghost"
                  disabled={busy}
                  onClick={() => run(`Ignore ${r.name}`, { op: "deps", refresh: false, fix: false, ignore: [mod.id, String(r.mod_id)], unignore: null })}
                >
                  Ignore
                </Button>
              )}
              {r.status === "ignored" && (
                <Button
                  size="sm"
                  variant="ghost"
                  disabled={busy}
                  onClick={() => run(`Stop ignoring ${r.name}`, { op: "deps", refresh: false, fix: false, ignore: null, unignore: [mod.id, String(r.mod_id)] })}
                >
                  Unignore
                </Button>
              )}
            </div>
          ))}
        </div>
      )}

      <h3 className="mt-6 mb-2 text-sm font-semibold uppercase tracking-wider text-ink-400">Earlier versions</h3>
      <ModHistory id={mod.id} locked={locked} />

      <h3 className="mt-6 mb-2 flex items-center gap-2 text-sm font-semibold uppercase tracking-wider text-ink-400">
        <Layers className="size-4" /> Files {info.data && <span className="normal-case tracking-normal">({info.data.files.length})</span>}
      </h3>
      {info.isLoading && <Spinner />}
      {info.error && <ErrorBox error={info.error} />}
      {info.data && (
        <div className="overflow-hidden rounded-xl border border-ink-700">
          {info.data.files.map((f, i) => (
            <div key={f.path} className={clsx("flex items-center gap-3 px-3 py-2 text-sm", i > 0 && "border-t border-ink-800")}>
              <div className="min-w-0 flex-1">
                <div className="truncate font-mono text-xs">{f.path}</div>
                {f.others.length > 0 && (
                  <div className="truncate text-xs text-ink-400">
                    also in {f.others.join(", ")} ·{" "}
                    {f.wins ? <span className="text-neon-lime">wins</span> : <span className="text-neon-amber">loses to {f.winner || "?"}</span>}
                  </div>
                )}
              </div>
              <IconButton label="Hide this file" disabled={busy || locked} onClick={() => run(`Hide ${f.path}`, { op: "hide", mod: mod.id, paths: [f.path] })}>
                <EyeOff className="size-4" />
              </IconButton>
            </div>
          ))}
          {info.data.hidden.map((h) => (
            <div key={`h-${h}`} className="flex items-center gap-3 border-t border-ink-800 bg-neon-violet/5 px-3 py-2 text-sm">
              <EyeOff className="size-4 text-neon-violet" />
              <div className="min-w-0 flex-1 truncate font-mono text-xs text-ink-400 line-through">{h}</div>
              <IconButton label="Unhide" disabled={busy || locked} onClick={() => run(`Unhide ${h}`, { op: "unhide", mod: mod.id, paths: [h] })}>
                <Eye className="size-4" />
              </IconButton>
            </div>
          ))}
        </div>
      )}

      {reconf && mod.nexus && (
        <FomodDialog
          source={String(mod.nexus.mod_id)}
          file={mod.nexus.version_gsid || null}
          title={mod.name}
          initial={mod.fomod}
          onCancel={() => setReconf(false)}
          onInstall={(choices) => {
            setReconf(false);
            const n = mod.nexus!;
            run(`Reinstall ${mod.name}`, {
              op: "install",
              source: String(n.mod_id),
              id: mod.id,
              file: n.version_gsid || null,
              missing: "warn",
              fomod: choices,
            });
          }}
        />
      )}
      <Confirm
        open={confirm}
        title={`Uninstall ${mod.name}?`}
        body={<>Its files move to cpmm's trash (Settings → Clean frees the space). Mods that require it will be flagged.</>}
        confirmLabel="Uninstall"
        danger
        onCancel={() => setConfirm(false)}
        onConfirm={async () => {
          setConfirm(false);
          if (await run(`Uninstall ${mod.name}`, { op: "remove", mod: mod.id, force: true })) onClose();
        }}
      />
    </Drawer>
  );
}

export function ReqBadge({ status }: { status: ModRow["requires"][number]["status"] }) {
  const map = {
    ok: ["lime", "installed"],
    disabled: ["amber", "disabled"],
    "optional-disabled": ["violet", "disabled"],
    ignored: ["muted", "ignored"],
    "optional-missing": ["violet", "optional"],
    missing: ["red", "missing"],
  } as const;
  const [tone, text] = map[status];
  return (
    <Badge tone={tone} className="w-20 justify-center">
      {text}
    </Badge>
  );
}
