# SSH_CLI on Windows

Windows is supported, and — unlike the Unix builds — it does not drive
`ssh.exe` for file work at all. It speaks SSH **in the app**, the way
WinSCP does.

## Why it works differently here

On macOS and Linux every `ssh`/`scp` the app runs attaches to an OpenSSH
`ControlMaster` socket, so the first login opens a master connection and
every later listing, stat and transfer rides it. Log in once in a
terminal tab — password, 2FA, whatever the cluster wants — and the rest
of the app is authenticated for free.

Microsoft's OpenSSH port has never implemented multiplexing: the control
socket is a Unix domain socket and `ssh.exe` has no equivalent. The
options are accepted and silently do nothing. (Worse, a `Host *` stanza
in `~/.ssh/config` that turns `ControlMaster` on — common if you also use
SSH from WSL or a Mac — makes `ssh.exe` try anyway and abort with
`getsockname failed: Not a socket`.)

So the Windows build does not try. It opens **one TCP connection per
host with libssh2 inside the process**, authenticates once, and then
multiplexes channels over it:

* an **SFTP channel** for every file operation — listings arrive as
  structured attributes, not parsed `ls`/`find` output;
* **exec channels** for the few genuine shell commands (`df`, `squeue`,
  `scontrol`, filename search).

Because the app owns the connection, it owns the login conversation too.
That has a pleasant consequence: **passwords, key passphrases and
one-time codes all work**, prompted in the app itself. You do not need
to set up keys first, and nothing re-authenticates behind your back.

## What you will see

The first time a pane touches a host:

1. **Host key** — on first contact the app shows the server's SHA256
   fingerprint and asks whether to trust it. Check it against what your
   administrators publish. Accepting writes an entry to
   `%USERPROFILE%\.ssh\known_hosts`, the same file `ssh.exe` uses. If a
   *known* key ever changes, the connection is refused outright and you
   are told why.
2. **Authentication**, in the order OpenSSH would try it:
   the `ssh-agent` service → the key configured on the session (asking
   for its passphrase only if the key is encrypted) → keyboard-interactive
   (this is the 2FA/OTP path) → password.
3. That is the last time you are asked. The session stays open, and
   browsing, editing, transfers and the queue panel all use it.

Keys are still the nicest way to live — `ssh-keygen -t ed25519`, then
`Get-Service ssh-agent | Set-Service -StartupType Automatic`,
`Start-Service ssh-agent`, `ssh-add $env:USERPROFILE\.ssh\id_ed25519` —
but they are no longer required.

## What this buys you over the Unix builds

| | Unix (OpenSSH) | Windows (in-process) |
| --- | --- | --- |
| Listings | GNU `find -printf`, parsed | SFTP attributes — works on servers without GNU coreutils |
| Transfer progress | scraped from scp's meter | real byte counts |
| "Is it connected?" | `ssh -O check` on a socket | a live object in memory |
| Auth prompts | in a terminal tab | in the app |

## Remaining differences on Windows

| Area | Behaviour |
| --- | --- |
| **Terminal tabs** | Still run `ssh.exe` under ConPTY, so opening a terminal to a host authenticates **separately** from the file panes — on a 2FA cluster that is a second code. Moving terminals onto the shared session is the next piece of work |
| **ProxyJump** | Not supported by the in-process transport yet. Sessions with a bastion are refused with a clear message; use WSL2 for those |
| **rsync engine** | Unavailable; the queue uses SFTP, which is resumable in practice anyway |
| **Servers without SFTP** | Refused. Every modern OpenSSH ships the SFTP subsystem, but a locked-down host that disables it needs the Unix build |
| Local terminal tabs | ConPTY running PowerShell 7 if present, else Windows PowerShell, else `cmd.exe` |
| `chmod` on local files | Not offered; POSIX mode bits have no meaning on NTFS. Remote `chmod` works (SFTP `SETSTAT`) |
| Local disk-free footer | `GetDiskFreeSpaceExW` instead of `df` |
| Local places menu | Every mounted drive, alongside Home/Desktop/Documents/Downloads |
| Local paths | `C:\Users\you\...`; breadcrumbs start at the drive |
| Console windows | Suppressed (`CREATE_NO_WINDOW`), so listings do not flash a black box |

## Which `ssh.exe` terminal tabs use

Terminals prefer `%SystemRoot%\System32\OpenSSH\ssh.exe` over whatever is
first on `PATH`, because `ssh` on a developer's `PATH` is often Git for
Windows' MSYS2 build, which cannot reach the Windows `ssh-agent` named
pipe. If OpenSSH is missing entirely:

```powershell
Add-WindowsCapability -Online -Name OpenSSH.Client~~~~0.0.1.0
```

## Building

```powershell
.\build_windows.ps1              # -> dist\ssh_cli.exe
.\build_windows.ps1 -Installer   # also an NSIS setup .exe
.\build_windows.ps1 -Install     # copy to %LOCALAPPDATA% + Start menu
```

You need Rust with the MSVC toolchain, the Visual Studio "Desktop
development with C++" workload (libssh2 is built from source, with the
native WinCNG crypto — no OpenSSL), and the WebView2 runtime, which is
already on Windows 11 and current Windows 10.

Tagged releases build this on a `windows-latest` runner, so you do not
need a Windows machine to produce a binary.

## Where this is heading

The in-process transport is currently Windows-only; Linux and macOS keep
the OpenSSH path that has been shipping. If it proves itself here, the
same transport on every platform would remove the external `ssh`/`scp`
dependency altogether, and bring real transfer progress and in-app
authentication everywhere.
