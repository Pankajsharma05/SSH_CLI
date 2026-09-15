"use strict";
// ============================================================
//  SSH_CLI v3 — file panes: browsing, selection, transfers
// ============================================================

let syncBrowse = false;
let syncGuard = false;

function iconBtn(svg, title, extraCls) {
  const b = el("button", "btn icon-btn" + (extraCls ? " " + extraCls : ""));
  b.innerHTML = svg;
  b.title = title;
  return b;
}

// ---------------------------------------------------------------- construction
function makePane(id) {
  const pane = {
    id,
    root: $("#" + id),
    target: null,
    path: "",
    entries: [],
    view: [],
    sel: new Set(),
    cursor: -1,
    anchor: -1,
    sortKey: "name",
    sortDir: 1,
    filter: "",
    showHidden: opt("showHidden", false),
    needsAuth: false,
    tbody: null,
    typeahead: "",
    typeaheadAt: 0,
  };

  const head = el("div", "pane-head");

  // --- row 1: host, filter, actions ---
  const row1 = el("div", "ph-row");
  pane.hostSel = el("select");
  pane.hostSel.title = "Which machine this pane shows";
  pane.filterInput = el("input", "filter");
  pane.filterInput.placeholder = "filter…";
  pane.filterInput.spellcheck = false;

  const btnHome = iconBtn(IC.home, `Home directory (${MOD}H)`);
  const btnUp = iconBtn(IC.up, "Parent directory (⌫)");
  const btnRefresh = iconBtn(IC.refresh, `Refresh (${MOD}R)`);
  pane.btnPlaces = iconBtn(IC.star, "Places: home, scratch, bookmarks, recents");
  pane.btnFind = iconBtn(IC.find, "Search this host by filename");
  const btnNew = iconBtn(IC.plus, "New folder (F7)");
  pane.btnDel = iconBtn(IC.trash, "Delete selected (Del)", "danger");
  const btnTerm = el("button", "btn mono term-btn", ">_");
  btnTerm.title = "Open a terminal here";
  pane.btnTerm = btnTerm;
  const btnMore = iconBtn(IC.dots, "More actions");

  row1.append(pane.hostSel, pane.filterInput, el("span", "spacer"),
    btnHome, btnUp, btnRefresh, pane.btnPlaces, pane.btnFind,
    btnNew, pane.btnDel, btnTerm, btnMore);

  // --- row 2: breadcrumb / editable path ---
  const row2 = el("div", "ph-row crumb-row");
  pane.hostChip = el("span", "host-chip");
  pane.crumbs = el("div", "crumbs");
  pane.pathInput = el("input", "path hidden");
  pane.pathInput.spellcheck = false;
  row2.append(pane.hostChip, pane.crumbs, pane.pathInput);

  head.append(row1, row2);

  pane.body = el("div", "pane-body");
  pane.body.tabIndex = 0;
  pane.foot = el("div", "pane-foot");
  pane.footDisk = el("span", null, "");
  pane.footItems = el("span", null, "");
  pane.foot.append(pane.footDisk, pane.footItems);
  pane.root.append(head, pane.body, pane.foot);

  // -- events --
  pane.root.addEventListener("mousedown", () => setActivePane(pane));
  pane.body.addEventListener("focus", () => setActivePane(pane));
  pane.body.addEventListener("contextmenu", (e) => {
    if (!e.target.closest("tr") || !e.target.closest("tbody")) bodyContextMenu(pane, e);
  });
  pane.hostSel.onchange = () => switchHost(pane, pane.hostSel.value === "" ? null : pane.hostSel.value);
  pane.pathInput.addEventListener("keydown", (e) => {
    if (e.key === "Enter") { hidePathEditor(pane); loadPane(pane, pane.pathInput.value.trim()); }
    if (e.key === "Escape") { hidePathEditor(pane); pane.body.focus(); }
  });
  pane.pathInput.addEventListener("blur", () => hidePathEditor(pane));
  pane.filterInput.addEventListener("input", () => {
    pane.filter = pane.filterInput.value.trim().toLowerCase();
    renderPane(pane);
  });
  pane.filterInput.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      pane.filterInput.value = "";
      pane.filter = "";
      renderPane(pane);
      pane.body.focus();
    }
    if (e.key === "Enter") pane.body.focus();
  });
  pane.crumbs.addEventListener("dblclick", () => showPathEditor(pane));

  btnHome.onclick = () => goHome(pane);
  btnUp.onclick = () => navUp(pane, true);
  btnRefresh.onclick = () => loadPane(pane, pane.path);
  pane.btnPlaces.onclick = () => placesMenu(pane);
  pane.btnFind.onclick = () => remoteSearchDialog(pane);
  btnNew.onclick = () => newFolderDialog(pane);
  pane.btnDel.onclick = () => deleteSelected(pane);
  btnTerm.onclick = () => openTerminalFor(pane);
  btnMore.onclick = () => menuUnder(btnMore, paneMenuItems(pane));

  updateToolbar(pane);
  return pane;
}

function setActivePane(pane) {
  activePane = pane;
  left.root.classList.toggle("active-pane", pane === left);
  right.root.classList.toggle("active-pane", pane === right);
  updateCounts(pane);
}

function otherPane(pane) { return pane === left ? right : left; }

function updateToolbar(pane) {
  const isLocal = pane.target === null;
  pane.btnFind.style.display = isLocal ? "none" : "";
  pane.hostChip.textContent = isLocal ? "local" : pane.target;
  pane.hostChip.className = "host-chip" + (isLocal ? " local" : "");
  const facts = hostFacts.get(pane.target);
  pane.hostChip.title = facts
    ? `${facts.user}@${facts.hostname}` + (facts.scheduler !== "none" ? ` · ${facts.scheduler}` : "")
    : (isLocal ? "this machine" : pane.target);
}

