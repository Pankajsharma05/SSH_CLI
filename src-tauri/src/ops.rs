use crate::config;
use crate::plat;
use crate::target::Target;
use serde::Serialize;
#[cfg(not(windows))]
use std::process::Stdio;
use std::time::UNIX_EPOCH;
use tauri::{AppHandle, Emitter};

#[derive(Clone, Serialize)]
pub struct Entry {
    pub name: String,
    pub is_dir: bool,
    pub is_link: bool,
    pub size: u64,
    pub mtime: u64,
    pub perms: String,
    pub owner: String,
}

#[derive(Clone, Serialize)]
struct XferEvent {
    id: u64,
    line: String,
}

#[derive(Clone, Serialize)]
struct XferDone {
    id: u64,
    ok: bool,
}

/// POSIX single-quote escaping for remote shell commands.
pub fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r#"'\''"#))
}

/// Non-interactive ssh invocation attached to the shared control socket.
/// stdin is /dev/null so a missing/locked key fails fast instead of
/// hanging on a password prompt (the terminal is where you authenticate).
#[cfg(not(windows))]
fn ssh_output(t: &Target, remote_cmd: &str) -> Result<std::process::Output, String> {
    let mut args = t.control_opts().map_err(|e| e.to_string())?;
    args.push("-o".into());
    args.push("ConnectTimeout=10".into());
    args.push("-o".into());
    args.push("BatchMode=yes".into());
    args.extend(t.host_opts("-p"));
    args.push(t.destination.clone());
    args.push("--".into());
    args.push(remote_cmd.to_string());
    plat::cmd(&plat::ssh_exe())
        .args(&args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("running ssh: {e}"))
}

/// Windows has no ControlMaster, so the same call rides the persistent
/// shell in `mux.rs` instead of paying for a fresh handshake and a
/// fresh authentication on every directory listing.
#[cfg(windows)]
fn ssh_output(t: &Target, remote_cmd: &str) -> Result<std::process::Output, String> {
    crate::mux::output(t, remote_cmd)
}

fn ssh_check(t: &Target, remote_cmd: &str) -> Result<(), String> {
    let out = ssh_output(t, remote_cmd)?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

pub fn remote_home(t: &Target) -> Result<String, String> {
    #[cfg(windows)]
    {
        crate::mux::home(t)
    }
    #[cfg(not(windows))]
    {
    let out = ssh_output(t, "pwd")?;
    if !out.status.success() {
        return Err(auth_hint(&out.stderr));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }
}

fn auth_hint(stderr: &[u8]) -> String {
    let msg = String::from_utf8_lossy(stderr).trim().to_string();
    if msg.contains("Permission denied") || msg.contains("Interactive authentication") {
        if cfg!(windows) {
            // The Unix advice does not transfer: without multiplexing a
            // password typed in a terminal tab cannot be reused.
            format!(
                "{msg}\n\nHint: on Windows the file panes need key-based \
                 authentication, because Windows OpenSSH cannot share an \
                 authenticated connection between processes. Add a key to the \
                 ssh-agent service — see Help ▸ Windows notes. Terminal tabs \
                 still accept passwords and 2FA."
            )
        } else {
            format!(
                "{msg}\n\nHint: open a Terminal tab to this host first and log in there \
                 (password/2FA works in the terminal). File operations then reuse that \
                 connection automatically."
            )
        }
    } else {
        msg
    }
}

/// List a remote directory using GNU find (tab-separated, handles
/// spaces in names). %Y dereferences symlinks so links to directories
/// stay navigable.
pub fn remote_list(t: &Target, path: &str) -> Result<Vec<Entry>, String> {
    // SFTP hands back structured attributes, so the Windows build needs
    // neither GNU find nor any particular userland on the server.
    #[cfg(windows)]
    {
        let mut entries = crate::mux::list_dir(t, path)?;
        sort_entries(&mut entries);
        Ok(entries)
    }
    #[cfg(not(windows))]
    {
    let cmd = format!(
        "cd -- {} && find . -mindepth 1 -maxdepth 1 -printf '%y\\t%Y\\t%s\\t%T@\\t%m\\t%u\\t%f\\n'",
        sh_quote(path)
    );
    let out = ssh_output(t, &cmd)?;
    if !out.status.success() {
        return Err(auth_hint(&out.stderr));
    }
    let mut entries = Vec::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let mut parts = line.splitn(7, '\t');
        let (Some(ty), Some(deref), Some(size), Some(mtime), Some(perms), Some(owner), Some(name)) = (
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
        ) else {
            continue;
        };
        entries.push(Entry {
            name: name.to_string(),
            is_dir: deref == "d",
            is_link: ty == "l",
            size: size.parse().unwrap_or(0),
            mtime: mtime.split('.').next().and_then(|s| s.parse().ok()).unwrap_or(0),
            perms: perms.to_string(),
            owner: owner.to_string(),
        });
    }
    sort_entries(&mut entries);
    Ok(entries)
    }
}

pub fn local_list(path: &str) -> Result<Vec<Entry>, String> {
    let mut entries = Vec::new();
    let rd = std::fs::read_dir(path).map_err(|e| format!("{path}: {e}"))?;
    for item in rd.flatten() {
        let name = item.file_name().to_string_lossy().into_owned();
        let ft = match item.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };
        let meta = item.metadata().ok();
        let target_is_dir = if ft.is_symlink() {
            std::fs::metadata(item.path()).map(|m| m.is_dir()).unwrap_or(false)
        } else {
            ft.is_dir()
        };
        #[cfg(unix)]
        let perms = {
            use std::os::unix::fs::PermissionsExt;
            meta.as_ref()
                .map(|m| format!("{:o}", m.permissions().mode() & 0o7777))
                .unwrap_or_default()
        };
        #[cfg(not(unix))]
        let perms = String::new();
        entries.push(Entry {
            name,
            is_dir: target_is_dir,
            is_link: ft.is_symlink(),
            size: meta.as_ref().map(|m| m.len()).unwrap_or(0),
            mtime: meta
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0),
            perms,
            owner: String::new(),
        });
    }
    sort_entries(&mut entries);
    Ok(entries)
}

