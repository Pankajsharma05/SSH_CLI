"use strict";
// ============================================================
//  SSH_CLI v3 — sessions, jobs, command palette, keymap, boot
// ============================================================

// ---------------------------------------------------------------- sessions
let refreshTimer = null;
function refreshSessionsSoon() {
  clearTimeout(refreshTimer);
  refreshTimer = setTimeout(refreshSessions, 800);
}

async function refreshSessions() {
  try {
    sessions = await inv("sessions_list");
    paneHostOptions(left);
    paneHostOptions(right);
    updateTermEmpty();
  } catch (e) {
    status("sessions: " + e);
  }
}

async function sessionManager() {
  await refreshSessions();
  let fwds = [];
  try { fwds = await inv("fwd_list"); } catch (_) {}
  const root = $("#modal-root");
  root.innerHTML = "";
  const box = el("div", "modal wide");
  box.appendChild(el("h3", null, "Sessions"));
  box.appendChild(el("div", "msg",
    "Hosts you can browse and open terminals on. A filled dot means the " +
    "multiplexed connection is live — file operations on that host need no " +
    "further authentication."));

  const close = () => { root.classList.add("hidden"); root.innerHTML = ""; };

  const list = el("div");
  const render = () => {
    list.innerHTML = "";
    if (!sessions.length) list.appendChild(el("div", "msg", "No saved sessions yet."));
    for (const s of sessions) {
      const row = el("div", "sess-row");
      row.appendChild(el("span", "dot " + (s.live ? "live" : "idle"), "●"));
      row.appendChild(el("strong", null, s.name));
      row.appendChild(el("span", "dest",
        s.destination + (s.port ? ":" + s.port : "") + (s.jump ? "  via " + s.jump : "")));
      const term = el("button", "btn tiny mono", ">_");
      term.title = "Open terminal";
      term.onclick = () => { close(); openTerminal(s.name); };
      const browseL = el("button", "btn tiny", "◧");
      browseL.title = "Browse in the left pane";
      browseL.onclick = () => { close(); switchHost(left, s.name); };
      const browseR = el("button", "btn tiny", "◨");
      browseR.title = "Browse in the right pane";
      browseR.onclick = () => { close(); switchHost(right, s.name); };
      const fwd = el("button", "btn tiny", "fwd+");
      fwd.title = "Add a port forward on this host";
      fwd.onclick = async () => {
        close();
        const r = await modal({
          title: "Port forward on " + s.name,
          message: "L = local forward (e.g. Jupyter on the cluster → this machine)\n" +
                   "R = remote forward · D = SOCKS proxy",
          fields: [
            { key: "kind", label: "Type (L, R, or D)", value: "L" },
            { key: "spec", label: "Spec", placeholder: "8888:localhost:8888   (for D: just a port, e.g. 1080)" },
          ],
          okLabel: "Start",
        });
        if (r && r.kind && r.spec) {
          try {
            await inv("fwd_start", { target: s.name, kind: r.kind.toUpperCase(), spec: r.spec });
            toast(`forward ${r.kind.toUpperCase()} ${r.spec} started on ${s.name}`);
          } catch (e) { await alertModal("Forward failed", String(e)); }
        }
        sessionManager();
      };
      const edit = el("button", "btn tiny", "edit");
      edit.onclick = () => { close(); sessionDialog(s); };
      const disc = el("button", "btn tiny", s.live ? "disconnect" : "—");
      disc.disabled = !s.live;
      disc.onclick = async () => {
        await inv("master_close", { target: s.name });
        await refreshSessions();
        render();
      };
      const del = el("button", "btn tiny danger", "remove");
      del.onclick = async () => {
        if (!(await confirmModal("Remove session?", s.name, "Remove"))) return;
        await inv("session_remove", { name: s.name });
        await refreshSessions();
        render();
      };
      row.append(term, browseL, browseR, fwd, edit, disc, del);
      list.appendChild(row);
    }
    if (fwds.length) {
      list.appendChild(el("div", "msg", "Active port forwards:"));
      for (const f of fwds) {
        const row = el("div", "sess-row");
        row.appendChild(el("span", "dot live", "●"));
        row.appendChild(el("span", "dest", f.label));
        const stop = el("button", "btn tiny danger", "stop");
        stop.onclick = async () => {
          await inv("fwd_stop", { id: f.id }).catch(() => {});
          sessionManager();
        };
        row.appendChild(stop);
        list.appendChild(row);
      }
    }
  };
  render();
  box.appendChild(list);

  const actions = el("div", "actions");
  const imp = el("button", "btn", "Import ~/.ssh/config…");
  const add = el("button", "btn primary", "Add session…");
  const done = el("button", "btn", "Done");
  actions.append(imp, add, done);
  box.appendChild(actions);
  root.appendChild(box);
  root.classList.remove("hidden");
  done.onclick = close;

  imp.onclick = async () => {
    close();
    let hosts = [];
    try { hosts = await inv("ssh_config_import"); } catch (_) {}
    const existing = new Set(sessions.map((s) => s.name));
    hosts = hosts.filter((h) => !existing.has(h.name));
    if (!hosts.length) {
      await alertModal("Import", "Nothing new to import from ~/.ssh/config.");
      return sessionManager();
    }
    const body = el("div");
    const checks = [];
    for (const h of hosts) {
      const row = el("label", "sess-row");
      const cb = el("input");
      cb.type = "checkbox";
      cb.checked = true;
      checks.push([cb, h]);
      row.appendChild(cb);
      row.appendChild(el("strong", null, h.name));
      row.appendChild(el("span", "dest",
        (h.user ? h.user + "@" : "") + h.host + (h.port ? ":" + h.port : "")));
      body.appendChild(row);
    }
    const r = await modal({ title: "Import hosts from ~/.ssh/config", body, okLabel: "Import selected" });
    if (r !== null) {
      let n = 0;
      for (const [cb, h] of checks) {
        if (!cb.checked) continue;
        try {
          await inv("session_add", {
            name: h.name,
            destination: (h.user ? h.user + "@" : "") + h.host,
            port: h.port || null,
            identity: h.identity || null,
            jump: h.jump || null,
            startup: null,
          });
          n++;
        } catch (_) {}
      }
      toast(`imported ${n} session(s)`);
      await refreshSessions();
    }
    sessionManager();
  };

  add.onclick = () => { close(); sessionDialog(null); };
}

