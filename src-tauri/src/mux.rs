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
    CheckResult, FileStat, KeyboardInteractivePrompt, KnownHostFileKind, Prompt, Session,
    Sftp,
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

/// How long a prompt waits for the human before giving up. Long enough
/// to fish a phone out of a pocket for a 2FA code, short enough that a
/// prompt which never reached the UI surfaces as an error instead of an
/// apparent hang.
const PROMPT_TIMEOUT: Duration = Duration::from_secs(90);

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
static ANSWERED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

type Pending = Mutex<HashMap<u64, SyncSender<Option<String>>>>;

fn pending() -> &'static Pending {
    static P: OnceLock<Pending> = OnceLock::new();
    P.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Answer an outstanding prompt. `None` means the user cancelled.
pub fn answer(id: u64, text: Option<String>) {
    ANSWERED.store(true, Ordering::Relaxed);
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

/// Did the interface ever answer a prompt? Used to tell "you cancelled"
/// apart from "the dialog never reached you", which look identical from
/// down here and need very different advice.
fn prompts_answered() -> bool {
    ANSWERED.load(Ordering::Relaxed)
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
            remember_host(&path, host, port, key)
        }
        CheckResult::Mismatch => {
            // libssh2 reports a mismatch whenever *some* key is stored for
            // this host and the offered one differs. That includes the
            // ordinary case where the stored key is of a type this build
            // cannot negotiate: OpenSSH writes an ed25519 key, and libssh2
            // on Windows' native crypto has no ed25519, so the server
            // offers ECDSA or RSA instead. Nothing has changed.
            //
            // Only a stored key of the *same* algorithm that differs is
            // alarming. Otherwise ask, exactly as OpenSSH does when it meets
            // a host key type it has not seen for a host before.
            let offered_alg = key_alg(key);
            let same_alg_stored = known
                .hosts()
                .map(|hosts| {
                    hosts.iter().any(|h| {
                        entry_matches_host(h.name(), host, port)
                            && b64_decode(h.key()).as_deref().and_then(key_alg) == offered_alg
                    })
                })
                .unwrap_or(false);

            if same_alg_stored {
                return Err(format!(
                    "WARNING: the host key for {host} does not match the one in \
                     known_hosts.\n\nOffered fingerprint: {fingerprint}\n\n\
                     This is expected after a server rebuild, and is also exactly \
                     what a machine-in-the-middle looks like. Verify the \
                     fingerprint with your administrators, then use \u{201c}Remove old \
                     host key\u{201d} in the session menu if it is genuinely a new key."
                ));
            }

            let alg = offered_alg.unwrap_or_else(|| "this".into());
            let answer = ask(
                "hostkey",
                host,
                &format!("Trust the {alg} key for {host}?"),
                &format!(
                    "{host} is already in your known_hosts, but with a key of a \
                     different type, which this app cannot use \u{2014} Windows' \
                     built-in crypto does not implement it.\n\n\
                     The same server also offers this {alg} key:\n  {fingerprint}\n\n\
                     If that fingerprint matches what your administrators publish, \
                     trusting it is safe: it adds a second entry and leaves your \
                     existing one untouched."
                ),
                true,
            );
            if !matches!(answer.as_deref(), Some("yes")) {
                return Err(format!("host key for {host} was not accepted"));
            }
            remember_host(&path, host, port, key)
        }
        CheckResult::Failure => Err("could not check the host key".into()),
    }
}

/// Record a trusted host key by **appending one line** to known_hosts.
///
/// libssh2 offers write_file(), but that rewrites the whole file from its
/// in-memory list: anything it did not parse or cannot represent comes
/// back changed or not at all. known_hosts belongs to OpenSSH and is
/// shared with ssh.exe, which the terminal tabs use — rewriting it broke
/// terminal logins on a real cluster. Appending is what OpenSSH itself
/// does, and it cannot disturb an existing entry.
fn remember_host(path: &Path, host: &str, port: u16, key: &[u8]) -> Result<(), String> {
    use std::io::Write as _;

    let entry = if port == 22 {
        host.to_string()
    } else {
        format!("[{host}]:{port}")
    };
    // The key type in a known_hosts line is exactly the algorithm name
    // stored at the front of the key blob.
    let alg = key_alg(key).ok_or("the server sent a host key in a format this build cannot record")?;
    let line = format!("{entry} {alg} {}\n", crate::ops::base64_encode(key));

    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    // A file that does not end in a newline would otherwise glue our entry
    // onto the last one.
    let needs_newline = std::fs::read(path)
        .ok()
        .and_then(|b| b.last().copied())
        .is_some_and(|b| b != b'\n');

    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| format!("could not open {}: {e}", path.display()))?;
    if needs_newline {
        f.write_all(b"\n").map_err(|e| e.to_string())?;
    }
    f.write_all(line.as_bytes())
        .map_err(|e| format!("could not write {}: {e}", path.display()))
}