fn sort_entries(entries: &mut [Entry]) {
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
}

pub fn remote_mkdir(t: &Target, path: &str) -> Result<(), String> {
    #[cfg(windows)]
    {
        crate::mux::mkdir(t, path)
    }
    #[cfg(not(windows))]
    {
        ssh_check(t, &format!("mkdir -p -- {}", sh_quote(path)))
    }
}

pub fn remote_delete(t: &Target, path: &str) -> Result<(), String> {
    #[cfg(windows)]
    {
        crate::mux::remove(t, path)
    }
    #[cfg(not(windows))]
    {
        ssh_check(t, &format!("rm -rf -- {}", sh_quote(path)))
    }
}

pub fn remote_rename(t: &Target, from: &str, to: &str) -> Result<(), String> {
    #[cfg(windows)]
    {
        crate::mux::rename(t, from, to)
    }
    #[cfg(not(windows))]
    {
        ssh_check(t, &format!("mv -- {} {}", sh_quote(from), sh_quote(to)))
    }
}

/// Same-host copy runs directly on the server — no data leaves it.
pub fn remote_copy_same_host(t: &Target, from: &str, to: &str) -> Result<(), String> {
    ssh_check(t, &format!("cp -r -- {} {}", sh_quote(from), sh_quote(to)))
}

/// One side of a transfer: local path, or (target, remote path).
pub enum Side {
    Local(String),
    Remote(Target, String),
}

pub type XferHandle = std::sync::Arc<std::sync::Mutex<Option<std::process::Child>>>;

