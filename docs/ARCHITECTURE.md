# Architecture

SSH_CLI is a Tauri 2 application: a Rust backend that shells out to OpenSSH,
and a plain HTML/CSS/JS frontend in a WebView. There is no bundler, no npm,
and no framework.

```
┌──────────────────────── WebView (ui/) ────────────────────────┐
│  core.js    state · modal/toast · theme · layout · prefs      │
│  panes.js   file panes · selection · transfers · connect watch│
│  drawer.js  terminals · editor tabs · image & plot viewers    │
│  app.js     sessions · queue panel · command palette · keymap │
└───────────────────────────┬───────────────────────────────────┘
                            │ Tauri IPC (invoke / events)
┌───────────────────────────┴───────────────────────────────────┐
│  main.rs    every #[tauri::command] — the whole IPC surface   │
│  term.rs    PTYs: open_ssh (ssh) · open_local (login shell)   │
│  ops.rs     listings · transfers · places · host facts · IO   │
│  target.rs  session → ssh flags                               │
│  config.rs  sessions.toml · UI state · ~/.ssh/config import   │
│  edit.rs    external-editor watch/upload loop                 │
│  fwd.rs     port forwards                                     │
└───────────────────────────┬───────────────────────────────────┘
                            │ subprocesses
                   ssh · scp · rsync · tar · ssh-keygen
```

## The transport: one connection per host

Everything rests on OpenSSH's connection multiplexing. `target.rs` adds the
same flags to every invocation:

```
-o ControlMaster=auto
-o ControlPath=~/.ssh/ssh_cli_sockets/%C
-o ControlPersist=600
-o ServerAliveInterval=30 -o ServerAliveCountMax=4
```

The first process to reach a host becomes the master and opens a Unix socket;
every later `ssh`/`scp` attaches to it. One TCP connection, one
authentication, shared by terminals, directory listings, stats and transfers.

This is what makes the cluster workflow possible. A login node that wants a
password, a 2FA push or a one-time code can only be satisfied **interactively**
— so the app asks you to do that once, in a terminal tab, and everything else
rides the socket it leaves behind.

Consequences worth understanding:

- Non-interactive work uses `BatchMode=yes` (`ops::ssh_output`) so it fails in
  a second instead of hanging on an invisible prompt.
- "Is this host connected?" is `ssh -O check` — a local socket poke, no
  network traffic. That is what `host_connected` does, and what the pane
  watcher polls.
- Disconnecting is `ssh -O exit`.
- The socket dies 10 minutes after last use, then the next action silently
  re-authenticates if it can (keys/agent) or shows the login card if it
  cannot.

## The login → cluster-home flow

The piece that makes the app feel like it understands clusters:

1. A pane pointed at an unauthenticated host fails its listing. `panes.js`
   asks `host_connected`; if the answer is no, it renders the *"Not
   connected"* card instead of ssh's error text, and calls `watchConnect()`.
2. `watchConnect` polls `host_connected` — briskly (1.2 s) while you are
   plausibly typing a password, then backing off to 3 s and 8 s, giving up
   after about ten minutes.
3. The moment the socket is live, `onHostConnected()` fetches host facts
   (`id -un`, `hostname`, `$HOME`, scheduler), refreshes the places list, and
   navigates every pane that was waiting on that host to its home directory.
   If no pane was showing the host, an *idle local* pane adopts it — never a
   pane already busy with another server.

Terminals call `watchConnect` too, so logging in from anywhere (the tab strip,
the palette, the Sessions dialog) triggers the same behaviour.

## Terminals

`term.rs` spawns everything on a real PTY via `portable-pty`, then streams
bytes to the frontend as `term-data` events, holding back any incomplete UTF-8
tail so multibyte characters never split across chunks.

- `open_ssh` runs `ssh`. When a start directory, a plot prelude, or a session
  startup command is present it builds one remote command —
  `cd <dir>; <prelude>; <startup>; exec $SHELL -l` — and passes `-t` to force
  a TTY.
- `open_local` uses `CommandBuilder::new_default_prog()`, which runs your
  passwd/`$SHELL` login shell with a leading-dash `argv0`, exactly like a
  terminal emulator. Plot support here is environment variables rather than a
  shell prelude.

## Remote file listings

`ops::remote_list` runs one `find` per directory:

