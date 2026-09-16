# Changelog

All notable changes to SSH_CLI are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/).

## [1.1.0] — 2026-09-16

Windows support, built on a different transport from the Unix builds.

### Added
- **Windows build** — `build_windows.ps1`, a `windows-latest` job in CI and in
  the release workflow, producing `ssh_cli.exe`.
- `mux.rs` — an **in-process SSH transport**, because Windows OpenSSH has no
  `ControlMaster` and the login-once model cannot be built out of `ssh.exe`
  there. libssh2 opens one connection per host and multiplexes an SFTP
  channel, exec channels and terminal PTY channels over it, the way WinSCP
  does. libssh2 uses the native WinCNG crypto, so no OpenSSL is pulled in.
- **One login serves everything.** Because the app owns the connection it owns
  the login: passwords, key passphrases and keyboard-interactive 2FA are
  prompted in the app, and the file panes, transfers and terminal tabs all
  ride the same authenticated session. Keys are not required.
- File operations are SFTP requests rather than shell commands: listings come
  from directory attributes (no GNU `find` dependency, so odd servers work),
  transfers report real byte progress and cancel mid-copy, `chmod` is an
  SFTP `SETSTAT`.
- Host keys are checked against `%USERPROFILE%\.ssh\known_hosts`, with a
  fingerprint prompt on first contact. The check is algorithm-aware: a stored
  key of the *same* type that differs is refused, while a host known only by a
  type this build cannot negotiate is offered for review — which is what
  OpenSSH does. Trusted keys are **appended**, never rewritten, so the file
  stays sound for `ssh.exe`.
- `plat.rs` — one home for the smaller platform differences: which `ssh.exe`
  to run, spawning without flashing console windows, the default-app opener,
  the local shell, free-space queries, drive enumeration.
- Local terminal tabs run PowerShell 7 → Windows PowerShell → `cmd.exe`
  through ConPTY.
- The local pane understands `C:\...` paths: breadcrumbs start at the drive,
  the places menu lists every mounted drive, and the free-space footer reads
  `GetDiskFreeSpaceExW`.
- [`docs/WINDOWS.md`](docs/WINDOWS.md) explains the transport and what differs.

### Fixed
- A failure during start-up left a blank window with no explanation. Uncaught
  errors now paint a readable panel; after start-up they are a toast, so one
  denied call cannot replace the interface.
- The window could not be dragged by its title bar — the capability grants
  `core:default`, which does not include `core:window:allow-start-dragging`.

### Known limitations on Windows
- ProxyJump bastions are not supported by the in-process transport; those
  sessions are refused with an explanation. WSL2 remains the workaround.
- Servers with the SFTP subsystem disabled are refused.
- The `rsync` engine is unavailable; SFTP is used instead.
- Local `chmod` is not offered (no POSIX mode bits on NTFS).

## [1.0.0] — 2026-09-15

First public release. It consolidates three internal iterations (v1–v3) into
one codebase.

### Added — window & terminals
- **Three window layouts**: files only (`⌘1`), files + terminal (`⌘2`), and a
  **full terminal panel** where the terminal is the whole window (`⌘3`).
  `Ctrl+\`` toggles full-terminal from anywhere; the choice is remembered.
- **Local terminal mode** — a real PTY running your login shell on this
  machine, in the same tab strip as remote sessions, with no SSH hop.
- Terminals start in the folder the pane is showing (remote and local).
- Tab management: `⌘T` new, `⌘W` close, `Ctrl+Tab` cycle, `Alt+1…9` jump,
  middle-click close, and a context menu with duplicate / rename /
  copy-all-output / clear / close-others.
- `Ctrl+Shift+C` / `Ctrl+Shift+V` copy and paste; `⌘+` / `⌘−` / `⌘0` resize the
  terminal font live.
- An empty-panel state with one-click buttons for your saved hosts.

### Added — clusters
- **Panes follow you onto a cluster**: a host you have not authenticated to
  shows a *"Not connected — open terminal & log in"* card, and the pane jumps
  to your cluster home the moment the login succeeds. An idle local pane will
  adopt a host you log into.
- **Per-host places menu** built by probing what the site actually has:
  `$HOME`, `$SCRATCH`, `$WORK`, `$PROJECT`, `/scratch/$USER`, `/lustre/$USER`,
  and friends — only directories that exist are listed.
- Host facts in the pane header (`user@loginnode`, scheduler detection).
- PBS `qstat` fallback in the queue panel alongside SLURM, and an
  *open in editor tab* button for job output.

### Added — ergonomics
- **Command palette** (`⌘K`): connect, browse, jump to a folder, change
  layout, toggle settings, focus a tab — fuzzy-matched.
- **Keyboard-first file panes**: cursor with `↑↓`, `Shift` to extend, `↵` to
  open, `⌫` up (landing on the folder you just left), type-ahead jump, `Tab`
  to switch panes, `F2`/`F5`/`F7`/`Del`, `⌘L` edit path, `⌘H` home, `⌘D`
  bookmark.
- **Clickable breadcrumbs** with a right-click menu per segment.
- Toast notifications for transfers, connections and errors.
- Session restore: hosts, folders, split width and panel height.
- Preferences dialog with startup/pane, terminal, and transfer sections.
- Pane toolbar tidied; rarely used actions moved to a `⋯` menu.
- New app icon.

### Carried over from the internal versions
Dual-pane transfers (scp / rsync / server-side `cp` / `tar⇣` bulk download)
with queue, progress, cancel, retry and history · drag between panes and from
the desktop · built-in CodeMirror editor with save-over-SSH · image viewer
with auto-refresh · `plt.show()` on display-less clusters via a bundled
matplotlib backend · SLURM panel · port forwarding · `~/.ssh/config` import ·
dark/light/system themes with seven accents and a colour picker.

### Known limitations
- Windows is not supported: its OpenSSH has no `ControlMaster` multiplexing,
  which the whole design rests on. WSL2 works.
- Remote listings use GNU `find`; BSD/macOS *servers* need adjustment.
- Releases are **not code-signed** — see [docs/SIGNING.md](docs/SIGNING.md).

[1.0.0]: ../../releases/tag/v1.0.0
