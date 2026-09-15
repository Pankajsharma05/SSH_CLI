//! Connection reuse on Windows, where ControlMaster does not exist.
//!
//! On macOS and Linux every `ssh`/`scp` this app runs attaches to a
//! ControlMaster socket, so the whole session costs one TCP handshake
//! and one authentication. Microsoft's OpenSSH port has never
//! implemented multiplexing — the control socket is a Unix domain
//! socket — so on Windows that model is simply unavailable.
//!
//! What this module does instead: keep **one long-lived `ssh.exe` per
//! host** running a plain `/bin/sh` on the far end, with its stdin and
//! stdout wired to pipes. Every command the file manager wants to run
//! (list a directory, stat a file, query the SLURM queue) is written
//! into that shell and framed with a unique marker line that carries
//! the exit status back. One connection, one authentication, and a
//! directory listing costs a round trip instead of a full handshake.
//!
//! What it deliberately does **not** do: file transfers. `scp` still
//! runs as its own process and authenticates again, because pushing
//! bulk data through this shell would mean base64 over a serialized
//! pipe. That is the remaining reason the native transport (russh,
//! with real SFTP channels) is the right long-term fix on every
//! platform — see docs/WINDOWS.md.
//!
//! Because the shell's stdin is a pipe rather than a terminal, it runs
//! with `BatchMode=yes` and cannot answer a password or 2FA prompt.
//! Key-based authentication (ideally with the key loaded into the
//! Windows `ssh-agent` service) is required for the file panes.
//! Terminal tabs are unaffected: those get a real ConPTY and prompt
//! interactively as usual.

#![cfg(windows)]

