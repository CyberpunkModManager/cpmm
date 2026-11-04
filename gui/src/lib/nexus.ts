import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { openUrl as openExternal } from "@tauri-apps/plugin-opener";
import { useEffect } from "react";
import { useJobs } from "./jobs";

/** Open the built-in Nexus browser (optionally at a nexusmods.com page). */
export const openNexus = (url?: string) => invoke("open_nexus", { url: url ?? null });

const NEXUS = /^https:\/\/([a-z0-9-]+\.)*nexusmods\.com(\/|$)/i;

/** Nexus pages open in the built-in browser (where downloads go to cpmm), anything else outside. */
export function openLink(url: string) {
  return NEXUS.test(url) ? openNexus(url) : openExternal(url);
}

function nexusToast(kind: "ok" | "error" | "info", text: string) {
  invoke("nexus_toast", { kind, text }).catch(() => {});
}

/** How an install went, in one line for a toast: the mods it installed, else its last line. */
function summary(r: { output: string; installed: string[] }): string {
  if (r.installed.length) return r.installed.join(", ");
  const last =
    r.output
      .trim()
      .split("\n")
      .map((l) => l.trim())
      .filter(Boolean)
      .pop() ?? "";
  return last.replace(/^error:\s*/, "");
}

/**
 * Downloads intercepted in the Nexus browser (nxm:// links and manual-download archives)
 * become normal cpmm jobs here, so they show in Activity; the result is echoed into the browser.
 */
export function useNexusBridge() {
  const { exec } = useJobs();
  useEffect(() => {
    const report = (r: { ok: boolean; output: string; installed: string[] }) =>
      nexusToast(r.ok ? "ok" : "error", r.ok ? `Installed ${summary(r)}` : `Install failed: ${summary(r)}`);
    const unNxm = listen<{ url: string }>("nexus-nxm", async (e) => {
      const id = /\/mods\/(\d+)/.exec(e.payload.url)?.[1];
      report(await exec(id ? `Install Nexus mod #${id}` : "Install from Nexus", { op: "nxm", url: e.payload.url, register: false }));
    });
    const unArchive = listen<{ path: string; mod_id: number | null }>("nexus-archive", async (e) => {
      const { path, mod_id } = e.payload;
      const name = path.split("/").pop() ?? path;
      // into the Downloads list; installed right away only with "Install downloads automatically"
      const add = { op: "downloads_add", path, nexus: mod_id || null, move_it: true, source: "browser", auto: true } as const;
      report(await exec(`Download ${name}`, add));
    });
    return () => {
      unNxm.then((f) => f());
      unArchive.then((f) => f());
    };
  }, [exec]);
}
