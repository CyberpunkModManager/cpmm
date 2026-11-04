import { useQuery } from "@tanstack/react-query";
import clsx from "clsx";
import { useState } from "react";
import { Badge, Button, Drawer, ErrorBox, Spinner } from "../components/ui";
import { api6 } from "../lib/cpmm";

/** Drawer with a mod's Nexus changelog: what an update brings, then older history on request. */
export function ChangelogDrawer({ id, onClose }: { id: string; onClose: () => void }) {
  const q = useQuery({ queryKey: ["changelog", id], queryFn: () => api6.changelog(id) });
  const [all, setAll] = useState(false);
  const d = q.data;
  const newer = d?.entries.filter((e) => e.newer) ?? [];
  const shown = d ? (all ? d.entries : newer.length ? [...newer, ...d.entries.filter((e) => e.installed)] : d.entries.slice(0, 3)) : [];
  return (
    <Drawer
      open
      onClose={onClose}
      title={
        <div>
          <div className="truncate text-lg font-semibold">What's new{d ? ` in ${d.name}` : ""}</div>
          {d && <div className="text-xs text-ink-400">installed: {d.installed || "unknown"}</div>}
        </div>
      }
    >
      {q.isLoading && <Spinner label="Fetching the changelog from Nexus…" />}
      {q.error && <ErrorBox error={q.error} />}
      {d && d.entries.length === 0 && <p className="text-sm text-ink-400">The author hasn't written a changelog on Nexus. Check the mod page or its posts.</p>}
      {d && d.entries.length > 0 && newer.length === 0 && !all && (
        <p className="mb-4 text-sm text-ink-400">No changelog entries for versions newer than yours (authors don't always add them). Latest entries:</p>
      )}
      <div className="space-y-3">
        {shown.map((e) => (
          <div key={e.version} className={clsx("rounded-xl border px-4 py-3", e.newer ? "border-neon-cyan/40 bg-neon-cyan/5" : "border-ink-700")}>
            <div className="mb-1 flex items-center gap-2">
              <span className="font-mono font-semibold">{e.version}</span>
              {e.newer && <Badge tone="cyan">new</Badge>}
              {e.installed && <Badge tone="lime">installed</Badge>}
            </div>
            <ul className="list-inside list-disc space-y-0.5 text-sm text-ink-300 select-text">
              {e.changes.map((c, i) => (
                <li key={i}>{c}</li>
              ))}
            </ul>
          </div>
        ))}
      </div>
      {d && d.entries.length > shown.length && (
        <Button variant="ghost" className="mt-4" onClick={() => setAll(true)}>
          Show all {d.entries.length} versions
        </Button>
      )}
    </Drawer>
  );
}
