import { Gamepad2, Keyboard, X } from "lucide-react";
import { useEffect, useRef } from "react";
import { ACTIONS, KEYBOARD, useControllerBindings } from "../lib/controls";
import { Card, IconButton } from "./ui";

function Key({ children }: { children: string }) {
  return (
    <kbd className="inline-block min-w-8 rounded-md border border-ink-600 bg-ink-800 px-1.5 py-0.5 text-center font-mono text-[11px] text-ink-100">
      {children}
    </kbd>
  );
}

/** The controller and keyboard controls, read from the bindings the app actually uses. */
export function ControlTable({ nexus = true }: { nexus?: boolean }) {
  const pad = useControllerBindings().filter((b) => ACTIONS[b.action]?.app || (nexus && ACTIONS[b.action]?.nexus));
  return (
    <div className="grid gap-6 md:grid-cols-[3fr_2fr]">
      <section>
        <h3 className="mb-2 flex items-center gap-2 text-sm font-semibold">
          <Gamepad2 className="size-4 text-neon-cyan" /> Controller
        </h3>
        <table className="w-full text-sm">
          <thead className="text-left text-[11px] tracking-wider text-ink-400 uppercase">
            <tr>
              <th className="pb-1 font-normal" />
              <th className="pb-1 font-normal">In cpmm</th>
              {nexus && <th className="pb-1 font-normal">In the Nexus browser</th>}
            </tr>
          </thead>
          <tbody>
            {pad.map((b) => (
              <tr key={b.action} className="border-t border-ink-800 align-top">
                <td className="py-1.5 pr-3 whitespace-nowrap">
                  <Key>{b.input}</Key>
                </td>
                <td className="py-1.5 pr-3 text-ink-200">{ACTIONS[b.action]?.app ?? <span className="text-ink-500">—</span>}</td>
                {nexus && <td className="py-1.5 text-ink-300">{ACTIONS[b.action]?.nexus ?? <span className="text-ink-500">—</span>}</td>}
              </tr>
            ))}
          </tbody>
        </table>
      </section>
      <section>
        <h3 className="mb-2 flex items-center gap-2 text-sm font-semibold">
          <Keyboard className="size-4 text-neon-cyan" /> Keyboard
        </h3>
        <table className="w-full text-sm">
          <tbody>
            {KEYBOARD.map((k) => (
              <tr key={k.keys} className="border-t border-ink-800 align-top">
                <td className="py-1.5 pr-3 whitespace-nowrap">
                  <Key>{k.keys}</Key>
                </td>
                <td className="py-1.5 text-ink-200">{k.does}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </section>
    </div>
  );
}

/** The control map: Select (or ?) opens it; B, Esc or Select closes it. */
export function ControlMap({ onClose }: { onClose: () => void }) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    setTimeout(() => ref.current?.querySelector<HTMLElement>("button")?.focus(), 30);
  }, []);
  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center p-6" data-nav-layer>
      <div className="absolute inset-0 bg-ink-950/70 backdrop-blur-sm" onClick={onClose} />
      <Card className="relative flex max-h-full w-full max-w-4xl flex-col">
        <div ref={ref} className="flex items-center justify-between gap-4 border-b border-ink-700 px-6 py-4">
          <h2 className="text-lg font-semibold">Controls</h2>
          <IconButton label="Close (B / Esc / Select)" onClick={onClose}>
            <X className="size-5" />
          </IconButton>
        </div>
        <div className="overflow-y-auto px-6 py-5">
          <ControlTable />
        </div>
      </Card>
    </div>
  );
}