function updateCounts(pane) {
  if (!pane) return;
  let selBytes = 0;
  for (const n of pane.sel) {
    const e = pane.view.find((x) => x.name === n);
    if (e && !e.is_dir) selBytes += e.size;
  }
  const selTxt = pane.sel.size
    ? `${pane.sel.size} selected` + (selBytes ? ` (${fmtSize(selBytes)})` : "")
    : "0 selected";
  if (pane === activePane) $("#counts").textContent = `${pane.view.length} items · ${selTxt}`;
  pane.footItems.textContent = `${pane.view.length} items` + (pane.sel.size ? ` · ${selTxt}` : "");
}

// ---------------------------------------------------------------- host switching
async function switchHost(pane, target) {
  pane.target = target;
  pane.needsAuth = false;
  pane.hostSel.value = target || "";
  updateToolbar(pane);
  fillPlaces(pane);
  await goHome(pane);
  if (target) {
    loadPlaces(target);
    loadFacts(target);
  }
  rememberPanes();
}

function paneHostOptions(pane) {
  const keep = pane.hostSel.value;
  pane.hostSel.innerHTML = "";
  const optLocal = el("option", null, "◻ Local");
  optLocal.value = "";
  pane.hostSel.appendChild(optLocal);
  for (const s of sessions) {
    const o = el("option", null, (s.live ? "● " : "○ ") + s.name);
    o.value = s.name;
    pane.hostSel.appendChild(o);
  }
  pane.hostSel.value = pane.target || keep || "";
}

async function loadFacts(target) {
  if (!target || hostFacts.has(target)) return;
  try {
    const f = await inv("host_info", { target });
    hostFacts.set(target, f);
    updateToolbar(left);
    updateToolbar(right);
  } catch (_) {}
}

// ---------------------------------------------------------------- places
async function loadPlaces(target) {
  const key = target || "local";
  try {
    placesCache.set(key, await inv("places", { target }));
  } catch (_) {
    placesCache.set(key, []);
  }
  for (const p of [left, right]) if (p && paneKey(p) === key) fillPlaces(p);
}

function fillPlaces(pane) {
  // The menu is built on demand; this only refreshes the button's hint.
  const key = paneKey(pane);
  const n = (placesCache.get(key) || []).length + (ui.bookmarks[key] || []).length;
  pane.btnPlaces.classList.toggle("has-places", n > 0);
}

function placesMenu(pane) {
  const key = paneKey(pane);
  const items = [];
  const cluster = placesCache.get(key) || [];
  const bookmarks = ui.bookmarks[key] || [];
  const recents = (ui.recents[key] || []).filter((p) => !bookmarks.includes(p));

  if (cluster.length) {
    for (const pl of cluster) {
      items.push({
        label: pl.label,
        hint: shortPath(pl.path, 2),
        action: () => loadPane(pane, pl.path),
      });
    }
  }
  if (bookmarks.length) {
    items.push("-");
    for (const p of bookmarks) {
      items.push({ label: "★ " + baseName(p), hint: shortPath(p, 2), action: () => loadPane(pane, p) });
    }
  }
  if (recents.length) {
    items.push("-");
    for (const p of recents.slice(0, 8)) {
      items.push({ label: baseName(p), hint: shortPath(p, 2), action: () => loadPane(pane, p) });
    }
  }
  if (items.length) items.push("-");
  const marked = bookmarks.includes(pane.path);
  items.push({
    label: marked ? "Remove bookmark" : "Bookmark this folder",
    hint: "★",
    action: () => toggleBookmark(pane),
  });
  if (pane.target) {
    items.push({ label: "Rescan cluster folders", action: () => loadPlaces(pane.target).then(() => toast("places refreshed")) });
  }
  menuUnder(pane.btnPlaces, items);
}

function toggleBookmark(pane) {
  const key = paneKey(pane);
  ui.bookmarks[key] = ui.bookmarks[key] || [];
  const i = ui.bookmarks[key].indexOf(pane.path);
  if (i >= 0) ui.bookmarks[key].splice(i, 1);
  else ui.bookmarks[key].unshift(pane.path);
  saveUi();
  fillPlaces(pane);
  toast(i >= 0 ? "bookmark removed" : "bookmarked " + pane.path);
}

// ---------------------------------------------------------------- loading
async function goHome(pane) {
  try {
    const facts = hostFacts.get(pane.target);
    const home = facts && facts.home
      ? facts.home
      : pane.target
        ? await inv("remote_home", { target: pane.target })
        : await inv("local_home");
    await loadPane(pane, home);
  } catch (e) {
    await handleLoadError(pane, e);
  }
}

async function handleLoadError(pane, e) {
  const msg = String(e);
  if (pane.target) {
    const up = await inv("host_connected", { target: pane.target }).catch(() => false);
    if (!up) {
      pane.needsAuth = true;
      renderDisconnected(pane, msg);
      watchConnect(pane.target);
      return;
    }
  }
  renderMessage(pane, msg);
}

async function loadPane(pane, path) {
  if (!path) return;
  renderMessage(pane, "loading…");
  let entries;
  try {
    entries = pane.target
      ? await inv("list_remote", { target: pane.target, path })
      : await inv("list_local", { path });
  } catch (e) {
    return handleLoadError(pane, e);
  }
  pane.needsAuth = false;
  pane.path = path;
  pane.pathInput.value = path;
  pane.entries = entries;
  pane.sel.clear();
  pane.cursor = entries.length ? 0 : -1;
  pane.anchor = -1;
  renderPane(pane);
  renderCrumbs(pane);

  const key = paneKey(pane);
  ui.recents[key] = [path, ...(ui.recents[key] || []).filter((p) => p !== path)].slice(0, 12);
  saveUi();
  fillPlaces(pane);
  rememberPanes();
  refreshSessionsSoon();
  inv("fs_disk", { target: pane.target, path })
    .then((d) => { pane.footDisk.textContent = d || ""; })
    .catch(() => { pane.footDisk.textContent = ""; });
}

async function navInto(pane, name, user) {
  await loadPane(pane, joinPath(pane.path, name));
  if (user && syncBrowse && !syncGuard) {
    syncGuard = true;
    const o = otherPane(pane);
    if (o.entries.some((e) => e.is_dir && e.name === name)) {
      await loadPane(o, joinPath(o.path, name));
    }
    syncGuard = false;
  }
}

