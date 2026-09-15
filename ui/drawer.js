"use strict";
// ============================================================
//  SSH_CLI v3 — the panel: terminals (remote AND local), editor
//  tabs, image/plot viewers
// ============================================================

const terms = new Map();      // id -> terminal tab
const editors = new Map();    // key -> editor tab
const viewers = new Map();    // key -> image tab
const tabs = [];              // creation order, for cycling and Alt+N

let activeTerm = null;
let activeEditor = null;
let activeViewer = null;
let broadcast = false;
let edSeq = 1;
let viewSeq = 1;

// ---------------------------------------------------------------- tab registry
function registerTab(kind, key) {
  tabs.push({ kind, key });
  updateTermEmpty();
}
function unregisterTab(kind, key) {
  const i = tabs.findIndex((t) => t.kind === kind && t.key === key);
  if (i >= 0) tabs.splice(i, 1);
  updateTermEmpty();
  return i;
}
function activateTab(t) {
  if (!t) return;
  if (t.kind === "term") activateTerm(t.key);
  else if (t.kind === "ed") activateEditor(t.key);
  else activateViewer(t.key);
}
function activateNeighbour(i) {
  if (!tabs.length) {
    updateTermEmpty();
    return;
  }
  activateTab(tabs[Math.max(0, Math.min(i, tabs.length - 1))]);
}
function activeTabRef() {
  if (activeTerm !== null) return { kind: "term", key: activeTerm };
  if (activeEditor) return { kind: "ed", key: activeEditor };
  if (activeViewer) return { kind: "img", key: activeViewer };
  return null;
}
function cycleTab(delta) {
  if (tabs.length < 2) return;
  const cur = activeTabRef();
  const i = cur ? tabs.findIndex((t) => t.kind === cur.kind && t.key === cur.key) : -1;
  activateTab(tabs[(i + delta + tabs.length) % tabs.length]);
}
function closeActiveTab() {
  const cur = activeTabRef();
  if (!cur) return;
  if (cur.kind === "term") closeTerminal(cur.key);
  else if (cur.kind === "ed" && editors.has(cur.key)) editors.get(cur.key).close();
  else if (cur.kind === "img" && viewers.has(cur.key)) viewers.get(cur.key).close();
}

function deactivateDrawerTabs() {
  for (const m of [terms, editors, viewers]) {
    for (const [, t] of m) {
      t.slot.classList.remove("active");
      t.tab.classList.remove("active");
    }
  }
}

function updateTermEmpty() {
  const empty = tabs.length === 0;
  $("#term-empty").classList.toggle("hidden", !empty);
  if (!empty) return;
  const box = $("#te-actions");
  box.innerHTML = "";
  const b = el("button", "btn primary", "⌂  Local shell");
  b.onclick = () => openTerminal(null, activePane && !activePane.target ? activePane.path : null);
  box.appendChild(b);
  for (const s of sessions.slice(0, 6)) {
    const sb = el("button", "btn", (s.live ? "● " : "○ ") + s.name);
    sb.onclick = () => openTerminal(s.name);
    box.appendChild(sb);
  }
  const more = el("button", "btn ghost", "Sessions…");
  more.onclick = () => sessionManager();
  box.appendChild(more);
}

// ---------------------------------------------------------------- terminals
function fitTerm(id, t) {
  if (!t) return;
  try {
    t.fit.fit();
    inv("term_resize", { id, rows: t.term.rows, cols: t.term.cols }).catch(() => {});
  } catch (_) {}
}
function refitActiveTerm() {
  if (activeTerm !== null) fitTerm(activeTerm, terms.get(activeTerm));
}
function refitAllTerms() {
  refitActiveTerm();
  for (const [, ed] of editors) if (ed.cm) ed.cm.refresh();
}

function activateTerm(id) {
  deactivateDrawerTabs();
  activeEditor = null;
  activeViewer = null;
  activeTerm = id;
  const t = terms.get(id);
  if (!t) return;
  t.slot.classList.add("active");
  t.tab.classList.add("active");
  drawerShow(true);
  setTimeout(() => { fitTerm(id, t); t.term.focus(); }, 30);
}

