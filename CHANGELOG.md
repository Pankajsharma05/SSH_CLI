# Changelog

All notable changes to SSH_CLI are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added — Windows support
- **Windows build** (`build_windows.ps1`, plus a `windows-latest` job in CI and
  the release workflow, producing `ssh_cli.exe` and an NSIS installer).
- `plat.rs` — one home for platform differences: which `ssh.exe` to run,
  spawning children without flashing console windows, default-app openers,
  the local shell, and free-space queries.
- `mux.rs` — connection reuse on Windows, where `ControlMaster` does not exist.
  One persistent `ssh.exe` per host runs a shell on the far end and commands
  are framed into its stdin, so the session still costs one handshake and one
  authentication instead of one per directory listing.
- Local terminal tabs run PowerShell 7 → Windows PowerShell → `cmd.exe`
  through ConPTY, rather than `portable-pty`'s `cmd.exe` default.
- The local pane understands `C:\...` paths: breadcrumbs start at the drive,
  the places menu lists every mounted drive, and the free-space footer reads
  `GetDiskFreeSpaceExW`.
- [`docs/WINDOWS.md`](docs/WINDOWS.md) — why the file panes need an SSH key
  there, how to set one up, and what else differs.

### Fixed
- Windows OpenSSH aborts with `getsockname failed: Not a socket` when a user's
  `~/.ssh/config` enables `ControlMaster` globally. Omitting the app's own
  multiplex flags is not enough, so the Windows build now passes an explicit
  `-o ControlMaster=no -o ControlPath=none` to override the config file.
- `%SystemRoot%\System32\OpenSSH\ssh.exe` is preferred over whatever `ssh`
  is first on `PATH`, which is often Git for Windows' MSYS2 build — that one
  cannot reach the Windows `ssh-agent` named pipe, so agent-held keys silently
  went unused.

### Known limitations on Windows
- The file panes require key-based authentication; passwords and 2FA work in
  terminal tabs only.
- The `rsync` transfer engine is unavailable and falls back to `scp`.
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
