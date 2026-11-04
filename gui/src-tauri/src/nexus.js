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
        .bar button:hover { background: rgb(34 211 238 / .15); color: #22d3ee; }
        .bar .done { background: #22d3ee; color: #07060c; font-weight: 650; }
        .bar .done:hover { background: #67e8f9; color: #07060c; }
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

  /* ---- cursor ----
   * The backend owns the cursor position and sends WebKit real pointer events (motion, clicks,
   * scrolling) at it, so pages see genuine user gestures: popups, nxm:// navigations and modal
   * buttons all work. The page only draws the cursor. */
  let cursorTimer = 0;
  let lastPadClick = 0;
  function cursorAt(x, y) {
    if (!ui()) return;
    cursorEl.style.opacity = "1";
    cursorEl.style.transform = `translate(${x - 4}px, ${y - 2}px)`;
    clearTimeout(cursorTimer);
    cursorTimer = setTimeout(() => (cursorEl.style.opacity = "0"), 6000);
  }

  function isText(el) {
    if (el instanceof HTMLTextAreaElement) return true;
    if (el instanceof HTMLInputElement) return !["button", "submit", "checkbox", "radio", "range", "color", "file", "image", "reset"].includes(el.type);
    return el?.isContentEditable === true;
  }
  // A pad click that lands in a text field asks for Steam's on-screen keyboard (page autofocus
  // doesn't, so the keyboard only appears when you picked the field).
  document.addEventListener(
    "focusin",
    (e) => {
      if (Date.now() - lastPadClick < 800 && isText(e.target)) signal("keyboard");
    },
    true,
  );

  // Diagnostics for gui.log: are clicks reaching the page as real (trusted) input?
  let logged = 0;
  document.addEventListener(
    "click",
    (e) => {
      if (logged++ >= 5) return;
      const t = e.target;
      const what = (t.tagName || "?") + (t.id ? "#" + t.id : "") + (t.textContent ? " '" + t.textContent.trim().slice(0, 30) + "'" : "");
      signal("log?" + encodeURIComponent(`click trusted=${e.isTrusted} activation=${navigator.userActivation ? navigator.userActivation.isActive : "?"} on ${what}`));
    },
    true,
  );

  function pad(a) {
    switch (a) {
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
    }
  }

  window.__cpmm = { pad, toast, cursorAt, padClick: () => (lastPadClick = Date.now()) };
})();
