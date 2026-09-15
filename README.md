<img src="src-tauri/icons/128x128@2x.png" width="104" align="right" alt="SSH_CLI icon">

# SSH_CLI

**A dual-pane SSH file manager with real terminals — remote *and* local — for
people who live on clusters.**

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-Linux%20%7C%20macOS%20%7C%20Windows-lightgrey)](#install)
[![Release](https://img.shields.io/badge/release-v1.0.0-brightgreen)](../../releases)

Two file panes (local↔server, server↔server), terminal tabs that behave like a
real terminal emulator, a built-in editor that saves straight back over SSH, an
image viewer that turns `plt.show()` into a tab even on a display-less login
node, and a SLURM queue panel — in one window.

Built with **Tauri 2 + Rust**. The transport is the OpenSSH already on your
machine, driven with `ControlMaster` multiplexing: **one authenticated
connection per host**, shared by every terminal, listing and transfer. Log in
once — with a password, a 2FA push or an OTP — and everything else is free.

No Node/npm toolchain. The frontend is plain HTML/CSS/JS; the build script
vendors xterm.js and CodeMirror for you.

---

## Why it exists

Working on an HPC cluster usually means juggling three windows: a terminal for
`ssh`, another for `scp`, and a text editor that either runs over a laggy X11
forward or forces you to copy files back and forth. Tools that fix this on
Windows (WinSCP) never came to macOS and Linux in a form that understood
clusters — 2FA logins, scratch filesystems, batch queues, and `matplotlib`
figures you can't see.

SSH_CLI is that missing tool: file manager, terminal, editor and plot viewer
sharing **one** SSH connection.

---

## Highlights

**Three window layouts** — `⌘1` files only · `⌘2` files + terminal · `⌘3` the
terminal *is* the window. `Ctrl+\`` toggles full-terminal from anywhere. The
layout is remembered, so SSH_CLI can be your everyday terminal and become a
file manager only when you need one.

**Terminals, remote and local** — real PTYs: vim, htop, tmux, 2FA logins and
inline images all work. The **Local** entry opens your login shell on this
machine in the same tab strip, no SSH hop. Terminals start in the folder the
pane is showing.

**Panes that follow you onto the cluster** — point a pane at a host you have
not authenticated to and you get a *"Not connected — open terminal & log in"*
card, not an ssh error. The app watches the connection while you type your
password and **jumps the pane to your cluster home the moment the login
succeeds**. The places menu (★) is then filled per host by probing what that
site actually has: `$HOME`, `$SCRATCH`, `$WORK`, `$PROJECT`, `/scratch/$USER`,
`/lustre/$USER`, …

**Command palette** — `⌘K` for everything: connect, browse left/right, jump to
a scratch directory, change layout, toggle rsync, focus a tab. Fuzzy-matched.

**Keyboard-first panes** — a real cursor with `↑↓`, `Shift` to extend, `↵` to
open, `⌫` up (landing on the folder you just left), type-ahead jump, `Tab` to
switch panes, `F2` rename, `F5` copy across, `F7` new folder, `Del` delete,
`⌘L` edit path. Breadcrumbs you can click.

**Transfers that survive real life** — scp, or rsync when you want resumable
delta-sync; pure server-side `cp` when both panes are the same host;
`tar⇣` compressed download for thousands of small files; a queue with progress,
cancel, retry and history; drag between panes or in from your desktop.

**Editor & viewers** — double-click any text file, local or remote: CodeMirror
with syntax highlighting for python/shell/SLURM/C++/LaTeX/YAML/TOML/Rust/Julia/
JS/Markdown, `⌘S` writes back over the connection (atomic tmp+mv on servers).
Double-click an image to view it, with auto-refresh when the file changes on
the server — re-run your script and watch the figure update.

**`plt.show()` on a cluster with no display** — turn on **plots** and the app
installs a small matplotlib backend into `~/.ssh_cli` on the host. Your scripts
need no changes: each figure opens as a tab, `plt.ion()` animations reuse one
tab. No X11, no XQuartz, no tunnels.

**Cluster extras** — SLURM panel (`squeue`, tail job output, `scancel`) with a
PBS `qstat` fallback · port forwarding (L/R/D) for Jupyter and friends ·
`~/.ssh/config` import · per-session startup commands · ProxyJump bastions ·
remote filename search · properties + chmod · synchronized browsing.

**Looks** — dark / light / system themes, seven accents plus a colour picker,
three interface sizes, live terminal font zoom, vendored Inter + JetBrains
Mono so it looks the same offline.

See [`docs/SHORTCUTS.md`](docs/SHORTCUTS.md) for the full key map.

---

## Install

### Linux (Ubuntu 22.04 / 24.04 and friends)

```bash
git clone https://github.com/Pankajsharma05/SSH_CLI.git
cd SSH_CLI
./build_linux.sh --deps      # once: webkit2gtk, gtk3, pkg-config … via apt
./build_linux.sh --install   # build + ~/.local/bin + app-menu entry
```

Or produce a package: `./build_linux.sh --deb` → `dist/ssh-cli_1.0.0_<arch>.deb`.

### macOS

```bash
./build_mac.sh --install          # -> /Applications/SSH_CLI.app
./build_mac.sh --universal --dmg  # Apple Silicon + Intel, plus a .dmg
```

Requires Rust (`rustup`) and the Xcode command-line tools. The first build
compiles Tauri (a few minutes); later builds take seconds.

### Prebuilt binaries

Push a `v*` tag and GitHub Actions builds and attaches the artifacts to
[Releases](../../releases): a Linux binary + `.deb`, and a universal macOS
`.app` in a `.dmg`, each with SHA-256 sums.

Build output is not committed to the repository — `dist/` is gitignored, so a
local `./build_linux.sh --deb` gives you `dist/ssh_cli` and
`dist/ssh-cli_1.0.0_amd64.deb` to install or attach to a release by hand.

**They are not code-signed or notarized** — see
[Code signing](#code-signing-and-what-that-means-for-you) below before you
double-click.

### Windows

```powershell
git clone https://github.com/Pankajsharma05/SSH_CLI
cd SSH_CLI
.\build_windows.ps1 -Install     # build + Start-menu entry
```

Needs Rust with the MSVC toolchain, the Visual Studio "Desktop development
with C++" workload, and the OpenSSH client (`Add-WindowsCapability -Online
-Name OpenSSH.Client~~~~0.0.1.0`). Tagged releases also publish a prebuilt
`.exe` and an NSIS installer.

**Read [`docs/WINDOWS.md`](docs/WINDOWS.md) first.** The Windows build works
differently under the hood: Windows' OpenSSH has no `ControlMaster`
multiplexing, so instead of driving `ssh.exe`, it speaks SSH **in-process**
with libssh2 and runs **SFTP channels** over one authenticated connection —
the same approach WinSCP takes. Because the app owns the login conversation,
**passwords, key passphrases and 2FA codes are prompted in the app** and
nothing needs keys set up in advance. Listings come from SFTP attributes
rather than GNU `find`, and transfers report real byte progress.

One wrinkle remains: terminal tabs still run `ssh.exe`, so opening a terminal
authenticates separately from the file panes — on a 2FA cluster, a second
code. ProxyJump bastions are not supported by that transport yet; use WSL2
for those.

---

## Using it

1. **Sessions** → *Add session*: a name, `user@host`, and optionally a port, a
   key, a ProxyJump bastion, and a startup command (`module load …`).
   Sessions live in `~/.config/ssh_cli/sessions.toml`; `~/.ssh/config` aliases
   can be imported in one click.
2. Pick the host in a pane. If it wants a password or an OTP, press **Open
   terminal & log in** — the pane fills itself in when you are through.
3. **→ / ←**, `F5`, or drag to copy between panes.
4. Double-click to edit a file, view an image, or enter a folder.
5. `⌘3` when you just want a terminal, `⌘K` when you forget where anything is.

### Passwords, 2FA, OTP

File operations run non-interactively (`BatchMode=yes`) so they fail fast
instead of hanging on a prompt you cannot see. Authenticate **in a terminal
tab**: that opens the master connection (kept alive ~10 minutes after last
use), and every file operation then rides it with no further authentication.
With keys or an agent, everything works immediately.

---

## Code signing, and what that means for you

**No. This app is not digitally signed.**

| Platform | What ships | What you'll see |
|---|---|---|
| **macOS** | An **ad-hoc** signature (`codesign -s -`), which is *not* an Apple Developer ID identity and is *not* notarized | Gatekeeper: *"SSH_CLI cannot be opened because the developer cannot be verified."* Right-click the app → **Open** → **Open**, once. Or `xattr -dr com.apple.quarantine /Applications/SSH_CLI.app` |
| **Linux** | Unsigned binary; the `.deb` is not GPG-signed | Nothing blocks you; `apt` may note the package is unsigned |
| **Windows** | Unsigned `.exe` and installer — no Authenticode certificate | SmartScreen: *"Windows protected your PC."* **More info** → **Run anyway**, once |
| **Windows** | — | Not supported |

Proper signing needs paid certificates: an Apple Developer Program membership
(US$99/year) for a Developer ID certificate plus notarization, and an
OV/EV code-signing certificate for Windows. The build scripts are ready for it
— [`docs/SIGNING.md`](docs/SIGNING.md) has the exact commands and the CI
secrets to set once you have a certificate.

Until then: **build it yourself from source** if you'd rather not trust an
unsigned download. That is the whole point of it being MIT.

---

## Security notes

- SSH_CLI never handles your credentials. Authentication is done by the system
  `ssh` binary, in a terminal, exactly as if you had typed it there.
- Control sockets live in `~/.ssh/ssh_cli_sockets/` with `0700` permissions and
  persist for 10 minutes after last use. *Sessions → disconnect* closes one
  immediately.
- Deletes are `rm -rf` after a confirmation you can switch off in Preferences.
  Be deliberate.
- The app never phones home. The only network traffic is your SSH connections
  (and the one-time CDN fetch of xterm.js/CodeMirror at build time).

Found a vulnerability? See [`SECURITY.md`](SECURITY.md).

---

## Repository layout

```
src-tauri/src/
  main.rs     Tauri commands — the whole IPC surface
  term.rs     PTY sessions: open_ssh / open_local
  ops.rs      listings, transfers, cluster places, host facts, file IO
  target.rs   session -> ssh flags (ControlMaster, -J, -i, -p)
  plat.rs     platform differences: ssh binary, openers, shells, disks
  mux.rs      Windows only — in-process SSH + SFTP, standing in for
              ControlMaster (one auth per host, framed commands)
  config.rs   sessions.toml, UI state, ~/.ssh/config import
  edit.rs     external-editor watch/upload loop
  fwd.rs      port forwards
  ssh_cli_mpl.py   the matplotlib backend installed onto hosts
ui/
  core.js     state, helpers, modal/toast/theme/layout/preferences
  panes.js    file panes, selection, transfers, connect-watching
  drawer.js   terminals, editor tabs, image/plot viewers
  app.js      sessions, jobs, command palette, keymap, boot
docs/         architecture, shortcuts, signing, Windows notes
demo/         scripts to try the plot features against
```

More detail in [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md).

---

## Contributing

Issues and pull requests are welcome — see [`CONTRIBUTING.md`](CONTRIBUTING.md).
Good first areas: BSD/macOS *servers* (remote listings assume GNU `find`),
moving the transport to an in-process SSH library (russh) so Windows gets
interactive auth and transfers get pipelined, directory synchronize/compare,
and a proper `sbatch` submitter.

---

## Credits


The implementation was written with **[Claude Code](https://claude.com/claude-code)**
(Anthropic) pair-programming.

Third-party components, each under its own license:
[Tauri](https://tauri.app) (MIT/Apache-2.0) ·
[xterm.js](https://xtermjs.org) (MIT) ·
[CodeMirror 5](https://codemirror.net/5/) (MIT) ·
[portable-pty](https://github.com/wezterm/wezterm) (MIT) ·
[Inter](https://rsms.me/inter/) and
[JetBrains Mono](https://www.jetbrains.com/lp/mono/) (SIL OFL 1.1).
The heavy lifting is done by [OpenSSH](https://www.openssh.com/).

## License

[MIT](LICENSE) © 2026 Pankaj Sharma
