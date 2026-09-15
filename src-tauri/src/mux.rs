//! In-process SSH transport — the WinSCP model.
//!
//! Windows OpenSSH has never implemented `ControlMaster`, so the
//! login-once-and-reuse-it design the rest of the app depends on cannot
//! be built out of `ssh.exe` there. Rather than fake it by keeping a
//! shell on a pipe (which works, but can never answer a password or a
//! one-time-code prompt), this module does what WinSCP does: it speaks
//! SSH **in this process**, using libssh2.
//!
//! One TCP connection per host, authenticated once, carrying:
//!
//! * an **SFTP channel** for every file operation — listings come back
//!   as structured attributes rather than parsed `find` output, which
//!   also means this path does not care whether the server has GNU
//!   coreutils;
//! * **exec channels** for the handful of things that really are shell
//!   commands (`df`, `squeue`, `scontrol`, filename search).
//!
//! Because the app owns the connection, it also owns the authentication
//! conversation: agent keys, key files, passwords and keyboard-interactive
//! challenges all work, with prompts shown in the UI. That is what makes
//! 2FA usable on Windows, and it means "is this host connected?" is a
//! question about a live object rather than a login attempt — so nothing
//! here hammers a cluster's auth log.
//!
//! Unix keeps using OpenSSH with ControlMaster (see `ops.rs`); this
//! module is compiled only on Windows.

use crate::target::Target;
use ssh2::{
    CheckResult, FileStat, KeyboardInteractivePrompt, KnownHostFileKind, KnownHostKeyFormat,
    Prompt, Session, Sftp,
};
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tauri::{AppHandle, Emitter};

/// Applies to the TCP connect and to every blocking libssh2 call. A busy
/// login node can take a while to answer; a dead one should not hang the
/// UI forever.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const OP_TIMEOUT_MS: u32 = 60_000;

/// How long a prompt waits for the human before giving up.
const PROMPT_TIMEOUT: Duration = Duration::from_secs(180);

// ---------------------------------------------------------------- app handle
// The transport needs to talk to the UI (auth prompts, host-key trust),
// and it is called from places that have no `AppHandle` to hand.

static APP: OnceLock<AppHandle> = OnceLock::new();

pub fn init(app: AppHandle) {
    let _ = APP.set(app);
}

fn app() -> Option<&'static AppHandle> {
    APP.get()
}

// ---------------------------------------------------------------- prompting
// Rust asks a question, the WebView answers it. `ask` blocks the calling
// worker thread until `answer` is called from the `auth_reply` command.

static NEXT_PROMPT: AtomicU64 = AtomicU64::new(1);

type Pending = Mutex<HashMap<u64, SyncSender<Option<String>>>>;

fn pending() -> &'static Pending {
    static P: OnceLock<Pending> = OnceLock::new();
    P.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Answer an outstanding prompt. `None` means the user cancelled.
pub fn answer(id: u64, text: Option<String>) {
    let tx = pending().lock().ok().and_then(|mut m| m.remove(&id));
    if let Some(tx) = tx {
        let _ = tx.send(text);
    }
}

/// Put a question to the user and wait for it. Returns `None` if they
/// cancel, if nobody is listening, or if they never answer.
fn ask(kind: &str, target: &str, title: &str, prompt: &str, echo: bool) -> Option<String> {
    let app = app()?;
    let id = NEXT_PROMPT.fetch_add(1, Ordering::SeqCst);
    let (tx, rx) = sync_channel(1);
    pending().lock().ok()?.insert(id, tx);
    let sent = app.emit(
        "auth-prompt",
        serde_json::json!({
            "id": id,
            "kind": kind,
            "target": target,
            "title": title,
            "prompt": prompt,
            "echo": echo,
        }),
    );
    if sent.is_err() {
        pending().lock().ok()?.remove(&id);
        return None;
    }
    match rx.recv_timeout(PROMPT_TIMEOUT) {
        Ok(v) => v,
        Err(_) => {
            pending().lock().ok()?.remove(&id);
            None
        }
    }
}

// ---------------------------------------------------------------- connections

/// One authenticated host connection: the SSH session and its SFTP
/// channel, which is opened once and reused.
pub struct Conn {
    sess: Session,
    sftp: Sftp,
    /// `realpath(".")` right after login — the user's home directory,
    /// which the file panes want before anything else.
    pub home: String,
}

