import { useQueryClient } from "@tanstack/react-query";
import { createContext, type ReactNode, useCallback, useContext, useMemo, useRef, useState } from "react";
import { type Action, act, command, runCpmm } from "./cpmm";

export interface Job {
  id: number;
  label: string;
  /** The cpmm command it is (shown with the log) */
  command: string;
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
  /** Run an action as a job: its log, a toast when it ends, every query refreshed. */
  run: (label: string, action: Action) => Promise<boolean>;
  /** Like run, but also returns its output and the mods it newly installed. */
  exec: (label: string, action: Action) => Promise<{ ok: boolean; output: string; installed: string[] }>;
  /** Run a cpmm CLI command as a job (argv; on the way out). */
  runArgs: (label: string, args: string[]) => Promise<boolean>;
  execArgs: (label: string, args: string[]) => Promise<{ ok: boolean; output: string }>;
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

  /** A job: its log, a toast when it ends (the last output line on failure), every query refreshed. */
  const job = useCallback(
    async (
      label: string,
      cmd: string,
      start: (onLine: (text: string, err: boolean) => void) => Promise<{ code: number; output: string; installed?: string[] }>,
    ) => {
      const local = seq.current++;
      setJobs((j) => [{ id: local, label, command: cmd, lines: [], status: "running" as const, startedAt: Date.now() } satisfies Job, ...j].slice(0, 30));
      const push = (text: string, err: boolean) => setJobs((js) => js.map((j) => (j.id === local ? { ...j, lines: [...j.lines, { text, err }] } : j)));
      try {
        const r = await start(push);
        const ok = r.code === 0;
        setJobs((js) => js.map((j) => (j.id === local ? { ...j, status: ok ? "ok" : "error" } : j)));
        const last = r.output.trim().split("\n").filter(Boolean).pop() ?? "";
        toast(ok ? "ok" : "error", ok ? `${label}: done` : `${label} failed: ${last.replace(/^error:\s*/, "")}`);
        return { ok, output: r.output, installed: r.installed ?? [] };
      } catch (e) {
        push(String(e), true);
        setJobs((js) => js.map((j) => (j.id === local ? { ...j, status: "error" } : j)));
        toast("error", `${label} failed: ${e}`);
        return { ok: false, output: String(e), installed: [] };
      } finally {
        await qc.invalidateQueries();
      }
    },
    [qc, toast],
  );
  const exec = useCallback((label: string, action: Action) => job(label, command(action), (push) => act(action, (l) => push(l.line, l.stderr))), [job]);
  const run = useCallback((label: string, action: Action) => exec(label, action).then((r) => r.ok), [exec]);
  const execArgs = useCallback((label: string, args: string[]) => job(label, args[0], (push) => runCpmm(args, (l) => push(l.line, l.stderr))), [job]);
  const runArgs = useCallback((label: string, args: string[]) => execArgs(label, args).then((r) => r.ok), [execArgs]);

  const value = useMemo<Ctx>(
    () => ({
      jobs,
      toasts,
      busy: jobs.some((j) => j.status === "running"),
      run,
      exec,
      runArgs,
      execArgs,
      toast,
      dismiss: (id) => setToasts((t) => t.filter((x) => x.id !== id)),
      clearFinished: () => setJobs((j) => j.filter((x) => x.status === "running")),
    }),
    [jobs, toasts, run, exec, runArgs, execArgs, toast],
  );
  return <JobsCtx.Provider value={value}>{children}</JobsCtx.Provider>;
}

export function useJobs() {
  const c = useContext(JobsCtx);
  if (!c) throw new Error("useJobs outside JobsProvider");
  return c;
}