async function navUp(pane, user) {
  const from = baseName(pane.path);
  await loadPane(pane, parentPath(pane.path));
  // Land the cursor on the folder you just left — the classic file-manager
  // courtesy that makes ⌫ ⌫ ⌫ navigation bearable.
  selectByName(pane, from);
  if (user && syncBrowse && !syncGuard) {
    syncGuard = true;
    await loadPane(otherPane(pane), parentPath(otherPane(pane).path));
    syncGuard = false;
  }
}

function selectByName(pane, name) {
  const i = pane.view.findIndex((e) => e.name === name);
  if (i < 0) return;
  pane.sel.clear();
  pane.sel.add(name);
  pane.cursor = i;
  paintSelection(pane);
  scrollCursorIntoView(pane);
}

// ---------------------------------------------------------------- path editing
function showPathEditor(pane) {
  pane.crumbs.classList.add("hidden");
  pane.pathInput.classList.remove("hidden");
  pane.pathInput.value = pane.path;
  pane.pathInput.focus();
  pane.pathInput.select();
}
function hidePathEditor(pane) {
  pane.pathInput.classList.add("hidden");
  pane.crumbs.classList.remove("hidden");
}

function renderCrumbs(pane) {
  pane.crumbs.innerHTML = "";
  const parts = pane.path.split("/").filter(Boolean);
  const mk = (label, path, last) => {
    const c = el("span", "crumb" + (last ? " last" : ""), label);
    c.onclick = () => { if (!last) loadPane(pane, path); };
    c.oncontextmenu = (e) => {
      e.preventDefault();
      ctxMenu([
        { label: "Copy path", action: () => copyText(path, "path copied") },
        { label: "Open in other pane", action: () => loadPane(otherPane(pane), path) },
        { label: "Open terminal here", action: () => openTerminal(pane.target, path) },
      ], e.clientX, e.clientY);
    };
    pane.crumbs.appendChild(c);
  };
  mk("/", "/", parts.length === 0);
  let acc = "";
  parts.forEach((p, i) => {
    acc += "/" + p;
    if (i) pane.crumbs.appendChild(el("span", "crumb-sep", "›"));
    mk(p, acc, i === parts.length - 1);
  });
  const edit = el("span", "crumb-edit", "✎");
  edit.title = `Edit the path (${MOD}L)`;
  edit.onclick = () => showPathEditor(pane);
  pane.crumbs.appendChild(edit);
}

// ---------------------------------------------------------------- rendering
function renderMessage(pane, msg) {
  pane.body.innerHTML = "";
  pane.tbody = null;
  pane.view = [];
  const box = el("div", "pane-msg", msg);
  pane.body.appendChild(box);
  return box;
}

/// The "you are not logged in yet" state. This is the normal first screen
/// for a cluster that wants a password or a one-time code, so it offers the
/// way forward instead of only printing ssh's complaint.
function renderDisconnected(pane, msg) {
  pane.body.innerHTML = "";
  pane.tbody = null;
  pane.view = [];
  const box = el("div", "pane-msg discon");
  box.appendChild(el("div", "dc-title", "Not connected to " + pane.target));
  box.appendChild(el("div", "dc-sub",
    "Open a terminal and log in (password, 2FA and OTP all work there). " +
    "This pane jumps to your home directory on that cluster the moment the login succeeds."));
  const acts = el("div", "dc-acts");
  const go = el("button", "btn primary", ">_  Open terminal & log in");
  go.onclick = () => openTerminal(pane.target);
  const retry = el("button", "btn", "Retry");
  retry.onclick = () => goHome(pane);
  acts.append(go, retry);
  box.appendChild(acts);

  if (/HOST IDENTIFICATION HAS CHANGED|Host key verification failed/i.test(msg)) {
    const fix = el("button", "btn danger", "Remove old host key & retry");
    fix.onclick = async () => {
      const ok = await confirmModal(
        "Remove old host key?",
        "This deletes the saved fingerprint for this host from ~/.ssh/known_hosts. " +
        "Only do this if the change is expected (server reinstall, multiple login nodes). " +
        "If in doubt, verify the new fingerprint with the cluster admins first.",
        "Remove key"
      );
      if (!ok) return;
      try {
        const log = await inv("forget_host_key", { target: pane.target });
        toast(log.split("\n")[0] || "old key removed");
        await goHome(pane);
      } catch (e) { alertModal("Could not remove key", String(e)); }
    };
    acts.appendChild(fix);
  }
  const det = el("details", "dc-detail");
  det.appendChild(el("summary", null, "ssh said"));
  det.appendChild(el("pre", null, msg));
  box.appendChild(det);
  pane.body.appendChild(box);
}

function visibleEntries(pane) {
  let v = pane.entries;
  if (!pane.showHidden) v = v.filter((e) => !e.name.startsWith("."));
  if (pane.filter) v = v.filter((e) => e.name.toLowerCase().includes(pane.filter));
  const k = pane.sortKey, d = pane.sortDir;
  v = [...v].sort((a, b) => {
    if (a.is_dir !== b.is_dir) return a.is_dir ? -1 : 1;
    let c = 0;
    if (k === "name") {
      const x = a.name.toLowerCase(), y = b.name.toLowerCase();
      c = x < y ? -1 : x > y ? 1 : 0;
    } else if (k === "size") c = a.size - b.size;
    else c = a.mtime - b.mtime;
    return c * d;
  });
  return v;
}