type Registry = Mutex<HashMap<String, Arc<Mutex<Conn>>>>;

fn registry() -> &'static Registry {
    static R: OnceLock<Registry> = OnceLock::new();
    R.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Everything that makes two targets a different connection.
fn key(t: &Target) -> String {
    format!("{}|{:?}|{:?}|{:?}", t.destination, t.port, t.jump, t.identity)
}

/// Split `user@host` into its parts, defaulting the user to the Windows
/// account name the way OpenSSH defaults it to the local user.
fn user_host(t: &Target) -> (String, String) {
    match t.destination.split_once('@') {
        Some((u, h)) => (u.to_string(), h.to_string()),
        None => (
            std::env::var("USERNAME").unwrap_or_else(|_| "user".into()),
            t.destination.clone(),
        ),
    }
}

fn connect(t: &Target) -> Result<Conn, String> {
    if t.jump.is_some() {
        return Err(
            "This host is configured with a ProxyJump bastion, which the Windows \
             transport cannot open yet. Connect to the bastion directly, or use \
             the Linux/macOS build (or WSL2) for jumped hosts."
                .into(),
        );
    }
    let (user, host) = user_host(t);
    let port = t.port.unwrap_or(22);

    // Resolve first so a bad hostname is a clear error rather than a timeout.
    let addr = (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|e| format!("cannot resolve {host}: {e}"))?
        .next()
        .ok_or_else(|| format!("no address for {host}"))?;
    let tcp = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT)
        .map_err(|e| format!("cannot reach {host}:{port} — {e}"))?;
    tcp.set_nodelay(true).ok();

    let mut sess = Session::new().map_err(|e| format!("libssh2: {e}"))?;
    sess.set_timeout(OP_TIMEOUT_MS);
    sess.set_tcp_stream(tcp);
    sess.handshake()
        .map_err(|e| format!("SSH handshake with {host} failed: {e}"))?;

    verify_host(&sess, &host, port)?;
    authenticate(&sess, &user, &host, t)?;

    let sftp = sess.sftp().map_err(|e| {
        format!(
            "connected to {host}, but its SFTP subsystem did not start: {e}\n\n\
             The Windows build browses files over SFTP. If this server genuinely \
             has SFTP disabled, use the Linux/macOS build, which drives scp/ssh \
             directly."
        )
    })?;
    let home = sftp
        .realpath(Path::new("."))
        .map(|p| to_remote_string(&p))
        .unwrap_or_else(|_| format!("/home/{user}"));

    Ok(Conn { sess, sftp, home })
}

/// Check the server's key against `~/.ssh/known_hosts`, asking the user
/// on first contact and refusing outright if a known key changed.
fn verify_host(sess: &Session, host: &str, port: u16) -> Result<(), String> {
    let (key, key_type) = sess.host_key().ok_or("server presented no host key")?;
    let mut known = sess
        .known_hosts()
        .map_err(|e| format!("known_hosts: {e}"))?;
    let path = dirs::home_dir()
        .ok_or("no home directory")?
        .join(".ssh")
        .join("known_hosts");
    // A missing file is normal on a fresh machine.
    let _ = known.read_file(&path, KnownHostFileKind::OpenSSH);

    let fingerprint = sess
        .host_key_hash(ssh2::HashType::Sha256)
        .map(|h| format!("SHA256:{}", b64_std(h)))
        .unwrap_or_else(|| "unknown".into());

    match known.check_port(host, port, key) {
        CheckResult::Match => Ok(()),
        CheckResult::NotFound => {
            let answer = ask(
                "hostkey",
                host,
                &format!("Trust {host}?"),
                &format!(
                    "This is the first connection to {host}:{port}.\n\n\
                     Its {key_type:?} key fingerprint is:\n  {fingerprint}\n\n\
                     Check it against what your administrators publish. \
                     Type yes to trust this host from now on."
                ),
                true,
            );
            if !matches!(answer.as_deref(), Some("yes") | Some("YES") | Some("Yes")) {
                return Err(format!("host key for {host} was not accepted"));
            }
            let entry = if port == 22 {
                host.to_string()
            } else {
                format!("[{host}]:{port}")
            };
            known
                .add(
                    &entry,
                    key,
                    "added by SSH_CLI",
                    KnownHostKeyFormat::from(key_type),
                )
                .map_err(|e| format!("could not record the host key: {e}"))?;
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            known
                .write_file(&path, KnownHostFileKind::OpenSSH)
                .map_err(|e| format!("could not write {}: {e}", path.display()))?;
            Ok(())
        }
        CheckResult::Mismatch => Err(format!(
            "WARNING: the host key for {host} does not match the one in \
             known_hosts.\n\nOffered fingerprint: {fingerprint}\n\n\
             This is expected after a server rebuild, and is also exactly what \
             a machine-in-the-middle looks like. Verify the fingerprint with \
             your administrators, then use “Remove old host key” in the session \
             menu if it is genuinely a new key."
        )),
        CheckResult::Failure => Err("could not check the host key".into()),
    }
}

