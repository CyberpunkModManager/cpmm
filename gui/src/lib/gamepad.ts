import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useEffect } from "react";

/**
 * Spatial navigation for handheld use (ROG Ally / Steam Deck in Game Mode).
 * - Arrow keys or D-pad / left stick: move focus to the nearest control in that direction
 * - Enter / A: activate; Escape / B: "back" (closes drawers & dialogs)
 * - LB / RB or [ / ]: previous / next section
 * - Select or ?: the control map (lib/controls.ts lists every binding)
 * Elements opt in by being focusable (buttons, inputs, [tabindex]).
 */

type Dir = "up" | "down" | "left" | "right";

const FOCUSABLE = 'button:not([disabled]), [href], input:not([disabled]), select, textarea, [tabindex]:not([tabindex="-1"])';

function visible(el: HTMLElement) {
  const r = el.getBoundingClientRect();
  if (r.width === 0 || r.height === 0) return false;
  const style = getComputedStyle(el);
  return style.visibility !== "hidden" && style.display !== "none";
}

/** Restrict navigation to the topmost open layer (dialog/drawer) if there is one. */
function scope(): ParentNode {
  const layers = document.querySelectorAll<HTMLElement>("[data-nav-layer]");
  return layers.length ? layers[layers.length - 1] : document;
}

/** "pointer" after touch/mouse, "pad" after controller/keyboard navigation. */
export function setInputMode(mode: "pointer" | "pad") {
  if (document.documentElement.dataset.input !== mode) document.documentElement.dataset.input = mode;
}

function inViewport(el: HTMLElement) {
  const r = el.getBoundingClientRect();
  return r.bottom > 0 && r.top < window.innerHeight && r.right > 0 && r.left < window.innerWidth;
}

/** Focus the control nearest the middle of the screen (where the user was looking). */
function focusNearestCenter(items: HTMLElement[]) {
  const cx = window.innerWidth / 2;
  const cy = window.innerHeight / 2;
  const onScreen = items.filter(inViewport);
  const pool = onScreen.length ? onScreen : items;
  let best = pool[0];
  let bd = Infinity;
  for (const el of pool) {
    const r = el.getBoundingClientRect();
    const d = Math.hypot(r.left + r.width / 2 - cx, r.top + r.height / 2 - cy);
    if (d < bd) {
      bd = d;
      best = el;
    }
  }
  best.focus({ preventScroll: true });
  best.scrollIntoView({ block: "nearest", inline: "nearest" });
}

export function move(dir: Dir): string {
  const items = Array.from(scope().querySelectorAll<HTMLElement>(FOCUSABLE)).filter(visible);
  if (!items.length) return "no focusable items";
  const cur = document.activeElement as HTMLElement | null;
  // After a touch the focused element may be off-screen, inside a closed layer, or <body>.
  if (!cur || cur === document.body || !items.includes(cur) || !inViewport(cur)) {
    focusNearestCenter(items);
    return `refocus (was ${cur?.tagName ?? "null"})`;
  }
  const a = cur.getBoundingClientRect();
  const ax = a.left + a.width / 2;
  const ay = a.top + a.height / 2;
  let best: HTMLElement | null = null;
  let bestScore = Infinity;
  for (const el of items) {
    if (el === cur) continue;
    const b = el.getBoundingClientRect();
    const bx = b.left + b.width / 2;
    const by = b.top + b.height / 2;
    const dx = bx - ax;
    const dy = by - ay;
    const along = dir === "left" ? -dx : dir === "right" ? dx : dir === "up" ? -dy : dy;
    if (along <= 4) continue;
    const across = dir === "left" || dir === "right" ? Math.abs(dy) : Math.abs(dx);
    const score = along + across * 2.5;
    if (score < bestScore) {
      bestScore = score;
      best = el;
    }
  }
  if (best) {
    best.focus({ preventScroll: true });
    best.scrollIntoView({ block: "nearest", inline: "nearest" });
    return "moved";
  }
  return "edge";
}

/* ---- left stick: analog scrolling ---- */
function scrollTarget(): HTMLElement | null {
  const layer = scope();
  const root = layer instanceof HTMLElement ? layer : document.querySelector("main");
  if (!root) return null;
  const all = [root as HTMLElement, ...Array.from((root as HTMLElement).querySelectorAll<HTMLElement>("*"))];
  return (
    all.find((el) => {
      const oy = getComputedStyle(el).overflowY;
      return (oy === "auto" || oy === "scroll") && el.scrollHeight > el.clientHeight + 4;
    }) ?? null
  );
}
function scrollBy(dx: number, dy: number) {
  const t = scrollTarget();
  if (t) t.scrollBy({ top: dy * 32, left: dx * 32 });
}