const EXT_KIND = {
  code: ["py", "pyw", "sh", "bash", "zsh", "slurm", "sbatch", "c", "h", "cpp", "hpp", "cc", "cu",
         "rs", "jl", "js", "mjs", "ts", "go", "java", "rb", "pl", "lua", "f90", "f", "m", "tex", "sty"],
  img:  ["png", "jpg", "jpeg", "gif", "webp", "svg", "bmp", "ico", "tif", "tiff", "heic"],
  arch: ["zip", "tar", "gz", "tgz", "bz2", "xz", "zst", "7z", "rar", "deb", "rpm"],
  doc:  ["md", "markdown", "txt", "rst", "pdf", "doc", "docx", "odt", "rtf", "log", "out", "err"],
  data: ["json", "yaml", "yml", "toml", "csv", "tsv", "xml", "ini", "cfg", "conf",
         "h5", "hdf5", "nc", "npz", "npy", "parquet", "db", "sqlite"],
};
const EXT_GLYPH = { code: "◆", img: "◍", arch: "▦", doc: "≡", data: "∷" };
const extKind = (name) => {
  const ext = (name.split(".").pop() || "").toLowerCase();
  for (const k in EXT_KIND) if (EXT_KIND[k].includes(ext)) return k;
  return null;
};
function fileIcon(ent) {
  if (ent.is_dir) return el("span", "icon", "▸");
  const k = extKind(ent.name);
  return el("span", "icon" + (k ? " ext-" + k : ""), k ? EXT_GLYPH[k] : "·");
}

function renderPane(pane) {
  pane.view = visibleEntries(pane);
  if (pane.cursor >= pane.view.length) pane.cursor = pane.view.length - 1;
  pane.body.innerHTML = "";
  const table = el("table", "files");
  const thead = el("thead");
  const hr = el("tr");
  for (const [key, label] of [["name", "Name"], ["size", "Size"], ["mtime", "Modified"]]) {
    const th = el("th", null, label);
    if (pane.sortKey === key) th.appendChild(el("span", "arrow", pane.sortDir === 1 ? "▲" : "▼"));
    th.onclick = () => {
      if (pane.sortKey === key) pane.sortDir *= -1;
      else { pane.sortKey = key; pane.sortDir = 1; }
      renderPane(pane);
    };
    hr.appendChild(th);
  }
  thead.appendChild(hr);
  table.appendChild(thead);
  const tbody = el("tbody");
  pane.tbody = tbody;

  pane.view.forEach((ent, idx) => {
    const tr = el("tr", ent.is_dir ? "dir" : "file");
    tr.dataset.name = ent.name;
    if (pane.sel.has(ent.name)) tr.classList.add("sel");
    if (idx === pane.cursor) tr.classList.add("cursor");

    const name = el("td", "name");
    name.appendChild(fileIcon(ent));
    name.appendChild(document.createTextNode(ent.name));
    if (ent.is_link) name.appendChild(el("span", "link-badge", "⇢"));
    tr.appendChild(name);
    tr.appendChild(el("td", "num", ent.is_dir ? "—" : fmtSize(ent.size)));
    tr.appendChild(el("td", "num", fmtDate(ent.mtime)));

    tr.addEventListener("click", (e) => {
      if (mdrag.suppressClick) { mdrag.suppressClick = false; return; }
      if (e.shiftKey && pane.cursor >= 0) {
        const [a, b] = [Math.min(pane.cursor, idx), Math.max(pane.cursor, idx)];
        if (!(e.metaKey || e.ctrlKey)) pane.sel.clear();
        for (let i = a; i <= b; i++) pane.sel.add(pane.view[i].name);
      } else if (e.metaKey || e.ctrlKey) {
        pane.sel.has(ent.name) ? pane.sel.delete(ent.name) : pane.sel.add(ent.name);
        pane.cursor = idx;
      } else {
        pane.sel.clear();
        pane.sel.add(ent.name);
        pane.cursor = idx;
      }
      paintSelection(pane);
      pane.body.focus();
    });
    tr.addEventListener("dblclick", () => openEntry(pane, ent));
    tr.addEventListener("mousedown", (e) => {
      if (e.button === 0) mdrag.press = { pane, name: ent.name, x: e.clientX, y: e.clientY };
    });
    tr.addEventListener("contextmenu", (e) => rowContextMenu(pane, ent, e));
    tbody.appendChild(tr);
  });

  pane.body.onclick = (e) => {
    if (e.target === pane.body) {
      pane.sel.clear();
      paintSelection(pane);
    }
  };

  table.appendChild(tbody);
  pane.body.appendChild(table);
  if (!pane.view.length) {
    pane.body.appendChild(el("div", "pane-empty",
      pane.filter ? "nothing matches “" + pane.filter + "”" : "this folder is empty"));
  }
  updateCounts(pane);
}

function paintSelection(pane) {
  if (!pane.tbody) return;
  [...pane.tbody.children].forEach((row, i) => {
    row.classList.toggle("sel", pane.sel.has(row.dataset.name));
    row.classList.toggle("cursor", i === pane.cursor);
  });
  updateCounts(pane);
}

function scrollCursorIntoView(pane) {
  if (!pane.tbody || pane.cursor < 0) return;
  const row = pane.tbody.children[pane.cursor];
  if (row) row.scrollIntoView({ block: "nearest" });
}

function openEntry(pane, ent) {
  const full = joinPath(pane.path, ent.name);
  if (ent.is_dir) return navInto(pane, ent.name, true);
  if (imgMime(ent.name)) return openImageViewer(pane.target, full);
  if (opt("dblclick", "edit") === "system" && pane.target === null) {
    return inv("local_open", { path: full }).catch((e) => alertModal("Could not open", String(e)));
  }
  return openEditor(pane.target, full);
}

// ---------------------------------------------------------------- keyboard
function moveCursor(pane, delta, extend) {
  if (!pane.view.length) return;
  const to = Math.max(0, Math.min(pane.view.length - 1, (pane.cursor < 0 ? 0 : pane.cursor) + delta));
  setCursor(pane, to, extend);
}

function setCursor(pane, to, extend) {
  if (!pane.view.length) return;
  if (extend) {
    if (pane.anchor < 0) pane.anchor = pane.cursor < 0 ? to : pane.cursor;
    pane.sel.clear();
    const [a, b] = [Math.min(pane.anchor, to), Math.max(pane.anchor, to)];
    for (let i = a; i <= b; i++) pane.sel.add(pane.view[i].name);
  } else {
    pane.anchor = to;
    pane.sel.clear();
    pane.sel.add(pane.view[to].name);
  }
  pane.cursor = to;
  paintSelection(pane);
  scrollCursorIntoView(pane);
}

