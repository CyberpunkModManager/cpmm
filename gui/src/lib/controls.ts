import { useQuery } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";

/** One control on the controller and the action it sends (gui/src-tauri/src/main.rs `BUTTONS`). */
export interface Binding {
  input: string;
  action: string;
}

/**
 * What each action does: in the app (gamepad.ts, App.tsx) and in the Nexus browser
 * (src-tauri/src/nexus.js). An action with no entry for a place does nothing there.
 */
export const ACTIONS: Record<string, { app?: string; nexus?: string }> = {
  move: { app: "Move between controls (hold to repeat)", nexus: "Nudge the pointer" },
  scroll: { app: "Scroll", nexus: "Scroll what is under the pointer" },
  cursor: { app: "Move the pointer", nexus: "Move the pointer" },
  click: { app: "Click at the pointer", nexus: "Click at the pointer" },
  a: { app: "Press the focused control", nexus: "Click at the pointer" },
  b: { app: "Back: close a panel or dialog", nexus: "Back a page" },
  y: { nexus: "On-screen keyboard" },
  lb: { app: "Previous section", nexus: "Back a page" },
  rb: { app: "Next section", nexus: "Forward a page" },
  start: { app: "Show or hide Activity", nexus: "Close the browser" },
  select: { app: "Show this control map", nexus: "Reload the page" },
};

/** The app's keyboard controls (gamepad.ts `onKey`, plus the browser's own Enter / Tab). */
export const KEYBOARD: { keys: string; does: string }[] = [
  { keys: "Arrow keys", does: "Move between controls" },
  { keys: "Enter / Space", does: "Press the focused control" },
  { keys: "Tab / Shift+Tab", does: "Next / previous control" },
  { keys: "Esc", does: "Back: close a panel or dialog" },
  { keys: "[ / ]", does: "Previous / next section" },
  { keys: "?", does: "Show this control map" },
];

/** Used when the backend can't be asked (a browser preview of the frontend). */
const FALLBACK: Binding[] = [
  { input: "D-pad", action: "move" },
  { input: "Left stick", action: "scroll" },
  { input: "Right stick", action: "cursor" },
  { input: "A", action: "a" },
  { input: "B", action: "b" },
  { input: "Y", action: "y" },
  { input: "LB", action: "lb" },
  { input: "RB", action: "rb" },
  { input: "Start", action: "start" },
  { input: "Select", action: "select" },
  { input: "R3", action: "click" },
  { input: "RT", action: "click" },
];

/** The controller bindings as the backend has them, controls sharing an action merged ("R3 / RT"). */
export function useControllerBindings(): Binding[] {
  const q = useQuery({
    queryKey: ["controller-bindings"],
    queryFn: () => invoke<Binding[]>("controller_bindings").catch(() => FALLBACK),
    staleTime: Number.POSITIVE_INFINITY,
  });
  const merged: Binding[] = [];
  for (const b of q.data ?? FALLBACK) {
    const same = merged.find((m) => m.action === b.action);
    if (same) same.input = `${same.input} / ${b.input}`;
    else merged.push({ ...b });
  }
  return merged;
}