/* ---- right stick: virtual cursor (gamescope-safe, drawn by the page) ---- */
const cursor = { x: window.innerWidth / 2, y: window.innerHeight / 2, shown: false, timer: 0, hover: null as HTMLElement | null };
function cursorEl(): HTMLElement {
  let el = document.getElementById("vcursor");
  if (!el) {
    el = document.createElement("div");
    el.id = "vcursor";
    el.innerHTML = `<svg width="28" height="28" viewBox="0 0 24 24"><path d="M4 2 L4 19 L8.6 14.6 L11.6 21.5 L14.4 20.3 L11.4 13.5 L18 13.5 Z" fill="#22d3ee" stroke="#07060c" stroke-width="1.5" stroke-linejoin="round"/></svg>`;
    Object.assign(el.style, {
      position: "fixed",
      left: "0",
      top: "0",
      zIndex: "9999",
      pointerEvents: "none",
      filter: "drop-shadow(0 0 6px rgb(34 211 238 / .6))",
      transition: "opacity .25s",
      opacity: "0",
    });
    document.body.appendChild(el);
  }
  return el;
}
function setHover(el: HTMLElement | null) {
  if (cursor.hover === el) return;
  cursor.hover?.classList.remove("vc-hover");
  el?.classList.add("vc-hover");
  cursor.hover = el;
}
function showCursor(show: boolean) {
  cursor.shown = show;
  cursorEl().style.opacity = show ? "1" : "0";
  if (!show) setHover(null);
  window.clearTimeout(cursor.timer);
  if (show) cursor.timer = window.setTimeout(() => showCursor(false), 4000);
}
function moveCursor(dx: number, dy: number) {
  const sp = 22;
  cursor.x = Math.min(window.innerWidth - 2, Math.max(0, cursor.x + dx * sp));
  cursor.y = Math.min(window.innerHeight - 2, Math.max(0, cursor.y + dy * sp));
  cursorEl().style.transform = `translate(${cursor.x - 4}px, ${cursor.y - 2}px)`;
  showCursor(true);
  const under = document.elementFromPoint(cursor.x, cursor.y) as HTMLElement | null;
  setHover((under?.closest(FOCUSABLE) as HTMLElement | null) ?? null);
}
function clickAtCursor(): string {
  if (!cursor.shown) {
    cursorEl().style.transform = `translate(${cursor.x - 4}px, ${cursor.y - 2}px)`;
    showCursor(true);
    return "cursor shown";
  }
  showCursor(true);
  const under = document.elementFromPoint(cursor.x, cursor.y) as HTMLElement | null;
  if (!under) return "nothing under cursor";
  const target = (under.closest(FOCUSABLE) as HTMLElement | null) ?? under;
  if (target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement || target instanceof HTMLSelectElement) target.focus();
  else target.click();
  return `clicked ${target.tagName}`;
}

export function useSpatialNav(onSection: (delta: number) => void, onBack: () => void, onStart: () => void = () => {}, onSelect: () => void = () => {}) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement;
      const typing = t instanceof HTMLInputElement || t instanceof HTMLTextAreaElement;
      const map: Record<string, Dir> = { ArrowUp: "up", ArrowDown: "down", ArrowLeft: "left", ArrowRight: "right" };
      if (map[e.key] && !(typing && (e.key === "ArrowLeft" || e.key === "ArrowRight"))) {
        e.preventDefault();
        setInputMode("pad");
        move(map[e.key]);
      } else if (e.key === "Escape") {
        onBack();
      } else if (!typing && (e.key === "[" || e.key === "]")) {
        onSection(e.key === "]" ? 1 : -1);
      } else if (!typing && e.key === "?") {
        onSelect();
      }
    };
    window.addEventListener("keydown", onKey);

    // Controller input arrives from the Rust backend (evdev), see spawn_gamepad in main.rs.
    const onPointer = () => setInputMode("pointer");
    window.addEventListener("pointerdown", onPointer, true);
    const unScroll = listen<{ dx: number; dy: number }>("scroll", (e) => scrollBy(e.payload.dx, e.payload.dy));
    const unPointer = listen<{ dx: number; dy: number }>("pointer", (e) => {
      setInputMode("pointer");
      moveCursor(e.payload.dx, e.payload.dy);
    });
    window.addEventListener("mousemove", onPointer, true);
    let logged = 0;
    const unlisten = listen<string>("gamepad", (e) => {
      const a = e.payload;
      const wasPointer = document.documentElement.dataset.input === "pointer";
      setInputMode("pad");
      let what = "";
      if (a === "up" || a === "down" || a === "left" || a === "right") {
        if (cursor.shown) showCursor(false);
        what = move(a);
      } else if (a === "click") {
        setInputMode("pointer");
        what = clickAtCursor();
      } else if (a === "start") onStart();
      else if (a === "select") onSelect();
      else if (a === "a") {
        const el = document.activeElement as HTMLElement | null;
        if (el && el !== document.body && !wasPointer) {
          el.click();
          what = `click ${el.tagName}`;
        } else what = move("down");
      } else if (a === "b") onBack();
      else if (a === "lb") onSection(-1);
      else if (a === "rb") onSection(1);
      if (logged++ < 300) {
        const el = document.activeElement as HTMLElement | null;
        invoke("gui_log", {
          msg: `${a}${wasPointer ? " (after pointer)" : ""} -> ${what || "ok"}; focus=${el?.tagName}${el?.getAttribute("aria-label") ? `[${el.getAttribute("aria-label")}]` : ""} visible=${document.visibilityState} hasFocus=${document.hasFocus()}`,
        }).catch(() => {});
      }
    });
    return () => {
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("pointerdown", onPointer, true);
      window.removeEventListener("mousemove", onPointer, true);
      unlisten.then((f) => f());
      unScroll.then((f) => f());
      unPointer.then((f) => f());
    };
  }, [onSection, onBack, onStart, onSelect]);
}