async function sessionDialog(existing) {
  const r = await modal({
    title: existing ? "Edit session " + existing.name : "Add session",
    message: "A session is a name for a host. Everything else is optional — " +
             "port, key, bastion, and a command to run each time a terminal opens " +
             "(module loads, cd into scratch…).",
    fields: [
      { key: "name", label: "Name", placeholder: "paramganga", value: existing ? existing.name : "" },
      { key: "destination", label: "Destination", placeholder: "user@host.example.ac.in",
        value: existing ? existing.destination : "" },
      { key: "port", label: "Port (optional)", value: existing && existing.port ? String(existing.port) : "" },
      { key: "identity", label: "Private key path (optional)", placeholder: "~/.ssh/id_ed25519" },
      { key: "jump", label: "ProxyJump bastion (optional)", placeholder: "user@gateway",
        value: existing && existing.jump ? existing.jump : "" },
      { key: "startup", label: "Startup command (optional)", placeholder: "cd /scratch/$USER && module load gcc" },
    ],
    okLabel: "Save",
  });
  if (r && r.name && r.destination) {
    try {
      await inv("session_add", {
        name: r.name,
        destination: r.destination,
        port: r.port ? parseInt(r.port, 10) : null,
        identity: r.identity || null,
        jump: r.jump || null,
        startup: r.startup || null,
      });
      await refreshSessions();
      toast("saved session " + r.name);
    } catch (e) { alertModal("Could not save session", String(e)); }
  }
  sessionManager();
}

// ---------------------------------------------------------------- jobs panel
let jobsTimer = null;

function jobsShow(show) {
  $("#jobs").classList.toggle("hidden", !show);
  $("#btn-jobs").classList.toggle("toggled", show);
  clearInterval(jobsTimer);
  if (show) {
    fillJobsHosts();
    refreshJobs();
    jobsTimer = setInterval(refreshJobs, 15000);
  }
}

function fillJobsHosts() {
  const sel = $("#jobs-host");
  sel.innerHTML = "";
  for (const s of sessions) {
    const o = el("option", null, s.name);
    o.value = s.name;
    sel.appendChild(o);
  }
  const prefer = ui.settings.jobsHost ||
    [left, right].map((p) => p.target).find(Boolean) ||
    (sessions[0] && sessions[0].name);
  if (prefer && sessions.some((s) => s.name === prefer)) sel.value = prefer;
}

