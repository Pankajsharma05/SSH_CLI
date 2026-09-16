"use strict";
// ============================================================
//  SSH_CLI v3 — core: state, helpers, chrome (modal/toast/theme/layout)
// ============================================================

const IS_MAC = /Mac/i.test(navigator.platform || "");
const IS_WIN = /Win/i.test(navigator.platform || "");
if (!IS_MAC) document.documentElement.classList.add("not-mac");
if (IS_WIN) document.documentElement.classList.add("is-win");

// ---------------------------------------------------------------- failure visibility
// A thrown error during start-up used to leave an empty window with no
// clue what happened — the worst possible thing to debug remotely. Show
// it on screen instead, and keep a copy the user can copy out.
let booted = false;
function markBooted() { booted = true; }

function fatal(err, where) {
  const text = (err && (err.stack || err.message)) || String(err);

  // Once the app is up, a single failed call is not a reason to replace
  // the interface with an error page — that turns a denied permission or
  // one unlucky request into what looks like a crash. Say it quietly and
  // carry on; only a failure during start-up, where there is nothing to
  // carry on with, takes the window.
  if (booted) {
    console.error(where || "error", err);
    try {
      toast(text.split("\n")[0].slice(0, 160), "error");
    } catch (_) {}
    return;
  }
  let box = document.getElementById("fatal");
  if (!box) {
    box = document.createElement("div");
    box.id = "fatal";
    document.body && document.body.appendChild(box);
  }
  if (!box.parentNode) return;
  box.innerHTML = "";
  const h = document.createElement("h2");
  h.textContent = "SSH_CLI hit an error during " + (where || "startup");
  const pre = document.createElement("pre");
  pre.textContent = text;
  const hint = document.createElement("p");
  hint.textContent =
    "Please send this text along with what you were doing. Select it and press " +
    (/Mac/i.test(navigator.platform || "") ? "Cmd" : "Ctrl") + "+C.";
  box.append(h, pre, hint);
  box.style.display = "block";
}

window.addEventListener("error", (e) => fatal(e.error || e.message, "startup"));
window.addEventListener("unhandledrejection", (e) => fatal(e.reason, "a background task"));

if (!window.__TAURI__ || !window.__TAURI__.core) {
  // The page loaded but the native bridge did not, so nothing can work.
  document.addEventListener("DOMContentLoaded", () =>
    fatal(new Error(
      "The Tauri bridge (window.__TAURI__) is missing, so the interface cannot " +
      "talk to the backend. This usually means the WebView2 runtime is too old " +
      "or the app was started from a broken copy."), "startup"));
}

const inv = (cmd, args) => window.__TAURI__.core.invoke(cmd, args || {});
const listen = window.__TAURI__.event.listen;

const $ = (sel) => document.querySelector(sel);
const $$ = (sel) => [...document.querySelectorAll(sel)];
const el = (tag, cls, text) => {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
};
const status = (msg) => { $("#statusbar").textContent = msg; };

const MOD = IS_MAC ? "⌘" : "Ctrl";
const MONO_FONT = '"JetBrains Mono Var", ui-monospace, Menlo, Consolas, "DejaVu Sans Mono", monospace';

const IC = {
  home: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M3 11l9-8 9 8"/><path d="M5 10v10h14V10"/></svg>',
  up: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 19V5M5 12l7-7 7 7"/></svg>',
  refresh: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21 12a9 9 0 1 1-2.64-6.36"/><path d="M21 3v6h-6"/></svg>',
  plus: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M12 5v14M5 12h14"/></svg>',
  trash: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M3 6h18M8 6V4h8v2M19 6l-1 14H6L5 6M10 11v6M14 11v6"/></svg>',
  star: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linejoin="round"><path d="M12 2l3.1 6.3 6.9 1-5 4.9 1.2 6.8L12 17.8 5.8 21l1.2-6.8-5-4.9 6.9-1z"/></svg>',
  find: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><circle cx="11" cy="11" r="7"/><path d="M21 21l-4.3-4.3"/></svg>',
  dots: '<svg viewBox="0 0 24 24" fill="currentColor"><circle cx="5" cy="12" r="1.7"/><circle cx="12" cy="12" r="1.7"/><circle cx="19" cy="12" r="1.7"/></svg>',
};