/// The algorithm name carried at the front of an SSH public key blob: a
/// 4-byte big-endian length followed by e.g. `ssh-ed25519`.
fn key_alg(blob: &[u8]) -> Option<String> {
    let len = u32::from_be_bytes(blob.get(..4)?.try_into().ok()?) as usize;
    String::from_utf8(blob.get(4..4 + len)?.to_vec()).ok()
}

/// Does a known_hosts entry refer to this host? Entries are a plain name,
/// a `[name]:port` form, or a comma-separated list. Hashed entries expose
/// no readable name and count as "not this host" — at worst that costs one
/// extra trust prompt.
fn entry_matches_host(name: Option<&str>, host: &str, port: u16) -> bool {
    let Some(name) = name else { return false };
    let bracketed = format!("[{host}]:{port}");
    name.split(',').any(|n| n.trim() == host || n.trim() == bracketed)
}

/// Stored keys come back base64; the algorithm is inside the decoded blob.
fn b64_decode(text: &str) -> Option<Vec<u8>> {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let (mut acc, mut bits) = (0u32, 0u32);
    let mut out = Vec::new();
    for c in text.bytes() {
        if c == b'=' || c.is_ascii_whitespace() {
            continue;
        }
        let v = T.iter().position(|&t| t == c)? as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
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
            return Err(no_answer_error());
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
        return Err(no_answer_error());
    }

    if sess.authenticated() {
        return Ok(());
    }
    Err(format!(
        "could not authenticate to {user}@{host}. The server offered: {}",
        if methods.is_empty() { "nothing usable" } else { &methods }
    ))
}