async function refreshJobs() {
  const host = $("#jobs-host").value;
  const body = $("#jobs-body");
  if (!host) { body.textContent = "no saved sessions"; return; }
  const facts = hostFacts.get(host);
  if (facts && facts.scheduler === "pbs") return refreshJobsPbs(host, body);
  let out;
  try {
    out = await inv("remote_exec", {
      target: host,
      cmd: "squeue -u $USER -h -o '%i|%j|%T|%M|%D|%R' 2>&1",
    });
  } catch (e) { body.textContent = String(e); return; }
  if (/command not found/.test(out)) {
    body.textContent = "no squeue on this host — not a SLURM cluster?";
    return;
  }
  const rows = out.split("\n").map((l) => l.trim()).filter(Boolean).map((l) => l.split("|"));
  body.innerHTML = "";
  if (!rows.length || rows[0].length < 3) {
    body.textContent = "no jobs in the queue for your user";
    return;
  }
  const tbl = el("table", "jobs-tbl");
  const hr = el("tr");
  for (const h of ["ID", "Name", "State", "Time", "N", "Reason", ""]) hr.appendChild(el("th", null, h));
  tbl.appendChild(hr);
  for (const r of rows) {
    const tr = el("tr");
    for (let i = 0; i < 6; i++) {
      const td = el("td", null, r[i] || "");
      if (i === 2) td.className = "job-" + (r[2] || "");
      tr.appendChild(td);
    }
    const act = el("td");
    const tail = el("button", "btn tiny", "tail");
    tail.title = "Show the last lines of this job's output file";
    tail.onclick = () => tailJob(host, r[0]);
    const cancel = el("button", "btn tiny danger", "✕");
    cancel.title = "scancel this job";
    cancel.onclick = async () => {
      if (!(await confirmModal("Cancel job " + r[0] + "?", r[1] || "", "scancel"))) return;
      await inv("remote_exec", { target: host, cmd: "scancel " + r[0].replace(/[^0-9_\[\]]/g, "") }).catch(() => {});
      refreshJobs();
    };
    act.append(tail, cancel);
    tr.appendChild(act);
    tbl.appendChild(tr);
  }
  body.appendChild(tbl);
}

async function refreshJobsPbs(host, body) {
  let out;
  try {
    out = await inv("remote_exec", { target: host, cmd: "qstat -u \"$USER\" 2>&1" });
  } catch (e) { body.textContent = String(e); return; }
  body.innerHTML = "";
  const pre = el("pre", "raw-out", out.trim() || "no jobs in the queue for your user");
  body.appendChild(pre);
}

async function tailJob(host, jobid) {
  const clean = jobid.replace(/[^0-9_\[\]]/g, "");
  status("fetching job output…");
  let info;
  try {
    info = await inv("remote_exec", { target: host, cmd: "scontrol show job " + clean + " 2>&1" });
  } catch (e) { status(""); return alertModal("scontrol failed", String(e)); }
  const m = info.match(/StdOut=(\S+)/);
  if (!m) {
    status("");
    return alertModal("Job output", "Could not determine the job's StdOut path.\n\n" + info.slice(0, 600));
  }
  let text;
  try {
    text = await inv("remote_exec", { target: host, cmd: `tail -n 200 -- '${m[1].replace(/'/g, "")}' 2>&1` });
  } catch (e) { status(""); return alertModal("tail failed", String(e)); }
  status("");
  const pre = el("pre", "raw-out");
  pre.textContent = text || "(empty)";
  const body = el("div");
  const head = el("div", "msg", m[1]);
  body.appendChild(head);
  const open = el("button", "btn tiny", "open in editor tab");
  open.onclick = () => { closeModal(); openEditor(host, m[1]); };
  body.appendChild(open);
  body.appendChild(pre);
  modal({ title: "Job " + clean + " — last 200 lines", body, okLabel: "Close" });
}

// ---------------------------------------------------------------- palette
let palFiltered = [];
let palIndex = 0;