function termLabel(t) {
  if (t.target) return t.target + (t.cwd ? " · " + baseName(t.cwd) : "");
  return "local" + (t.cwd ? " · " + baseName(t.cwd) : "");
}

/// Open a terminal. `target` null means a shell on this machine — same
/// tab strip, same fonts, no SSH hop. `cwd` starts it in a folder.
async function openTerminal(target, cwd) {
  if (typeof Terminal === "undefined") {
    return alertModal("Terminal unavailable",
      "xterm.js was not vendored. Run the build script once, then rebuild.");
  }
  drawerShow(true);
  const slot = el("div", "term-slot active");
  $("#term-holder").appendChild(slot);
  const term = new Terminal({
    fontFamily: MONO_FONT,
    fontSize: ui.settings.monoSize || 13,
    cursorBlink: true,
    scrollback: opt("scrollback", 8000),
    allowProposedApi: true,
    theme: termTheme(),
  });
  const fit = new FitAddon.FitAddon();
  term.loadAddon(fit);
  let search = null;
  if (typeof SearchAddon !== "undefined") {
    search = new SearchAddon.SearchAddon();
    term.loadAddon(search);
  }
  if (typeof WebLinksAddon !== "undefined") {
    term.loadAddon(new WebLinksAddon.WebLinksAddon((e, uri) => {
      if (navigator.clipboard) navigator.clipboard.writeText(uri);
      toast("link copied: " + uri);
    }));
  }
  if (typeof ImageAddon !== "undefined") {
    try { term.loadAddon(new ImageAddon.ImageAddon()); } catch (_) {}
  }
  term.open(slot);
  try { fit.fit(); } catch (_) {}

  // Remote plots ride on a shell prelude; local ones just need the env,
  // which the backend sets directly.
  let prelude = null;
  let plots = false;
  if (plotsOn()) {
    const w = await plotsWatch(target);
    if (w) {
      if (target) prelude = PLOT_PRELUDE;
      else plots = true;
    } else {
      toast("plots setup failed — this terminal opens without plot support", "error");
    }
  }

  const startDir = opt("cwdTerm", true) ? (cwd || null) : null;
  let id;
  try {
    id = await inv("term_open", {
      target, cwd: startDir, rows: term.rows, cols: term.cols, prelude, plots,
    });
  } catch (e) {
    slot.remove();
    return alertModal("Could not open terminal", String(e));
  }

  const rec = { term, fit, search, slot, target, cwd: startDir, id };
  const tab = el("div", "term-tab" + (target ? "" : " local"));
  const lbl = el("span", "tl", (target ? "" : "⌂ ") + termLabel(rec));
  tab.appendChild(lbl);
  const x = el("span", "x", "✕");
  tab.appendChild(x);
  $("#term-tab-list").appendChild(tab);
  rec.tab = tab;
  rec.lbl = lbl;

  tab.onclick = (e) => { if (e.button === 0) activateTerm(id); };
  tab.addEventListener("auxclick", (e) => { if (e.button === 1) closeTerminal(id); });
  tab.addEventListener("contextmenu", (e) => {
    e.preventDefault();
    ctxMenu([
      { label: "Duplicate", action: () => openTerminal(rec.target, rec.cwd) },
      { label: "Rename tab…", action: async () => {
        const r = await modal({ title: "Rename tab", fields: [{ key: "n", label: "Label", value: lbl.textContent }] });
        if (r && r.n) { lbl.textContent = r.n; rec.custom = true; }
      } },
      "-",
      { label: "Copy all output", action: () => {
        term.selectAll();
        copyText(term.getSelection(), "scrollback copied");
        term.clearSelection();
      } },
      { label: "Clear", action: () => term.clear() },
      "-",
      { label: "Close", hint: MOD + "W", danger: true, action: () => closeTerminal(id) },
      { label: "Close others", danger: true, action: () => {
        for (const other of [...terms.keys()]) if (other !== id) closeTerminal(other);
      } },
    ], e.clientX, e.clientY);
  });
  x.onclick = (e) => { e.stopPropagation(); closeTerminal(id); };

  const ro = new ResizeObserver(() => { if (activeTerm === id) fitTerm(id, terms.get(id)); });
  ro.observe(slot);
  rec.ro = ro;

  term.onData((data) => {
    if (broadcast) {
      for (const [tid, t] of terms) {
        if (!t.tab.classList.contains("dead")) inv("term_write", { id: tid, data }).catch(() => {});
      }
    } else {
      inv("term_write", { id, data }).catch(() => {});
    }
  });
  // Terminal-emulator conventions: Ctrl+Shift+C / V, and keep the rest
  // of Ctrl+… for the shell.
  term.attachCustomKeyEventHandler((ev) => {
    if (ev.type !== "keydown") return true;
    if (ev.ctrlKey && ev.shiftKey && ev.code === "KeyC") {
      const s = term.getSelection();
      if (s) { copyText(s, "copied"); return false; }
      return false;
    }
    if (ev.ctrlKey && ev.shiftKey && ev.code === "KeyV") {
      pasteIntoTerm(id);
      return false;
    }
    return true;
  });

  slot.addEventListener("contextmenu", (e) => {
    e.preventDefault();
    const sel = term.getSelection();
    ctxMenu([
      sel ? { label: "Copy", hint: "⌃⇧C", action: () => copyText(sel, "copied") } : null,
      { label: "Paste", hint: "⌃⇧V", action: () => pasteIntoTerm(id) },
      "-",
      { label: "Search…", hint: MOD + "F", action: () => toggleTermSearch(true) },
      { label: "Clear", action: () => term.clear() },
      { label: "Arm plt.show() here", action: () => armActiveTerminal() },
      "-",
      { label: currentLayout() === "term" ? "Exit full panel" : "Full terminal panel",
        hint: "⌃`", action: toggleTermMax },
      { label: "Close tab", danger: true, action: () => closeTerminal(id) },
    ].filter(Boolean), e.clientX, e.clientY);
  });

  terms.set(id, rec);
  registerTab("term", id);
  activateTerm(id);
  refreshSessionsSoon();
  if (target) watchConnect(target);
  return id;
}

