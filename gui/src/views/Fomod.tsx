import { useQuery } from "@tanstack/react-query";
import clsx from "clsx";
import { ArrowLeft, ArrowRight, Check, Package } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { Badge, Button, Card, ErrorBox, Spinner } from "../components/ui";
import { api, type FomodPlugin } from "../lib/cpmm";

type Choices = Record<string, string[]>;

const single = (kind: string) => /exactlyone|atmostone/i.test(kind);

/**
 * FOMOD installer chooser. Steps that only appear for some choices are re-evaluated by cpmm
 * after every change; the result is passed to `cpmm install --fomod-choices`.
 */
export function FomodDialog({
  source,
  file,
  title,
  initial,
  onCancel,
  onInstall,
}: {
  source: string;
  file?: string | null;
  title: string;
  initial?: Choices | null;
  onCancel: () => void;
  onInstall: (choices: Choices) => void;
}) {
  const [choices, setChoices] = useState<Choices>(initial ?? {});
  const [page, setPage] = useState(0);
  const [focus, setFocus] = useState<FomodPlugin | null>(null);
  const root = useRef<HTMLDivElement>(null);
  const q = useQuery({ queryKey: ["fomod", source, file, choices], queryFn: () => api.fomod(source, file, choices), placeholderData: (p) => p });
  const steps = useMemo(() => (q.data?.steps ?? []).filter((s) => s.visible), [q.data]);
  const step = steps[Math.min(page, Math.max(0, steps.length - 1))];
  // biome-ignore lint/correctness/useExhaustiveDependencies: move controller focus when the page changes
  useEffect(() => {
    setTimeout(() => root.current?.querySelector<HTMLElement>("[data-opts] button")?.focus(), 30);
  }, [page]);

  // effective selection of a group: what the user picked, else cpmm's defaults
  const selected = (key: string, plugins: FomodPlugin[]) => choices[key] ?? plugins.filter((p) => p.selected).map((p) => p.name);
  const toggle = (key: string, kind: string, plugins: FomodPlugin[], p: FomodPlugin) => {
    if (/required/i.test(p.kind) || /notusable/i.test(p.kind) || /selectall/i.test(kind)) return;
    const cur = selected(key, plugins);
    const next = single(kind)
      ? cur.includes(p.name) && /atmostone/i.test(kind)
        ? []
        : [p.name]
      : cur.includes(p.name)
        ? cur.filter((n) => n !== p.name)
        : [...cur, p.name];
    setChoices({ ...choices, [key]: next });
  };
  const final = () => {
    // send the complete effective selection so the install matches what was shown
    const out: Choices = {};
    for (const s of steps) for (const g of s.groups) out[g.key] = selected(g.key, g.plugins);
    onInstall(out);
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-ink-950/85 p-4 backdrop-blur" data-nav-layer ref={root}>
      <div className="absolute inset-0" onClick={onCancel} />
      <Card className="relative flex max-h-full w-full max-w-5xl flex-col overflow-hidden">
        <div className="flex items-center gap-3 border-b border-ink-700 px-6 py-4">
          <Package className="size-5 text-neon-cyan" />
          <div className="min-w-0 flex-1">
            <div className="truncate font-semibold">{q.data?.name || title}</div>
            <div className="text-xs text-ink-400">
              Installer options{step ? ` · ${step.name} (${Math.min(page, steps.length - 1) + 1}/${steps.length})` : ""}
            </div>
          </div>
        </div>
        {q.isLoading && <Spinner label="Downloading and reading the installer…" />}
        {q.error && (
          <div className="p-6">
            <ErrorBox error={q.error} />
          </div>
        )}
        {step && (
          <div className="grid min-h-0 flex-1 grid-cols-1 md:grid-cols-[1fr_20rem]">
            <div className="min-h-0 space-y-6 overflow-y-auto px-6 py-5" data-opts>
              {step.groups.map((g) => {
                const sel = selected(g.key, g.plugins);
                return (
                  <div key={g.key}>
                    <div className="mb-2 flex items-center gap-2">
                      <h3 className="font-medium">{g.name}</h3>
                      <Badge>{single(g.kind) ? "pick one" : /selectall/i.test(g.kind) ? "all included" : "pick any"}</Badge>
                    </div>
                    <div className="grid gap-2 sm:grid-cols-2">
                      {g.plugins.map((p) => {
                        const on = sel.includes(p.name) || /required/i.test(p.kind);
                        const off = /notusable/i.test(p.kind);
                        return (
                          <button
                            type="button"
                            key={p.name}
                            disabled={off}
                            onFocus={() => setFocus(p)}
                            onMouseEnter={() => setFocus(p)}
                            onClick={() => toggle(g.key, g.kind, g.plugins, p)}
                            className={clsx(
                              "flex items-center gap-3 rounded-xl border px-3 py-2 text-left text-sm cursor-pointer disabled:opacity-40",
                              on ? "border-neon-cyan/60 bg-neon-cyan/10" : "border-ink-700 hover:bg-ink-800",
                            )}
                          >
                            <span
                              className={clsx(
                                "flex size-5 shrink-0 items-center justify-center border",
                                single(g.kind) ? "rounded-full" : "rounded-md",
                                on ? "border-neon-cyan bg-neon-cyan text-ink-950" : "border-ink-600",
                              )}
                            >
                              {on && <Check className="size-3.5" />}
                            </span>
                            <span className="min-w-0 flex-1 truncate">{p.name}</span>
                            {/recommended/i.test(p.kind) && <Badge tone="lime">recommended</Badge>}
                            {/required/i.test(p.kind) && <Badge>required</Badge>}
                          </button>
                        );
                      })}
                    </div>
                  </div>
                );
              })}
            </div>
            <div className="hidden border-l border-ink-700 bg-ink-900/60 p-4 md:block">
              {focus ? (
                <>
                  {focus.image && <img src={focus.image} alt="" className="mb-3 w-full rounded-lg border border-ink-700 object-contain" />}
                  <div className="font-medium">{focus.name}</div>
                  <p className="mt-1 text-xs whitespace-pre-wrap text-ink-400">{focus.description || "No description."}</p>
                </>
              ) : (
                <p className="text-xs text-ink-400">Highlight an option to see its preview and description.</p>
              )}
            </div>
          </div>
        )}
        <div className="flex items-center justify-between border-t border-ink-700 px-6 py-4">
          <Button variant="ghost" onClick={onCancel}>
            Cancel
          </Button>
          <div className="flex gap-2">
            {page > 0 && (
              <Button icon={<ArrowLeft className="size-4" />} onClick={() => setPage(page - 1)}>
                Back
              </Button>
            )}
            {page < steps.length - 1 ? (
              <Button variant="primary" icon={<ArrowRight className="size-4" />} onClick={() => setPage(page + 1)}>
                Next
              </Button>
            ) : (
              <Button variant="primary" icon={<Check className="size-4" />} disabled={!steps.length || q.isFetching} onClick={final}>
                Install
              </Button>
            )}
          </div>
        </div>
      </Card>
    </div>
  );
}