function paletteItems() {
  const p = activePane || left;
  const o = p ? otherPane(p) : null;
  const items = [];
  const add = (group, title, sub, run, hint) => items.push({ group, title, sub, run, hint });

  // hosts
  add("Terminal", "New local terminal", "a shell on this machine",
    () => openTerminal(null, p && !p.target ? p.path : null), MOD + "T");
  for (const s of sessions) {
    add("Hosts", "Terminal on " + s.name, s.destination + (s.live ? " · connected" : ""),
      () => openTerminal(s.name));
    add("Hosts", "Browse " + s.name + " (left pane)", s.destination, () => switchHost(left, s.name));
    add("Hosts", "Browse " + s.name + " (right pane)", s.destination, () => switchHost(right, s.name));
  }
  add("Hosts", "Local files (left pane)", "this machine", () => switchHost(left, null));
  add("Hosts", "Local files (right pane)", "this machine", () => switchHost(right, null));

  // layout
  add("Layout", "Files only", "hide the terminal panel", () => setLayout("files"), MOD + "1");
  add("Layout", "Split — files + terminal", "", () => setLayout("split"), MOD + "2");
  add("Layout", "Full terminal panel", "terminal takes the whole window", () => setLayout("term"), MOD + "3");
  add("Layout", "Swap panes", "", swapPanes);
  add("Layout", "Even out the panes", "", () => {
    left.root.style.flex = "";
    left.root.style.width = "";
    delete ui.layoutState.splitW;
    saveUi();
  });

  // places of the active pane
  if (p) {
    for (const pl of placesCache.get(paneKey(p)) || []) {
      add("Go to", pl.label, pl.path, () => loadPane(p, pl.path));
    }
    for (const b of ui.bookmarks[paneKey(p)] || []) {
      add("Go to", "★ " + baseName(b), b, () => loadPane(p, b));
    }
    for (const rct of (ui.recents[paneKey(p)] || []).slice(0, 8)) {
      add("Go to", baseName(rct), rct, () => loadPane(p, rct));
    }
    add("Go to", "Home", "this pane's home directory", () => goHome(p), MOD + "H");
    add("Go to", "Parent folder", "", () => navUp(p, true), "⌫");
    if (o) add("Go to", "Mirror the other pane", o.path, () => loadPane(p, o.path));
  }

  // file actions
  if (p) {
    add("Files", "New folder…", p.path, () => newFolderDialog(p), "F7");
    add("Files", "New file…", p.path, () => newFileDialog(p));
    add("Files", "Rename selected…", "", () => renameDialog(p), "F2");
    add("Files", "Delete selected…", "", () => deleteSelected(p), "Del");
    add("Files", "Properties & permissions…", "", () => propertiesDialog(p));
    add("Files", "Copy selection to the other pane", "", () => startTransfer(p, o, [...p.sel]), "F5");
    add("Files", "Compressed download (tar)", "fast for many small files", () => startTarDownload());
    add("Files", (p.showHidden ? "Hide" : "Show") + " hidden files", "",
      () => { p.showHidden = !p.showHidden; renderPane(p); });
    add("Files", "Bookmark this folder", p.path, () => toggleBookmark(p), MOD + "D");
    if (p.target) add("Files", "Search " + p.target + " by filename…", p.path, () => remoteSearchDialog(p));
    add("Files", "Refresh", "", () => loadPane(p, p.path), MOD + "R");
  }

  // toggles and panels
  add("Panels", "Sessions…", "add, edit, connect, forward ports", () => sessionManager());
  add("Panels", "Queue / jobs panel", "squeue, tail, scancel", () => jobsShow($("#jobs").classList.contains("hidden")));
  add("Panels", "Transfer history", "", () => $("#q-history").click());
  add("Panels", "Preferences…", "theme, accent, behaviour", () => settingsDialog());
  add("Panels", "Keyboard shortcuts", "", () => helpDialog(), "?");
  add("Toggles", (ui.settings.rsync ? "Disable" : "Enable") + " rsync transfers", "resumable, delta-sync",
    () => $("#btn-rsync").click());
  add("Toggles", (ui.settings.plots ? "Disable" : "Enable") + " remote plots",
    "plt.show() opens a tab, no X11", () => plotsToggle());
  add("Toggles", (syncBrowse ? "Disable" : "Enable") + " synchronized browsing", "",
    () => $("#btn-sync").click());
  add("Toggles", (broadcast ? "Disable" : "Enable") + " broadcast typing", "types into every terminal",
    () => $("#btn-broadcast").click());
  add("Toggles", "Theme: " + (resolvedTheme() === "dark" ? "switch to light" : "switch to dark"), "",
    () => { ui.settings.theme = resolvedTheme() === "dark" ? "light" : "dark"; saveUi(); applyAppearance(); });

  // connections
  for (const s of sessions.filter((x) => x.live)) {
    add("Connections", "Disconnect " + s.name, s.destination, async () => {
      await inv("master_close", { target: s.name }).catch(() => {});
      await refreshSessions();
      toast("disconnected " + s.name);
    });
  }
  for (const t of tabs) {
    if (t.kind !== "term") continue;
    const rec = terms.get(t.key);
    if (rec) add("Tabs", "Focus terminal: " + (rec.lbl ? rec.lbl.textContent : rec.target),
      rec.cwd || "", () => activateTerm(t.key));
  }
  for (const [key, ed] of editors) {
    add("Tabs", "Focus editor: " + baseName(ed.path), ed.target || "local", () => activateEditor(key));
  }
  return items;
}