/// Run scp/rsync in a background thread; stream output lines as
/// `xfer-log` events and finish with `xfer-done`. The child handle is
/// stored in `handle` so the transfer can be cancelled.
#[cfg(not(windows))]
pub fn transfer(
    app: AppHandle,
    id: u64,
    sources: Vec<Side>,
    dest: Side,
    engine: &str,
    handle: XferHandle,
) -> Result<(), String> {
    let mut args: Vec<String> = vec!["-r".into()];

    // Any remote side supplies control opts (shared socket dir).
    let a_target = sources.iter().find_map(|s| match s {
        Side::Remote(t, _) => Some(t.clone()),
        _ => None,
    });
    let b_target = match &dest {
        Side::Remote(t, _) => Some(t.clone()),
        Side::Local(_) => None,
    };
    let any_target = a_target.clone().or_else(|| b_target.clone());
    if let Some(t) = &any_target {
        args.extend(t.control_opts().map_err(|e| e.to_string())?);
        args.push("-o".into());
        args.push("ConnectTimeout=10".into());
    }

    // Host flags (scp applies them globally — refuse conflicts).
    match (&a_target, &b_target) {
        (Some(a), Some(b)) => merge_host_flags(&mut args, a, b)?,
        (Some(t), None) | (None, Some(t)) => args.extend(t.host_opts("-P")),
        (None, None) => return Err("at least one side must be remote".into()),
    }

    // Remote↔remote streams through this machine, no temp files.
    if a_target.is_some() && b_target.is_some() {
        args.push("-3".into());
    }

    // rsync engine: local<->remote only, resumable + delta transfer.
    // Not on Windows: there is no rsync in the OpenSSH package, and a
    // Git-Bash or MSYS2 rsync mangles drive-letter paths, so the queue
    // quietly falls back to scp there.
    #[cfg(windows)]
    let program = {
        let _ = engine;
        plat::scp_exe()
    };
    #[cfg(not(windows))]
    let mut program = plat::scp_exe();
    #[cfg(not(windows))]
    if engine == "rsync"
        && !(a_target.is_some() && b_target.is_some())
        && sources.iter().chain(std::iter::once(&dest)).all(|s| !side_path(s).contains(' '))
    {
        if let Some(t) = &any_target {
            let mut ssh_cmd: Vec<String> = vec![plat::ssh_exe()];
            ssh_cmd.extend(t.control_opts().map_err(|e| e.to_string())?);
            ssh_cmd.push("-o".into());
            ssh_cmd.push("ConnectTimeout=10".into());
            for a in t.host_opts("-p") {
                ssh_cmd.push(a);
            }
            args = vec![
                "-rlpt".into(),
                "--partial".into(),
                "--info=progress2".into(),
                "-e".into(),
                ssh_cmd.join(" "),
            ];
            program = "rsync".to_string();
        }
    }

    for s in &sources {
        args.push(side_spec(s));
    }
    args.push(side_spec(&dest));

    std::thread::spawn(move || {
        let child = plat::cmd(&program)
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();
        let mut child = match child {
            Ok(c) => c,
            Err(e) => {
                let _ = app.emit("xfer-log", XferEvent { id, line: format!("failed to start {program}: {e}") });
                let _ = app.emit("xfer-done", XferDone { id, ok: false });
                return;
            }
        };
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        *handle.lock().unwrap() = Some(child);
        let mut readers = Vec::new();
        for stream in [
            stdout.map(|s| Box::new(s) as Box<dyn std::io::Read + Send>),
            stderr.map(|s| Box::new(s) as Box<dyn std::io::Read + Send>),
        ]
        .into_iter()
        .flatten()
        {
            let app2 = app.clone();
            readers.push(std::thread::spawn(move || {
                pump_lines(stream, |line| {
                    let _ = app2.emit("xfer-log", XferEvent { id, line });
                });
            }));
        }
        for r in readers {
            let _ = r.join();
        }
        let ok = loop {
            {
                let mut g = handle.lock().unwrap();
                match g.as_mut() {
                    None => break false,
                    Some(c) => match c.try_wait() {
                        Ok(Some(st)) => break st.success(),
                        Err(_) => break false,
                        Ok(None) => {}
                    },
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(150));
        };
        *handle.lock().unwrap() = None;
        let _ = app.emit("xfer-done", XferDone { id, ok });
    });
    Ok(())
}

/// Transfers over the in-process SFTP session.
///
/// Unlike the Unix path this is not a child process, so progress is real
/// byte counts rather than scraped from scp's meter, and cancelling sets
/// a flag the copy loop checks instead of killing a process.
#[cfg(windows)]
pub fn transfer(
    app: AppHandle,
    id: u64,
    sources: Vec<Side>,
    dest: Side,
    _engine: &str,
    _handle: XferHandle,
) -> Result<(), String> {
    std::thread::spawn(move || {
        crate::mux::clear_cancel(id);
        let emit = |line: String| {
            let _ = app.emit("xfer-log", XferEvent { id, line });
        };

        // Size everything first so the bar is honest.
        let mut total = 0u64;
        for s in &sources {
            total += match s {
                Side::Local(p) => crate::mux::local_size(std::path::Path::new(p)),
                Side::Remote(t, p) => crate::mux::remote_size(t, p),
            };
        }

        let mut prog = crate::mux::Progress {
            app: &app,
            id,
            label: String::new(),
            done: 0,
            total,
        };

        let mut ok = true;
        for src in &sources {
            let raw = side_path(src);
            let name = raw.trim_end_matches('/').rsplit('/').next().unwrap_or("item");
            let name = if name.is_empty() { "item" } else { name };
            prog.label = name.to_string();
            emit(format!("{name}…"));

            let result = match (src, &dest) {
                (Side::Local(from), Side::Remote(t, to)) => crate::mux::upload(
                    t,
                    std::path::Path::new(from),
                    &join_remote(to, name),
                    &mut prog,
                ),
                (Side::Remote(t, from), Side::Local(to)) => {
                    crate::mux::download(t, from, &std::path::Path::new(to).join(name), &mut prog)
                }
                (Side::Remote(ft, from), Side::Remote(tt, to)) => {
                    // Server to server stages through a temp file here, the
                    // same way scp -3 routes the bytes through this machine.
                    let tmp = std::env::temp_dir().join(format!("ssh_cli_relay_{id}_{name}"));
                    let r = crate::mux::download(ft, from, &tmp, &mut prog).and_then(|_| {
                        prog.done = 0;
                        crate::mux::upload(tt, &tmp, &join_remote(to, name), &mut prog)
                    });
                    let _ = std::fs::remove_file(&tmp);
                    r
                }
                (Side::Local(_), Side::Local(_)) => {
                    Err("at least one side must be remote".to_string())
                }
            };
            if let Err(e) = result {
                emit(e);
                ok = false;
                break;
            }
        }
        crate::mux::clear_cancel(id);
        let _ = app.emit("xfer-done", XferDone { id, ok });
    });
    Ok(())
}

#[cfg(windows)]
fn join_remote(dir: &str, name: &str) -> String {
    if dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

/// Names each item for the progress line, and on Unix also tells the
/// rsync engine whether a path is safe to hand it.
fn side_path(s: &Side) -> &str {
    match s {
        Side::Local(p) => p,
        Side::Remote(_, p) => p,
    }
}

#[cfg(not(windows))]
fn side_spec(s: &Side) -> String {
    match s {
        Side::Local(p) => p.clone(),
        Side::Remote(t, p) => format!("{}:{}", t.destination, p),
    }
}

#[cfg(not(windows))]
fn merge_host_flags(args: &mut Vec<String>, a: &Target, b: &Target) -> Result<(), String> {
    match (a.port, b.port) {
        (Some(x), Some(y)) if x != y => {
            return Err(format!(
                "hosts use different ports ({x} vs {y}); configure them in ~/.ssh/config \
                 and save the sessions without explicit ports"
            ))
        }
        (Some(p), _) | (_, Some(p)) => {
            args.push("-P".into());
            args.push(p.to_string());
        }
        _ => {}
    }
    for id in [&a.identity, &b.identity].into_iter().flatten() {
        args.push("-i".into());
        args.push(config::expand_tilde(id));
    }
    match (&a.jump, &b.jump) {
        (Some(x), Some(y)) if x != y => {
            return Err("hosts use different ProxyJump bastions; configure them in ~/.ssh/config".into())
        }
        (Some(j), _) | (_, Some(j)) => {
            args.push("-J".into());
            args.push(j.clone());
        }
        _ => {}
    }
    Ok(())
}

/// Read a byte stream, emitting on both \n and \r so scp's
/// carriage-return progress meter arrives as it updates.
#[cfg(not(windows))]
fn pump_lines<R: std::io::Read>(mut r: R, mut emit: impl FnMut(String)) {
    let mut buf = [0u8; 4096];
    let mut acc: Vec<u8> = Vec::new();
    loop {
        match r.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                for &b in &buf[..n] {
                    if b == b'\n' || b == b'\r' {
                        if !acc.is_empty() {
                            emit(String::from_utf8_lossy(&acc).into_owned());
                            acc.clear();
                        }
                    } else {
                        acc.push(b);
                    }
                }
            }
        }
    }
    if !acc.is_empty() {
        emit(String::from_utf8_lossy(&acc).into_owned());
    }
}

