import { useEffect, useRef } from "react";
import { type AppState, api, api7, type SettingsData } from "./cpmm";
import { useJobs } from "./jobs";

/**
 * Background chores driven by Settings: check for updates on start and every refresh interval,
 * install them if allowed (never while the game runs), and install new downloads.
 */
export function useAutomation(state: AppState | undefined, settings: SettingsData | undefined) {
  const { run, busy, toast } = useJobs();
  const started = useRef(false);
  const toldModio = useRef("");
  const latest = useRef({ state, settings, busy });
  latest.current = { state, settings, busy };

  useEffect(() => {
    if (!settings) return;
    const v = settings.values;
    const tick = async (first: boolean) => {
      const { state: st, busy: b } = latest.current;
      if (!st || b) return;
      const gameUp = st.game_running || !!st.session;
      if (v.check_updates_on_start || !first) {
        try {
          const ups = (await api.updates(!first)).filter((u) => u.update);
          if (ups.length && v.auto_install_updates && !gameUp) await run(`Auto-update (${ups.length})`, ["update", "--all"]);
          else if (ups.length && first) toast("info", `${ups.length} mod update${ups.length > 1 ? "s" : ""} available`);
        } catch {
          /* offline: try again next interval */
        }
      }
      // mod.io watcher: say once when new subscriptions appear (the in-game menu makes that easy)
      try {
        const m = await api7.modio();
        const added = m.changes.first_look ? [] : m.changes.added;
        const key = added.join("|");
        if (added.length && key !== toldModio.current) {
          toldModio.current = key;
          toast("error", `New mod.io subscription${added.length > 1 ? "s" : ""}: ${added.join(", ")}. Check Health & order for conflicts.`);
        }
      } catch {
        /* no mod.io folder */
      }
    };
    if (!started.current) {
      started.current = true;
      void tick(true);
    }
    const minutes = Math.max(1, Number(v.refresh_minutes) || 60);
    const t = window.setInterval(() => void tick(false), minutes * 60_000);
    return () => {
      window.clearInterval(t);
    };
  }, [settings, run, toast]);
}