fn no_answer_error() -> String {
    if prompts_answered() {
        "authentication cancelled".into()
    } else {
        "the password prompt never reached the window, so the login could not \
         be completed. Please report this — it is a bug in SSH_CLI, not in \
         your cluster account."
            .into()
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A real ed25519 host key blob, base64 as it appears in known_hosts.
    const ED25519: &str =
        "AAAAC3NzaC1lZDI1NTE5AAAAIJ1DbWGNP1IAPoyDs6bxPlhFPCdHEUEAu0OEZxIU0AZs";

    #[test]
    fn reads_the_algorithm_out_of_a_key_blob() {
        let blob = b64_decode(ED25519).expect("decodes");
        assert_eq!(key_alg(&blob).as_deref(), Some("ssh-ed25519"));
        // Truncated or empty input must not panic.
        assert_eq!(key_alg(&[]), None);
        assert_eq!(key_alg(&[0, 0, 0, 200, b'x']), None);
    }

    #[test]
    fn base64_round_trips_against_the_encoder() {
        for sample in [&b""[..], b"a", b"ab", b"abc", b"hello world", &[0u8, 255, 17][..]] {
            let encoded = crate::ops::base64_encode(sample);
            assert_eq!(b64_decode(&encoded).as_deref(), Some(sample), "{encoded}");
        }
    }

    /// The bug from the field: an ed25519 entry written by OpenSSH and an
    /// ECDSA key offered to libssh2 are *different types*, not a changed
    /// key, and must not be treated as a mismatch.
    #[test]
    fn different_key_types_are_not_the_same_algorithm() {
        let stored = key_alg(&b64_decode(ED25519).unwrap());
        let offered_ecdsa = key_alg(
            &b64_decode("AAAAE2VjZHNhLXNoYTItbmlzdHAyNTYAAAAIbmlzdHAyNTY=").unwrap(),
        );
        assert_eq!(stored.as_deref(), Some("ssh-ed25519"));
        assert_eq!(offered_ecdsa.as_deref(), Some("ecdsa-sha2-nistp256"));
        assert_ne!(stored, offered_ecdsa);
    }

    /// The field bug: recording a key must never disturb what is already
    /// in known_hosts, because ssh.exe shares that file and the terminal
    /// tabs depend on it.
    #[test]
    fn appending_a_host_key_preserves_the_file() {
        let dir = std::env::temp_dir().join(format!("sshcli_kh_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("known_hosts");

        // An existing file whose last line has no trailing newline, and an
        // ed25519 entry of the kind libssh2 cannot negotiate on Windows.
        let existing = format!("cluster.example ssh-ed25519 {ED25519}\nother.example ssh-rsa AAAAB3NzaC1yc2E=");
        std::fs::write(&path, &existing).unwrap();

        let key = b64_decode(ED25519).unwrap();
        remember_host(&path, "cluster.example", 22, &key).unwrap();

        let after = std::fs::read_to_string(&path).unwrap();
        assert!(after.starts_with(&existing), "existing entries were modified:\n{after}");
        let lines: Vec<&str> = after.lines().collect();
        assert_eq!(lines.len(), 3, "expected exactly one line added:\n{after}");
        assert_eq!(lines[0], format!("cluster.example ssh-ed25519 {ED25519}"));
        assert_eq!(lines[1], "other.example ssh-rsa AAAAB3NzaC1yc2E=");
        // Appended in OpenSSH's own format: host, key type, base64 key.
        let added: Vec<&str> = lines[2].split(' ').collect();
        assert_eq!(added[0], "cluster.example");
        assert_eq!(added[1], "ssh-ed25519");
        assert_eq!(b64_decode(added[2]).unwrap(), key);

        // A non-default port is recorded in bracket form.
        remember_host(&path, "cluster.example", 2222, &key).unwrap();
        let last = std::fs::read_to_string(&path).unwrap();
        assert!(last.lines().last().unwrap().starts_with("[cluster.example]:2222 ssh-ed25519 "));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn matches_host_entry_forms() {
        assert!(entry_matches_host(Some("10.14.2.12"), "10.14.2.12", 22));
        assert!(entry_matches_host(Some("[10.14.2.12]:2222"), "10.14.2.12", 2222));
        assert!(entry_matches_host(Some("alias,10.14.2.12"), "10.14.2.12", 22));
        assert!(!entry_matches_host(Some("10.14.2.13"), "10.14.2.12", 22));
        // Hashed entries have no readable name.
        assert!(!entry_matches_host(None, "10.14.2.12", 22));
    }
}

// ---------------------------------------------------------------- shells
//
// A terminal tab is a channel on the connection the file panes already
// authenticated, so opening one costs no second login — the whole point
// of owning the transport. Reading is a poll rather than a blocking
// read: libssh2 serialises everything on one session, and a terminal
// parked in a blocking read would freeze every directory listing and
// transfer behind it.

struct Shell {
    channel: ssh2::Channel,
    conn: Arc<Mutex<Conn>>,
    stop: Arc<std::sync::atomic::AtomicBool>,
}

fn shells() -> &'static Mutex<HashMap<u64, Shell>> {
    static S: OnceLock<Mutex<HashMap<u64, Shell>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Is this terminal id one of ours? Tells `term.rs` whether to route to
/// the shared session or to a local PTY.
pub fn shell_exists(id: u64) -> bool {
    shells().lock().map(|s| s.contains_key(&id)).unwrap_or(false)
}

/// How long to wait between polls when the shell has nothing to say.
/// Fast enough to feel like a terminal, slow enough to stay invisible.
const SHELL_IDLE_POLL: Duration = Duration::from_millis(15);

pub fn shell_open(
    app: AppHandle,
    t: &Target,
    id: u64,
    rows: u16,
    cols: u16,
    cwd: Option<&str>,
    prelude: Option<&str>,
) -> Result<(), String> {
    // Reuses the authenticated connection, or makes it (prompting once).
    let conn = get(t)?;

    let channel = {
        let g = conn.lock().map_err(|_| "connection poisoned")?;
        let mut ch = g
            .sess
            .channel_session()
            .map_err(|e| format!("opening a terminal channel: {e}"))?;
        ch.request_pty("xterm-256color", None, Some((cols as u32, rows as u32, 0, 0)))
            .map_err(|e| format!("requesting a pty: {e}"))?;

        // Same shape as the ssh -t command the Unix build uses, so a
        // start directory, the plot prelude and the session's startup
        // command all behave identically.
        let mut pre = String::new();
        if let Some(d) = cwd.filter(|d| !d.trim().is_empty()) {
            pre.push_str(&format!("cd {} 2>/dev/null; ", crate::ops::sh_quote(d)));
        }
        if let Some(p) = prelude.filter(|p| !p.trim().is_empty()) {
            pre.push_str(p.trim());
            if !pre.ends_with(';') {
                pre.push(';');
            }
            pre.push(' ');
        }
        if let Some(startup) = &t.startup {
            pre.push_str(startup);
            pre.push_str("; ");
        }
        if pre.is_empty() {
            ch.shell().map_err(|e| format!("starting a shell: {e}"))?;
        } else {
            pre.push_str("exec $SHELL -l");
            ch.exec(&pre).map_err(|e| format!("starting a shell: {e}"))?;
        }
        ch
    };

    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    shells().lock().map_err(|_| "shell registry poisoned")?.insert(
        id,
        Shell { channel: channel.clone(), conn: conn.clone(), stop: stop.clone() },
    );

    let mut reader = channel;
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        let mut carry: Vec<u8> = Vec::new();
        loop {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            let mut got = 0usize;
            let mut finished = false;
            {
                let Ok(g) = conn.lock() else { break };
                g.sess.set_blocking(false);
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            carry.extend_from_slice(&buf[..n]);
                            got += n;
                            if got >= 256 * 1024 {
                                break; // give the lock back; more next pass
                            }
                        }
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                        Err(_) => {
                            finished = true;
                            break;
                        }
                    }
                }
                if reader.eof() {
                    finished = true;
                }
                g.sess.set_blocking(true);
            }

            // Hold back an incomplete UTF-8 tail so multibyte characters
            // never get split across two events.
            if !carry.is_empty() {
                let valid_to = match std::str::from_utf8(&carry) {
                    Ok(_) => carry.len(),
                    Err(e) => e.valid_up_to(),
                };
                if valid_to > 0 {
                    let text = String::from_utf8_lossy(&carry[..valid_to]).into_owned();
                    let _ = app.emit("term-data", serde_json::json!({ "id": id, "data": text }));
                    carry.drain(..valid_to);
                } else if carry.len() > 4 {
                    let text = String::from_utf8_lossy(&carry).into_owned();
                    let _ = app.emit("term-data", serde_json::json!({ "id": id, "data": text }));
                    carry.clear();
                }
            }

            if finished {
                break;
            }
            if got == 0 {
                std::thread::sleep(SHELL_IDLE_POLL);
            }
        }
        let _ = app.emit("term-exit", serde_json::json!({ "id": id }));
        shells().lock().ok().map(|mut s| s.remove(&id));
    });

    Ok(())
}