use crate::plat;
use crate::target::Target;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::windows::process::ExitStatusExt;
use std::process::{Child, ChildStdin, ExitStatus, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

/// Marker that frames one command's output. It carries the request id
/// (so a desynchronised stream can be resynchronised rather than
/// returning another command's output) and the exit status.
const MARK: &str = "@@SSHCLI-MUX:";

/// How long to wait for the first byte after connecting. Generous:
/// this covers DNS, TCP, key exchange and the server's login banner
/// on a busy cluster login node.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// How long a single command may go without producing *any* output
/// before the connection is considered wedged. A recursive `find` over
/// a large scratch directory can be slow but is rarely silent.
const IDLE_TIMEOUT: Duration = Duration::from_secs(90);

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

struct Conn {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
    /// Whatever `ssh.exe` itself wrote to stderr — auth failures, host
    /// key complaints, banners. Only interesting when things break.
    diag: Arc<Mutex<String>>,
}

impl Conn {
    fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

type Registry = Mutex<HashMap<String, Arc<Mutex<Conn>>>>;

fn registry() -> &'static Registry {
    static R: OnceLock<Registry> = OnceLock::new();
    R.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Everything that makes two targets a different connection.
fn key(t: &Target) -> String {
    format!(
        "{}|{:?}|{:?}|{:?}",
        t.destination, t.port, t.jump, t.identity
    )
}

// ---------------------------------------------------------------- lifecycle

fn spawn(t: &Target) -> Result<Conn, String> {
    let mut args = plat::mux_opts();
    args.push("-o".into());
    args.push("BatchMode=yes".into());
    args.push("-o".into());
    args.push("ConnectTimeout=15".into());
    args.extend(t.host_opts("-p"));
    args.push(t.destination.clone());
    args.push("--".into());
    // A bare `sh` reading from the pipe. `exec` replaces the login
    // shell so there is no extra process to reap.
    args.push("exec /bin/sh".into());

    let mut child = plat::cmd(&plat::ssh_exe())
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not start {}: {e}", plat::ssh_exe()))?;

    let stdin = child.stdin.take().ok_or("no stdin on ssh")?;
    let stdout = child.stdout.take().ok_or("no stdout on ssh")?;
    let stderr = child.stderr.take().ok_or("no stderr on ssh")?;

    // Bounded channel: if the UI thread stops draining, the reader
    // blocks rather than buffering a runaway `cat` of a huge file.
    let (tx, rx): (SyncSender<String>, Receiver<String>) = sync_channel(512);
    std::thread::spawn(move || {
        let r = BufReader::new(stdout);
        for line in r.lines() {
            match line {
                Ok(l) => {
                    if tx.send(l).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    let diag = Arc::new(Mutex::new(String::new()));
    let diag2 = diag.clone();
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        let mut s = stderr;
        while let Ok(n) = s.read(&mut buf) {
            if n == 0 {
                break;
            }
            if let Ok(mut g) = diag2.lock() {
                g.push_str(&String::from_utf8_lossy(&buf[..n]));
                // Keep only the tail; a chatty MOTD is not a diagnosis.
                if g.len() > 8192 {
                    let cut = g.len() - 4096;
                    *g = g[cut..].to_string();
                }
            }
        }
    });

    let mut conn = Conn { child, stdin, lines: rx, diag };

    // Handshake. Nothing has authenticated yet at this point — the
    // first marker to come back is proof that it did.
    match exchange(&mut conn, "true", CONNECT_TIMEOUT) {
        Ok(_) => Ok(conn),
        Err(e) => {
            let why = conn.diag.lock().map(|g| g.trim().to_string()).unwrap_or_default();
            conn.kill();
            Err(connect_error(&why, &e))
        }
    }
}

/// Turn an ssh failure into something actionable rather than a raw log.
fn connect_error(diag: &str, fallback: &str) -> String {
    let msg = if diag.is_empty() { fallback } else { diag };
    let lower = msg.to_lowercase();

    if lower.contains("permission denied")
        || lower.contains("batch mode")
        || lower.contains("publickey")
    {
        return format!(
            "{msg}\n\n\
             On Windows the file panes need key-based authentication. \
             Windows OpenSSH has no connection multiplexing, so unlike on \
             macOS and Linux a password typed into a terminal tab cannot be \
             reused for file operations.\n\n\
             Set a key up once, from PowerShell:\n\
             \x20 ssh-keygen -t ed25519\n\
             \x20 Get-Service ssh-agent | Set-Service -StartupType Automatic\n\
             \x20 Start-Service ssh-agent\n\
             \x20 ssh-add $env:USERPROFILE\\.ssh\\id_ed25519\n\
             \x20 type $env:USERPROFILE\\.ssh\\id_ed25519.pub | ssh USER@HOST \
             \"mkdir -p ~/.ssh && cat >> ~/.ssh/authorized_keys\"\n\n\
             Terminal tabs keep working with passwords and 2FA either way."
        );
    }
    if lower.contains("host key") || lower.contains("known_hosts") {
        return format!(
            "{msg}\n\n\
             Use “Forget host key” in the session menu, then open a terminal \
             tab once to accept the new key."
        );
    }
    if lower.contains("getsockname") || lower.contains("muxclient") {
        return format!(
            "{msg}\n\n\
             This is Windows OpenSSH refusing a ControlMaster directive from \
             your ~/.ssh/config. SSH_CLI overrides it per command, so if you \
             are seeing this, something is invoking ssh without the override."
        );
    }
    msg.to_string()
}

/// Hand back the live connection for this target, dialling if needed.
fn get(t: &Target) -> Result<Arc<Mutex<Conn>>, String> {
    let k = key(t);
    {
        let map = registry().lock().map_err(|_| "mux registry poisoned")?;
        if let Some(c) = map.get(&k) {
            return Ok(c.clone());
        }
    }
    // Dial outside the registry lock: a slow handshake must not block
    // every other host's operations.
    let conn = Arc::new(Mutex::new(spawn(t)?));
    let mut map = registry().lock().map_err(|_| "mux registry poisoned")?;
    Ok(map.entry(k).or_insert(conn).clone())
}

fn forget(t: &Target) {
    if let Ok(mut map) = registry().lock() {
        if let Some(c) = map.remove(&key(t)) {
            if let Ok(mut g) = c.lock() {
                g.kill();
            }
        }
    }
}

// ---------------------------------------------------------------- framing

/// Write one command into the shell and read back everything it
/// printed, up to the marker line that carries its exit status.
///
/// stderr is folded into stdout for the duration of the command, so a
/// failing command's message comes back as its output. The framing
/// survives output that does not end in a newline: the marker is
/// searched for anywhere in the line, not just at its start.
fn exchange(conn: &mut Conn, remote_cmd: &str, timeout: Duration) -> Result<(String, i32), String> {
    let id = NEXT_ID.fetch_add(1, Ordering::SeqCst);
    let framed = format!(
        "{{ {cmd}\n}} 2>&1\n__sshcli_rc=$?\nprintf '{mark}{id}:%s@@\\n' \"$__sshcli_rc\"\n",
        cmd = remote_cmd,
        mark = MARK,
        id = id,
    );
    conn.stdin
        .write_all(framed.as_bytes())
        .map_err(|e| format!("connection lost while sending: {e}"))?;
    conn.stdin
        .flush()
        .map_err(|e| format!("connection lost while sending: {e}"))?;

    let want = format!("{MARK}{id}:");
    let mut out = String::new();
    loop {
        let line = match conn.lines.recv_timeout(timeout) {
            Ok(l) => l,
            Err(RecvTimeoutError::Timeout) => {
                return Err("the connection stopped responding".into())
            }
            Err(RecvTimeoutError::Disconnected) => return Err("the connection closed".into()),
        };
        let Some(at) = line.find(&want) else {
            // A marker for an *older* id means a previous command was
            // abandoned mid-flight; skip it rather than returning it.
            if line.contains(MARK) {
                continue;
            }
            out.push_str(&line);
            out.push('\n');
            continue;
        };
        out.push_str(&line[..at]);
        let rest = &line[at + want.len()..];
        let code = rest
            .split("@@")
            .next()
            .and_then(|s| s.trim().parse::<i32>().ok())
            .unwrap_or(0);
        return Ok((out, code));
    }
}

// ---------------------------------------------------------------- public API

/// Run a command on the host and return its combined output plus exit
/// status, shaped like `std::process::Output` so call sites that used
/// to spawn `ssh` directly do not have to change.
///
/// A connection dropped by the server (idle timeout, login node
/// reboot) is retried once on a fresh connection before failing.
pub fn output(t: &Target, remote_cmd: &str) -> Result<Output, String> {
    let (text, code) = match run(t, remote_cmd) {
        Ok(v) => v,
        Err(first) => {
            forget(t);
            run(t, remote_cmd).map_err(|second| {
                if second == first {
                    second
                } else {
                    format!("{first}\n{second}")
                }
            })?
        }
    };
    let bytes = text.into_bytes();
    Ok(Output {
        status: ExitStatus::from_raw(code as u32),
        stderr: if code == 0 { Vec::new() } else { bytes.clone() },
        stdout: bytes,
    })
}

fn run(t: &Target, remote_cmd: &str) -> Result<(String, i32), String> {
    let conn = get(t)?;
    let mut g = conn.lock().map_err(|_| "connection poisoned")?;
    exchange(&mut g, remote_cmd, IDLE_TIMEOUT)
}

/// Is there a working connection to this host right now?
///
/// Called where Unix builds ask `ssh -O check`. It *establishes* the
/// connection rather than merely reporting on one, which is what makes
/// the file pane jump to the remote home directory as soon as the key
/// is available.
pub fn alive(t: &Target) -> bool {
    matches!(run(t, "true"), Ok((_, 0)))
}

/// Is a connection to this host already open?
///
/// Unlike `alive`, this never dials — it only reports on connections
/// that already exist, so drawing the session list does not
/// accidentally authenticate to every saved host.
pub fn established(t: &Target) -> bool {
    let Ok(map) = registry().lock() else { return false };
    let Some(c) = map.get(&key(t)) else { return false };
    let Ok(mut g) = c.try_lock() else {
        // Locked means a command is in flight, which means it is live.
        return true;
    };
    matches!(g.child.try_wait(), Ok(None))
}

/// Tear the connection down — the Windows equivalent of `ssh -O exit`.
pub fn close(t: &Target) {
    forget(t);
}

/// Stream a file to the host through the shared connection.
///
/// The payload travels as base64 inside a quoted here-document, so
/// arbitrary bytes (including the delimiter's own text, quotes and
/// backslashes) survive intact. Writing to a temporary file and
/// renaming it means an interrupted save cannot truncate a job script
/// that is about to be submitted.
pub fn write_file(t: &Target, path: &str, data: &str) -> Result<(), String> {
    let q = crate::ops::sh_quote(path);
    let tmp = crate::ops::sh_quote(&format!("{path}.ssh_cli_tmp"));
    let b64 = crate::ops::base64_encode(data.as_bytes());

    let mut payload = String::with_capacity(b64.len() + b64.len() / 76 + 64);
    for chunk in b64.as_bytes().chunks(76) {
        payload.push_str(&String::from_utf8_lossy(chunk));
        payload.push('\n');
    }

    let cmd = format!(
        "base64 -d > {tmp} <<'@@SSHCLI-B64@@'\n{payload}@@SSHCLI-B64@@\nmv -- {tmp} {q}"
    );
    let (out, code) = run(t, &cmd)?;
    if code == 0 {
        Ok(())
    } else {
        Err(out.trim().to_string())
    }
}

/// Read raw bytes from the host through the shared connection.
pub fn read_file(t: &Target, path: &str, max: usize) -> Result<Vec<u8>, String> {
    let cmd = format!(
        "head -c {} -- {} | base64",
        max + 1,
        crate::ops::sh_quote(path)
    );
    let (out, code) = run(t, &cmd)?;
    if code != 0 {
        return Err(out.trim().to_string());
    }
    crate::ops::base64_decode(&out)
}