/// Public scp argument base for one remote target (used by edit + compress).
pub fn scp_args(t: &Target) -> Result<Vec<String>, String> {
    let mut args: Vec<String> = vec!["-q".into()];
    args.extend(t.control_opts().map_err(|e| e.to_string())?);
    args.push("-o".into());
    args.push("ConnectTimeout=10".into());
    args.extend(t.host_opts("-P"));
    Ok(args)
}

/// Run an arbitrary command on the host (SLURM panel, tails, etc.).
/// Output is truncated to keep the UI responsive.
pub fn remote_exec(t: &Target, cmd: &str) -> Result<String, String> {
    let out = ssh_output(t, cmd)?;
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    let err = String::from_utf8_lossy(&out.stderr);
    if !err.trim().is_empty() {
        text.push_str(&err);
    }
    const MAX: usize = 200_000;
    if text.len() > MAX {
        let cut = text
            .char_indices()
            .take_while(|(i, _)| *i < MAX)
            .last()
            .map(|(i, c)| i + c.len_utf8())
            .unwrap_or(0);
        text.truncate(cut);
        text.push_str("\n… (truncated)");
    }
    if !out.status.success() && text.trim().is_empty() {
        return Err("command failed".into());
    }
    Ok(text)
}

/// Search under a directory by name (case-insensitive substring).
pub fn remote_search(t: &Target, base: &str, query: &str) -> Result<Vec<String>, String> {
    let pattern = sh_quote(&format!("*{}*", query.replace(['*', '?'], "")));
    let cmd = format!(
        "find {} -iname {} 2>/dev/null | head -300",
        sh_quote(base),
        pattern
    );
    let out = ssh_output(t, &cmd)?;
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.to_string())
        .filter(|l| !l.is_empty())
        .collect())
}