async function pasteIntoTerm(id) {
  try {
    const text = await navigator.clipboard.readText();
    if (text) await inv("term_write", { id, data: text });
  } catch (_) {
    toast("clipboard unavailable — use the terminal's own paste", "error");
  }
}

/// Open a terminal for whatever a pane is pointing at, in its folder.
function openTerminalFor(pane) {
  return openTerminal(pane.target, pane.path);
}
function openTerminalSmart() {
  const p = activePane || left;
  return p ? openTerminalFor(p) : openTerminal(null);
}

function closeTerminal(id) {
  const t = terms.get(id);
  if (!t) return;
  inv("term_close", { id }).catch(() => {});
  t.ro.disconnect();
  t.term.dispose();
  t.slot.remove();
  t.tab.remove();
  terms.delete(id);
  const i = unregisterTab("term", id);
  if (activeTerm === id) {
    activeTerm = null;
    activateNeighbour(i);
  }
}

function newTerminalMenu(btn) {
  const p = activePane || left;
  const items = [];
  items.push({
    label: "Local shell",
    hint: p && !p.target ? shortPath(p.path, 1) : "home",
    action: () => openTerminal(null, p && !p.target ? p.path : null),
  });
  if (p && p.target) {
    items.push({ label: "Terminal on " + p.target, hint: shortPath(p.path, 1),
      action: () => openTerminalFor(p) });
  }
  if (sessions.length) items.push("-");
  for (const s of sessions) {
    items.push({
      label: (s.live ? "● " : "○ ") + s.name,
      hint: s.destination,
      action: () => openTerminal(s.name),
    });
  }
  items.push("-");
  items.push({ label: "Add session…", action: () => sessionManager() });
  menuUnder(btn, items);
}