/// Subsequence scoring — "brp" finds "Browse paramganga".
function palScore(item, q) {
  if (!q) return 1;
  const hay = (item.title + " " + (item.sub || "") + " " + item.group).toLowerCase();
  const title = item.title.toLowerCase();
  if (title.startsWith(q)) return 1000 - title.length;
  let score = 0, i = 0;
  if (title.includes(q)) score += 400;
  else if (hay.includes(q)) score += 150;
  for (const ch of q) {
    const at = hay.indexOf(ch, i);
    if (at < 0) return -1;
    score += at === i ? 3 : 1;
    i = at + 1;
  }
  return score;
}

function renderPalette(q) {
  const all = paletteItems();
  const query = q.trim().toLowerCase();
  palFiltered = all
    .map((it) => ({ it, s: palScore(it, query) }))
    .filter((x) => x.s >= 0)
    .sort((a, b) => b.s - a.s)
    .slice(0, 60)
    .map((x) => x.it);
  palIndex = 0;
  paintPalette();
}

function paintPalette() {
  const list = $("#pal-list");
  list.innerHTML = "";
  if (!palFiltered.length) {
    list.appendChild(el("div", "pal-empty", "no matching command"));
    return;
  }
  palFiltered.forEach((it, i) => {
    const row = el("div", "pal-row" + (i === palIndex ? " sel" : ""));
    row.appendChild(el("span", "pal-group", it.group));
    const mid = el("span", "pal-mid");
    mid.appendChild(el("span", "pal-title", it.title));
    if (it.sub) mid.appendChild(el("span", "pal-sub", it.sub));
    row.appendChild(mid);
    if (it.hint) row.appendChild(el("kbd", null, it.hint));
    row.onmouseenter = () => { palIndex = i; paintPalette(); };
    row.onclick = () => runPalette(i);
    list.appendChild(row);
  });
  const sel = list.children[palIndex];
  if (sel && sel.scrollIntoView) sel.scrollIntoView({ block: "nearest" });
}

function runPalette(i) {
  const it = palFiltered[i];
  closePalette();
  if (it) {
    try { it.run(); } catch (e) { toast(String(e), "error"); }
  }
}

function openPalette() {
  $("#palette").classList.remove("hidden");
  const inp = $("#pal-input");
  inp.value = "";
  renderPalette("");
  inp.focus();
}
function closePalette() {
  $("#palette").classList.add("hidden");
}
const paletteOpen = () => !$("#palette").classList.contains("hidden");

function wirePalette() {
  const inp = $("#pal-input");
  inp.addEventListener("input", () => renderPalette(inp.value));
  inp.addEventListener("keydown", (e) => {
    if (e.key === "ArrowDown") { palIndex = Math.min(palFiltered.length - 1, palIndex + 1); paintPalette(); e.preventDefault(); }
    else if (e.key === "ArrowUp") { palIndex = Math.max(0, palIndex - 1); paintPalette(); e.preventDefault(); }
    else if (e.key === "Enter") { runPalette(palIndex); e.preventDefault(); }
    else if (e.key === "Escape") { closePalette(); e.preventDefault(); }
  });
  $("#palette").addEventListener("mousedown", (e) => {
    if (e.target.id === "palette") closePalette();
  });
}

