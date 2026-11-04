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

/** What an install printed, in one line for a toast. */
function summary(output: string): string {
  const lines = output.trim().split("\n").map((l) => l.trim()).filter(Boolean);
  const installed = lines.filter((l) => /^installed \S+: \d+ files/.test(l));
  if (installed.length) return installed.map((l) => l.replace(/^installed (\S+): .*/, "$1")).join(", ");
  return (lines.pop() ?? "").replace(/^error:\s*/, "");
}

/**
 * Downloads intercepted in the Nexus browser (nxm:// links and manual-download archives)
 * become normal cpmm jobs here, so they show in Activity; the result is echoed into the browser.
 */
export function useNexusBridge() {
  const { exec } = useJobs();
  useEffect(() => {
    const report = (ok: boolean, output: string) =>
      nexusToast(ok ? "ok" : "error", ok ? `Installed ${summary(output)}` : `Install failed: ${summary(output)}`);
    const unNxm = listen<{ url: string }>("nexus-nxm", async (e) => {
      const id = /\/mods\/(\d+)/.exec(e.payload.url)?.[1];
      const r = await exec(id ? `Install Nexus mod #${id}` : "Install from Nexus", ["nxm", e.payload.url]);
      report(r.ok, r.output);
    });
    const unArchive = listen<{ path: string; mod_id: number | null }>("nexus-archive", async (e) => {
      const { path, mod_id } = e.payload;
      const name = path.split("/").pop() ?? path;
      const args = ["install", path, "--with-deps", ...(mod_id ? ["--nexus", String(mod_id)] : [])];
      const r = await exec(`Install ${name}`, args);
      report(r.ok, r.output);
    });
    return () => {
      unNxm.then((f) => f());
      unArchive.then((f) => f());
    };
  }, [exec]);
}