function wireTerminalEvents() {
  listen("term-data", (ev) => {
    const t = terms.get(ev.payload.id);
    if (t) t.term.write(ev.payload.data);
  });
  listen("term-exit", (ev) => {
    const t = terms.get(ev.payload.id);
    if (!t) return;
    t.term.write("\r\n\x1b[90m[session closed — ⌘W to close this tab]\x1b[0m\r\n");
    t.tab.classList.add("dead");
    t.dead = true;
  });
}

// ---------------------------------------------------------------- term search
function toggleTermSearch(show) {
  const bar = $("#term-search");
  const want = show !== undefined ? show : bar.classList.contains("hidden");
  bar.classList.toggle("hidden", !want);
  if (want) $("#term-search-input").focus();
  else if (activeTerm !== null) {
    const t = terms.get(activeTerm);
    if (t) t.term.focus();
  }
}
function termFind(dir) {
  const q = $("#term-search-input").value;
  if (!q || activeTerm === null) return;
  const t = terms.get(activeTerm);
  if (!t || !t.search) return;
  if (dir < 0) t.search.findPrevious(q);
  else t.search.findNext(q);
}

// ---------------------------------------------------------------- font zoom
function setMonoSize(n) {
  ui.settings.monoSize = Math.max(8, Math.min(28, n));
  saveUi();
  applyAppearance();
  toast("font size " + ui.settings.monoSize);
}

// ---------------------------------------------------------------- image viewer
const IMG_MIME = {
  png: "image/png", jpg: "image/jpeg", jpeg: "image/jpeg", gif: "image/gif",
  webp: "image/webp", svg: "image/svg+xml", bmp: "image/bmp", ico: "image/x-icon",
};
const imgMime = (name) => IMG_MIME[(name.split(".").pop() || "").toLowerCase()] || null;

function activateViewer(key) {
  deactivateDrawerTabs();
  activeTerm = null;
  activeEditor = null;
  activeViewer = key;
  const v = viewers.get(key);
  if (!v) return;
  v.slot.classList.add("active");
  v.tab.classList.add("active");
  drawerShow(true);
}