// ---------------------------------------------------------------- utils
// Paths come from two worlds at once: the remote pane is always POSIX
// (the servers are Linux), while the local pane is POSIX on macOS and
// Linux but `C:\Users\...` on Windows. Rather than thread a platform
// flag through every call site, these helpers infer the separator from
// the path itself — a drive letter or any backslash means Windows.
const winPath = (p) => /^[A-Za-z]:/.test(p) || p.includes("\\");
const sepOf = (p) => (winPath(p) ? "\\" : "/");
/// `C:\` and `/` are roots: they already end in their separator.
const isRoot = (p) => p === "/" || /^[A-Za-z]:[\\/]?$/.test(p);

const joinPath = (dir, name) => {
  const s = sepOf(dir);
  return /[\\/]$/.test(dir) ? dir + name : dir + s + name;
};
const parentPath = (p) => {
  if (isRoot(p)) return p;
  const s = sepOf(p);
  const q = p.replace(/[\\/]+$/, "");
  const i = Math.max(q.lastIndexOf("/"), q.lastIndexOf("\\"));
  if (i < 0) return q;
  if (winPath(q)) {
    // Stop at the drive root rather than producing a bare "C:".
    const head = q.slice(0, i);
    return /^[A-Za-z]:$/.test(head) ? head + s : head;
  }
  return i === 0 ? "/" : q.slice(0, i);
};
const baseName = (p) => {
  if (isRoot(p)) return p;
  return p.replace(/[\\/]+$/, "").split(/[\\/]/).pop() || "/";
};
const fmtSize = (n) => {
  if (n < 1024) return n + " B";
  const u = ["KB", "MB", "GB", "TB"];
  let i = -1;
  do { n /= 1024; i++; } while (n >= 1024 && i < u.length - 1);
  return n.toFixed(n >= 10 ? 0 : 1) + " " + u[i];
};
const fmtDate = (secs) => {
  if (!secs) return "";
  const d = new Date(secs * 1000);
  const pad = (x) => String(x).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
};
const debounce = (fn, ms) => {
  let t;
  return (...a) => { clearTimeout(t); t = setTimeout(() => fn(...a), ms); };
};
/// Shorten a long path for a tab/label: /very/deep/dir -> …/deep/dir
const shortPath = (p, keep = 2) => {
  const s = sepOf(p);
  const parts = p.split(/[\\/]/).filter(Boolean);
  if (parts.length <= keep) return p;
  return "…" + s + parts.slice(-keep).join(s);
};

// ---------------------------------------------------------------- shared state
let ui = { bookmarks: {}, recents: {}, history: [], settings: {}, layoutState: {} };
const saveUi = debounce(() => inv("ui_save", { data: JSON.stringify(ui) }).catch(() => {}), 500);
const saveUiNow = () => inv("ui_save", { data: JSON.stringify(ui) }).catch(() => {});

let left = null;              // pane objects, built at boot
let right = null;
let activePane = null;
let sessions = [];            // [{name, destination, port, jump, live}]
const hostFacts = new Map();  // target -> {user, hostname, home, scheduler}
const placesCache = new Map();// "local" | target -> [{label, path}]

const paneKey = (pane) => pane.target || "local";
const setOpt = (k, v) => { ui.settings[k] = v; saveUi(); };
const opt = (k, dflt) => (ui.settings[k] === undefined ? dflt : ui.settings[k]);

// ---------------------------------------------------------------- toasts
function toast(msg, kind) {
  const t = el("div", "toast" + (kind ? " " + kind : ""));
  t.appendChild(el("span", null, msg));
  $("#toasts").appendChild(t);
  const kill = () => {
    t.classList.add("out");
    setTimeout(() => t.remove(), 220);
  };
  t.onclick = kill;
  setTimeout(kill, kind === "error" ? 8000 : 4200);
  status(msg);
}