function typeAhead(pane, ch) {
  const now = Date.now();
  pane.typeahead = now - pane.typeaheadAt > 800 ? ch : pane.typeahead + ch;
  pane.typeaheadAt = now;
  const q = pane.typeahead.toLowerCase();
  const from = pane.typeahead.length === 1 ? pane.cursor + 1 : pane.cursor;
  const n = pane.view.length;
  for (let k = 0; k < n; k++) {
    const i = (from + k + n) % n;
    if (pane.view[i].name.toLowerCase().startsWith(q)) return setCursor(pane, i, false);
  }
}

/// Key handling for the active file pane. Returns true when it consumed
/// the event.
function paneKeydown(pane, e) {
  const meta = e.metaKey || e.ctrlKey;
  const k = e.key;

  if (k === "ArrowDown") { moveCursor(pane, 1, e.shiftKey); return true; }
  if (k === "ArrowUp") { moveCursor(pane, -1, e.shiftKey); return true; }
  if (k === "PageDown") { moveCursor(pane, 15, e.shiftKey); return true; }
  if (k === "PageUp") { moveCursor(pane, -15, e.shiftKey); return true; }
  if (k === "Home") { setCursor(pane, 0, e.shiftKey); return true; }
  if (k === "End") { setCursor(pane, pane.view.length - 1, e.shiftKey); return true; }
  if (k === "ArrowRight" && !meta) {
    const ent = pane.view[pane.cursor];
    if (ent && ent.is_dir) navInto(pane, ent.name, true);
    return true;
  }
  if (k === "ArrowLeft" && !meta) { navUp(pane, true); return true; }
  if (k === "Enter") {
    const ent = pane.view[pane.cursor];
    if (ent) openEntry(pane, ent);
    return true;
  }
  if (k === " ") {
    const ent = pane.view[pane.cursor];
    if (ent) {
      pane.sel.has(ent.name) ? pane.sel.delete(ent.name) : pane.sel.add(ent.name);
      paintSelection(pane);
    }
    return true;
  }
  if (k === "Backspace" && !meta) { navUp(pane, true); return true; }
  if (meta && k === "ArrowUp") { navUp(pane, true); return true; }
  if (meta && k.toLowerCase() === "a") {
    pane.view.forEach((en) => pane.sel.add(en.name));
    paintSelection(pane);
    return true;
  }
  if (meta && k.toLowerCase() === "r") { loadPane(pane, pane.path); return true; }
  if (meta && k.toLowerCase() === "l") { showPathEditor(pane); return true; }
  if (meta && k.toLowerCase() === "h") { goHome(pane); return true; }
  if (meta && k.toLowerCase() === "d") { toggleBookmark(pane); return true; }
  if (k === "Delete" || (meta && k === "Backspace")) { deleteSelected(pane); return true; }
  if (k === "F2") { renameDialog(pane); return true; }
  if (k === "F5") { startTransfer(pane, otherPane(pane), [...pane.sel]); return true; }
  if (k === "F7") { newFolderDialog(pane); return true; }
  if (k === "Tab") { setActivePane(otherPane(pane)); otherPane(pane).body.focus(); return true; }
  if (k === "Escape" && pane.filter) {
    pane.filterInput.value = "";
    pane.filter = "";
    renderPane(pane);
    return true;
  }
  if (!meta && !e.altKey && k.length === 1 && k !== " ") {
    if (/[\w.\-+]/.test(k)) { typeAhead(pane, k); return true; }
  }
  return false;
}

// ---------------------------------------------------------------- actions
async function newFolderDialog(pane) {
  const r = await modal({ title: "New folder", fields: [{ key: "name", label: "Folder name" }] });
  if (!r || !r.name) return;
  try {
    await inv("fs_mkdir", { target: pane.target, path: joinPath(pane.path, r.name) });
    await loadPane(pane, pane.path);
    selectByName(pane, r.name);
  } catch (e) { alertModal("Could not create folder", String(e)); }
}

async function newFileDialog(pane) {
  const r = await modal({ title: "New file", fields: [{ key: "name", label: "File name" }] });
  if (!r || !r.name) return;
  const path = joinPath(pane.path, r.name);
  if (pane.entries.some((e) => e.name === r.name)) {
    return alertModal("New file", r.name + " already exists here.");
  }
  try {
    await inv("file_write", { target: pane.target, path, data: "" });
    await loadPane(pane, pane.path);
    openEditor(pane.target, path);
  } catch (e) { alertModal("Could not create file", String(e)); }
}

async function renameDialog(pane, name) {
  const oldName = name || (pane.sel.size === 1 ? [...pane.sel][0] : null);
  if (!oldName) return alertModal("Rename", "Select exactly one item.");
  const r = await modal({
    title: "Rename",
    fields: [{ key: "name", label: "New name", value: oldName }],
    okLabel: "Rename",
  });
  if (!r || !r.name || r.name === oldName) return;
  try {
    await inv("fs_rename", {
      target: pane.target,
      from: joinPath(pane.path, oldName),
      to: joinPath(pane.path, r.name),
    });
    await loadPane(pane, pane.path);
    selectByName(pane, r.name);
  } catch (e) { alertModal("Rename failed", String(e)); }
}

async function deleteSelected(pane) {
  if (!pane.sel.size) return;
  const names = [...pane.sel];
  if (opt("confirmDelete", true)) {
    const ok = await confirmModal(
      `Delete ${names.length} item(s) on ${pane.target || "this machine"}?`,
      names.slice(0, 20).map((n) => "  " + joinPath(pane.path, n)).join("\n") +
      (names.length > 20 ? `\n  … and ${names.length - 20} more` : "") +
      "\n\nThis cannot be undone."
    );
    if (!ok) return;
  }
  let failed = 0;
  for (const n of names) {
    try {
      await inv("fs_delete", { target: pane.target, path: joinPath(pane.path, n) });
    } catch (e) { failed++; await alertModal("Delete failed", n + ": " + e); break; }
  }
  toast(`deleted ${names.length - failed} item(s)`);
  loadPane(pane, pane.path);
}

