import { useQueryClient } from "@tanstack/react-query";
import { createContext, type ReactNode, useCallback, useContext, useMemo, useRef, useState } from "react";
import { runCpmm } from "./cpmm";

export interface Job {
  id: number;
  label: string;
  args: string[];
  lines: { text: string; err: boolean }[];
  status: "running" | "ok" | "error";
  startedAt: number;
}

export interface Toast {
  id: number;
  kind: "ok" | "error" | "info";
  text: string;
}

interface Ctx {
  jobs: Job[];
  toasts: Toast[];
  busy: boolean;
  run: (label: string, args: string[]) => Promise<boolean>;
  /** Like run, but also returns the command's output. */
  exec: (label: string, args: string[]) => Promise<{ ok: boolean; output: string }>;
  toast: (kind: Toast["kind"], text: string) => void;
  dismiss: (id: number) => void;
  clearFinished: () => void;
}

const JobsCtx = createContext<Ctx | null>(null);

export function JobsProvider({ children }: { children: ReactNode }) {
  const qc = useQueryClient();
  const [jobs, setJobs] = useState<Job[]>([]);
  const [toasts, setToasts] = useState<Toast[]>([]);
  const seq = useRef(1);

  const toast = useCallback((kind: Toast["kind"], text: string) => {
    const id = seq.current++;
    setToasts((t) => [...t, { id, kind, text }]);
    setTimeout(() => setToasts((t) => t.filter((x) => x.id !== id)), kind === "error" ? 9000 : 4500);
  }, []);

  const exec = useCallback(
    async (label: string, args: string[]) => {
      const local = seq.current++;
      setJobs((j) => [{ id: local, label, args, lines: [], status: "running" as const, startedAt: Date.now() } satisfies Job, ...j].slice(0, 30));
      const push = (text: string, err: boolean) => setJobs((js) => js.map((j) => (j.id === local ? { ...j, lines: [...j.lines, { text, err }] } : j)));
      try {
        const r = await runCpmm(args, (l) => push(l.line, l.stderr));
        const ok = r.code === 0;
        setJobs((js) => js.map((j) => (j.id === local ? { ...j, status: ok ? "ok" : "error" } : j)));
        const last = r.output.trim().split("\n").filter(Boolean).pop() ?? "";
        toast(ok ? "ok" : "error", ok ? `${label}: done` : `${label} failed: ${last.replace(/^error:\s*/, "")}`);
        return { ok, output: r.output };
      } catch (e) {
        push(String(e), true);
        setJobs((js) => js.map((j) => (j.id === local ? { ...j, status: "error" } : j)));
        toast("error", `${label} failed: ${e}`);
        return { ok: false, output: String(e) };
      } finally {
        await qc.invalidateQueries();
      }
    },
    [qc, toast],
  );
  const run = useCallback((label: string, args: string[]) => exec(label, args).then((r) => r.ok), [exec]);

  const value = useMemo<Ctx>(
    () => ({
      jobs,
      toasts,
      busy: jobs.some((j) => j.status === "running"),
      run,
      exec,
      toast,
      dismiss: (id) => setToasts((t) => t.filter((x) => x.id !== id)),
      clearFinished: () => setJobs((j) => j.filter((x) => x.status === "running")),
    }),
    [jobs, toasts, run, exec, toast],
  );
  return <JobsCtx.Provider value={value}>{children}</JobsCtx.Provider>;
}

export function useJobs() {
  const c = useContext(JobsCtx);
  if (!c) throw new Error("useJobs outside JobsProvider");
  return c;
}
