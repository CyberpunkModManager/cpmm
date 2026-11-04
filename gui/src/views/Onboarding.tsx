import { useQuery, useQueryClient } from "@tanstack/react-query";
import { openUrl } from "@tauri-apps/plugin-opener";
import clsx from "clsx";
import { ArrowLeft, ArrowRight, Check, CircleAlert, ExternalLink, FolderOpen, Gamepad2, KeyRound, Layers, Rocket, Settings2, Sparkles } from "lucide-react";
import { type ReactNode, useEffect, useRef, useState } from "react";
import { Button, Card, Input, Spinner, Switch } from "../components/ui";
import { api } from "../lib/cpmm";
import { useJobs } from "../lib/jobs";

const STEPS = ["Welcome", "Game", "Nexus Mods", "Steam", "Preferences", "Done"] as const;
const PREFS = [
  "check_updates_on_start",
  "auto_install_updates",
  "auto_sort",
  "auto_install_downloads",
  "snapshot_saves",
  "profiles_enabled",
  "close_on_launch",
];

/** First-launch setup: game folder, Nexus key, Steam launch options and the main preferences. */
export function Onboarding({ onClose }: { onClose: () => void }) {
  const { runArgs: run, busy } = useJobs();
  const qc = useQueryClient();
  const st = useQuery({ queryKey: ["onboarding"], queryFn: api.onboarding });
  const cfg = useQuery({ queryKey: ["settings"], queryFn: api.settings });
  const [step, setStep] = useState(0);
  const [gamePath, setGamePath] = useState("");
  const [key, setKey] = useState("");
  const root = useRef<HTMLDivElement>(null);
  // biome-ignore lint/correctness/useExhaustiveDependencies: runs on every step change to move controller focus
  useEffect(() => {
    // step changes: put controller focus on the first control of the new step
    setTimeout(() => root.current?.querySelector<HTMLElement>("[data-step] button, [data-step] input")?.focus(), 30);
  }, [step]);
  const s = st.data;
  const finish = async () => {
    await run("Finish setup", ["settings", "set", "onboarded", "on"]);
    await qc.invalidateQueries();
    onClose();
  };
  const ok = (good: boolean, text: ReactNode) => (
    <div className={clsx("flex items-center gap-2 text-sm", good ? "text-neon-lime" : "text-neon-amber")}>
      {good ? <Check className="size-4" /> : <CircleAlert className="size-4" />} {text}
    </div>
  );

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-ink-950/90 p-6 backdrop-blur" data-nav-layer ref={root}>
      <Card className="flex max-h-full w-full max-w-3xl flex-col overflow-hidden">
        <div className="flex items-center gap-2 border-b border-ink-700 px-6 py-4">
          {STEPS.map((n, i) => (
            <div key={n} className="flex items-center gap-2">
              <span
                className={clsx(
                  "flex size-7 items-center justify-center rounded-full text-xs font-semibold",
                  i < step ? "bg-neon-lime/20 text-neon-lime" : i === step ? "bg-neon-cyan text-ink-950" : "bg-ink-800 text-ink-400",
                )}
              >
                {i < step ? <Check className="size-3.5" /> : i + 1}
              </span>
              <span className={clsx("hidden text-xs md:inline", i === step ? "text-ink-100" : "text-ink-400")}>{n}</span>
              {i < STEPS.length - 1 && <span className="mx-1 h-px w-4 bg-ink-700" />}
            </div>
          ))}
        </div>

        <div className="flex-1 overflow-y-auto px-8 py-7" data-step>
          {!s ? (
            <Spinner />
          ) : step === 0 ? (
            <div>
              <div className="mb-4 flex size-14 items-center justify-center rounded-2xl bg-neon-cyan/15 text-neon-cyan">
                <Layers className="size-7" />
              </div>
              <h1 className="text-2xl font-semibold">Welcome to cpmm</h1>
              <p className="mt-2 text-ink-300">A mod manager for Cyberpunk 2077 on Linux, Steam Deck and other handhelds. A few steps and you're ready:</p>
              <ul className="mt-4 space-y-2 text-sm text-ink-300">
                <li>• Mods are mounted over the game at launch and never copied into the game folder, so it stays vanilla.</li>
                <li>• Conflicts, requirements and the load order are handled automatically.</li>
                <li>• Every update can be undone, and your saves are snapshotted before risky changes.</li>
              </ul>
              <p className="mt-4 flex items-center gap-2 text-xs text-ink-400">
                <Gamepad2 className="size-4" /> D-pad / stick to move, A to select, B to go back.
              </p>
            </div>
          ) : step === 1 ? (
            <div className="space-y-4">
              <h2 className="flex items-center gap-2 text-xl font-semibold">
                <FolderOpen className="size-5 text-neon-cyan" /> Cyberpunk 2077
              </h2>
              {ok(s.game_found, s.game_found ? `Found at ${s.game_dir}` : `Not found at ${s.game_dir}`)}
              {!s.game_found && (
                <form
                  className="flex gap-2"
                  onSubmit={(e) => {
                    e.preventDefault();
                    if (gamePath.trim()) run("Set game folder", ["init", "--game", gamePath.trim()]).then(() => st.refetch());
                  }}
                >
                  <Input
                    value={gamePath}
                    onChange={(e) => setGamePath(e.target.value)}
                    placeholder="/path/to/steamapps/common/Cyberpunk 2077"
                    className="min-w-0 flex-1"
                  />
                  <Button type="submit" disabled={busy || !gamePath.trim()}>
                    Use this folder
                  </Button>
                </form>
              )}
            </div>
          ) : step === 2 ? (
            <div className="space-y-4">
              <h2 className="flex items-center gap-2 text-xl font-semibold">
                <KeyRound className="size-5 text-neon-pink" /> Nexus Mods API key
              </h2>
              <p className="text-sm text-ink-300">
                cpmm uses your personal API key to look up mods, requirements and updates. It is stored only on this machine (~/.config/nexusmods/apikey).
                Premium accounts can download directly; free accounts use the site's "Mod Manager Download" buttons.
              </p>
              {ok(s.api_key, s.api_key ? "A key is configured" : "No key yet")}
              <div className="flex flex-wrap gap-2">
                <Button icon={<ExternalLink className="size-4" />} onClick={() => openUrl("https://www.nexusmods.com/users/myaccount?tab=api")}>
                  Get your key
                </Button>
                {s.api_key && (
                  <Button variant="ghost" disabled={busy} onClick={() => run("Check Nexus key", ["nexus-key", "--check"])}>
                    Check it
                  </Button>
                )}
              </div>
              <form
                className="flex gap-2"
                onSubmit={(e) => {
                  e.preventDefault();
                  if (key.trim())
                    run("Save Nexus key", ["nexus-key", key.trim()]).then((good) => {
                      if (good) setKey("");
                      st.refetch();
                    });
                }}
              >
                <Input type="password" value={key} onChange={(e) => setKey(e.target.value)} placeholder="Paste your API key" className="min-w-0 flex-1" />
                <Button type="submit" variant="primary" disabled={busy || !key.trim()}>
                  Save key
                </Button>
              </form>
            </div>
          ) : step === 3 ? (
            <div className="space-y-4">
              <h2 className="flex items-center gap-2 text-xl font-semibold">
                <Rocket className="size-5 text-neon-violet" /> Launch through cpmm
              </h2>
              <p className="text-sm text-ink-300">
                Mods load only when Steam starts the game through cpmm. cpmm adds itself to Cyberpunk's launch options and keeps whatever you already have
                there.
              </p>
              {ok(s.launch_ok, s.launch_ok ? "Steam already launches the game through cpmm" : "Not set up yet")}
              <pre className="rounded-lg bg-ink-900 p-3 font-mono text-xs whitespace-pre-wrap text-ink-300 select-text">
                {s.launch_ok ? s.launch_options : s.launch_suggested}
              </pre>
              {!s.launch_ok && (
                <>
                  <Button variant="primary" disabled={busy} onClick={() => run("Set launch options", ["launch-options", "--apply"]).then(() => st.refetch())}>
                    Set it up
                  </Button>
                  {s.steam_running && !s.steam_cef && (
                    <p className="text-xs text-neon-amber">
                      Steam is running without its debugger, so cpmm can't change this live. Quit Steam and press the button, or paste the line above into
                      Cyberpunk → Properties → Launch Options.
                    </p>
                  )}
                </>
              )}
            </div>
          ) : step === 4 ? (
            <div className="space-y-5">
              <h2 className="flex items-center gap-2 text-xl font-semibold">
                <Settings2 className="size-5 text-neon-amber" /> Preferences
              </h2>
              {cfg.data?.schema
                .filter((f) => PREFS.includes(f.key))
                .map((f) => (
                  <div key={f.key} className="flex items-start gap-4">
                    <div className="min-w-0 flex-1">
                      <div className="text-sm font-medium">{f.label}</div>
                      <div className="mt-0.5 text-xs text-ink-400">{f.help}</div>
                    </div>
                    <Switch
                      checked={!!cfg.data?.values[f.key]}
                      label={f.label}
                      disabled={busy}
                      onChange={(v) => run(`Setting: ${f.key}`, ["settings", "set", f.key, v ? "on" : "off"])}
                    />
                  </div>
                ))}
              <div className="flex items-start gap-4">
                <div className="min-w-0 flex-1">
                  <div className="text-sm font-medium">Open Nexus "Mod Manager Download" links with cpmm</div>
                  <div className="mt-0.5 text-xs text-ink-400">Works in Firefox and desktop browsers (Steam's browser can't open these links).</div>
                </div>
                {s.nxm_handler ? (
                  ok(true, "Registered")
                ) : (
                  <Button size="sm" disabled={busy} onClick={() => run("Register link handler", ["register-handler"]).then(() => st.refetch())}>
                    Register
                  </Button>
                )}
              </div>
            </div>
          ) : (
            <div>
              <div className="mb-4 flex size-14 items-center justify-center rounded-2xl bg-neon-lime/15 text-neon-lime">
                <Sparkles className="size-7" />
              </div>
              <h1 className="text-2xl font-semibold">You're set</h1>
              <div className="mt-4 space-y-2">
                {ok(s.game_found, "Game folder")}
                {ok(s.api_key, "Nexus API key")}
                {ok(s.launch_ok, "Steam launches through cpmm")}
                {ok(s.nxm_handler, "Mod Manager Download links")}
              </div>
              <p className="mt-4 text-sm text-ink-300">
                Install mods from the Install tab (or the built-in Nexus browser). Everything here can be changed later in Settings.
              </p>
            </div>
          )}
        </div>

        <div className="flex items-center justify-between border-t border-ink-700 px-6 py-4">
          <Button variant="ghost" disabled={busy} onClick={finish}>
            Skip setup
          </Button>
          <div className="flex gap-2">
            {step > 0 && (
              <Button icon={<ArrowLeft className="size-4" />} onClick={() => setStep(step - 1)}>
                Back
              </Button>
            )}
            {step < STEPS.length - 1 ? (
              <Button variant="primary" icon={<ArrowRight className="size-4" />} onClick={() => setStep(step + 1)}>
                Next
              </Button>
            ) : (
              <Button variant="primary" icon={<Check className="size-4" />} disabled={busy} onClick={finish}>
                Start modding
              </Button>
            )}
          </div>
        </div>
      </Card>
    </div>
  );
}