/// Agent first, then a configured key, then whatever the server will
/// accept interactively. This mirrors what OpenSSH does, so a host that
/// works in a terminal works here.
fn authenticate(sess: &Session, user: &str, host: &str, t: &Target) -> Result<(), String> {
    if sess.userauth_agent(user).is_ok() && sess.authenticated() {
        return Ok(());
    }

    if let Some(id) = &t.identity {
        let key = PathBuf::from(crate::config::expand_tilde(id));
        if key.exists() {
            let pubkey = key.with_extension("pub");
            let pubkey = pubkey.exists().then_some(pubkey);
            // Unencrypted key first; only ask for a passphrase if needed.
            if sess
                .userauth_pubkey_file(user, pubkey.as_deref(), &key, None)
                .is_ok()
                && sess.authenticated()
            {
                return Ok(());
            }
            if let Some(pass) = ask(
                "passphrase",
                host,
                &format!("Unlock {}", key.display()),
                "Enter the passphrase for this private key.",
                false,
            ) {
                if sess
                    .userauth_pubkey_file(user, pubkey.as_deref(), &key, Some(&pass))
                    .is_ok()
                    && sess.authenticated()
                {
                    return Ok(());
                }
            }
        }
    }

    let methods = sess.auth_methods(user).unwrap_or("").to_string();

    if methods.contains("keyboard-interactive") {
        let mut prompter = UiPrompter { host: host.to_string(), cancelled: false };
        if sess
            .userauth_keyboard_interactive(user, &mut prompter)
            .is_ok()
            && sess.authenticated()
        {
            return Ok(());
        }
        if prompter.cancelled {
            return Err("authentication cancelled".into());
        }
    }

    if methods.contains("password") {
        if let Some(pass) = ask(
            "password",
            host,
            &format!("Password for {user}@{host}"),
            "This is sent straight to the server over the encrypted connection.",
            false,
        ) {
            if sess.userauth_password(user, &pass).is_ok() && sess.authenticated() {
                return Ok(());
            }
            return Err(format!("{user}@{host}: password rejected"));
        }
        return Err("authentication cancelled".into());
    }

    if sess.authenticated() {
        return Ok(());
    }
    Err(format!(
        "could not authenticate to {user}@{host}. The server offered: {}",
        if methods.is_empty() { "nothing usable" } else { &methods }
    ))
}

/// Feeds the server's keyboard-interactive challenges to the UI. This is
/// the path a 2FA/OTP login takes, and the reason the whole in-process
/// transport is worth the trouble.
struct UiPrompter {
    host: String,
    cancelled: bool,
}

impl KeyboardInteractivePrompt for UiPrompter {
    fn prompt<'a>(
        &mut self,
        username: &str,
        instructions: &str,
        prompts: &[Prompt<'a>],
    ) -> Vec<String> {
        let mut out = Vec::with_capacity(prompts.len());
        for p in prompts {
            let text = p.text.trim();
            let body = if instructions.trim().is_empty() {
                format!("{username}@{}", self.host)
            } else {
                format!("{}\n\n{username}@{}", instructions.trim(), self.host)
            };
            match ask(
                "challenge",
                &self.host,
                if text.is_empty() { "Authentication" } else { text },
                &body,
                p.echo,
            ) {
                Some(v) => out.push(v),
                None => {
                    self.cancelled = true;
                    out.push(String::new());
                }
            }
        }
        out
    }
}