// ---------------------------------------------------------------- misc wiring
function swapPanes() {
  const a = { target: left.target, path: left.path };
  const b = { target: right.target, path: right.path };
  left.target = b.target;
  right.target = a.target;
  left.hostSel.value = b.target || "";
  right.hostSel.value = a.target || "";
  updateToolbar(left);
  updateToolbar(right);
  loadPane(left, b.path);
  loadPane(right, a.path);
}

function wireTopbar() {
  $("#btn-sessions").onclick = () => sessionManager();
  $("#btn-palette").onclick = () => openPalette();
  $("#btn-help").onclick = () => helpDialog();
  $("#btn-settings").onclick = () => settingsDialog();
  $("#btn-jobs").onclick = () => jobsShow($("#jobs").classList.contains("hidden"));
  $("#jobs-close").onclick = () => jobsShow(false);
  $("#jobs-refresh").onclick = () => refreshJobs();
  $("#jobs-host").onchange = () => {
    ui.settings.jobsHost = $("#jobs-host").value;
    saveUi();
    refreshJobs();
  };
  $("#btn-rsync").onclick = () => {
    ui.settings.rsync = !ui.settings.rsync;
    $("#btn-rsync").classList.toggle("toggled", !!ui.settings.rsync);
    saveUi();
    toast(ui.settings.rsync
      ? "rsync mode ON — resumable, delta-sync transfers where possible"
      : "rsync mode off — using scp");
  };
  $("#btn-plots").onclick = () => plotsToggle();
  $("#btn-sync").onclick = () => {
    syncBrowse = !syncBrowse;
    $("#btn-sync").classList.toggle("toggled", syncBrowse);
    toast(syncBrowse ? "synchronized browsing ON" : "synchronized browsing off");
  };
  $("#btn-swap").onclick = swapPanes;
  $("#xfer-right").onclick = () => startTransfer(left, right, [...left.sel]);
  $("#xfer-left").onclick = () => startTransfer(right, left, [...right.sel]);
  $("#xfer-tar").onclick = () => startTarDownload();
  for (const b of $$("#layout-seg .btn")) b.onclick = () => setLayout(b.dataset.layout);
  $("#kbd-palette").textContent = MOD + "K";
}

// ---------------------------------------------------------------- keyboard
const inTerminal = () => {
  const a = document.activeElement;
  return !!(a && a.closest && a.closest(".term-slot"));
};
const inEditorTab = () => {
  const a = document.activeElement;
  return !!(a && a.closest && a.closest(".ed-slot"));
};
const inTextField = () => {
  const a = document.activeElement;
  if (!a) return false;
  if (inTerminal() || inEditorTab()) return false;
  return a.tagName === "INPUT" || a.tagName === "TEXTAREA" || a.tagName === "SELECT" || a.isContentEditable;
};

