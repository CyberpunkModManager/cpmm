import { useQuery } from "@tanstack/react-query";
import clsx from "clsx";
import {
  ArrowDownToLine,
  BookmarkCheck,
  CheckCircle2,
  CircleAlert,
  Crown,
  Download,
  ExternalLink,
  FileCode2,
  Files,
  Gamepad2,
  Globe,
  Link2,
  Package,
  RefreshCw,
  Search,
  Sparkles,
  Trash2,
  Undo2,
  Wand2,
} from "lucide-react";
import { useState } from "react";
import { Badge, Button, Card, Confirm, Empty, ErrorBox, Input, PageHeader, Spinner, Stat } from "../components/ui";
import { type AppState, api, api2, api3, ownerLabel, timeAgo } from "../lib/cpmm";
import { useJobs } from "../lib/jobs";
import { openLink, openNexus } from "../lib/nexus";
import { ReqBadge } from "./Mods";

const locked = (s: AppState) => s.game_running || !!s.session;

/* ------------------------------------------------------------------ Conflicts */

export function ConflictsView({ state }: { state: AppState }) {
  const { run, busy } = useJobs();
  const [showSame, setShowSame] = useState(false);
  const q = useQuery({ queryKey: ["conflicts"], queryFn: api.conflicts });
  const arch = useQuery({ queryKey: ["archives"], queryFn: api2.archives });
  const name = (id: string) => state.mods.find((m) => m.id === id)?.name ?? id;

  return (
    <>
      <PageHeader
        title="Conflicts"
        subtitle="Files shipped by more than one enabled mod, and redscript methods replaced by more than one mod."
        actions={
          <Button variant="ghost" onClick={() => setShowSame((v) => !v)}>
            {showSame ? "Hide identical copies" : "Show identical copies"}
          </Button>
        }
      />
      {q.isLoading && <Spinner label="Hashing conflicting files…" />}
      {q.error && <ErrorBox error={q.error} />}
      {q.data && (
        <div className="space-y-8">
          <section>
            <h2 className="mb-3 flex items-center gap-2 font-semibold">
              <Files className="size-4 text-neon-cyan" /> File overwrites
            </h2>
            {q.data.files.filter((f) => showSame || !f.identical).length === 0 ? (
              <Card className="px-5 py-4 text-sm text-ink-400">
                No differing file conflicts{q.data.files.length ? ` (${q.data.files.length} identical copies hidden)` : ""}.
              </Card>
            ) : (
              <div className="glass overflow-hidden rounded-2xl">
                {q.data.files
                  .filter((f) => showSame || !f.identical)
                  .map((f, i) => (
                    <div key={f.path} className={clsx("px-4 py-3", i > 0 && "border-t border-ink-800")}>
                      <div className="flex items-center gap-2">
                        <span className="min-w-0 flex-1 truncate font-mono text-sm">{f.path}</span>
                        {f.identical && <Badge>identical</Badge>}
                        {f.overrides_base && <Badge tone="violet">overrides base</Badge>}
                      </div>
                      <div className="mt-2 flex flex-wrap items-center gap-2">
                        {f.mods.map((m) => (
                          <button
                            type="button"
                            key={m}
                            disabled={busy || locked(state) || m === f.winner}
                            onClick={() => run(`Prefer ${name(m)} for ${f.path}`, ["prefer", m, f.path])}
                            title={m === f.winner ? "Currently wins" : `Make ${name(m)} win this file`}
                            className={clsx(
                              "inline-flex h-8 items-center gap-1.5 rounded-lg border px-3 text-xs cursor-pointer disabled:cursor-default",
                              m === f.winner ? "border-neon-lime/40 bg-neon-lime/10 text-neon-lime" : "border-ink-700 text-ink-300 hover:border-neon-cyan/50",
                            )}
                          >
                            {m === f.winner && <Crown className="size-3" />} {name(m)}
                          </button>
                        ))}
                      </div>
                    </div>
                  ))}
              </div>
            )}
            <p className="mt-2 text-xs text-ink-400">
              Pick a mod to make it win that file; the other copies are hidden (undo from the mod's file list). {q.data.base_overrides} files override base-game
              files.
            </p>
          </section>
          <section>
            <h2 className="mb-3 flex items-center gap-2 font-semibold">
              <Package className="size-4 text-neon-pink" /> Archive resources
            </h2>
            {arch.isLoading && <Spinner label="Reading archive indexes…" />}
            {arch.error && <ErrorBox error={arch.error} />}
            {arch.data &&
              (arch.data.filter((p) => showSame || p.identical < p.shared).length === 0 ? (
                <Card className="px-5 py-4 text-sm text-ink-400">No two mods replace the same game assets differently.</Card>
              ) : (
                <div className="glass overflow-hidden rounded-2xl">
                  {arch.data
                    .filter((p) => showSame || p.identical < p.shared)
                    .map((p, i) => {
                      const both = p.a.kind === "mod" && p.b.kind === "mod";
                      return (
                        <div key={p.archive_a + p.archive_b} className={clsx("px-4 py-3", i > 0 && "border-t border-ink-800")}>
                          <div className="flex flex-wrap items-center gap-2 text-sm">
                            <span className="font-mono text-xs text-ink-300">{p.archive_a}</span>
                            <span className="text-ink-400">vs</span>
                            <span className="font-mono text-xs text-ink-300">{p.archive_b}</span>
                            <Badge tone={p.shared > p.identical ? "amber" : "muted"}>{p.shared - p.identical} differing</Badge>
                            {p.identical > 0 && <Badge>{p.identical} identical</Badge>}
                            {p.related && <Badge tone="violet">related</Badge>}
                          </div>
                          <div className="mt-2 flex flex-wrap items-center gap-2">
                            {[p.a, p.b].map((o, k) => {
                              const other = k === 0 ? p.b : p.a;
                              const wins = o.kind === "mod" && p.winner === o.id;
                              return (
                                <button
                                  type="button"
                                  key={k}
                                  disabled={!both || busy || wins}
                                  onClick={() => run(`${name(o.id)} wins`, ["rule", o.id, "--after", other.id])}
                                  title={
                                    wins
                                      ? "Currently wins"
                                      : both
                                        ? `Make ${name(o.id)} win these resources`
                                        : "mod.io/unmanaged order isn't controlled by cpmm"
                                  }
                                  className={clsx(
                                    "inline-flex h-8 items-center gap-1.5 rounded-lg border px-3 text-xs cursor-pointer disabled:cursor-default",
                                    wins ? "border-neon-lime/40 bg-neon-lime/10 text-neon-lime" : "border-ink-700 text-ink-300 hover:border-neon-cyan/50",
                                  )}
                                >
                                  {wins && <Crown className="size-3" />} {o.kind === "mod" ? name(o.id) : ownerLabel(o)}
                                </button>
                              );
                            })}
                          </div>
                        </div>
                      );
                    })}
                </div>
              ))}
            <p className="mt-2 text-xs text-ink-400">
              Tap the mod that should win. cpmm saves it as a rule, re-sorts, and rewrites modlist.txt for the next launch.
            </p>
          </section>
          <section>
            <h2 className="mb-3 flex items-center gap-2 font-semibold">
              <FileCode2 className="size-4 text-neon-violet" /> Redscript
            </h2>
            {q.data.scripts.length === 0 ? (
              <Card className="px-5 py-4 text-sm text-ink-400">No annotation conflicts.</Card>
            ) : (
              <div className="glass overflow-hidden rounded-2xl">
                {q.data.scripts.map((c, i) => (
                  <div key={c.kind + c.target} className={clsx("flex flex-wrap items-center gap-3 px-4 py-3", i > 0 && "border-t border-ink-800")}>
                    <Badge tone={c.severe ? "red" : "muted"} className="w-44 justify-center">
                      {c.kind}
                    </Badge>
                    <span className="font-mono text-sm">{c.target}</span>
                    <span className="text-xs text-ink-400">{c.mods.join(" · ")}</span>
                    {c.severe && (
                      <div className="flex w-full flex-wrap gap-2 pl-[11.75rem]">
                        {c.mods
                          .map((m) => m.replace(/^(replace|wrap):/, ""))
                          .filter((m) => state.mods.some((x) => x.id === m && x.enabled))
                          .map((m) => (
                            <Button key={m} size="sm" variant="ghost" disabled={busy} onClick={() => run(`Disable ${name(m)}`, ["disable", m])}>
                              Keep the other, disable {name(m)}
                            </Button>
                          ))}
                      </div>
                    )}
                  </div>
                ))}
              </div>
            )}
          </section>
        </div>
      )}
    </>
  );
}

/* ---------------------------------------------------------------- Dependencies */

export function DepsView({ state }: { state: AppState }) {
  const { run, busy } = useJobs();
  const rows = state.mods.filter((m) => m.enabled).flatMap((m) => m.requires.filter((r) => r.status !== "ok").map((r) => ({ m, r })));
  const hard = rows.filter(({ r }) => r.status === "missing" || r.status === "disabled");
  return (
    <>
      <PageHeader
        title="Dependencies"
        subtitle="Requirements from each mod's Nexus page plus file-level dependencies. Checked when a mod is installed or updated and cached after that; re-check to pick up changes authors made since."
        actions={
          <>
            <Button icon={<RefreshCw className="size-4" />} disabled={busy} onClick={() => run("Re-check requirements", ["deps", "--refresh"])}>
              Re-check on Nexus
            </Button>
            <Button
              variant="primary"
              icon={<Wand2 className="size-4" />}
              disabled={busy || hard.length === 0}
              onClick={() => run("Fix requirements", ["deps", "--fix"])}
            >
              Fix all ({hard.length})
            </Button>
          </>
        }
      />
      <div className="mb-6 grid grid-cols-1 gap-3 sm:grid-cols-3">
        <Stat
          label="Unmet hard"
          value={hard.length}
          tone={hard.length ? "red" : "lime"}
          icon={hard.length ? <CircleAlert className="size-5" /> : <CheckCircle2 className="size-5" />}
        />
        <Stat
          label="Optional missing"
          value={rows.filter(({ r }) => r.status === "optional-missing").length}
          tone="violet"
          icon={<Sparkles className="size-5" />}
        />
        <Stat
          label="Ignored by choice"
          value={rows.filter(({ r }) => r.status === "ignored").length}
          tone="muted"
          icon={<BookmarkCheck className="size-5" />}
        />
      </div>
      {rows.length === 0 ? (
        <Empty icon={<CheckCircle2 className="size-6" />} title="Every requirement is met" />
      ) : (
        <div className="glass overflow-hidden rounded-2xl">
          {rows.map(({ m, r }, i) => (
            <div key={m.id + r.mod_id} className={clsx("flex flex-wrap items-center gap-3 px-4 py-3", i > 0 && "border-t border-ink-800")}>
              <ReqBadge status={r.status} />
              <div className="min-w-0 flex-1">
                <div className="truncate text-sm">
                  <span className="text-ink-400">{m.name} needs</span> {r.name} <span className="font-mono text-xs text-ink-400">#{r.mod_id}</span>
                </div>
                {r.notes && <div className="truncate text-xs text-ink-400">{r.notes}</div>}
              </div>
              <Button size="sm" variant="ghost" icon={<ExternalLink className="size-3.5" />} onClick={() => openLink(r.url)}>
                Nexus
              </Button>
              {(r.status === "missing" || r.status === "optional-missing") && (
                <Button
                  size="sm"
                  variant={r.status === "missing" ? "primary" : "secondary"}
                  disabled={busy}
                  onClick={() => run(`Install ${r.name}`, ["install", String(r.mod_id), "--with-deps"])}
                >
                  Install
                </Button>
              )}
              {r.status === "disabled" && r.installed_as && (
                <Button size="sm" disabled={busy} onClick={() => run(`Enable ${r.name}`, ["enable", r.installed_as!])}>
                  Enable
                </Button>
              )}
              {r.status === "missing" && (
                <Button size="sm" variant="ghost" disabled={busy} onClick={() => run(`Ignore ${r.name}`, ["deps", "--ignore", m.id, String(r.mod_id)])}>
                  Ignore
                </Button>
              )}
              {r.status === "ignored" && (
                <Button size="sm" variant="ghost" disabled={busy} onClick={() => run(`Unignore ${r.name}`, ["deps", "--unignore", m.id, String(r.mod_id)])}>
                  Unignore
                </Button>
              )}
            </div>
          ))}
        </div>
      )}
    </>
  );
}

/* --------------------------------------------------------------------- Updates */

export function UpdatesView({ state }: { state: AppState }) {
  const { run, busy } = useJobs();
  const [refresh, setRefresh] = useState(false);
  const safety = useQuery({ queryKey: ["safety"], queryFn: api3.safety, staleTime: 30_000 });
  const lastBatch = safety.data?.batches[0];
  const q = useQuery({ queryKey: ["updates", refresh], queryFn: () => api.updates(refresh), staleTime: 5 * 60_000 });
  const name = (id: string) => state.mods.find((m) => m.id === id)?.name ?? id;
  const avail = q.data?.filter((u) => u.update) ?? [];
  const other = q.data?.filter((u) => !u.update && u.status !== "up to date") ?? [];
  return (
    <>
      <PageHeader
        title="Updates"
        subtitle="Newer uploads of the exact Nexus file each mod was installed from."
        actions={
          <>
            <Button
              icon={<RefreshCw className={clsx("size-4", q.isFetching && "animate-spin")} />}
              disabled={q.isFetching}
              onClick={() => (refresh ? q.refetch() : setRefresh(true))}
            >
              Check now
            </Button>
            {lastBatch && (
              <Button
                icon={<Undo2 className="size-4" />}
                disabled={busy || locked(state)}
                onClick={() => run("Undo last update run", ["undo-update", lastBatch.batch])}
                title={lastBatch.mods.map(([id, v]) => `${id} → ${v}`).join(", ")}
              >
                Undo last update ({lastBatch.mods.length})
              </Button>
            )}
            <Button
              variant="primary"
              icon={<ArrowDownToLine className="size-4" />}
              disabled={busy || locked(state) || avail.length === 0}
              onClick={() => run("Update all", ["update", "--all"])}
            >
              Update all ({avail.length})
            </Button>
          </>
        }
      />
      {locked(state) && <p className="-mt-3 mb-4 text-xs text-neon-amber">Quit the game to install updates.</p>}
      {q.isLoading && <Spinner label="Asking Nexus…" />}
      {q.error && <ErrorBox error={q.error} />}
      {q.data && (
        <>
          <div className="mb-6 grid grid-cols-1 gap-3 sm:grid-cols-3">
            <Stat label="Updates" value={avail.length} tone={avail.length ? "cyan" : "lime"} icon={<Download className="size-5" />} />
            <Stat label="Up to date" value={q.data.filter((u) => u.status === "up to date").length} tone="lime" icon={<CheckCircle2 className="size-5" />} />
            <Stat label="Needs attention" value={other.length} tone={other.length ? "amber" : "muted"} icon={<CircleAlert className="size-5" />} />
          </div>
          {avail.length + other.length === 0 ? (
            <Empty icon={<CheckCircle2 className="size-6" />} title="Everything is up to date" />
          ) : (
            <div className="glass overflow-hidden rounded-2xl">
              {[...avail, ...other].map((u, i) => (
                <div key={u.id} className={clsx("flex items-center gap-3 px-4 py-3", i > 0 && "border-t border-ink-800")}>
                  <div className="min-w-0 flex-1">
                    <div className="truncate font-medium">{name(u.id)}</div>
                    <div className="truncate text-xs text-ink-400">
                      {u.update ? `${u.status.replace(/^UPDATE /, "")} · uploaded ${timeAgo(u.update.uploaded_at)}` : u.status}
                    </div>
                  </div>
                  {u.update ? (
                    <Button size="sm" variant="primary" disabled={busy || locked(state)} onClick={() => run(`Update ${name(u.id)}`, ["update", u.id])}>
                      Update
                    </Button>
                  ) : (
                    <Badge tone="amber">check</Badge>
                  )}
                </div>
              ))}
            </div>
          )}
        </>
      )}
    </>
  );
}

/* ------------------------------------------------------------------- Overwrite */

export function OverwriteView({ state }: { state: AppState }) {
  const { run, busy } = useJobs();
  const q = useQuery({ queryKey: ["overwrite"], queryFn: api.overwrite });
  const [target, setTarget] = useState("");
  const [confirm, setConfirm] = useState(false);
  const tone = { deleted: "red", "edited mod file": "amber", "edited base file": "violet", new: "cyan" } as const;
  return (
    <>
      <PageHeader
        title="Overwrite"
        subtitle="Everything the game and its tools wrote while running: logs, settings, caches. It sits on top of all mods."
        actions={
          <Button variant="danger" icon={<Trash2 className="size-4" />} disabled={busy || locked(state) || !q.data?.length} onClick={() => setConfirm(true)}>
            Clear all
          </Button>
        }
      />
      <Card className="mb-6 flex flex-wrap items-center gap-3 px-5 py-4">
        <div className="min-w-56 flex-1 text-sm text-ink-300">
          Keep these files as a mod (e.g. tuned settings), so they survive clearing and can be toggled:
        </div>
        <Input placeholder="Mod name, new or existing" value={target} onChange={(e) => setTarget(e.target.value)} className="w-72" />
        <Button
          variant="primary"
          disabled={busy || locked(state) || !target.trim()}
          onClick={() => run(`Move overwrite to ${target}`, ["overwrite", "move", target.trim()])}
        >
          Move all into mod
        </Button>
      </Card>
      {locked(state) && <p className="-mt-3 mb-4 text-xs text-neon-amber">Quit the game to move or clear these files.</p>}
      {q.isLoading && <Spinner />}
      {q.error && <ErrorBox error={q.error} />}
      {q.data &&
        (q.data.length === 0 ? (
          <Empty icon={<Sparkles className="size-6" />} title="Overwrite is empty" />
        ) : (
          <div className="glass overflow-hidden rounded-2xl">
            {q.data.map((o, i) => (
              <div key={o.path} className={clsx("flex items-center gap-3 px-4 py-2.5", i > 0 && "border-t border-ink-800")}>
                <Badge tone={tone[o.kind]} className="w-32 justify-center">
                  {o.kind}
                </Badge>
                <span className="min-w-0 flex-1 truncate font-mono text-xs">{o.path}</span>
                {o.shadows && <span className="text-xs text-ink-400">over {o.shadows}</span>}
              </div>
            ))}
          </div>
        ))}
      <Confirm
        open={confirm}
        title="Clear the overwrite layer?"
        body="All runtime files (and deletion markers) move to cpmm's trash. Mods' own settings files reappear as shipped."
        confirmLabel="Clear"
        danger
        onCancel={() => setConfirm(false)}
        onConfirm={() => {
          setConfirm(false);
          run("Clear overwrite", ["overwrite", "clear"]);
        }}
      />
    </>
  );
}

/* --------------------------------------------------------------------- Install */

export function InstallView({ state }: { state: AppState }) {
  const { run, busy } = useJobs();
  const [src, setSrc] = useState("");
  const [lookupId, setLookupId] = useState<string | null>(null);
  const [file, setFile] = useState<string | null>(null);
  const look = useQuery({ queryKey: ["lookup", lookupId], queryFn: () => api.lookup(lookupId!), enabled: !!lookupId });
  const tracked = useQuery({ queryKey: ["tracked"], queryFn: api.tracked });
  const installedIds = new Set(state.mods.map((m) => m.nexus?.mod_id));
  const isLocal = src.startsWith("/") || src.startsWith("~");

  return (
    <>
      <PageHeader title="Install" subtitle="From a Nexus mod page, a mod id, or a local archive. Requirements are installed with it." />
      <Card className="mb-6 flex flex-wrap items-center gap-4 p-5">
        <Globe className="size-8 shrink-0 text-neon-cyan" />
        <div className="min-w-60 flex-1">
          <div className="font-semibold">Browse Nexus Mods</div>
          <div className="text-sm text-ink-400">
            Nexus in a cpmm window. <em>Mod Manager Download</em> and manual downloads install straight away, with requirements. Works in Game Mode, where
            Steam's browser ignores these links.
          </div>
        </div>
        <Button variant="primary" icon={<Globe className="size-4" />} onClick={() => openNexus().catch((e) => alert(String(e)))}>
          Open browser
        </Button>
      </Card>
      <Card className="mb-6 p-5">
        <form
          className="flex flex-wrap gap-3"
          onSubmit={(e) => {
            e.preventDefault();
            if (!src.trim()) return;
            if (isLocal) run(`Install ${src}`, ["install", src.trim()]);
            else {
              setFile(null);
              setLookupId(src.trim());
            }
          }}
        >
          <Input
            icon={<Link2 className="size-4" />}
            placeholder="https://www.nexusmods.com/cyberpunk2077/mods/… or 107 or ~/Downloads/mod.zip"
            value={src}
            onChange={(e) => setSrc(e.target.value)}
            className="min-w-80 flex-1"
          />
          <Button
            type="submit"
            variant="primary"
            icon={isLocal ? <Download className="size-4" /> : <Search className="size-4" />}
            disabled={busy || !src.trim()}
          >
            {isLocal ? "Install archive" : "Look up"}
          </Button>
        </form>

        {look.isFetching && <Spinner label="Fetching mod details…" />}
        {look.error && (
          <div className="mt-4">
            <ErrorBox error={look.error} />
          </div>
        )}
        {look.data && !look.isFetching && (
          <div className="mt-5 border-t border-ink-700 pt-5">
            <div className="flex flex-wrap items-start justify-between gap-3">
              <div>
                <div className="text-lg font-semibold">{look.data.name}</div>
                <div className="text-sm text-ink-400">
                  v{look.data.version} by {look.data.author} · #{look.data.mod_id}
                </div>
              </div>
              <div className="flex gap-2">
                <Button variant="ghost" icon={<ExternalLink className="size-4" />} onClick={() => openLink(look.data!.url)}>
                  Nexus
                </Button>
                <Button
                  variant="primary"
                  icon={<Download className="size-4" />}
                  disabled={busy}
                  onClick={() => run(`Install ${look.data!.name}`, ["install", String(look.data!.mod_id), "--with-deps", ...(file ? ["--file", file] : [])])}
                >
                  {installedIds.has(look.data.mod_id) ? "Reinstall / switch file" : "Install"}
                </Button>
              </div>
            </div>
            <div className="mt-4 grid gap-4 lg:grid-cols-2">
              <div>
                <div className="mb-2 text-xs font-semibold uppercase tracking-wider text-ink-400">File</div>
                <div className="space-y-1.5">
                  {look.data.files.map((f) => {
                    const sel = file ? file === f.gsid : f.category === "main" && look.data!.files.filter((x) => x.category === "main").length === 1;
                    return (
                      <button
                        type="button"
                        key={f.gsid}
                        onClick={() => setFile(f.gsid)}
                        className={clsx(
                          "flex w-full items-center gap-3 rounded-lg border px-3 py-2 text-left text-sm cursor-pointer",
                          sel ? "border-neon-cyan/50 bg-neon-cyan/10" : "border-ink-700 hover:bg-ink-800",
                        )}
                      >
                        <Badge tone={f.category === "main" ? "cyan" : "muted"} className="w-20 justify-center">
                          {f.category}
                        </Badge>
                        <span className="min-w-0 flex-1 truncate">{f.name}</span>
                        <span className="font-mono text-xs text-ink-400">v{f.version}</span>
                      </button>
                    );
                  })}
                </div>
              </div>
              <div>
                <div className="mb-2 text-xs font-semibold uppercase tracking-wider text-ink-400">Requirements</div>
                {look.data.requires.length === 0 && <div className="text-sm text-ink-400">None</div>}
                <div className="space-y-1.5">
                  {look.data.requires.map((r) => (
                    <div key={r.mod_id || r.name} className="flex items-center gap-2 text-sm">
                      {r.external ? (
                        <Badge>external</Badge>
                      ) : installedIds.has(r.mod_id) ? (
                        <Badge tone="lime">installed</Badge>
                      ) : r.optional ? (
                        <Badge tone="violet">optional</Badge>
                      ) : (
                        <Badge tone="amber">will install</Badge>
                      )}
                      <span className="truncate">{r.name}</span>
                      {r.notes && <span className="truncate text-xs text-ink-400">— {r.notes}</span>}
                    </div>
                  ))}
                </div>
              </div>
            </div>
          </div>
        )}
      </Card>

      <div className="grid gap-6 lg:grid-cols-2">
        <Card className="p-5">
          <div className="mb-3 flex items-center justify-between">
            <h2 className="flex items-center gap-2 font-semibold">
              <BookmarkCheck className="size-4 text-neon-pink" /> Tracked on Nexus
            </h2>
            <Button
              size="sm"
              disabled={busy || !tracked.data?.some((t) => !t.installed_as)}
              onClick={() => run("Install tracked mods", ["tracked", "--install", "--untrack"])}
            >
              Install all
            </Button>
          </div>
          <p className="mb-3 text-xs text-ink-400">Press Track on a mod page in any browser (works in Steam's, which can't open mod-manager links).</p>
          {tracked.isLoading && <Spinner />}
          {tracked.error && <ErrorBox error={tracked.error} />}
          {tracked.data?.length === 0 && <div className="text-sm text-ink-400">Nothing tracked for Cyberpunk.</div>}
          <div className="space-y-1.5">
            {tracked.data?.map((t) => (
              <div key={t.mod_id} className="flex items-center gap-2 text-sm">
                {t.installed_as ? <Badge tone="lime">installed</Badge> : <Badge tone="cyan">new</Badge>}
                <span className="min-w-0 flex-1 truncate">{t.name}</span>
                {!t.installed_as && (
                  <Button size="sm" variant="ghost" disabled={busy} onClick={() => run(`Install ${t.name}`, ["install", String(t.mod_id), "--with-deps"])}>
                    Install
                  </Button>
                )}
              </div>
            ))}
          </div>
        </Card>
        <Card className="p-5">
          <h2 className="mb-3 flex items-center gap-2 font-semibold">
            <Gamepad2 className="size-4 text-neon-violet" /> Mod manager links
          </h2>
          <p className="text-sm text-ink-300">
            {state.nxm_handler ? (
              <>
                <CheckCircle2 className="mr-1 inline size-4 text-neon-lime" />
                “Mod Manager Download” buttons open cpmm (Firefox / desktop browsers).
              </>
            ) : (
              "cpmm is not the nxm:// handler yet."
            )}
          </p>
          <div className="mt-4">
            <Button disabled={busy} onClick={() => run("Register link handler", ["register-handler"])}>
              {state.nxm_handler ? "Re-register handler" : "Register as handler"}
            </Button>
          </div>
        </Card>
      </div>
    </>
  );
}

/* ---------------------------------------------------------------------- System */

export function SystemView({ state }: { state: AppState }) {
  const { run, busy } = useJobs();
  return (
    <>
      <PageHeader title="System" subtitle="Session, paths and housekeeping." />
      <div className="grid gap-6 lg:grid-cols-2">
        <Card className="p-5">
          <h2 className="mb-3 font-semibold">Game session</h2>
          {state.session ? (
            <p className="text-sm text-ink-300">
              Running since {new Date(state.session.started).toLocaleTimeString()} with <span className="text-neon-cyan">{state.session.layers}</span> mod
              layers mounted.
            </p>
          ) : (
            <p className="text-sm text-ink-300">
              Not running. Launch Cyberpunk from Steam; its launch options include <code className="font-mono text-neon-cyan">cpmm run -- %command%</code>.
            </p>
          )}
        </Card>
        <Card className="p-5">
          <h2 className="mb-3 font-semibold">Disk</h2>
          <p className="mb-4 text-sm text-ink-300">
            Empty the trash (uninstalled mods, replaced versions, cleared overwrites), temp files and cached downloads.
          </p>
          <Button icon={<Trash2 className="size-4" />} disabled={busy} onClick={() => run("Clean", ["clean"])}>
            Clean up
          </Button>
        </Card>
        <Card className="p-5 lg:col-span-2">
          <h2 className="mb-3 font-semibold">Paths</h2>
          <dl className="grid grid-cols-[8rem_1fr] gap-y-2 text-sm">
            <dt className="text-ink-400">Game</dt>
            <dd className="truncate font-mono text-xs leading-5">{state.game_dir}</dd>
            <dt className="text-ink-400">Data</dt>
            <dd className="truncate font-mono text-xs leading-5">{state.data_dir}</dd>
          </dl>
        </Card>
        {state.modio.length > 0 && (
          <Card className="p-5 lg:col-span-2">
            <h2 className="mb-1 font-semibold">mod.io mods ({state.modio.length})</h2>
            <p className="mb-3 text-xs text-ink-400">Installed by the in-game Mods menu, outside the game folder. Manage them there.</p>
            <div className="grid gap-x-6 gap-y-1 sm:grid-cols-2">
              {state.modio.map((m) => (
                <div key={m.id} className="truncate text-sm">
                  {m.name}{" "}
                  <span className="text-ink-400">
                    {m.version} · {m.author}
                  </span>
                </div>
              ))}
            </div>
          </Card>
        )}
      </div>
    </>
  );
}