/// One dial at a time per host.
///
/// Pointing a pane at a host fires several requests at once — the home
/// directory, the places list, the host facts — and without this they
/// would each find an empty registry and start their own login, so the
/// user would be asked for the same password two or three times over.
fn dial_gate(k: &str) -> Arc<Mutex<()>> {
    static G: OnceLock<Mutex<HashMap<String, Arc<Mutex<()>>>>> = OnceLock::new();
    let gates = G.get_or_init(|| Mutex::new(HashMap::new()));
    let mut m = gates.lock().unwrap_or_else(|e| e.into_inner());
    m.entry(k.to_string())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

/// Hand back the connection for this target, dialling (and prompting) if
/// there is not one yet.
fn get(t: &Target) -> Result<Arc<Mutex<Conn>>, String> {
    let k = key(t);
    let lookup = || registry().lock().ok().and_then(|m| m.get(&k).cloned());
    if let Some(c) = lookup() {
        return Ok(c);
    }

    // Dial outside the registry lock, so a slow login does not freeze
    // operations on other hosts — but behind this host's own gate, so
    // only one login happens.
    let gate = dial_gate(&k);
    let _dialling = gate.lock().map_err(|_| "dial gate poisoned")?;

    // Someone may have finished connecting while we waited for the gate.
    if let Some(c) = lookup() {
        return Ok(c);
    }
    let conn = Arc::new(Mutex::new(connect(t)?));
    let mut map = registry().lock().map_err(|_| "connection registry poisoned")?;
    Ok(map.entry(k).or_insert(conn).clone())
}

/// Drop a connection — the equivalent of `ssh -O exit`.
pub fn close(t: &Target) {
    if let Ok(mut map) = registry().lock() {
        map.remove(&key(t));
    }
}

/// Is there a live, authenticated connection to this host?
///
/// This never dials and never authenticates: it reports on what already
/// exists. The pane watcher polls this, so it has to stay free.
pub fn alive(t: &Target) -> bool {
    let Ok(map) = registry().lock() else { return false };
    let Some(c) = map.get(&key(t)).cloned() else { return false };
    drop(map);
    // Bound to a local so the guard's temporary ends before `c` does.
    let live = match c.try_lock() {
        // Busy means a command is in flight, which means it is live.
        Err(_) => true,
        Ok(g) => g.sess.authenticated(),
    };
    live
}

/// Same question, and the answer the session list wants too.
pub fn established(t: &Target) -> bool {
    alive(t)
}


/// Run something with the connection, retrying once on a fresh one if the
/// link turns out to be stale.
fn with<T>(t: &Target, mut f: impl FnMut(&mut Conn) -> Result<T, String>) -> Result<T, String> {
    let conn = get(t)?;
    let first = {
        let mut g = conn.lock().map_err(|_| "connection poisoned")?;
        f(&mut g)
    };
    match first {
        Ok(v) => Ok(v),
        Err(e) if is_disconnect(&e) => {
            close(t);
            let conn = get(t)?;
            let mut g = conn.lock().map_err(|_| "connection poisoned")?;
            f(&mut g)
        }
        Err(e) => Err(e),
    }
}

fn is_disconnect(e: &str) -> bool {
    let l = e.to_lowercase();
    l.contains("would block")
        || l.contains("timeout")
        || l.contains("timed out")
        || l.contains("broken pipe")
        || l.contains("socket disconnect")
        || l.contains("socket send")
        || l.contains("socket recv")
        || l.contains("connection reset")
        || l.contains("sftp protocol error")
}

// ---------------------------------------------------------------- exec

/// Run a shell command on the host, shaped like `std::process::Output`
/// so the Unix call sites in `ops.rs` do not have to change.
pub fn output(t: &Target, remote_cmd: &str) -> Result<Output, String> {
    let (text, code) = with(t, |c| {
        let mut ch = c
            .sess
            .channel_session()
            .map_err(|e| format!("opening a channel: {e}"))?;
        ch.exec(remote_cmd).map_err(|e| format!("exec: {e}"))?;
        let mut out = String::new();
        ch.read_to_string(&mut out).map_err(|e| format!("read: {e}"))?;
        // Fold stderr in, the way the Unix path does.
        let mut err = String::new();
        let _ = ch.stderr().read_to_string(&mut err);
        if !err.trim().is_empty() {
            out.push_str(&err);
        }
        let _ = ch.wait_close();
        let code = ch.exit_status().unwrap_or(0);
        Ok((out, code))
    })?;

    let bytes = text.into_bytes();
    Ok(Output {
        status: exit_status(code),
        stderr: if code == 0 { Vec::new() } else { bytes.clone() },
        stdout: bytes,
    })
}

/// `ExitStatus` has no portable constructor; each platform exposes its
/// own. The Unix arm exists only so this module still type-checks on a
/// Linux workstation — it is never compiled into a shipped Unix build.
#[cfg(windows)]
fn exit_status(code: i32) -> ExitStatus {
    use std::os::windows::process::ExitStatusExt;
    ExitStatus::from_raw(code as u32)
}
#[cfg(not(windows))]
fn exit_status(code: i32) -> ExitStatus {
    use std::os::unix::process::ExitStatusExt;
    ExitStatus::from_raw(code << 8)
}

// ---------------------------------------------------------------- paths

/// Remote paths are POSIX even though we are on Windows, so they are
/// handled as strings rather than `Path`s wherever it matters.
fn to_remote_string(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

fn rpath(p: &str) -> PathBuf {
    PathBuf::from(p)
}

// ---------------------------------------------------------------- SFTP ops

/// One directory listing, straight from SFTP attributes — no `find`, no
/// output parsing, and it works on servers without GNU coreutils.
pub fn list_dir(t: &Target, path: &str) -> Result<Vec<crate::ops::Entry>, String> {
    with(t, |c| {
        let entries = c
            .sftp
            .readdir(rpath(path))
            .map_err(|e| format!("{path}: {e}"))?;
        let mut out = Vec::with_capacity(entries.len());
        for (p, st) in entries {
            let name = match p.file_name() {
                Some(n) => n.to_string_lossy().into_owned(),
                None => continue,
            };
            if name == "." || name == ".." {
                continue;
            }
            let is_link = st.file_type().is_symlink();
            // A symlink's own attributes say nothing about what it points
            // at; follow it so links to directories stay navigable.
            let resolved = if is_link {
                c.sftp.stat(&p).unwrap_or_else(|_| clone_stat(&st))
            } else {
                clone_stat(&st)
            };
            out.push(crate::ops::Entry {
                name,
                is_dir: resolved.is_dir(),
                is_link,
                size: resolved.size.unwrap_or(0),
                mtime: resolved.mtime.unwrap_or(0),
                perms: resolved
                    .perm
                    .map(|m| format!("{:o}", m & 0o7777))
                    .unwrap_or_default(),
                owner: resolved.uid.map(|u| u.to_string()).unwrap_or_default(),
            });
        }
        Ok(out)
    })
}

fn clone_stat(s: &FileStat) -> FileStat {
    FileStat {
        size: s.size,
        uid: s.uid,
        gid: s.gid,
        perm: s.perm,
        atime: s.atime,
        mtime: s.mtime,
    }
}

pub fn home(t: &Target) -> Result<String, String> {
    with(t, |c| Ok(c.home.clone()))
}

pub fn stat(t: &Target, path: &str) -> Result<(u64, u64), String> {
    with(t, |c| {
        let st = c
            .sftp
            .stat(&rpath(path))
            .map_err(|e| format!("{path}: {e}"))?;
        Ok((st.mtime.unwrap_or(0), st.size.unwrap_or(0)))
    })
}

pub fn mkdir(t: &Target, path: &str) -> Result<(), String> {
    with(t, |c| {
        c.sftp
            .mkdir(&rpath(path), 0o755)
            .map_err(|e| format!("{path}: {e}"))
    })
}

pub fn rename(t: &Target, from: &str, to: &str) -> Result<(), String> {
    with(t, |c| {
        c.sftp
            .rename(&rpath(from), &rpath(to), None)
            .map_err(|e| format!("{from} -> {to}: {e}"))
    })
}

pub fn chmod(t: &Target, path: &str, mode: u32) -> Result<(), String> {
    with(t, |c| {
        let mut st = c
            .sftp
            .stat(&rpath(path))
            .map_err(|e| format!("{path}: {e}"))?;
        st.perm = Some(mode);
        c.sftp
            .setstat(&rpath(path), st)
            .map_err(|e| format!("{path}: {e}"))
    })
}

/// Delete a file, or a directory and everything under it.
pub fn remove(t: &Target, path: &str) -> Result<(), String> {
    with(t, |c| remove_inner(&c.sftp, path))
}

fn remove_inner(sftp: &Sftp, path: &str) -> Result<(), String> {
    let st = sftp
        .lstat(&rpath(path))
        .map_err(|e| format!("{path}: {e}"))?;
    if st.is_dir() {
        let kids = sftp
            .readdir(rpath(path))
            .map_err(|e| format!("{path}: {e}"))?;
        for (p, _) in kids {
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned());
            match name.as_deref() {
                None | Some(".") | Some("..") => continue,
                Some(n) => remove_inner(sftp, &join(path, n))?,
            }
        }
        sftp.rmdir(&rpath(path))
            .map_err(|e| format!("{path}: {e}"))
    } else {
        sftp.unlink(&rpath(path))
            .map_err(|e| format!("{path}: {e}"))
    }
}

fn join(dir: &str, name: &str) -> String {
    if dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

// ---------------------------------------------------------------- file IO

pub fn read_file(t: &Target, path: &str, max: usize) -> Result<Vec<u8>, String> {
    with(t, |c| {
        let f = c
            .sftp
            .open(rpath(path))
            .map_err(|e| format!("{path}: {e}"))?;
        let mut buf = Vec::new();
        f.take(max as u64 + 1)
            .read_to_end(&mut buf)
            .map_err(|e| format!("{path}: {e}"))?;
        Ok(buf)
    })
}

/// Write a file the way a job script deserves: to a temporary name, then
/// rename over the original, so an interrupted save cannot truncate it.
pub fn write_file(t: &Target, path: &str, data: &[u8]) -> Result<(), String> {
    let tmp = format!("{path}.ssh_cli_tmp");
    with(t, |c| {
        {
            let mut f = c
                .sftp
                .create(&rpath(&tmp))
                .map_err(|e| format!("{tmp}: {e}"))?;
            f.write_all(data).map_err(|e| format!("{tmp}: {e}"))?;
            f.flush().map_err(|e| format!("{tmp}: {e}"))?;
        }
        // SFTP rename is not required to replace an existing file.
        let _ = c.sftp.unlink(&rpath(path));
        c.sftp
            .rename(&rpath(&tmp), &rpath(path), None)
            .map_err(|e| format!("{path}: {e}"))
    })
}

// ---------------------------------------------------------------- transfers

/// Transfers can be cancelled; the copy loops watch this set.
fn cancels() -> &'static Mutex<HashSet<u64>> {
    static C: OnceLock<Mutex<HashSet<u64>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashSet::new()))
}