// ---------------------------------------------------------------- modal
function modal({ title, message, fields = [], okLabel = "OK", danger = false, body = null }) {
  return new Promise((resolve) => {
    const root = $("#modal-root");
    root.innerHTML = "";
    const box = el("div", "modal");
    box.appendChild(el("h3", null, title));
    if (message) box.appendChild(el("div", "msg", message));
    if (body) box.appendChild(body);
    const inputs = {};
    for (const f of fields) {
      const row = el("div", "row");
      row.appendChild(el("label", null, f.label));
      const inp = el("input");
      inp.value = f.value || "";
      if (f.placeholder) inp.placeholder = f.placeholder;
      inputs[f.key] = inp;
      row.appendChild(inp);
      box.appendChild(row);
    }
    const actions = el("div", "actions");
    const cancel = el("button", "btn", "Cancel");
    const ok = el("button", "btn primary" + (danger ? " danger" : ""), okLabel);
    actions.append(cancel, ok);
    box.appendChild(actions);
    root.appendChild(box);
    root.classList.remove("hidden");
    const close = (val) => { root.classList.add("hidden"); root.innerHTML = ""; resolve(val); };
    cancel.onclick = () => close(null);
    ok.onclick = () => {
      const out = {};
      for (const k in inputs) out[k] = inputs[k].value.trim();
      close(out);
    };
    const first = box.querySelector("input");
    if (first) { first.focus(); first.select(); }
    else ok.focus();
    box.addEventListener("keydown", (e) => {
      if (e.key === "Enter" && e.target.tagName === "INPUT") ok.click();
      if (e.key === "Escape") cancel.click();
    });
  });
}
const confirmModal = (title, message, okLabel = "Delete") =>
  modal({ title, message, okLabel, danger: true }).then((r) => r !== null);
const alertModal = (title, message) => modal({ title, message, okLabel: "OK" });

// ---------------------------------------------------------------- auth prompts
// The Windows transport speaks SSH in-process, so *it* has to ask for
// passwords, key passphrases and 2FA codes — there is no terminal for
// the server to prompt through. Rust raises `auth-prompt`, we answer
// with the `auth_reply` command. Challenges are queued: a server may ask
// twice (password, then one-time code) and the second question must not
// overwrite the first.
let authQueue = Promise.resolve();

/// Returns the listen() promise so boot can *await* it. Registering a
/// listener is itself an IPC round trip, and Tauri drops events that
/// arrive before it completes. The first thing the app does is load the
/// panes, which on Windows dials the cluster and asks for a password —
/// so an unawaited registration loses that very first prompt and the
/// pane waits on an answer that can never come.
function wireAuthPrompts() {
  return listen("auth-prompt", (ev) => {
    const p = ev.payload || {};
    authQueue = authQueue.then(() => askAuth(p)).catch(() => {});
  });
}

function askAuth(p) {
  return new Promise((resolve) => {
    const done = (text) => {
      closeModal();
      inv("auth_reply", { id: p.id, text }).catch(() => {});
      resolve();
    };

    const root = $("#modal-root");
    root.innerHTML = "";
    const box = el("div", "modal auth-modal");
    box.appendChild(el("h3", null, p.title || "Authentication"));
    if (p.target) box.appendChild(el("div", "auth-host", p.target));
    if (p.prompt) box.appendChild(el("div", "msg", p.prompt));

    let input = null;
    if (p.kind !== "hostkey") {
      const row = el("div", "row");
      input = el("input");
      // echo=false is the server telling us this is a secret.
      input.type = p.echo ? "text" : "password";
      input.autocomplete = "off";
      input.spellcheck = false;
      row.appendChild(input);
      box.appendChild(row);
    }

    const actions = el("div", "actions");
    const cancel = el("button", "btn", "Cancel");
    const ok = el("button", "btn primary",
      p.kind === "hostkey" ? "Trust this host" : "OK");
    cancel.onclick = () => done(null);
    ok.onclick = () => done(p.kind === "hostkey" ? "yes" : (input ? input.value : ""));
    actions.append(cancel, ok);
    box.appendChild(actions);

    root.appendChild(box);
    root.classList.remove("hidden");
    setTimeout(() => (input || ok).focus(), 30);
    box.addEventListener("keydown", (e) => {
      if (e.key === "Enter") { e.preventDefault(); ok.click(); }
      if (e.key === "Escape") { e.preventDefault(); cancel.click(); }
    });
  });
}
const modalOpen = () => !$("#modal-root").classList.contains("hidden");
const closeModal = () => {
  $("#modal-root").classList.add("hidden");
  $("#modal-root").innerHTML = "";
};