/// Free-space summary for a path, local or remote, via POSIX `df -Pk`.
pub fn disk_usage(t: Option<&Target>, path: &str) -> Result<String, String> {
    let text = match t {
        Some(t) => {
            let out = ssh_output(t, &format!("df -Pk {}", sh_quote(path)))?;
            if !out.status.success() {
                return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
            }
            String::from_utf8_lossy(&out.stdout).into_owned()
        }
        // Windows has no df; ask the filesystem API directly and
        // return early with the formatted answer.
        #[cfg(windows)]
        None => {
            let Some((free, total)) = plat::local_free_total(path) else {
                return Ok(String::new());
            };
            let gib = |b: u64| b as f64 / (1024.0 * 1024.0 * 1024.0);
            let used_pct = if total > 0 {
                ((total - free) as f64 / total as f64 * 100.0).round() as u64
            } else {
                0
            };
            return Ok(format!(
                "{:.1} GB free of {:.1} GB ({}% used)",
                gib(free),
                gib(total),
                used_pct
            ));
        }
        #[cfg(not(windows))]
        None => {
            let out = plat::cmd("df")
                .args(["-Pk", path])
                .output()
                .map_err(|e| e.to_string())?;
            String::from_utf8_lossy(&out.stdout).into_owned()
        }
    };
    let line = text.lines().last().unwrap_or("");
    let cols: Vec<&str> = line.split_whitespace().collect();
    if cols.len() < 5 {
        return Ok(String::new());
    }
    let kb = |s: &str| s.parse::<f64>().unwrap_or(0.0);
    let gib = |k: f64| k / (1024.0 * 1024.0);
    Ok(format!(
        "{:.1} GB free of {:.1} GB ({} used)",
        gib(kb(cols[3])),
        gib(kb(cols[1])),
        cols[4]
    ))
}

/// A named folder offered in a pane's "places" dropdown.
#[derive(Clone, Serialize)]
pub struct Place {
    pub label: String,
    pub path: String,
}

