import { useQuery } from "@tanstack/react-query";
import { Download, PackageCheck, RotateCcw, Trash2 } from "lucide-react";
import { useState } from "react";
import { Badge, Button, Confirm, Empty, ErrorBox, PageHeader, Spinner } from "../components/ui";
import { type AppState, api, type DownloadEntry, timeAgo } from "../lib/cpmm";
import { useJobs } from "../lib/jobs";
import { FomodDialog } from "./Fomod";

const tone = { installed: "lime", "other version installed": "amber", "not installed": "cyan" } as const;
const mb = (b: number) => (b >= 1 << 30 ? `${(b / (1 << 30)).toFixed(1)} GB` : `${(b / (1 << 20)).toFixed(1)} MB`);

/** Everything downloaded, separate from what's installed (like Vortex's / MO2's downloads list). */
export function DownloadsView({ state }: { state: AppState }) {
  const { run, busy } = useJobs();
  const q = useQuery({ queryKey: ["downloads"], queryFn: api.downloads, refetchInterval: 10_000 });
  const settings = useQuery({ queryKey: ["settings"], queryFn: api.settings, staleTime: 60_000 });
  const [del, setDel] = useState<DownloadEntry | null>(null);
  const [fomod, setFomod] = useState<DownloadEntry | null>(null);
  const [checking, setChecking] = useState<string | null>(null);
  const locked = state.game_running || !!state.session;

  const install = async (d: DownloadEntry) => {
    setChecking(d.file);
    try {
      const f = await api.fomod(d.path);
      if (f.fomod) {
        setFomod(d);
        return;
      }
    } catch {
      /* the install reports problems itself */
    } finally {
      setChecking(null);
    }
    run(`Install ${d.name}`, { op: "downloads_install", key: d.file, fomod: null, ask: false, with_deps: true });
  };

  const list = q.data ?? [];
  const waiting = list.filter((d) => d.status !== "installed").length;
  return (
    <>
      <PageHeader
        title="Downloads"
        subtitle={
          <>
            Everything you've downloaded, installed or not. {waiting > 0 && <span className="text-neon-cyan">{waiting} waiting to be installed. </span>}
            {settings.data?.values.auto_install_downloads
              ? "New downloads install automatically."
              : "New downloads wait here until you install them (Settings → Downloads)."}
          </>
        }
      />
      {q.isLoading && <Spinner />}
      {q.error && <ErrorBox error={q.error} />}
      {q.data && list.length === 0 && (
        <Empty icon={<Download className="size-6" />} title="No downloads yet">
          Use a mod page's "Mod Manager Download" button, the built-in Nexus browser, or Download on the Install page.
        </Empty>
      )}
      {list.length > 0 && (
        <div className="glass overflow-hidden rounded-2xl">
          {list.map((d, i) => (
            <div key={d.file} className={i > 0 ? "flex items-center gap-3 border-t border-ink-800 px-4 py-3" : "flex items-center gap-3 px-4 py-3"}>
              <Badge tone={tone[d.status]} className="w-36 justify-center">
                {d.status}
              </Badge>
              <div className="min-w-0 flex-1">
                <div className="truncate font-medium">{d.name || d.file}</div>
                <div className="truncate text-xs text-ink-400">
                  {[
                    d.version && `v${d.version}`,
                    d.file_name || d.file,
                    mb(d.size),
                    d.source,
                    d.downloaded_at && timeAgo(d.downloaded_at),
                    d.installed_as && `as ${d.installed_as}`,
                  ]
                    .filter(Boolean)
                    .join(" · ")}
                </div>
              </div>
              {d.status === "installed" ? (
                <Button size="sm" icon={<RotateCcw className="size-3.5" />} disabled={busy || locked} onClick={() => install(d)}>
                  Reinstall
                </Button>
              ) : (
                <Button
                  size="sm"
                  variant="primary"
                  icon={<PackageCheck className="size-3.5" />}
                  loading={checking === d.file}
                  disabled={busy || locked || !!checking}
                  onClick={() => install(d)}
                >
                  {d.status === "other version installed" ? "Install this version" : "Install"}
                </Button>
              )}
              <Button size="sm" variant="ghost" icon={<Trash2 className="size-3.5" />} disabled={busy} onClick={() => setDel(d)}>
                Delete
              </Button>
            </div>
          ))}
        </div>
      )}
      {locked && <p className="mt-3 text-xs text-neon-amber">Quit the game to install downloads.</p>}
      <Confirm
        open={!!del}
        title={`Delete ${del?.name || del?.file}?`}
        body={del?.installed_as ? `The archive is removed; the installed mod (${del.installed_as}) stays.` : "The archive is removed from disk."}
        confirmLabel="Delete"
        danger
        onCancel={() => setDel(null)}
        onConfirm={() => {
          const d = del!;
          setDel(null);
          run(`Delete ${d.name || d.file}`, { op: "downloads_remove", key: d.file });
        }}
      />
      {fomod && (
        <FomodDialog
          source={fomod.path}
          title={fomod.name}
          initial={state.mods.find((m) => m.id === fomod.installed_as)?.fomod ?? null}
          onCancel={() => setFomod(null)}
          onInstall={(choices) => {
            const d = fomod;
            setFomod(null);
            run(`Install ${d.name}`, { op: "downloads_install", key: d.file, fomod: choices, ask: false, with_deps: true });
          }}
        />
      )}
    </>
  );
}