function onKeyDown(e) {
  const mod = e.metaKey || e.ctrlKey;
  const code = e.code;

  // --- escape is universal ---
  if (e.key === "Escape") {
    if (paletteOpen()) { closePalette(); e.preventDefault(); return; }
    if (mdrag.active) { dragCleanup(); mdrag.press = null; return; }
    return;
  }

  // --- always available, even inside a terminal ---
  if (mod && !e.shiftKey && !e.altKey && ["Digit1", "Digit2", "Digit3"].includes(code)) {
    setLayout(["files", "split", "term"][Number(code.slice(-1)) - 1]);
    return e.preventDefault();
  }
  if (e.ctrlKey && !e.metaKey && (e.key === "`" || code === "Backquote")) {
    toggleTermMax();
    return e.preventDefault();
  }
  if (e.ctrlKey && code === "Tab") {
    cycleTab(e.shiftKey ? -1 : 1);
    return e.preventDefault();
  }
  if (e.altKey && /^Digit[1-9]$/.test(code)) {
    const i = Number(code.slice(-1)) - 1;
    if (tabs[i]) { activateTab(tabs[i]); return e.preventDefault(); }
  }
  if (mod && e.shiftKey) {
    if (code === "KeyK" || code === "KeyP") { openPalette(); return e.preventDefault(); }
    if (code === "KeyT") { openTerminalSmart(); return e.preventDefault(); }
    if (code === "KeyW") { closeActiveTab(); return e.preventDefault(); }
    if (code === "KeyF") { toggleTermSearch(true); return e.preventDefault(); }
  }
  if (e.key === "F1") { helpDialog(); return e.preventDefault(); }
  if (mod && (code === "Equal" || code === "NumpadAdd")) {
    setMonoSize((ui.settings.monoSize || 13) + 1);
    return e.preventDefault();
  }
  if (mod && (code === "Minus" || code === "NumpadSubtract")) {
    setMonoSize((ui.settings.monoSize || 13) - 1);
    return e.preventDefault();
  }
  if (mod && code === "Digit0") { setMonoSize(13); return e.preventDefault(); }

  // --- the terminal owns every other Ctrl/Cmd chord ---
  if (inTerminal()) return;

  if (mod && code === "KeyS" && activeEditor && editors.has(activeEditor)) {
    editors.get(activeEditor).save();
    return e.preventDefault();
  }
  if (inEditorTab()) {
    if (mod && code === "KeyW") { closeActiveTab(); return e.preventDefault(); }
    if (mod && code === "KeyK") { openPalette(); return e.preventDefault(); }
    return;
  }

  if (mod && code === "KeyK") { openPalette(); return e.preventDefault(); }
  if (mod && code === "KeyP") { openPalette(); return e.preventDefault(); }
  if (mod && code === "KeyT") { openTerminalSmart(); return e.preventDefault(); }
  if (mod && code === "KeyW") { closeActiveTab(); return e.preventDefault(); }
  if (mod && code === "KeyF" && tabs.length) { toggleTermSearch(true); return e.preventDefault(); }

  if (inTextField() || modalOpen() || paletteOpen()) return;

  if (e.key === "?") { helpDialog(); return e.preventDefault(); }

  const p = activePane;
  if (!p) return;
  if (paneKeydown(p, e)) e.preventDefault();
}

// ---------------------------------------------------------------- boot
async function boot() {
  try {
    const raw = await inv("ui_load");
    const parsed = JSON.parse(raw || "{}");
    ui = Object.assign({ bookmarks: {}, recents: {}, history: [], settings: {}, layoutState: {} }, parsed);
    ui.layoutState = ui.layoutState || {};
  } catch (_) {}

  applyAppearance();
  wireTopbar();
  wirePalette();
  wireDrawerButtons();
  wireResizers();
  wireDrag();
  wireTransferEvents();
  wireQueueButtons();
  wireTerminalEvents();
  wireEditEvents();
  wireFileDrop();
  wireAuthPrompts();
  document.addEventListener("keydown", onKeyDown, true);

  $("#btn-rsync").classList.toggle("toggled", !!ui.settings.rsync);
  $("#btn-plots").classList.toggle("toggled", !!ui.settings.plots);

  left = makePane("pane-left");
  right = makePane("pane-right");
  setActivePane(left);

  // restore geometry
  if (ui.layoutState.splitW) {
    left.root.style.flex = "none";
    left.root.style.width = ui.layoutState.splitW + "px";
  }
  if (ui.layoutState.drawerH) $("#drawer").style.height = ui.layoutState.drawerH + "px";
  setLayout(ui.settings.layout || "split", true);

  await refreshSessions();
  loadPlaces(null);

  // Where were we? Restore hosts and folders, or fall back to
  // local ↔ first session.
  const saved = ui.layoutState;
  const known = new Set(sessions.map((s) => s.name));
  const wants = opt("restore", true) && saved.left && saved.right
    ? [saved.left, saved.right]
    : [{ target: null, path: null },
       { target: sessions.length ? sessions[0].name : null, path: null }];

  for (const [i, pane] of [left, right].entries()) {
    const w = wants[i] || {};
    const target = w.target && known.has(w.target) ? w.target : null;
    pane.target = target;
    pane.hostSel.value = target || "";
    updateToolbar(pane);
    if (target) { loadPlaces(target); loadFacts(target); }
  }
  // Both panes load in parallel: a server that needs a login should not
  // hold up the local side of the window.
  await Promise.all([left, right].map((pane, i) => {
    const w = wants[i] || {};
    return (w.path && (w.target || null) === pane.target)
      ? loadPane(pane, w.path)
      : goHome(pane);
  }));

  updateTermEmpty();
  toast(`ready — ${MOD}K for commands, ${MOD}3 for a full terminal`);
}

boot();