async function openImageViewer(target, path) {
  for (const [key, v] of viewers) {
    if (v.target === target && v.path === path) {
      drawerShow(true);
      return activateViewer(key);
    }
  }
  status("loading image…");
  let b64, st;
  try {
    b64 = await inv("file_read_b64", { target, path });
    st = await inv("file_stat", { target, path }).catch(() => null);
  } catch (e) {
    status("");
    return alertModal("Cannot view " + baseName(path), String(e));
  }
  status("");
  drawerShow(true);

  const key = "img" + viewSeq++;
  const slot = el("div", "ed-slot active");
  const bar = el("div", "ed-bar");
  const fname = el("span", "fname", (target ? target + ":" : "") + baseName(path));
  const stLbl = el("span", "ed-status", "");
  const sp = el("span", "spacer");
  const btnFit = el("button", "btn tiny on", "fit");
  const btnFull = el("button", "btn tiny", "1:1");
  const btnAuto = el("button", "btn tiny toggled", "auto-refresh");
  btnAuto.title = "Reload automatically when the file changes on disk/server";
  const btnReload = el("button", "btn tiny", "↻");
  btnReload.title = "Reload now";
  const btnSave = el("button", "btn tiny", "copy path");
  btnSave.onclick = () => copyText(path, "path copied");
  bar.append(fname, stLbl, sp, btnFit, btnFull, btnAuto, btnReload, btnSave);
  const body = el("div", "img-body fit");
  const img = el("img");
  body.appendChild(img);
  slot.append(bar, body);
  $("#term-holder").appendChild(slot);

  const tab = el("div", "term-tab");
  tab.appendChild(el("span", "tl", "◍ " + baseName(path)));
  const x = el("span", "x", "✕");
  tab.appendChild(x);
  $("#term-tab-list").appendChild(tab);

  const v = {
    key, target, path, slot, tab, img,
    mtime: st ? st.mtime : 0, size: st ? st.size : 0,
    auto: true, timer: null, busy: false,
  };
  viewers.set(key, v);
  registerTab("img", key);

  const show = (data) => {
    img.src = `data:${imgMime(path) || "image/png"};base64,${data}`;
    const now = new Date();
    const pad = (n) => String(n).padStart(2, "0");
    stLbl.textContent =
      (v.size ? fmtSize(v.size) + " · " : "") +
      `${pad(now.getHours())}:${pad(now.getMinutes())}:${pad(now.getSeconds())}`;
  };
  show(b64);
  img.onload = () => {
    stLbl.textContent = `${img.naturalWidth}×${img.naturalHeight} · ` + stLbl.textContent;
  };

  const reload = async () => {
    if (v.busy) return;
    v.busy = true;
    try {
      const data = await inv("file_read_b64", { target, path });
      const s2 = await inv("file_stat", { target, path }).catch(() => null);
      if (s2) { v.mtime = s2.mtime; v.size = s2.size; }
      show(data);
    } catch (_) { /* transient (file mid-write) — next poll retries */ }
    v.busy = false;
  };

  v.timer = setInterval(async () => {
    if (!v.auto || v.busy) return;
    try {
      const s2 = await inv("file_stat", { target, path });
      if (s2.mtime !== v.mtime || s2.size !== v.size) {
        v.mtime = s2.mtime;
        v.size = s2.size;
        await reload();
        status("image updated: " + baseName(path));
      }
    } catch (_) {}
  }, 2000);

  btnFit.onclick = () => {
    body.classList.add("fit");
    btnFit.classList.add("on");
    btnFull.classList.remove("on");
  };
  btnFull.onclick = () => {
    body.classList.remove("fit");
    btnFull.classList.add("on");
    btnFit.classList.remove("on");
  };
  btnAuto.onclick = () => {
    v.auto = !v.auto;
    btnAuto.classList.toggle("toggled", v.auto);
  };
  btnReload.onclick = reload;

  v.close = () => {
    clearInterval(v.timer);
    viewers.delete(key);
    slot.remove();
    tab.remove();
    const i = unregisterTab("img", key);
    if (activeViewer === key) {
      activeViewer = null;
      activateNeighbour(i);
    }
  };
  tab.onclick = () => activateViewer(key);
  tab.addEventListener("auxclick", (e) => { if (e.button === 1) v.close(); });
  x.onclick = (e) => { e.stopPropagation(); v.close(); };

  activateViewer(key);
}

// ---------------------------------------------------------------- remote plots
const plotWatch = new Map();
const PLOT_EXPORT =
  'export PYTHONPATH="$HOME/.ssh_cli:$PYTHONPATH" MPLBACKEND="module://ssh_cli_mpl"';
const PLOT_ECHO = 'echo "[SSH_CLI] plots armed — plt.show() opens in the app"';
const PLOT_PRELUDE = PLOT_EXPORT + "; " + PLOT_ECHO;
const plotsOn = () => !!ui.settings.plots;

function armTerminal(id) {
  return inv("term_write", { id, data: PLOT_PRELUDE + "\n" });
}

async function plotPoll(target, w) {
  if (w.busy) return;
  w.busy = true;
  try {
    const list = target
      ? await inv("list_remote", { target, path: w.dir })
      : await inv("list_local", { path: w.dir });
    for (const e of list) {
      if (e.is_dir || !imgMime(e.name)) continue;
      if (w.seen.has(e.name)) continue;
      w.seen.add(e.name);
      if (w.ready) {
        openImageViewer(target, joinPath(w.dir, e.name));
        status(`figure from ${target || "local"} — ${e.name}`);
      }
    }
  } catch (_) { /* dir may not exist yet, or link briefly down */ }
  w.busy = false;
}