pub fn shell_write(id: u64, data: &str) -> Result<(), String> {
    let (channel, conn) = {
        let s = shells().lock().map_err(|_| "shell registry poisoned")?;
        let sh = s.get(&id).ok_or("no such terminal")?;
        (sh.channel.clone(), sh.conn.clone())
    };
    let g = conn.lock().map_err(|_| "connection poisoned")?;
    let mut ch = channel;
    g.sess.set_blocking(true);
    ch.write_all(data.as_bytes())
        .map_err(|e| format!("write: {e}"))?;
    ch.flush().map_err(|e| format!("flush: {e}"))
}

pub fn shell_resize(id: u64, rows: u16, cols: u16) -> Result<(), String> {
    let (channel, conn) = {
        let s = shells().lock().map_err(|_| "shell registry poisoned")?;
        let sh = s.get(&id).ok_or("no such terminal")?;
        (sh.channel.clone(), sh.conn.clone())
    };
    let g = conn.lock().map_err(|_| "connection poisoned")?;
    let mut ch = channel;
    g.sess.set_blocking(true);
    ch.request_pty_size(cols as u32, rows as u32, None, None)
        .map_err(|e| format!("resize: {e}"))
}

pub fn shell_close(id: u64) {
    let sh = shells().lock().ok().and_then(|mut s| s.remove(&id));
    if let Some(sh) = sh {
        sh.stop.store(true, Ordering::Relaxed);
        if let Ok(g) = sh.conn.lock() {
            g.sess.set_blocking(true);
            let mut ch = sh.channel;
            let _ = ch.close();
        }
    }
}
