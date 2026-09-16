#[cfg(not(windows))]
use crate::ops;
use crate::plat;
use crate::state::AppState;
use crate::target::Target;
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use std::io::{Read, Write};
use std::sync::atomic::Ordering;
use tauri::{AppHandle, Emitter};

pub struct TermSession {
    pub writer: Box<dyn Write + Send>,
    pub master: Box<dyn MasterPty + Send>,
    pub child: Box<dyn Child + Send + Sync>,
}

#[derive(Clone, Serialize)]
struct TermData {
    id: u64,
    data: String,
}

#[derive(Clone, Serialize)]
struct TermExit {
    id: u64,
}

/// Put a built command on a PTY, register it, and stream its output to the
/// frontend as `term-data` events; EOF emits `term-exit`.
fn spawn(
    app: AppHandle,
    state: &AppState,
    cmd: CommandBuilder,
    rows: u16,
    cols: u16,
) -> Result<u64, String> {
    let pty = native_pty_system();
    let pair = pty
        .openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
        .map_err(|e| format!("openpty: {e}"))?;

    let child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| format!("spawn: {e}"))?;
    drop(pair.slave);

    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| format!("pty reader: {e}"))?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|e| format!("pty writer: {e}"))?;

    let id = state.next_id.fetch_add(1, Ordering::SeqCst);
    state.terms.lock().unwrap().insert(
        id,
        TermSession { writer, master: pair.master, child },
    );

    // Reader thread: forward PTY bytes to the UI, keeping any
    // incomplete UTF-8 tail for the next chunk so multibyte
    // characters never get split.
    std::thread::spawn(move || {
        let mut carry: Vec<u8> = Vec::new();
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    carry.extend_from_slice(&buf[..n]);
                    let valid_to = match std::str::from_utf8(&carry) {
                        Ok(_) => carry.len(),
                        Err(e) => e.valid_up_to(),
                    };
                    if valid_to > 0 {
                        let text =
                            String::from_utf8_lossy(&carry[..valid_to]).into_owned();
                        let _ = app.emit("term-data", TermData { id, data: text });
                        carry.drain(..valid_to);
                    }
                    // Safety valve: if garbage accumulates, flush lossily.
                    if carry.len() > 4 {
                        let text = String::from_utf8_lossy(&carry).into_owned();
                        let _ = app.emit("term-data", TermData { id, data: text });
                        carry.clear();
                    }
                }
            }
        }
        let _ = app.emit("term-exit", TermExit { id });
    });

    Ok(id)
}

/// Spawn `ssh <target>` under a real PTY.
///
/// The terminal uses the same ControlMaster socket directory as file
/// operations, so an interactive login (password / 2FA / OTP typed in
/// the terminal) opens the master connection that file browsing and
/// transfers then reuse with zero extra authentication.
///
/// `cwd` is a directory on the *server* to start in (the pane's folder),
/// `prelude` carries environment set-up (e.g. the matplotlib backend that
/// makes plt.show() display in the app). Both run before the session's
/// startup command, then everything hands over to a login shell.
pub fn open_ssh(
    app: AppHandle,
    state: &AppState,
    t: &Target,
    cwd: Option<&str>,
    rows: u16,
    cols: u16,
    prelude: Option<&str>,
) -> Result<u64, String> {
    // On Windows the app owns the SSH connection, so a terminal is just
    // another channel on it — no second ssh.exe, no second login, and no
    // second opinion about host keys.
    #[cfg(windows)]
    {
        let id = state.next_id.fetch_add(1, Ordering::SeqCst);
        crate::mux::shell_open(app, t, id, rows, cols, cwd, prelude)?;
        Ok(id)
    }

    #[cfg(not(windows))]
    {
    let mut cmd = CommandBuilder::new(plat::ssh_exe());
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    for a in t.control_opts().map_err(|e| e.to_string())? {
        cmd.arg(a);
    }
    for a in t.host_opts("-p") {
        cmd.arg(a);
    }

    let prelude = prelude.filter(|p| !p.trim().is_empty());
    let cwd = cwd.filter(|p| !p.trim().is_empty());
    if prelude.is_some() || cwd.is_some() || t.startup.is_some() {
        let mut remote = String::new();
        // A missing directory must not abort the login — fall back to $HOME.
        if let Some(d) = cwd {
            remote.push_str(&format!("cd {} 2>/dev/null; ", ops::sh_quote(d)));
        }
        if let Some(p) = prelude {
            remote.push_str(p.trim());
            if !remote.ends_with(';') {
                remote.push(';');
            }
            remote.push(' ');
        }
        if let Some(startup) = &t.startup {
            remote.push_str(startup);
            remote.push_str("; ");
        }
        remote.push_str("exec $SHELL -l");
        cmd.arg("-t");
        cmd.arg(&t.destination);
        cmd.arg("--");
        cmd.arg(remote);
    } else {
        cmd.arg(&t.destination);
    }

    spawn(app, state, cmd, rows, cols)
    }
}