async function propertiesDialog(pane) {
  if (pane.sel.size !== 1) return alertModal("Properties", "Select exactly one item.");
  const name = [...pane.sel][0];
  const ent = pane.view.find((e) => e.name === name);
  if (!ent) return;
  const body = el("div", "kv");
  const kv = (k, v) => {
    body.appendChild(el("div", "k", k));
    body.appendChild(el("div", "v", v));
  };
  kv("Name", ent.name);
  kv("Path", joinPath(pane.path, ent.name));
  kv("Host", pane.target || "local");
  kv("Type", ent.is_dir ? "directory" : ent.is_link ? "symlink" : "file");
  kv("Size", ent.is_dir ? "—" : fmtSize(ent.size));
  kv("Modified", fmtDate(ent.mtime));
  if (ent.owner) kv("Owner", ent.owner);
  const r = await modal({
    title: "Properties",
    body,
    fields: [{ key: "mode", label: "Permissions (octal — change to chmod)", value: ent.perms || "" }],
    okLabel: "Apply",
  });
  if (r && r.mode && r.mode !== ent.perms) {
    try {
      await inv("fs_chmod", { target: pane.target, path: joinPath(pane.path, name), mode: r.mode });
      toast(`chmod ${r.mode} ${name}`);
      loadPane(pane, pane.path);
    } catch (e) { alertModal("chmod failed", String(e)); }
  }
}

async function remoteSearchDialog(pane) {
  if (!pane.target) return;
  const r = await modal({
    title: `Search on ${pane.target}`,
    message: "Case-insensitive filename search under:\n" + pane.path,
    fields: [{ key: "q", label: "Name contains" }],
    okLabel: "Search",
  });
  if (!r || !r.q) return;
  status("searching…");
  let results;
  try {
    results = await inv("remote_search", { target: pane.target, base: pane.path, query: r.q });
  } catch (e) { status(""); return alertModal("Search failed", String(e)); }
  status("");
  const body = el("div");
  if (!results.length) body.appendChild(el("div", "msg", "No matches."));
  for (const p of results) {
    const row = el("div", "result-row", p);
    row.onclick = async () => {
      closeModal();
      await loadPane(pane, parentPath(p));
      selectByName(pane, baseName(p));
    };
    body.appendChild(row);
  }
  await modal({ title: `${results.length} match(es)`, body, okLabel: "Close" });
}

// ---------------------------------------------------------------- menus
function paneMenuItems(pane) {
  return [
    { label: "New folder…", hint: "F7", action: () => newFolderDialog(pane) },
    { label: "New file…", action: () => newFileDialog(pane) },
    "-",
    { label: "Rename…", hint: "F2", action: () => renameDialog(pane) },
    { label: "Properties & permissions…", action: () => propertiesDialog(pane) },
    "-",
    { label: (pane.showHidden ? "Hide" : "Show") + " hidden files", on: pane.showHidden,
      action: () => { pane.showHidden = !pane.showHidden; renderPane(pane); } },
    { label: "Copy current path", action: () => copyText(pane.path, "path copied") },
    { label: "Open in other pane", action: () => loadPane(otherPane(pane), pane.path) },
    { label: "Mirror other pane's path", action: () => loadPane(pane, otherPane(pane).path) },
    "-",
    { label: "Open terminal here", hint: ">_", action: () => openTerminalFor(pane) },
    pane.target ? { label: "Disconnect " + pane.target, danger: true, action: async () => {
      await inv("master_close", { target: pane.target }).catch(() => {});
      await refreshSessions();
      toast("disconnected " + pane.target);
      goHome(pane);
    } } : null,
  ].filter(Boolean);
}

function rowContextMenu(pane, ent, e) {
  e.preventDefault();
  e.stopPropagation();
  setActivePane(pane);
  if (!pane.sel.has(ent.name)) {
    pane.sel.clear();
    pane.sel.add(ent.name);
    pane.cursor = pane.view.indexOf(ent);
    paintSelection(pane);
  }
  const other = otherPane(pane);
  const many = pane.sel.size > 1;
  const full = joinPath(pane.path, ent.name);
  const items = [];
  if (!many) {
    items.push(ent.is_dir
      ? { label: "Open", hint: "↵", action: () => navInto(pane, ent.name, true) }
      : imgMime(ent.name)
        ? { label: "View image", hint: "↵", action: () => openImageViewer(pane.target, full) }
        : { label: "Edit", hint: "↵", action: () => openEditor(pane.target, full) });
    if (ent.is_dir) {
      items.push({ label: "Open in other pane", action: () => loadPane(other, full) });
      items.push({ label: "Open terminal here", action: () => openTerminal(pane.target, full) });
    }
  }
  items.push({
    label: `Copy to other pane (${pane.sel.size})`,
    hint: "F5",
    action: () => startTransfer(pane, other, [...pane.sel]),
  });
  items.push("-");
  if (!many) {
    items.push({ label: "Rename…", hint: "F2", action: () => renameDialog(pane, ent.name) });
    items.push({ label: "Copy full path", action: () => copyText(full, "path copied") });
    items.push({ label: "Copy name", action: () => copyText(ent.name, "name copied") });
    items.push({ label: "Properties…", action: () => propertiesDialog(pane) });
  } else {
    items.push({ label: "Copy names", action: () => copyText([...pane.sel].join("\n"), "names copied") });
  }
  items.push("-");
  items.push({ label: `Delete (${pane.sel.size})…`, hint: "Del", danger: true,
    action: () => deleteSelected(pane) });
  ctxMenu(items, e.clientX, e.clientY);
}

function bodyContextMenu(pane, e) {
  e.preventDefault();
  setActivePane(pane);
  ctxMenu([
    { label: "Refresh", hint: MOD + "R", action: () => loadPane(pane, pane.path) },
    "-",
    ...paneMenuItems(pane),
  ], e.clientX, e.clientY);
}

// ---------------------------------------------------------------- drag
function clearDropHints() {
  $$(".drop-into").forEach((n) => n.classList.remove("drop-into"));
  $$(".drop-ok").forEach((n) => n.classList.remove("drop-ok"));
}

const mdrag = { press: null, active: false, from: null, names: [], ghost: null, suppressClick: false };

