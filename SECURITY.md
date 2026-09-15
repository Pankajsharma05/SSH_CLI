# Security Policy

## Supported versions

| Version | Supported |
|---|---|
| 1.0.x | ✅ |
| internal v1–v3 pre-releases | ❌ |

## Reporting a vulnerability

Please **do not** open a public issue for a security problem. Use GitHub's
[private vulnerability reporting](../../security/advisories/new) on this
repository, or contact the maintainer directly.

Include what you were doing, what happened, and — if you have one — a minimal
reproduction. You'll get an acknowledgement within a few days; this is a
small project maintained in spare time, so please allow reasonable time for a
fix before disclosing publicly.

## How SSH_CLI handles your credentials

Worth knowing before you audit anything:

- **The app never sees your password, passphrase, or OTP.** Authentication is
  performed by the system `ssh` binary inside a PTY, exactly as if you typed
  it in a terminal. Nothing is captured, stored, or forwarded.
- Non-interactive operations (listings, transfers, stats) run with
  `BatchMode=yes`, so they fail fast rather than prompting somewhere you
  cannot see.
- Connection reuse is OpenSSH's own `ControlMaster`. Sockets live in
  `~/.ssh/ssh_cli_sockets/`, created `0700`, and persist for 10 minutes after
  last use (`ControlPersist=600`). *Sessions → disconnect* tears one down
  immediately.
- Saved sessions (`~/.config/ssh_cli/sessions.toml`) hold hostnames, usernames,
  ports, key *paths*, and bastions — never secrets.
- Remote commands are built with POSIX single-quote escaping (`ops::sh_quote`)
  for every user-supplied path.
- The app makes no network connections of its own: no telemetry, no update
  check. The only non-SSH traffic is the one-time CDN fetch of xterm.js and
  CodeMirror performed by the **build script**, not the app.

## Things that are your responsibility

- Deletes are `rm -rf` on the remote host after a confirmation you can turn
  off in Preferences. There is no undo and no trash.
- *Remove old host key & retry* runs `ssh-keygen -R`. Only use it when a host
  key change is expected; verify the new fingerprint with your admins first.
- Releases are **not code-signed or notarized** (see
  [docs/SIGNING.md](docs/SIGNING.md)). If that matters to you, build from
  source — that is why it is MIT.