/// Spawn the user's login shell on this machine under a PTY — the same
/// terminal experience as a remote tab, without an SSH hop. Handy for
/// preparing files before an upload, running git, or `scp`-free work.
///
/// `plots` wires the bundled matplotlib backend into the shell's
/// environment so `plt.show()` opens a viewer tab for local scripts too.
pub fn open_local(
    app: AppHandle,
    state: &AppState,
    cwd: Option<&str>,
    rows: u16,
    cols: u16,
    plots: bool,
) -> Result<u64, String> {
    // A default-prog builder runs the passwd/$SHELL login shell with a
    // leading-dash argv0, exactly like a terminal emulator would. On
    // Windows that would be cmd.exe; PowerShell is the better default,
    // so pick explicitly there.
    #[cfg(windows)]
    let mut cmd = {
        let shell = plat::local_shell();
        let mut c = CommandBuilder::new(&shell[0]);
        for a in &shell[1..] {
            c.arg(a);
        }
        c
    };
    #[cfg(not(windows))]
    let mut cmd = CommandBuilder::new_default_prog();
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    if let Some(d) = cwd.filter(|p| !p.trim().is_empty()) {
        cmd.cwd(d);
    }
    if plots {
        if let Some(home) = dirs::home_dir() {
            let base = home.join(".ssh_cli");
            let base = base.to_string_lossy().into_owned();
            // PYTHONPATH is ';'-separated on Windows, ':' elsewhere.
            let pp = match std::env::var("PYTHONPATH") {
                Ok(v) if !v.is_empty() => format!("{base}{}{v}", plat::path_sep()),
                _ => base,
            };
            cmd.env("PYTHONPATH", pp);
            cmd.env("MPLBACKEND", "module://ssh_cli_mpl");
        }
    }
    spawn(app, state, cmd, rows, cols)
}

pub fn write(state: &AppState, id: u64, data: &str) -> Result<(), String> {
    #[cfg(windows)]
    if crate::mux::shell_exists(id) {
        return crate::mux::shell_write(id, data);
    }
    let mut terms = state.terms.lock().unwrap();
    let t = terms.get_mut(&id).ok_or("no such terminal")?;
    t.writer
        .write_all(data.as_bytes())
        .map_err(|e| format!("write: {e}"))?;
    let _ = t.writer.flush();
    Ok(())
}

pub fn resize(state: &AppState, id: u64, rows: u16, cols: u16) -> Result<(), String> {
    #[cfg(windows)]
    if crate::mux::shell_exists(id) {
        return crate::mux::shell_resize(id, rows, cols);
    }
    let terms = state.terms.lock().unwrap();
    let t = terms.get(&id).ok_or("no such terminal")?;
    t.master
        .resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
        .map_err(|e| format!("resize: {e}"))
}

pub fn close(state: &AppState, id: u64) -> Result<(), String> {
    #[cfg(windows)]
    if crate::mux::shell_exists(id) {
        crate::mux::shell_close(id);
        return Ok(());
    }
    let mut terms = state.terms.lock().unwrap();
    if let Some(mut t) = terms.remove(&id) {
        let _ = t.child.kill();
    }
    Ok(())
}