function dragBegin() {
  const { pane, name } = mdrag.press;
  if (!pane.sel.has(name)) {
    pane.sel.clear();
    pane.sel.add(name);
    paintSelection(pane);
  }
  mdrag.active = true;
  mdrag.from = pane;
  mdrag.names = [...pane.sel];
  const g = el("div", "drag-ghost",
    mdrag.names.length === 1 ? mdrag.names[0] : mdrag.names.length + " items");
  document.body.appendChild(g);
  mdrag.ghost = g;
}

function dragCleanup() {
  mdrag.active = false;
  mdrag.from = null;
  if (mdrag.ghost) { mdrag.ghost.remove(); mdrag.ghost = null; }
  clearDropHints();
}

function wireDrag() {
  window.addEventListener("mousemove", (e) => {
    if (mdrag.press && !mdrag.active) {
      if (Math.hypot(e.clientX - mdrag.press.x, e.clientY - mdrag.press.y) > 6) dragBegin();
    }
    if (!mdrag.active) return;
    mdrag.ghost.style.left = e.clientX + 14 + "px";
    mdrag.ghost.style.top = e.clientY + 14 + "px";
    clearDropHints();
    const other = otherPane(mdrag.from);
    const under = document.elementFromPoint(e.clientX, e.clientY);
    if (under && other.body.contains(under)) {
      other.body.classList.add("drop-ok");
      const row = under.closest && under.closest("tr.dir");
      if (row && other.body.contains(row)) row.classList.add("drop-into");
    }
  });

  window.addEventListener("mouseup", (e) => {
    if (mdrag.active) {
      const under = document.elementFromPoint(e.clientX, e.clientY);
      const other = otherPane(mdrag.from);
      if (under && other.body.contains(under)) {
        let destDir = other.path;
        const row = under.closest && under.closest("tr.dir");
        if (row && other.body.contains(row)) destDir = joinPath(other.path, row.dataset.name);
        startTransfer(mdrag.from, other, mdrag.names, destDir);
      }
      mdrag.suppressClick = true;
      dragCleanup();
    }
    mdrag.press = null;
  });
}

// ---------------------------------------------------------------- transfers
let xferSeq = 1;
const queue = new Map();

function queueItem(id, desc, cancellable = true) {
  $("#queue").classList.remove("hidden");
  const item = el("div", "q-item");
  const d = el("div", "desc", desc);
  const bar = el("div", "bar");
  const fill = el("div");
  bar.appendChild(fill);
  const row2 = el("div", "row2");
  const line = el("div", "line", "starting…");
  row2.appendChild(line);
  let cancelBtn = null;
  if (cancellable) {
    cancelBtn = el("button", "btn tiny", "✕");
    cancelBtn.title = "Cancel";
    cancelBtn.onclick = () => inv("xfer_cancel", { id }).catch(() => {});
    row2.appendChild(cancelBtn);
  }
  item.append(d, bar, row2);
  $("#q-items").prepend(item);
  queue.set(id, { node: item, line, fill, row2, cancelBtn, desc });
}

function recordHistory(desc, ok) {
  ui.history.unshift({ desc, ok, ts: Date.now() });
  ui.history = ui.history.slice(0, 60);
  saveUi();
}

function startTransferRaw(sources, destSpec, desc, refresh, retryFn) {
  const id = xferSeq++;
  queueItem(id, desc);
  const q = queue.get(id);
  q.refresh = refresh;
  q.retryFn = retryFn;
  const engine = ui.settings.rsync ? "rsync" : "scp";
  inv("start_transfer", { id, sources, dest: destSpec, engine }).catch((e) => {
    q.node.classList.add("fail");
    q.line.textContent = String(e);
  });
  return id;
}

function startTransfer(from, to, names, destDir) {
  if (!names.length) { toast("nothing selected"); return; }
  if (from.target === null && to.target === null) {
    return alertModal("Transfer",
      "Both panes are Local — point one pane at a server first, or use a local terminal (cp works fine there).");
  }
  const sources = names.map((n) => ({ target: from.target, path: joinPath(from.path, n) }));
  const dir = destDir || to.path;
  const dest = { target: to.target, path: dir.endsWith("/") ? dir : dir + "/" };
  const desc = `${from.target || "local"} → ${to.target || "local"}: ${names.join(", ")}`;
  startTransferRaw(sources, dest, desc,
    () => loadPane(to, to.path),
    () => startTransfer(from, to, names, destDir));
  status(`transferring ${names.length} item(s)…`);
}

function startTarDownload() {
  let from, to;
  if (left.target && !right.target) { from = left; to = right; }
  else if (right.target && !left.target) { from = right; to = left; }
  else return alertModal("Compressed download",
    "Point one pane at a server and the other at Local, then select remote files.");
  const names = [...from.sel];
  if (!names.length) return toast("nothing selected on the server pane");
  const id = xferSeq++;
  queueItem(id, `tar⇣ ${from.target} → local: ${names.join(", ")}`, false);
  const q = queue.get(id);
  q.refresh = () => loadPane(to, to.path);
  q.retryFn = () => startTarDownload();
  inv("compress_download", {
    id,
    target: from.target,
    remoteDir: from.path,
    names,
    localDir: to.path,
  }).catch((e) => {
    q.node.classList.add("fail");
    q.line.textContent = String(e);
  });
}

function wireTransferEvents() {
  listen("xfer-log", (ev) => {
    const q = queue.get(ev.payload.id);
    if (!q) return;
    q.line.textContent = ev.payload.line;
    const m = ev.payload.line.match(/(\d{1,3})%/);
    if (m) q.fill.style.width = Math.min(100, parseInt(m[1], 10)) + "%";
  });
  listen("xfer-done", (ev) => {
    const q = queue.get(ev.payload.id);
    if (!q) return;
    q.node.classList.add(ev.payload.ok ? "done" : "fail");
    if (q.cancelBtn) q.cancelBtn.remove();
    if (ev.payload.ok) {
      q.fill.style.width = "100%";
      q.line.textContent = "done";
      if (q.refresh) q.refresh();
    } else {
      if (q.line.textContent === "starting…") q.line.textContent = "failed";
      if (q.retryFn) {
        const retry = el("button", "btn tiny", "↻ retry");
        retry.onclick = () => { q.node.remove(); queue.delete(ev.payload.id); q.retryFn(); };
        q.row2.appendChild(retry);
      }
    }
    recordHistory(q.desc, ev.payload.ok);
    if (opt("toastXfer", true)) {
      toast(ev.payload.ok ? "transfer complete — " + q.desc : "transfer FAILED — " + q.desc,
        ev.payload.ok ? "" : "error");
    } else {
      status(ev.payload.ok ? "transfer complete" : "transfer failed");
    }
  });
}

