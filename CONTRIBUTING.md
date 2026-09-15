# Contributing to SSH_CLI

Thanks for taking a look. This is a small, deliberately dependency-light
project — a Rust backend and a plain HTML/CSS/JS frontend, no bundler, no npm.

## Getting set up

```bash
git clone https://github.com/Pankajsharma05/SSH_CLI.git
cd SSH_CLI
./build_linux.sh --deps     # Linux, once (apt)
./build_linux.sh            # -> dist/ssh_cli
./dist/ssh_cli
```

macOS: `./build_mac.sh` needs Rust and the Xcode command-line tools.

The first build compiles Tauri and takes several minutes; later builds take
about a minute and a half. The build script also vendors xterm.js and
CodeMirror into `ui/vendor/` (gitignored) on first run — that step needs
network access once.

## Where things live

| Path | What it does |
|---|---|
| `src-tauri/src/main.rs` | every Tauri command — the whole IPC surface |
| `src-tauri/src/term.rs` | PTY sessions: `open_ssh`, `open_local` |
| `src-tauri/src/ops.rs` | listings, transfers, cluster places, host facts, file IO |
| `src-tauri/src/target.rs` | session → ssh flags (ControlMaster, `-J`, `-i`, `-p`) |
| `ui/core.js` | state, helpers, modal/toast/theme/layout/preferences |
| `ui/panes.js` | file panes, selection, transfers, connect-watching |
| `ui/drawer.js` | terminals, editor tabs, image/plot viewers |
| `ui/app.js` | sessions, jobs, command palette, keymap, boot |

[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) explains how the pieces fit,
and why the transport works the way it does.

## House style

- **Match the surrounding code.** Same comment density, same naming, same
  idioms. Comments explain *why*, not *what*.
- Rust: `cargo fmt`, and keep `cargo clippy` quiet. No new dependencies
  without a good reason — the dependency list is short on purpose.
- JS: no frameworks, no build step, no npm. Plain functions and `const`/`let`.
  If you need a library, vendor it in the build script like the others.
- Every new user-facing action wants: a keyboard path, a context-menu entry,
  and a command-palette entry. That consistency is the point of the app.
- Anything that can fail against a remote host should fail *visibly* and
  suggest the next step, rather than printing ssh's raw complaint.

## Testing

There is no test runner in-tree yet. Before opening a PR, please exercise:

- a password/2FA host: pane shows the "not connected" card, then jumps to the
  cluster home after you log in through a terminal tab;
- a transfer in both directions, and one that you cancel;
- all three layouts, including opening a local terminal from `⌘3`;
- the editor (`⌘S` over SSH) and the image viewer's auto-refresh.

If you add UI logic, the frontend can be driven headlessly with jsdom and a
stubbed `window.__TAURI__` — contributions that turn that into a committed
test suite are very welcome.

## Pull requests

- One topic per PR, with a short description of the user-visible change.
- Note which platforms you tested on (Linux / macOS, and what kind of host).
- Update `CHANGELOG.md` under an *Unreleased* heading.

## Good first issues

- BSD/macOS **servers**: remote listings assume GNU `find -printf`.
- Directory synchronize/compare between the two panes.
- An `sbatch` submitter: right-click a job script → set resources → submit.
- A Windows port, which realistically means replacing the OpenSSH-subprocess
  transport with an in-process SSH library (`russh`).
- Quota display (`lfs quota`) beside the disk-free footer.

## Licence

By contributing you agree that your work is released under the
[MIT licence](LICENSE).