pub fn cancel(id: u64) {
    if let Ok(mut c) = cancels().lock() {
        c.insert(id);
    }
}

fn cancelled(id: u64) -> bool {
    cancels().lock().map(|c| c.contains(&id)).unwrap_or(false)
}

pub fn clear_cancel(id: u64) {
    if let Ok(mut c) = cancels().lock() {
        c.remove(&id);
    }
}

/// 256 KiB at a time: large enough that the round trips disappear into
/// the throughput, small enough for responsive progress and cancelling.
const CHUNK: usize = 256 * 1024;

pub struct Progress<'a> {
    pub app: &'a AppHandle,
    pub id: u64,
    pub label: String,
    pub done: u64,
    pub total: u64,
}

impl Progress<'_> {
    fn emit(&self) {
        let pct = self
            .done
            .saturating_mul(100)
            .checked_div(self.total)
            .unwrap_or(0)
            .min(100);
        let _ = self.app.emit(
            "xfer-log",
            serde_json::json!({
                "id": self.id,
                "line": format!("{} {}%  {} / {}", self.label, pct,
                                human(self.done), human(self.total)),
            }),
        );
    }
}

fn human(n: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", U[i])
    }
}

/// Upload a local file or directory tree to the host.
pub fn upload(t: &Target, local: &Path, remote: &str, p: &mut Progress) -> Result<(), String> {
    if local.is_dir() {
        with(t, |c| {
            let _ = c.sftp.mkdir(&rpath(remote), 0o755);
            Ok(())
        })?;
        let entries = std::fs::read_dir(local).map_err(|e| format!("{}: {e}", local.display()))?;
        for e in entries.flatten() {
            if cancelled(p.id) {
                return Err("cancelled".into());
            }
            let name = e.file_name().to_string_lossy().into_owned();
            upload(t, &e.path(), &join(remote, &name), p)?;
        }
        return Ok(());
    }

    let mut f = std::fs::File::open(local).map_err(|e| format!("{}: {e}", local.display()))?;
    let mut buf = vec![0u8; CHUNK];
    with(t, |c| {
        let mut rf = c
            .sftp
            .create(&rpath(remote))
            .map_err(|e| format!("{remote}: {e}"))?;
        use std::io::Seek;
        f.rewind().map_err(|e| e.to_string())?;
        loop {
            if cancelled(p.id) {
                return Err("cancelled".into());
            }
            let n = f.read(&mut buf).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            rf.write_all(&buf[..n]).map_err(|e| format!("{remote}: {e}"))?;
            p.done += n as u64;
            p.emit();
        }
        rf.flush().map_err(|e| format!("{remote}: {e}"))?;
        Ok(())
    })
}