/// Well-known folders on a cluster login node. Only directories that
/// actually exist come back, so the list is honest per host: $HOME
/// always, plus whatever scratch/work/project space the site defines
/// (either through the usual environment variables or the usual
/// /scratch/$USER-style layouts).
pub fn remote_places(t: &Target) -> Result<Vec<Place>, String> {
    let script = "p(){ [ -n \"$2\" ] && [ -d \"$2\" ] && printf '%s\\t%s\\n' \"$1\" \"$2\"; }; \
        p home \"$HOME\"; \
        p scratch \"$SCRATCH\"; p scratch \"$SCRATCHDIR\"; p scratch \"/scratch/$USER\"; \
        p scratch \"/lustre/$USER\"; p scratch \"/lscratch/$USER\"; \
        p work \"$WORK\"; p work \"/work/$USER\"; \
        p project \"$PROJECT\"; p project \"$PROJECTS\"; p project \"/projects/$USER\"; \
        p data \"$DATA\"; p data \"/data/$USER\"; \
        p store \"$STORE\"; p store \"$ARCHIVE\"; \
        p software \"$HOME/software\"; p apps \"/opt/apps\"; p modules \"/opt/ohpc\"; \
        p tmp /tmp; true";
    let out = ssh_output(t, script)?;
    if !out.status.success() {
        return Err(auth_hint(&out.stderr));
    }
    let mut places = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let Some((label, path)) = line.split_once('\t') else { continue };
        let path = path.trim_end_matches('/');
        let path = if path.is_empty() { "/" } else { path };
        if !seen.insert(path.to_string()) {
            continue;
        }
        places.push(Place { label: label.to_string(), path: path.to_string() });
    }
    Ok(places)
}

/// The local equivalent — home plus the usual user folders.
pub fn local_places() -> Vec<Place> {
    let mut places = Vec::new();
    // Scoped so the closure's borrow of `places` ends before the
    // Windows drive loop below wants it back.
    {
        let mut push = |label: &str, p: Option<std::path::PathBuf>| {
            if let Some(p) = p {
                if p.is_dir() {
                    places.push(Place {
                        label: label.to_string(),
                        path: p.to_string_lossy().into_owned(),
                    });
                }
            }
        };
        push("home", dirs::home_dir());
        push("desktop", dirs::desktop_dir());
        push("documents", dirs::document_dir());
        push("downloads", dirs::download_dir());
        push("tmp", Some(std::env::temp_dir()));
    }
    // Every mounted drive, so the pane can leave the user profile.
    #[cfg(windows)]
    for d in plat::drives() {
        places.push(Place { label: d.trim_end_matches('\\').to_lowercase(), path: d });
    }
    places
}

/// Identity of a host, fetched once per connection: who you are, which
/// login node you landed on, and whether there is a batch scheduler
/// worth showing the Jobs panel for.
pub fn host_info(t: &Target) -> Result<serde_json::Value, String> {
    let script = "printf '%s\\t%s\\t%s\\t%s\\n' \
        \"$(id -un 2>/dev/null)\" \
        \"$(hostname 2>/dev/null)\" \
        \"$HOME\" \
        \"$(command -v squeue >/dev/null 2>&1 && echo slurm || \
           { command -v qstat >/dev/null 2>&1 && echo pbs || echo none; })\"";
    let out = ssh_output(t, script)?;
    if !out.status.success() {
        return Err(auth_hint(&out.stderr));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().next().unwrap_or("");
    let f: Vec<&str> = line.split('\t').collect();
    let get = |i: usize| f.get(i).copied().unwrap_or("").to_string();
    Ok(serde_json::json!({
        "user": get(0),
        "hostname": get(1),
        "home": get(2),
        "scheduler": get(3),
    }))
}

pub fn chmod_remote(t: &Target, path: &str, mode: &str) -> Result<(), String> {
    if !mode.chars().all(|c| c.is_ascii_digit()) || mode.is_empty() || mode.len() > 4 {
        return Err("mode must be octal digits, e.g. 644 or 755".into());
    }
    #[cfg(windows)]
    {
        let bits = u32::from_str_radix(mode, 8).map_err(|_| "mode must be octal")?;
        crate::mux::chmod(t, path, bits)
    }
    #[cfg(not(windows))]
    ssh_check(t, &format!("chmod {} -- {}", mode, sh_quote(path)))
}

pub fn chmod_local(path: &str, mode: &str) -> Result<(), String> {
    let bits = u32::from_str_radix(mode, 8).map_err(|_| "mode must be octal, e.g. 644")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(bits))
            .map_err(|e| e.to_string())
    }
    #[cfg(not(unix))]
    {
        let _ = (bits, path);
        Err("chmod is unix-only".into())
    }
}