```
cd -- <dir> && find . -mindepth 1 -maxdepth 1 \
  -printf '%y\t%Y\t%s\t%T@\t%m\t%u\t%f\n'
```

Tab-separated so names with spaces survive; `%Y` dereferences symlinks so a
link to a directory stays navigable. This is GNU `find` — solid on Linux
clusters, and the main thing standing between the app and BSD/macOS *servers*.

## Transfers

`ops::transfer` picks the cheapest correct mechanism:

| Case | Mechanism |
|---|---|
| same host, single item | `cp -r` **on the server** — no bytes cross the network |
| local ↔ remote | `scp`, or `rsync --partial --info=progress2` when the rsync toggle is on |
| remote → remote | `scp -3`, streamed through this machine, no temp files |
| many small files, remote → local | `tar czf` on the server, one download, unpack locally |

Output is pumped line-by-line on both `\n` and `\r` so scp's carriage-return
progress meter arrives live, and the child handle is kept so a transfer can be
cancelled.

## Plots without a display

`plots_enable` writes a small matplotlib backend (`ssh_cli_mpl.py`, embedded in
the binary with `include_str!`) into `~/.ssh_cli` on the host, and terminals
are started with `MPLBACKEND=module://ssh_cli_mpl`. Each `plt.show()` renders a
PNG into `~/.ssh_cli/plots`; the frontend polls that directory over the
existing connection and opens each new figure as a viewer tab. `plt.ion()`
animations reuse one tab instead of flooding you.

## Frontend conventions

- One global `ui` object holds settings, bookmarks, recents, history and
  layout state; it is debounce-saved to `~/.config/ssh_cli/uistate.json`.
- Every action should be reachable three ways: keyboard, context menu, and
  command palette. `paletteItems()` in `app.js` is the index of everything the
  app can do.
- The keymap has tiers, because a terminal must keep its own chords: inside a
  terminal only `⌘1/2/3`, `Ctrl+\``, `Ctrl+Tab`, `Alt+N` and `Ctrl+Shift+…`
  are intercepted; everything else goes to the shell.

## Portability

Linux and macOS are first-class. Windows is supported with one documented
difference, described below and in [`WINDOWS.md`](WINDOWS.md).

Platform differences are confined to two modules. `plat.rs` holds the small
ones: which `ssh` binary to run, spawning children without flashing a console
window, the default-app opener, the local login shell, free-space queries, and
drive enumeration. `mux.rs` holds the large one.

The large one is that Win32 OpenSSH has never implemented `ControlMaster` —
the control socket is a Unix domain socket and there is no equivalent. Without
it every listing and transfer re-authenticates and `ssh -O check` always fails,
so the login-once model collapses. Merely omitting the flags is not enough
either: `ssh.exe` still reads `~/.ssh/config`, and a global `ControlMaster auto`
stanza makes it try anyway and abort with `getsockname failed: Not a socket`.
The Windows build therefore passes an explicit `-o ControlMaster=no -o
ControlPath=none`.

In its place, `mux.rs` keeps one long-lived `ssh.exe` per host running `/bin/sh`
on the far end, with stdin and stdout on pipes. Each command is written into
that shell framed by a marker line carrying a request id and the exit status,
so the session still costs one handshake and one authentication, and a listing
costs a round trip rather than a full login. `Target::control_opts()` and
`ops::ssh_output()` are the two seams: the rest of the code does not know which
transport it is on.

The cost is that the shell's stdin is a pipe, not a terminal, so it runs under
`BatchMode=yes` and cannot answer a password or 2FA prompt — the Windows file
panes need key-based authentication. Terminal tabs are unaffected; they get a
real ConPTY through `portable-pty` and prompt interactively as before. Bulk
transfers also still spawn their own `scp.exe` and authenticate again, since
pushing file data through a serialized text pipe would be worse than paying
for a second handshake.

The endgame on every platform is to replace the OpenSSH-subprocess transport
with an in-process SSH library such as [`russh`](https://github.com/Eugeny/russh),
holding the authenticated session inside the app and opening real SFTP
channels. That removes `mux.rs` entirely, gives Windows interactive auth
(because the app would own the prompt), and gives real byte-level transfer
progress instead of scraping scp's meter. Until then, WSL2 also runs the Linux
build unchanged.