// ---------------------------------------------------------------- context menu
function ctxMenu(items, x, y) {
  $$(".ctx-menu").forEach((n) => n.remove());
  const m = el("div", "ctx-menu");
  for (const it of items) {
    if (!it) continue;
    if (it === "-") { m.appendChild(el("div", "ctx-sep")); continue; }
    const row = el("div", "ctx-item" + (it.danger ? " danger" : "") + (it.on ? " on" : ""));
    row.appendChild(el("span", null, it.label));
    if (it.hint) row.appendChild(el("span", "hint", it.hint));
    row.onclick = () => { m.remove(); cleanup(); it.action(); };
    m.appendChild(row);
  }
  document.body.appendChild(m);
  const r = m.getBoundingClientRect();
  m.style.left = Math.max(6, Math.min(x, window.innerWidth - r.width - 8)) + "px";
  m.style.top = Math.max(6, Math.min(y, window.innerHeight - r.height - 8)) + "px";
  const dismiss = (e) => { if (!m.contains(e.target)) { m.remove(); cleanup(); } };
  const onKey = (e) => { if (e.key === "Escape") { m.remove(); cleanup(); } };
  const cleanup = () => {
    window.removeEventListener("mousedown", dismiss, true);
    window.removeEventListener("keydown", onKey, true);
  };
  setTimeout(() => {
    window.addEventListener("mousedown", dismiss, true);
    window.addEventListener("keydown", onKey, true);
  }, 0);
  return m;
}
/// Drop a menu directly under a button.
function menuUnder(btn, items) {
  const r = btn.getBoundingClientRect();
  ctxMenu(items, r.left, r.bottom + 4);
}

const copyText = (t, msg) => {
  (navigator.clipboard ? navigator.clipboard.writeText(t) : Promise.reject())
    .then(() => toast(msg || "copied"))
    .catch(() => toast("clipboard unavailable", "error"));
};

// ---------------------------------------------------------------- appearance
const UI_SIZES = { s: "12.5px", m: "13.5px", l: "15px" };
const ACCENTS = {
  teal: "#34d3b6", blue: "#4c9dff", violet: "#a78bfa",
  amber: "#f5b855", rose: "#f27d98", emerald: "#4ade80", sky: "#38bdf8",
};

