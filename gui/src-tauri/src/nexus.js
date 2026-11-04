// Injected into every page of cpmm's Nexus browser window (main frame only).
// The page gets no Tauri IPC. It talks to cpmm only by navigating to cpmm://<action>, which the
// backend intercepts and cancels (see nexus.rs). The backend drives it through window.__cpmm.
(() => {
  if (window.top !== window || window.__cpmm) return;

  const signal = (action) => {
    try {
      window.location.href = "cpmm://" + action;
    } catch (_) {}
  };

  /* ---- UI in a shadow root so the site's CSS can't touch it (and ours can't touch the site) ---- */
  let root = null;
  let cursorEl = null;
  let toastsEl = null;
  const pending = [];
  function ui() {
    if (root) return root;
    if (!document.body) return null;
    const host = document.createElement("cpmm-overlay");
    host.style.cssText = "all:initial;position:fixed;inset:0;pointer-events:none;z-index:2147483647";
    root = host.attachShadow({ mode: "closed" });
    root.innerHTML = `
      <style>
        :host { all: initial; }
        * { box-sizing: border-box; font: 500 14px/1.3 system-ui, sans-serif; }
        .bar { position: fixed; left: 50%; bottom: 14px; transform: translateX(-50%); display: flex; gap: 6px; padding: 6px;
               border-radius: 14px; background: rgb(12 11 20 / .92); border: 1px solid rgb(34 211 238 / .35);
               box-shadow: 0 8px 30px rgb(0 0 0 / .5); pointer-events: auto; }
        .bar button { all: unset; cursor: pointer; color: #d6d3e6; padding: 8px 12px; border-radius: 9px; display: flex; gap: 6px; align-items: center; }
        .bar button:hover, .bar button.hover { background: rgb(34 211 238 / .15); color: #22d3ee; }
        .bar .done { background: #22d3ee; color: #07060c; font-weight: 650; }
        .bar .done:hover, .bar .done.hover { background: #67e8f9; color: #07060c; }
        .bar kbd { font-size: 11px; opacity: .65; }
        .toasts { position: fixed; right: 16px; top: 16px; display: flex; flex-direction: column; gap: 8px; width: 360px; }
        .toast { padding: 11px 14px; border-radius: 12px; color: #ece9f7; background: rgb(18 16 30 / .96);
                 border: 1px solid #3a3550; box-shadow: 0 8px 30px rgb(0 0 0 / .5); transition: opacity .3s; }
        .toast.ok { border-color: rgb(163 230 53 / .5); }
        .toast.error { border-color: rgb(248 113 113 / .6); }
        .toast.info { border-color: rgb(34 211 238 / .45); }
        .cursor { position: fixed; left: 0; top: 0; filter: drop-shadow(0 0 6px rgb(34 211 238 / .6)); transition: opacity .25s; opacity: 0; }
      </style>
      <div class="toasts"></div>
      <div class="bar">
        <button data-a="back" title="Back (B)">◀ <kbd>B</kbd></button>
        <button data-a="forward" title="Forward (RB)">▶</button>
        <button data-a="reload" title="Reload (Select)">⟳</button>
        <button data-a="home" title="Cyberpunk mods">Cyberpunk mods</button>
        <button data-a="keyboard" title="On-screen keyboard (Y)">⌨ <kbd>Y</kbd></button>
        <button data-a="close" class="done" title="Back to cpmm (Start)">Back to cpmm <kbd>Start</kbd></button>
      </div>
      <div class="cursor"><svg width="28" height="28" viewBox="0 0 24 24"><path d="M4 2 L4 19 L8.6 14.6 L11.6 21.5 L14.4 20.3 L11.4 13.5 L18 13.5 Z" fill="#22d3ee" stroke="#07060c" stroke-width="1.5" stroke-linejoin="round"/></svg></div>`;
    root.querySelector(".bar").addEventListener("click", (e) => {
      const b = e.target.closest("button");
      if (b) action(b.dataset.a);
    });
    cursorEl = root.querySelector(".cursor");
    toastsEl = root.querySelector(".toasts");
    document.documentElement.appendChild(host);
    while (pending.length) toast(...pending.shift());
    return root;
  }
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", ui, { once: true });
  else ui();
  window.addEventListener("load", () => signal("log?loaded " + encodeURIComponent(location.pathname)), { once: true });

  function toast(kind, text) {
    if (!ui()) {
      pending.push([kind, text]);
      return;
    }
    const el = document.createElement("div");
    el.className = "toast " + kind;
    el.textContent = text;
    toastsEl.appendChild(el);
    setTimeout(() => {
      el.style.opacity = "0";
      setTimeout(() => el.remove(), 400);
    }, kind === "error" ? 9000 : 5000);
  }

  function action(a) {
    if (a === "back") history.back();
    else if (a === "forward") history.forward();
    else if (a === "reload") location.reload();
    else if (a === "home") location.href = "https://www.nexusmods.com/games/cyberpunk2077/mods";
    else if (a === "keyboard" || a === "close") signal(a);
  }

  /* ---- virtual cursor (right stick / d-pad), clicks (A / RT / R3), scrolling (left stick) ---- */
  const cur = { x: innerWidth / 2, y: innerHeight / 2, hover: null, overlayHover: null, timer: 0 };
  const CLICKABLE = 'a[href], button, input, select, textarea, label, summary, [role="button"], [role="link"], [role="tab"], [role="menuitem"], [onclick], [tabindex]';

  function showCursor() {
    if (!ui()) return;
    cursorEl.style.opacity = "1";
    cursorEl.style.transform = `translate(${cur.x - 4}px, ${cur.y - 2}px)`;
    clearTimeout(cur.timer);
    cur.timer = setTimeout(() => (cursorEl.style.opacity = "0"), 6000);
  }

  /** Our own toolbar button under the cursor (the shadow root hides it from elementFromPoint). */
  function overlayButtonAt(x, y) {
    if (!root) return null;
    for (const b of root.querySelectorAll(".bar button")) {
      const r = b.getBoundingClientRect();
      if (x >= r.left && x <= r.right && y >= r.top && y <= r.bottom) return b;
    }
    return null;
  }

  function fire(el, type, x, y) {
    const Ctor = type.startsWith("pointer") ? PointerEvent : MouseEvent;
    el.dispatchEvent(new Ctor(type, { bubbles: true, cancelable: true, composed: true, clientX: x, clientY: y, view: window, button: 0, buttons: type.endsWith("down") ? 1 : 0, pointerType: "mouse", isPrimary: true }));
  }

  function hoverAt() {
    const ob = overlayButtonAt(cur.x, cur.y);
    if (cur.overlayHover !== ob) {
      cur.overlayHover?.classList.remove("hover");
      ob?.classList.add("hover");
      cur.overlayHover = ob;
    }
    const el = ob ? null : document.elementFromPoint(cur.x, cur.y);
    if (el !== cur.hover) {
      if (cur.hover) {
        fire(cur.hover, "pointerout", cur.x, cur.y);
        fire(cur.hover, "mouseout", cur.x, cur.y);
      }
      if (el) {
        fire(el, "pointerover", cur.x, cur.y);
        fire(el, "mouseover", cur.x, cur.y);
      }
      cur.hover = el;
    }
    if (el) fire(el, "mousemove", cur.x, cur.y);
  }

  function pointer(dx, dy, speed = 22) {
    cur.x = Math.min(innerWidth - 2, Math.max(0, cur.x + dx * speed));
    cur.y = Math.min(innerHeight - 2, Math.max(0, cur.y + dy * speed));
    showCursor();
    hoverAt();
  }

  function isText(el) {
    if (el instanceof HTMLTextAreaElement) return true;
    if (el instanceof HTMLInputElement) return !["button", "submit", "checkbox", "radio", "range", "color", "file", "image", "reset"].includes(el.type);
    return el?.isContentEditable === true;
  }

  function click() {
    showCursor();
    const ob = overlayButtonAt(cur.x, cur.y);
    if (ob) {
      action(ob.dataset.a);
      return;
    }
    const under = document.elementFromPoint(cur.x, cur.y);
    if (!under) return;
    const target = under.closest(CLICKABLE) || under;
    fire(target, "pointerdown", cur.x, cur.y);
    fire(target, "mousedown", cur.x, cur.y);
    if (isText(target)) {
      target.focus();
      signal("keyboard");
    }
    fire(target, "pointerup", cur.x, cur.y);
    fire(target, "mouseup", cur.x, cur.y);
    if (target instanceof HTMLSelectElement) target.focus();
    else if (!isText(target)) target.click();
  }

  function scrollable(el) {
    for (; el && el !== document.body && el !== document.documentElement; el = el.parentElement) {
      const oy = getComputedStyle(el).overflowY;
      if ((oy === "auto" || oy === "scroll") && el.scrollHeight > el.clientHeight + 4) return el;
    }
    return null;
  }

  function scroll(dx, dy) {
    const t = scrollable(document.elementFromPoint(cur.x, cur.y));
    (t || window).scrollBy({ left: dx * 36, top: dy * 36 });
  }

  function pad(a) {
    switch (a) {
      case "a":
      case "click":
        click();
        break;
      case "b":
      case "lb":
        history.back();
        break;
      case "rb":
        history.forward();
        break;
      case "start":
        signal("close");
        break;
      case "select":
        location.reload();
        break;
      case "y":
        signal("keyboard");
        break;
      case "up":
        pointer(0, -1, 28);
        break;
      case "down":
        pointer(0, 1, 28);
        break;
      case "left":
        pointer(-1, 0, 28);
        break;
      case "right":
        pointer(1, 0, 28);
        break;
    }
  }

  window.__cpmm = { pad, pointer: (dx, dy) => pointer(dx, dy), scroll, toast };
})();