function wireQueueButtons() {
  $("#q-clear").onclick = () => {
    $("#q-items").innerHTML = "";
    queue.clear();
    if (!$("#edit-items").children.length) $("#queue").classList.add("hidden");
  };
  $("#q-history").onclick = () => {
    const body = el("div");
    if (!ui.history.length) body.appendChild(el("div", "msg", "No transfers yet."));
    for (const h of ui.history) {
      const row = el("div", "result-row", (h.ok ? "✓ " : "✗ ") + h.desc);
      row.style.cursor = "default";
      body.appendChild(row);
    }
    modal({ title: "Transfer history", body, okLabel: "Close" });
  };
}

// ---------------------------------------------------------------- native drops
function wireFileDrop() {
  const paneAt = (x) => (x < left.root.getBoundingClientRect().right ? left : right);
  const scale = () => window.devicePixelRatio || 1;
  try {
    listen("tauri://drag-over", (ev) => {
      const pos = (ev.payload && ev.payload.position) || { x: 0 };
      const pane = paneAt((pos.x || 0) / scale());
      left.body.classList.toggle("drop-ok", pane === left && left.target !== null);
      right.body.classList.toggle("drop-ok", pane === right && right.target !== null);
    }).catch(() => {});
    listen("tauri://drag-leave", () => clearDropHints()).catch(() => {});
    listen("tauri://drag-drop", (ev) => {
      clearDropHints();
      const paths = (ev.payload && ev.payload.paths) || [];
      const pos = (ev.payload && ev.payload.position) || { x: 0 };
      if (!paths.length) return;
      const pane = paneAt((pos.x || 0) / scale());
      if (pane.target === null) return toast("drop onto the server pane to upload");
      const names = paths.map((p) => baseName(p));
      const sources = paths.map((p) => ({ target: null, path: p }));
      const dest = { target: pane.target, path: pane.path.endsWith("/") ? pane.path : pane.path + "/" };
      startTransferRaw(sources, dest, `local → ${pane.target}: ${names.join(", ")}`,
        () => loadPane(pane, pane.path), null);
      status(`uploading ${names.length} item(s)…`);
    }).catch(() => {});
  } catch (_) {}
}

// ---------------------------------------------------------------- connect watch
// A cluster that wants a password or a 2FA code can only be authenticated
// in a terminal. Once that login lands, the ControlMaster socket goes live
// — these watchers notice within a second or two and bring the file panes
// to the cluster's home directory without the user doing anything.
const connectWatchers = new Map();

function watchConnect(target) {
  if (!target || connectWatchers.has(target)) return;
  const w = { tries: 0, stopped: false };
  connectWatchers.set(target, w);
  const tick = async () => {
    if (w.stopped) return;
    w.tries++;
    let up = false;
    try { up = await inv("host_connected", { target }); } catch (_) {}
    if (w.stopped) return;
    if (up) {
      stopWatch(target);
      return onHostConnected(target);
    }
    // Brisk while you are typing the password, relaxed afterwards;
    // gives up after ~10 minutes rather than polling forever.
    const delay = w.tries < 25 ? 1200 : w.tries < 100 ? 3000 : 8000;
    if (w.tries > 160) return stopWatch(target);
    w.timer = setTimeout(tick, delay);
  };
  w.timer = setTimeout(tick, 1000);
}

function stopWatch(target) {
  const w = connectWatchers.get(target);
  if (w) { w.stopped = true; clearTimeout(w.timer); }
  connectWatchers.delete(target);
}

/// Which idle pane should adopt a host we just logged into? Never one that
/// is already showing another server — only a local pane gets taken over,
/// and only when the preference allows it.
function paneToAdopt() {
  if (!opt("autoPane", true)) return null;
  const inactive = activePane === left ? right : left;
  if (inactive.target === null) return inactive;
  if (activePane && activePane.target === null) return activePane;
  return null;
}

async function onHostConnected(target) {
  await refreshSessions();
  await loadFacts(target);
  loadPlaces(target);
  const facts = hostFacts.get(target);
  const who = facts && facts.user ? ` as ${facts.user}@${facts.hostname}` : "";
  if (facts && facts.scheduler && facts.scheduler !== "none" && !ui.settings.jobsHost) {
    ui.settings.jobsHost = target;
    saveUi();
  }

  // Panes already pointed at this host but stuck on the login screen.
  const waiting = [left, right].filter((p) => p.target === target && (p.needsAuth || !p.path));
  if (waiting.length) {
    for (const p of waiting) await goHome(p);
    updateToolbar(waiting[0]);
    rememberPanes();
    return toast(`${target} connected${who} — showing ${waiting[0].path}`);
  }
  if ([left, right].some((p) => p.target === target)) {
    updateToolbar(left);
    updateToolbar(right);
    return toast(`${target} connected${who}`);
  }

  const pane = paneToAdopt();
  if (!pane) return toast(`${target} connected${who} — ${MOD}K to browse it`);
  pane.target = target;
  pane.hostSel.value = target;
  pane.needsAuth = false;
  updateToolbar(pane);
  await goHome(pane);
  rememberPanes();
  toast(`${target} connected${who} — ${pane === left ? "left" : "right"} pane at ${pane.path}`);
}

// ---------------------------------------------------------------- persistence
const rememberPanes = debounce(() => {
  ui.layoutState = ui.layoutState || {};
  for (const [k, p] of [["left", left], ["right", right]]) {
    if (!p) continue;
    ui.layoutState[k] = { target: p.target, path: p.path };
  }
  saveUi();
}, 700);