/// Compress a set of remote files into a tarball on the server, download
/// the single archive, unpack it locally, and clean up both sides.
/// Massively faster than per-file scp for many small files.
pub fn compress_download(
    app: AppHandle,
    id: u64,
    t: Target,
    remote_dir: String,
    names: Vec<String>,
    local_dir: String,
) -> Result<(), String> {
    std::thread::spawn(move || {
        let emit = |line: String| {
            let _ = app.emit("xfer-log", XferEvent { id, line });
        };
        let fail = |app: &AppHandle, msg: String| {
            let _ = app.emit("xfer-log", XferEvent { id, line: msg });
            let _ = app.emit("xfer-done", XferDone { id, ok: false });
        };
        let remote_tmp = format!("/tmp/ssh_cli_{id}.tar.gz");
        let quoted: Vec<String> = names.iter().map(|n| sh_quote(n)).collect();
        emit("compressing on server…".into());
        let cmd = format!(
            "cd -- {} && tar czf {} -- {}",
            sh_quote(&remote_dir),
            sh_quote(&remote_tmp),
            quoted.join(" ")
        );
        if let Err(e) = ssh_check(&t, &cmd) {
            return fail(&app, format!("tar failed: {e}"));
        }
        emit("downloading archive…".into());
        let local_tmp = std::env::temp_dir().join(format!("ssh_cli_{id}.tar.gz"));

        // Windows pulls the archive down the session we already have,
        // instead of authenticating a fresh scp.exe.
        #[cfg(windows)]
        let ok = {
            let total = crate::mux::remote_size(&t, &remote_tmp);
            let mut prog = crate::mux::Progress {
                app: &app,
                id,
                label: "archive".into(),
                done: 0,
                total,
            };
            crate::mux::download(&t, &remote_tmp, &local_tmp, &mut prog).is_ok()
        };
        #[cfg(not(windows))]
        let ok = {
            let mut args = match scp_args(&t) {
                Ok(a) => a,
                Err(e) => return fail(&app, e),
            };
            args.push(format!("{}:{}", t.destination, remote_tmp));
            args.push(local_tmp.to_string_lossy().into_owned());
            plat::cmd(&plat::scp_exe())
                .args(&args)
                .stdin(Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        };
        let _ = ssh_check(&t, &format!("rm -f -- {}", sh_quote(&remote_tmp)));
        if !ok {
            let _ = std::fs::remove_file(&local_tmp);
            return fail(&app, "archive download failed".into());
        }
        emit("unpacking…".into());
        // Windows 10 1803+ ships bsdtar as tar.exe, so one invocation
        // covers every platform.
        let ok = plat::cmd(plat::tar_exe())
            .args(["-xzf", &local_tmp.to_string_lossy(), "-C", &local_dir])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        let _ = std::fs::remove_file(&local_tmp);
        if !ok {
            return fail(&app, "unpack failed".into());
        }
        let _ = app.emit("xfer-log", XferEvent { id, line: "done (compressed)".into() });
        let _ = app.emit("xfer-done", XferDone { id, ok: true });
    });
    Ok(())
}

const EDIT_MAX: usize = 2_000_000;

/// Read a remote text file for the built-in editor (UTF-8, size-capped).
pub fn remote_read_text(t: &Target, path: &str) -> Result<String, String> {
    #[cfg(windows)]
    {
        text_guard(crate::mux::read_file(t, path, EDIT_MAX)?)
    }
    #[cfg(not(windows))]
    {
    let cmd = format!("head -c {} -- {}", EDIT_MAX + 1, sh_quote(path));
    let out = ssh_output(t, &cmd)?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    text_guard(out.stdout)
    }
}

/// Write the built-in editor's buffer back to a remote file, atomically
/// enough for job scripts: write to a temp file, then mv into place.
#[cfg(windows)]
pub fn remote_write_text(t: &Target, path: &str, data: &str) -> Result<(), String> {
    crate::mux::write_file(t, path, data.as_bytes())
}

#[cfg(not(windows))]
pub fn remote_write_text(t: &Target, path: &str, data: &str) -> Result<(), String> {
    use std::io::Write;
    let tmp = format!("{}.ssh_cli_tmp", path);
    let mut args = t.control_opts().map_err(|e| e.to_string())?;
    args.push("-o".into());
    args.push("ConnectTimeout=10".into());
    args.push("-o".into());
    args.push("BatchMode=yes".into());
    args.extend(t.host_opts("-p"));
    args.push(t.destination.clone());
    args.push("--".into());
    args.push(format!(
        "cat > {tmp} && mv -- {tmp} {orig}",
        tmp = sh_quote(&tmp),
        orig = sh_quote(path)
    ));
    let mut child = plat::cmd(&plat::ssh_exe())
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawn ssh: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(data.as_bytes())
            .map_err(|e| format!("write: {e}"))?;
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(())
}

pub fn local_read_text(path: &str) -> Result<String, String> {
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if meta.len() as usize > EDIT_MAX {
        return Err("file is too large for the built-in editor (2 MB limit)".into());
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    text_guard(bytes)
}

pub fn local_write_text(path: &str, data: &str) -> Result<(), String> {
    std::fs::write(path, data).map_err(|e| e.to_string())
}

fn text_guard(bytes: Vec<u8>) -> Result<String, String> {
    if bytes.len() > EDIT_MAX {
        return Err("file is too large for the built-in editor (2 MB limit)".into());
    }
    if bytes.contains(&0) {
        return Err("this looks like a binary file".into());
    }
    String::from_utf8(bytes).map_err(|_| "file is not valid UTF-8 text".into())
}

/// Hand a path to the desktop's default application.
pub fn open_path(path: &str) -> Result<(), String> {
    plat::open_path(path)
}

// ---------- image viewer support ----------

const IMG_MAX: usize = 20_000_000;

/// Raw bytes of a remote file (size-capped) for the image viewer.
///
/// The Windows transport is a text pipe, so the bytes come back base64
/// encoded and are decoded here.
#[cfg(windows)]
pub fn remote_read_bytes(t: &Target, path: &str) -> Result<Vec<u8>, String> {
    let bytes = crate::mux::read_file(t, path, IMG_MAX)?;
    if bytes.len() > IMG_MAX {
        return Err("file is too large for the image viewer (20 MB limit)".into());
    }
    Ok(bytes)
}

#[cfg(not(windows))]
pub fn remote_read_bytes(t: &Target, path: &str) -> Result<Vec<u8>, String> {
    let cmd = format!("head -c {} -- {}", IMG_MAX + 1, sh_quote(path));
    let out = ssh_output(t, &cmd)?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    if out.stdout.len() > IMG_MAX {
        return Err("file is too large for the image viewer (20 MB limit)".into());
    }
    Ok(out.stdout)
}

pub fn local_read_bytes(path: &str) -> Result<Vec<u8>, String> {
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if meta.len() as usize > IMG_MAX {
        return Err("file is too large for the image viewer (20 MB limit)".into());
    }
    std::fs::read(path).map_err(|e| e.to_string())
}

/// mtime + size of a remote file — drives the viewer's auto-refresh.
/// GNU stat first (Linux servers), BSD stat as fallback.
pub fn remote_stat(t: &Target, path: &str) -> Result<(u64, u64), String> {
    #[cfg(windows)]
    {
        crate::mux::stat(t, path)
    }
    #[cfg(not(windows))]
    {
    let q = sh_quote(path);
    let cmd = format!("stat -c '%Y %s' -- {q} 2>/dev/null || stat -f '%m %z' {q}");
    let out = ssh_output(t, &cmd)?;
    let text = String::from_utf8_lossy(&out.stdout);
    let mut it = text.split_whitespace();
    let parse = |s: Option<&str>| s.and_then(|v| v.parse::<u64>().ok());
    match (parse(it.next()), parse(it.next())) {
        (Some(m), Some(s)) => Ok((m, s)),
        _ => Err(String::from_utf8_lossy(&out.stderr).trim().to_string()),
    }
    }
}

/// Dependency-free base64 (bytes travel to the frontend as a data: URL).
pub fn base64_encode(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b1 = *chunk.first().unwrap_or(&0) as u32;
        let b2 = *chunk.get(1).unwrap_or(&0) as u32;
        let b3 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b1 << 16) | (b2 << 8) | b3;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