/// Download a remote file or directory tree.
pub fn download(t: &Target, remote: &str, local: &Path, p: &mut Progress) -> Result<(), String> {
    let st = with(t, |c| {
        c.sftp
            .stat(&rpath(remote))
            .map_err(|e| format!("{remote}: {e}"))
    })?;

    if st.is_dir() {
        std::fs::create_dir_all(local).map_err(|e| format!("{}: {e}", local.display()))?;
        let kids = with(t, |c| {
            c.sftp
                .readdir(rpath(remote))
                .map_err(|e| format!("{remote}: {e}"))
        })?;
        for (rp, _) in kids {
            if cancelled(p.id) {
                return Err("cancelled".into());
            }
            let Some(name) = rp.file_name().map(|n| n.to_string_lossy().into_owned()) else {
                continue;
            };
            if name == "." || name == ".." {
                continue;
            }
            download(t, &join(remote, &name), &local.join(&name), p)?;
        }
        return Ok(());
    }

    if let Some(dir) = local.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let mut out =
        std::fs::File::create(local).map_err(|e| format!("{}: {e}", local.display()))?;
    with(t, |c| {
        let mut rf = c
            .sftp
            .open(rpath(remote))
            .map_err(|e| format!("{remote}: {e}"))?;
        let mut buf = vec![0u8; CHUNK];
        loop {
            if cancelled(p.id) {
                return Err("cancelled".into());
            }
            let n = rf.read(&mut buf).map_err(|e| format!("{remote}: {e}"))?;
            if n == 0 {
                break;
            }
            out.write_all(&buf[..n]).map_err(|e| e.to_string())?;
            p.done += n as u64;
            p.emit();
        }
        out.flush().map_err(|e| e.to_string())?;
        Ok(())
    })
}