const hexRgb = (h) => {
  const m = /^#?([0-9a-f]{6})$/i.exec(h || "");
  if (!m) return null;
  const n = parseInt(m[1], 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
};
const shade = (h, f) => {
  const c = hexRgb(h) || [52, 211, 182];
  return "#" + c.map((v) => Math.round(v * f).toString(16).padStart(2, "0")).join("");
};
const alpha = (h, a) => {
  const c = hexRgb(h) || [52, 211, 182];
  return `rgba(${c[0]},${c[1]},${c[2]},${a})`;
};
function accentHex() {
  const st = ui.settings;
  if (st.accent === "custom" && hexRgb(st.accentCustom)) return st.accentCustom;
  return ACCENTS[st.accent] || ACCENTS.teal;
}
function resolvedTheme() {
  const t = ui.settings.theme || "dark";
  if (t !== "system") return t;
  return window.matchMedia && window.matchMedia("(prefers-color-scheme: light)").matches
    ? "light" : "dark";
}
function termTheme() {
  const light = resolvedTheme() === "light";
  return light
    ? { background: "#eef1f7", foreground: "#1a2230", cursor: accentHex(),
        selectionBackground: "rgba(45,115,210,.25)" }
    : { background: "#0a0d13", foreground: "#e2e8f4", cursor: accentHex(),
        selectionBackground: "rgba(52,130,220,.3)" };
}

function applyAppearance() {
  const st = ui.settings;
  const rootEl = document.documentElement;
  rootEl.dataset.theme = resolvedTheme();
  rootEl.dataset.accent = st.accent === "custom" ? "teal" : (st.accent || "teal");
  if (st.accent === "custom" && hexRgb(st.accentCustom)) {
    const a = st.accentCustom;
    rootEl.style.setProperty("--accent", a);
    rootEl.style.setProperty("--accent-dim", shade(a, 0.72));
    rootEl.style.setProperty("--accent-soft", alpha(a, 0.13));
  } else {
    rootEl.style.removeProperty("--accent");
    rootEl.style.removeProperty("--accent-dim");
    rootEl.style.removeProperty("--accent-soft");
  }
  rootEl.style.fontSize = UI_SIZES[st.uiSize || "m"];
  const mono = st.monoSize || 13;
  for (const [, t] of terms) {
    try {
      t.term.options.fontSize = mono;
      t.term.options.theme = termTheme();
    } catch (_) {}
  }
  refitActiveTerm();
  for (const [, ed] of editors) {
    ed.cmEl.style.fontSize = mono + "px";
    if (ed.cm) ed.cm.refresh();
  }
}

if (window.matchMedia) {
  window.matchMedia("(prefers-color-scheme: light)").addEventListener("change", () => {
    if ((ui.settings.theme || "dark") === "system") applyAppearance();
  });
}

// ---------------------------------------------------------------- layout modes
// files  — panes fill the window, no terminal
// split  — panes + terminal panel (classic)
// term   — the terminal panel IS the window
const LAYOUTS = ["files", "split", "term"];

function currentLayout() {
  return document.body.dataset.layout || "split";
}

function setLayout(mode, quiet) {
  if (!LAYOUTS.includes(mode)) mode = "split";
  document.body.dataset.layout = mode;
  ui.settings.layout = mode;
  saveUi();
  for (const b of $$("#layout-seg .btn")) b.classList.toggle("on", b.dataset.layout === mode);
  $("#btn-maximize").textContent = mode === "term" ? "⤡" : "⤢";
  $("#btn-maximize").title = mode === "term"
    ? "Back to split view (⌘2)" : "Full terminal panel (⌘3)";
  updateTermEmpty();
  // Terminals must re-measure whenever their box changes.
  setTimeout(refitAllTerms, 30);
  setTimeout(refitAllTerms, 220);
  if (!quiet && mode === "term" && !terms.size) openTerminalSmart();
}

function toggleTermMax() {
  setLayout(currentLayout() === "term" ? "split" : "term");
}

/// Anything that wants the terminal panel visible calls this.
function drawerShow(show) {
  if (show) {
    if (currentLayout() === "files") setLayout("split", true);
  } else if (currentLayout() !== "files") {
    setLayout("files", true);
  }
  setTimeout(refitAllTerms, 30);
}

// ---------------------------------------------------------------- settings
function settingsDialog() {
  const st = ui.settings;
  const body = el("div", "settings");

  const section = (t) => body.appendChild(el("div", "set-sec", t));
  const label = (t) => body.appendChild(el("div", "set-label", t));
  const segment = (options, get, set) => {
    const seg = el("div", "seg");
    const paint = () => seg.querySelectorAll(".btn").forEach((b) =>
      b.classList.toggle("on", b.dataset.val === String(get())));
    for (const [val, text] of options) {
      const b = el("button", "btn", text);
      b.dataset.val = String(val);
      b.onclick = () => { set(val); saveUi(); applyAppearance(); paint(); };
      seg.appendChild(b);
    }
    paint();
    body.appendChild(seg);
    return seg;
  };
  const toggle = (key, title, desc, dflt = false, onChange) => {
    const row = el("label", "set-toggle");
    const cb = el("input");
    cb.type = "checkbox";
    cb.checked = opt(key, dflt);
    cb.onchange = () => {
      ui.settings[key] = cb.checked;
      saveUi();
      if (onChange) onChange(cb.checked);
    };
    const txt = el("div");
    txt.appendChild(el("div", "t", title));
    if (desc) txt.appendChild(el("div", "d", desc));
    row.append(cb, txt);
    body.appendChild(row);
  };

  section("Appearance");
  label("Theme");
  segment([["dark", "dark"], ["light", "light"], ["system", "system"]],
    () => st.theme || "dark", (v) => { st.theme = v; });

  label("Accent color");
  const sw = el("div", "swatches");
  const markSel = () => {
    sw.querySelectorAll(".swatch").forEach((x) =>
      x.classList.toggle("sel", x.title === (st.accent || "teal")));
    picker.style.outline = st.accent === "custom" ? "2px solid var(--fg)" : "none";
  };
  for (const [name, color] of Object.entries(ACCENTS)) {
    const d = el("div", "swatch");
    d.style.background = color;
    d.title = name;
    d.onclick = () => { st.accent = name; saveUi(); applyAppearance(); markSel(); };
    sw.appendChild(d);
  }
  const picker = el("input");
  picker.type = "color";
  picker.title = "Custom accent — pick any color";
  picker.value = hexRgb(st.accentCustom) ? st.accentCustom : "#34d3b6";
  picker.oninput = () => {
    st.accent = "custom";
    st.accentCustom = picker.value;
    saveUi();
    applyAppearance();
    markSel();
  };
  sw.appendChild(picker);
  markSel();
  body.appendChild(sw);

  label("Interface size");
  segment([["s", "compact"], ["m", "default"], ["l", "large"]],
    () => st.uiSize || "m", (v) => { st.uiSize = v; });

  label("Terminal & editor font size");
  segment([11, 12, 13, 14, 15, 16].map((n) => [n, String(n)]),
    () => st.monoSize || 13, (v) => { st.monoSize = v; });

  section("Startup & panes");
  toggle("restore", "Restore panes on launch",
    "Reopen the hosts and folders you were browsing last time.", true);
  toggle("autoPane", "Show cluster home after login",
    "When a terminal login to a host succeeds, point a file pane at that host and open its home directory.", true);
  toggle("showHidden", "Show hidden files by default", "Dotfiles are listed in new panes.", false);
  toggle("confirmDelete", "Confirm before deleting", "Ask first — deletes are rm -rf and cannot be undone.", true);

  label("Double-click on a file");
  segment([["edit", "built-in editor"], ["system", "system app"]],
    () => opt("dblclick", "edit"), (v) => setOpt("dblclick", v));

  section("Terminal");
  label("Scrollback lines");
  segment([2000, 8000, 30000, 100000].map((n) => [n, n >= 1000 ? n / 1000 + "k" : String(n)]),
    () => opt("scrollback", 8000), (v) => setOpt("scrollback", v));
  body.appendChild(el("div", "set-note", "Applies to terminals you open from now on."));
  toggle("cwdTerm", "New terminals start in the pane's folder",
    "Opening a terminal from a pane cds into the folder you are looking at.", true);
  toggle("closeTabConfirm", "Confirm before closing a terminal tab", null, false);

  section("Transfers");
  toggle("rsync", "Prefer rsync", "Resumable, delta-sync transfers where both sides allow it.", false,
    (v) => $("#btn-rsync").classList.toggle("toggled", v));
  toggle("toastXfer", "Notify when transfers finish", null, true);

  modal({ title: "Preferences", body, okLabel: "Done" });
}

// ---------------------------------------------------------------- help
function helpDialog() {
  const body = el("div", "help-grid");
  const sec = (t) => body.appendChild(el("div", "help-sec", t));
  const kbd = (s) => `<kbd>${s}</kbd>`;
  const row = (keys, desc) => {
    const k = el("div", "k");
    k.innerHTML = keys;
    body.append(k, el("div", "d", desc));
  };

  sec("Everywhere");
  row(kbd(MOD) + kbd("K"), "Command palette — every action and host");
  row(kbd(MOD) + kbd("1") + " / " + kbd("2") + " / " + kbd("3"), "Layout: files · split · full terminal");
  row(kbd("Ctrl") + kbd("`"), "Toggle the full terminal panel");
  row(kbd(MOD) + kbd("T"), "New terminal on the active pane's host");
  row(kbd(MOD) + kbd("W"), "Close the active terminal / editor tab");
  row(kbd("Ctrl") + kbd("Tab"), "Next terminal tab");
  row(kbd("Alt") + kbd("1…9"), "Jump to terminal tab");
  row(kbd("?"), "This help · " + kbd("Esc") + " closes dialogs");

  sec("Files");
  row(kbd("↑") + kbd("↓"), "Move the cursor · " + kbd("Shift") + " extends the selection");
  row(kbd("↵"), "Open folder / edit file");
  row(kbd("⌫"), "Parent folder · " + kbd(MOD) + kbd("↑") + " too");
  row("type letters", "Jump to the matching name");
  row(kbd("Tab"), "Switch the active pane");
  row(kbd("F5"), "Copy selection to the other pane");
  row(kbd("F2"), "Rename · " + kbd("F7") + " new folder · " + kbd("Del") + " delete");
  row(kbd(MOD) + kbd("A"), "Select all · " + kbd(MOD) + kbd("R") + " refresh");
  row(kbd(MOD) + kbd("L"), "Edit the path · " + kbd(MOD) + kbd("H") + " home");
  row("drag", "Copy files to the other pane (Esc cancels)");
  row("right-click", "Context menu everywhere");

  sec("Terminal");
  row(kbd("Ctrl") + kbd("Shift") + kbd("C") + " / " + kbd("V"), "Copy / paste");
  row(kbd(MOD) + kbd("F"), "Search the scrollback");
  row(kbd(MOD) + kbd("+") + " / " + kbd("−") + " / " + kbd("0"), "Font size");
  row("Local", "A terminal on this machine — same tabs, no SSH hop");

  sec("Editor & plots");
  row(kbd(MOD) + kbd("S"), "Save the active editor tab");
  row("plots", "Toggle, then plt.show() opens a figure tab — no X11, no savefig");

  modal({ title: "Keyboard shortcuts", body, okLabel: "Close" });
}