async function plotsWatch(target) {
  const key = target || "local";
  if (plotWatch.has(key)) return plotWatch.get(key);
  let info;
  try {
    info = await inv("plots_enable", { target });
  } catch (e) {
    await alertModal(
      "Could not set up plots on " + key,
      String(e) +
        "\n\nThe app installs a small matplotlib backend into ~/.ssh_cli on " +
        "the host, which needs a live connection. Open a terminal to " +
        (target || "the host") + " first (so the login completes), then " +
        "toggle plots again."
    );
    return null;
  }
  const w = { dir: info.dir, seen: new Set(), ready: false, busy: false, timer: null };
  plotWatch.set(key, w);
  await plotPoll(target, w);          // baseline
  w.ready = true;
  w.timer = setInterval(() => plotPoll(target, w), 2000);
  return w;
}

function plotsStopAll() {
  for (const [, w] of plotWatch) clearInterval(w.timer);
  plotWatch.clear();
}

async function plotsToggle() {
  ui.settings.plots = !ui.settings.plots;
  $("#btn-plots").classList.toggle("toggled", ui.settings.plots);
  saveUi();
  if (!ui.settings.plots) {
    plotsStopAll();
    return toast("remote plots off — reopen terminals to drop the setting");
  }
  status("setting up remote plots…");
  await plotsWatch(null);

  const live = [...terms].filter(([, t]) => t.target && !t.dead);
  const hosts = new Set(live.map(([, t]) => t.target));
  let ok = 0;
  for (const h of hosts) if (await plotsWatch(h)) ok++;

  if (live.length && ok) {
    const yes = await confirmModal(
      `Arm ${live.length} open terminal(s)?`,
      "Terminals only pick up the plot settings when they start, so the ones " +
      "already open won't show figures yet.\n\n" +
      "This types one line into each of them:\n\n  " + PLOT_EXPORT +
      "\n\nSkip it if a terminal is busy (in vim, or running a job) and " +
      "reopen that terminal instead.",
      "Arm them"
    );
    if (yes) {
      for (const [id] of live) await armTerminal(id).catch(() => {});
      return toast("plots ON — armed " + live.length + " terminal(s); try plt.show()");
    }
  }
  toast(hosts.size
    ? "plots ON — open a NEW terminal on the host, then plt.show()"
    : "plots ON — open a terminal, then just call plt.show()");
}

async function armActiveTerminal() {
  if (activeTerm === null || !terms.has(activeTerm)) {
    return alertModal("Arm plots", "Focus a terminal tab first.");
  }
  const t = terms.get(activeTerm);
  const w = await plotsWatch(t.target);
  if (!w) return;
  if (t.target) await armTerminal(activeTerm).catch(() => {});
  else await inv("term_write", { id: activeTerm, data: PLOT_PRELUDE + "\n" }).catch(() => {});
  ui.settings.plots = true;
  $("#btn-plots").classList.toggle("toggled", true);
  saveUi();
  toast("armed this terminal — run your script, plt.show() opens a tab");
}

// ---------------------------------------------------------------- editor
const CM_MODES = {
  py: "python", pyw: "python",
  sh: "text/x-sh", bash: "text/x-sh", zsh: "text/x-sh", slurm: "text/x-sh", sbatch: "text/x-sh",
  c: "text/x-csrc", h: "text/x-chdr", cpp: "text/x-c++src", hpp: "text/x-c++src",
  cc: "text/x-c++src", cu: "text/x-c++src",
  tex: "stex", sty: "stex", bib: "stex",
  yml: "yaml", yaml: "yaml",
  toml: "text/x-toml",
  rs: "rust",
  jl: "julia",
  js: "javascript", mjs: "javascript",
  json: { name: "javascript", json: true },
  md: "markdown", markdown: "markdown",
};
const cmMode = (name) => CM_MODES[(name.split(".").pop() || "").toLowerCase()] || null;

function activateEditor(key) {
  deactivateDrawerTabs();
  activeTerm = null;
  activeViewer = null;
  activeEditor = key;
  const ed = editors.get(key);
  if (!ed) return;
  ed.slot.classList.add("active");
  ed.tab.classList.add("active");
  drawerShow(true);
  setTimeout(() => { ed.cm.refresh(); ed.cm.focus(); }, 30);
}