/// Total bytes behind a path, so the progress bar means something.
pub fn remote_size(t: &Target, remote: &str) -> u64 {
    fn walk(sftp: &Sftp, path: &str, acc: &mut u64, depth: u32) {
        if depth > 32 {
            return;
        }
        let Ok(st) = sftp.stat(&rpath(path)) else { return };
        if st.is_dir() {
            let Ok(kids) = sftp.readdir(rpath(path)) else { return };
            for (p, _) in kids {
                let Some(n) = p.file_name().map(|n| n.to_string_lossy().into_owned()) else {
                    continue;
                };
                if n == "." || n == ".." {
                    continue;
                }
                walk(sftp, &join(path, &n), acc, depth + 1);
            }
        } else {
            *acc += st.size.unwrap_or(0);
        }
    }
    with(t, |c| {
        let mut acc = 0;
        walk(&c.sftp, remote, &mut acc, 0);
        Ok(acc)
    })
    .unwrap_or(0)
}

pub fn local_size(path: &Path) -> u64 {
    if path.is_dir() {
        let Ok(rd) = std::fs::read_dir(path) else { return 0 };
        rd.flatten().map(|e| local_size(&e.path())).sum()
    } else {
        std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
    }
}

// ---------------------------------------------------------------- base64
// Only used for the host-key fingerprint, which wants standard base64
// with padding — `ops::base64_encode` already does exactly that.

fn b64_std(data: &[u8]) -> String {
    crate::ops::base64_encode(data).trim_end_matches('=').to_string()
}
