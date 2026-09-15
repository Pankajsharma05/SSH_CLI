# SSH_CLI on Windows

Windows is supported, with one architectural difference you need to
know about before the file panes will work.

## The short version

**Set up an SSH key and load it into the Windows ssh-agent service.**
Terminal tabs work with passwords and 2FA exactly as on macOS and
Linux, but the file panes, transfers and the SLURM panel need key-based
authentication. The rest of this page explains why, and how.

## Why keys are required here and not elsewhere

SSH_CLI is built on connection reuse. On macOS and Linux it passes
`ControlMaster=auto` to every `ssh` and `scp` it runs, so the first
login opens a master connection and everything afterwards — every
directory listing, every `stat`, every transfer — rides that already
authenticated session. That is what makes logging in once in a terminal
tab enough for the whole app, password or OTP included.

Microsoft's OpenSSH port has never implemented multiplexing. The
control socket is a Unix domain socket, and `ssh.exe` has no equivalent.
The options are accepted and silently do nothing; worse, if your
`~/.ssh/config` sets `ControlMaster`/`ControlPath` in a `Host *` stanza
(common if you also use SSH from WSL, Git Bash or a Mac), `ssh.exe`
tries anyway and dies with `getsockname failed: Not a socket`. SSH_CLI
passes an explicit `-o ControlMaster=no -o ControlPath=none` on Windows
so your global config cannot break the app's own connections.

In place of ControlMaster, the Windows build keeps **one long-lived
`ssh.exe` per host** running a shell on the far end, and writes framed
commands into its stdin (`src-tauri/src/mux.rs`). That recovers the
important property — one TCP handshake and one authentication per host,
so browsing stays fast — but the shell's stdin is a pipe, not a
terminal, so it cannot answer a password or verification-code prompt.
Hence: keys.

File transfers still run `scp.exe` as a separate process and
authenticate again per transfer. With an agent-held key that is
invisible; without one it is a prompt you cannot answer.

## Setting up a key

From any PowerShell prompt:

```powershell
# 1. make a key (press Enter through the prompts, or set a passphrase)
ssh-keygen -t ed25519

# 2. turn on the agent so the passphrase is asked at most once per boot
Get-Service ssh-agent | Set-Service -StartupType Automatic
Start-Service ssh-agent
ssh-add $env:USERPROFILE\.ssh\id_ed25519

# 3. install the public half on the server (one password prompt, here)
type $env:USERPROFILE\.ssh\id_ed25519.pub |
  ssh USER@HOST "mkdir -p ~/.ssh && cat >> ~/.ssh/authorized_keys && chmod 700 ~/.ssh && chmod 600 ~/.ssh/authorized_keys"
```

Check it took:

```powershell
ssh -o BatchMode=yes USER@HOST true   # silence means success
```

If that command prints `Permission denied`, SSH_CLI's file panes will
fail the same way, and the app's error message will say so.

### If your cluster requires 2FA on every login

Some sites reject key-only authentication or demand an OTP each time.
There is no way around that on Windows today: without multiplexing,
every operation is a fresh login, and prompting per directory listing is
not usable. Options, in order of how well they work:

1. Ask whether the site allows key-based access from a registered
   machine — many do, and that is what the Linux/macOS builds are
   effectively relying on once the master is up.
2. Run SSH_CLI inside WSL2 (it is the normal Linux build there, with
   full ControlMaster support) and use WSLg to display it.
3. Use terminal tabs only — they prompt interactively and work fine.

## Which `ssh.exe` gets used

SSH_CLI prefers `%SystemRoot%\System32\OpenSSH\ssh.exe` over whatever
is first on `PATH`. This matters: `ssh` on a developer's PATH is very
often Git for Windows' MSYS2 build, which cannot talk to the Windows
`ssh-agent` service, because that agent lives behind a named pipe the
MSYS2 binary does not speak. Preferring the inbox client is what makes
agent-held keys actually work.

If OpenSSH is missing entirely:

```powershell
Add-WindowsCapability -Online -Name OpenSSH.Client~~~~0.0.1.0
```

## Other differences from the Unix builds

| Area | Behaviour on Windows |
| --- | --- |
| Local terminal tabs | ConPTY, running PowerShell 7 if present, else Windows PowerShell, else `cmd.exe` |
| rsync transfer engine | Unavailable — the queue silently uses `scp`. There is no rsync in the OpenSSH package, and a Git-Bash rsync mangles drive-letter paths |
| `chmod` on local files | Not offered; POSIX mode bits have no meaning on NTFS. Remote `chmod` works normally |
| Local disk-free footer | Reads `GetDiskFreeSpaceExW` instead of `df` |
| Local places menu | Adds every mounted drive alongside Home/Desktop/Documents/Downloads |
| Local file paths | `C:\Users\you\...`; the breadcrumb bar starts at the drive rather than `/` |
| Console windows | Suppressed (`CREATE_NO_WINDOW`) — without this every directory listing would flash a black window |

## Building

See `build_windows.ps1`. You need Rust with the MSVC toolchain, the
Visual Studio "Desktop development with C++" workload, and the WebView2
runtime (already present on Windows 11 and current Windows 10).

```powershell
.\build_windows.ps1              # -> dist\ssh_cli.exe
.\build_windows.ps1 -Installer   # also an NSIS setup .exe
.\build_windows.ps1 -Install     # copy to %LOCALAPPDATA% + Start menu
```

Tagged releases build this automatically on a `windows-latest` runner,
so you do not need a Windows machine to produce a binary.

## The long-term fix

All of the above is a consequence of shelling out to the system OpenSSH
client. The planned v2 transport — [russh](https://github.com/Eugeny/russh)
with native SFTP channels — opens one TCP connection, authenticates
once, and multiplexes channels itself, in-process. That removes the
external `ssh`/`scp` dependency on every platform and makes Windows a
first-class target with interactive password and keyboard-interactive
auth, because the app would own the prompt. It also makes transfers
pipelined rather than one `scp` per file.