async function openEditor(target, path) {
  if (typeof CodeMirror === "undefined") {
    return alertModal("Editor unavailable",
      "CodeMirror was not vendored. Run the build script once, then rebuild.");
  }
  for (const [key, ed] of editors) {
    if (ed.target === target && ed.path === path) {
      drawerShow(true);
      return activateEditor(key);
    }
  }
  status("opening…");
  let text;
  try {
    text = await inv("file_read", { target, path });
  } catch (e) {
    status("");
    if (target === null) return inv("local_open", { path }).catch(() => {});
    return alertModal("Cannot edit " + baseName(path), String(e) +
      "\n\nTip: use “open externally” for non-text files.");
  }
  status("");
  drawerShow(true);

  const key = "ed" + edSeq++;
  const slot = el("div", "ed-slot active");
  const bar = el("div", "ed-bar");
  const fname = el("span", "fname", (target ? target + ":" : "") + baseName(path));
  const st = el("span", "ed-status", "");
  const sp = el("span", "spacer");
  const btnSave = el("button", "btn tiny", "save " + MOD + "S");
  const btnSaveClose = el("button", "btn tiny", "save & close");
  const btnWrap = el("button", "btn tiny", "wrap");
  const btnExt = el("button", "btn tiny", "open externally");
  bar.append(fname, st, sp, btnSave, btnSaveClose, btnWrap, btnExt);
  const cmEl = el("div", "ed-cm");
  cmEl.style.fontSize = (ui.settings.monoSize || 13) + "px";
  slot.append(bar, cmEl);
  $("#term-holder").appendChild(slot);

  const cm = CodeMirror(cmEl, {
    value: text,
    mode: cmMode(path),
    theme: "sshcli",
    lineNumbers: true,
    indentUnit: 4,
    lineWrapping: false,
  });

  const tab = el("div", "term-tab");
  const tl = el("span", "tl", "✎ " + baseName(path));
  tab.appendChild(tl);
  const x = el("span", "x", "✕");
  tab.appendChild(x);
  $("#term-tab-list").appendChild(tab);

  const ed = { key, target, path, slot, tab, cm, cmEl, st, dirty: false, saving: false };
  editors.set(key, ed);
  registerTab("ed", key);

  const markDirty = (d) => {
    ed.dirty = d;
    tl.textContent = (d ? "• " : "✎ ") + baseName(path);
    st.textContent = d ? "modified" : "saved";
  };
  cm.on("change", () => { if (!ed.saving) markDirty(true); });
  st.textContent = "opened";

  ed.save = async () => {
    if (ed.saving) return;
    ed.saving = true;
    st.textContent = "saving…";
    try {
      await inv("file_write", { target: ed.target, path: ed.path, data: ed.cm.getValue() });
      markDirty(false);
      toast("saved " + baseName(ed.path));
    } catch (e) {
      st.textContent = "SAVE FAILED";
      alertModal("Save failed", String(e));
    }
    ed.saving = false;
  };
  ed.close = async () => {
    if (ed.dirty) {
      const ok = await confirmModal("Discard changes?",
        baseName(ed.path) + " has unsaved changes.", "Discard");
      if (!ok) return;
    }
    editors.delete(key);
    slot.remove();
    tab.remove();
    const i = unregisterTab("ed", key);
    if (activeEditor === key) {
      activeEditor = null;
      activateNeighbour(i);
    }
  };

  btnSave.onclick = () => ed.save();
  btnSaveClose.onclick = async () => { await ed.save(); if (!ed.dirty) ed.close(); };
  btnWrap.onclick = () => {
    const on = !cm.getOption("lineWrapping");
    cm.setOption("lineWrapping", on);
    btnWrap.classList.toggle("toggled", on);
  };
  btnExt.onclick = () => {
    if (ed.target === null) inv("local_open", { path: ed.path }).catch(() => {});
    else startEditExternal(ed.target, ed.path);
  };
  tab.onclick = () => activateEditor(key);
  tab.addEventListener("auxclick", (e) => { if (e.button === 1) ed.close(); });
  x.onclick = (e) => { e.stopPropagation(); ed.close(); };

  activateEditor(key);
}

// ---------------------------------------------------------------- external edit
async function startEditExternal(target, path) {
  status("opening for edit…");
  let r;
  try {
    r = await inv("edit_open", { target, path });
  } catch (e) { status(""); return alertModal("Edit failed", String(e)); }
  toast("editing " + baseName(path) + " — saving in your editor uploads automatically");
  $("#queue").classList.remove("hidden");
  const item = el("div", "edit-item");
  const lbl = el("span", "lbl", "✎ " + r.label);
  const st = el("span", "st", "watching");
  const stop = el("button", "btn tiny", "stop");
  stop.onclick = () => { inv("edit_stop", { id: r.id }).catch(() => {}); item.remove(); };
  item.append(lbl, st, stop);
  item.dataset.editId = r.id;
  $("#edit-items").appendChild(item);
}

function wireEditEvents() {
  listen("edit-status", (ev) => {
    const item = document.querySelector(`[data-edit-id="${ev.payload.id}"]`);
    if (!item) return;
    item.querySelector(".st").textContent = ev.payload.msg;
    if (!ev.payload.alive) item.style.opacity = "0.5";
    if (/uploaded/.test(ev.payload.msg)) status("remote file updated");
  });
}

// ---------------------------------------------------------------- resizers
function wireResizers() {
  const mid = $("#midbar");
  mid.addEventListener("mousedown", (e) => {
    if (e.target.closest("button")) return;
    e.preventDefault();
    const startX = e.clientX;
    const startW = left.root.getBoundingClientRect().width;
    const move = (ev) => {
      const w = Math.min(Math.max(startW + ev.clientX - startX, 240), window.innerWidth - 300);
      left.root.style.flex = "none";
      left.root.style.width = w + "px";
      ui.layoutState.splitW = w;
    };
    const up = () => {
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
      saveUi();
    };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
  });
  mid.addEventListener("dblclick", (e) => {
    if (e.target.closest("button")) return;
    left.root.style.flex = "";
    left.root.style.width = "";
    delete ui.layoutState.splitW;
    saveUi();
  });

  const grip = $("#drawer-grip");
  const drawer = $("#drawer");
  grip.addEventListener("mousedown", (e) => {
    e.preventDefault();
    const startY = e.clientY;
    const startH = drawer.getBoundingClientRect().height;
    const move = (ev) => {
      const h = Math.min(Math.max(startH + (startY - ev.clientY), 120), window.innerHeight * 0.85);
      drawer.style.height = h + "px";
      ui.layoutState.drawerH = h;
      refitActiveTerm();
    };
    const up = () => {
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
      saveUi();
    };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
  });
  grip.addEventListener("dblclick", () => toggleTermMax());
}

function wireDrawerButtons() {
  $("#btn-new-term").onclick = () => newTerminalMenu($("#btn-new-term"));
  $("#btn-maximize").onclick = () => toggleTermMax();
  $("#btn-broadcast").onclick = () => {
    broadcast = !broadcast;
    $("#btn-broadcast").classList.toggle("toggled", broadcast);
    toast(broadcast ? "broadcast ON — typing goes to every terminal" : "broadcast off");
  };
  $("#btn-arm-plots").onclick = () => armActiveTerminal();
  $("#ts-next").onclick = () => termFind(1);
  $("#ts-prev").onclick = () => termFind(-1);
  $("#ts-close").onclick = () => toggleTermSearch(false);
  $("#term-search-input").addEventListener("keydown", (e) => {
    if (e.key === "Enter") termFind(e.shiftKey ? -1 : 1);
    if (e.key === "Escape") toggleTermSearch(false);
  });
  // Horizontal scroll over the tab strip
  $("#term-tab-list").addEventListener("wheel", (e) => {
    if (e.deltaY) { $("#term-tab-list").scrollLeft += e.deltaY; e.preventDefault(); }
  }, { passive: false });
}
