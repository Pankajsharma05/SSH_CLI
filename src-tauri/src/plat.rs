//! Platform differences, kept in one place.
//!
//! Everything that behaves differently on Windows than on macOS/Linux
//! lives here so the rest of the code can stay readable: which `ssh`
//! binary to run, how to spawn a child without flashing a console
//! window, how to open a file in the desktop's default application,
//! which shell a local terminal tab should start, and how to ask the
//! OS for free disk space.

use std::process::Command;

// ---------------------------------------------------------------- spawning

/// Build a `Command` that never flashes a console window.
///
/// The app is linked as a GUI binary (`windows_subsystem = "windows"`),
/// so every `ssh.exe` / `scp.exe` / `tar.exe` child would otherwise pop
/// a black console window for as long as it runs — and a directory
/// listing spawns one of those. `CREATE_NO_WINDOW` suppresses it.
/// On Unix this is just `Command::new`.
pub fn cmd(program: &str) -> Command {
    #[allow(unused_mut)]
    let mut c = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    c
}

// ---------------------------------------------------------------- ssh binaries

/// Absolute path to a Windows OpenSSH tool, if the inbox copy is there.
///
/// `ssh` on a developer's PATH is very often Git-for-Windows' MSYS2
/// build, which cannot talk to the Windows `ssh-agent` service (that
/// agent lives behind a named pipe the MSYS2 binary does not speak).
/// Preferring `%SystemRoot%\System32\OpenSSH\` means saved keys in the
/// Windows agent actually get used, which is what makes unattended
/// file operations work at all.
#[cfg(windows)]
fn inbox(tool: &str) -> Option<String> {
    let root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
    let p = std::path::Path::new(&root)
        .join("System32")
        .join("OpenSSH")
        .join(format!("{tool}.exe"));
    p.exists().then(|| p.to_string_lossy().into_owned())
}

#[cfg(windows)]
fn tool(name: &str) -> String {
    inbox(name).unwrap_or_else(|| name.to_string())
}

#[cfg(not(windows))]
fn tool(name: &str) -> String {
    name.to_string()
}

pub fn ssh_exe() -> String {
    tool("ssh")
}
pub fn scp_exe() -> String {
    tool("scp")
}
pub fn ssh_keygen_exe() -> String {
    tool("ssh-keygen")
}

/// Multiplexing options, or the explicit opt-out.
///
/// On Unix these set up the shared ControlMaster socket. On Windows
/// multiplexing does not exist, and simply *omitting* the flags is not
/// enough: `ssh.exe` still reads `~/.ssh/config`, and a `Host *` stanza
/// with `ControlMaster auto` (very common for anyone who also uses SSH
/// from WSL, Git Bash or a Mac) makes it try anyway and abort with
/// `getsockname failed: Not a socket`. A command-line `-o` beats the
/// config file, so we override it explicitly.
#[cfg(windows)]
pub fn mux_opts() -> Vec<String> {
    vec![
        "-o".into(),
        "ControlMaster=no".into(),
        "-o".into(),
        "ControlPath=none".into(),
        "-o".into(),
        "ServerAliveInterval=30".into(),
        "-o".into(),
        "ServerAliveCountMax=4".into(),
    ]
}

// ---------------------------------------------------------------- opening files

/// Hand a path to the desktop's default application.
pub fn open_path(path: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let mut c = {
        let mut c = cmd("open");
        c.arg(path);
        c
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut c = {
        let mut c = cmd("xdg-open");
        c.arg(path);
        c
    };
    // `start` is a cmd.exe builtin, not a program. The empty "" is the
    // window-title argument: without it cmd swallows a quoted path as
    // the title and opens nothing.
    #[cfg(windows)]
    let mut c = {
        let mut c = cmd("cmd");
        c.args(["/C", "start", "", path]);
        c
    };
    c.spawn().map(|_| ()).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------- local shell

/// Program + args for a local terminal tab's login shell.
///
/// `portable-pty`'s default program on Windows is whatever `ComSpec`
/// says, i.e. `cmd.exe`. PowerShell is the better default in 2025 —
/// prefer PowerShell 7 (`pwsh`) if it is installed, then Windows
/// PowerShell, then `cmd`.
#[cfg(windows)]
pub fn local_shell() -> Vec<String> {
    if which("pwsh.exe").is_some() {
        return vec!["pwsh.exe".into(), "-NoLogo".into()];
    }
    let root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
    let ps = std::path::Path::new(&root)
        .join("System32")
        .join("WindowsPowerShell")
        .join("v1.0")
        .join("powershell.exe");
    if ps.exists() {
        return vec![ps.to_string_lossy().into_owned(), "-NoLogo".into()];
    }
    vec![std::env::var("ComSpec").unwrap_or_else(|_| "cmd.exe".into())]
}

/// Minimal PATH lookup — no extra dependency for one question.
#[cfg(windows)]
fn which(exe: &str) -> Option<std::path::PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|d| d.join(exe))
            .find(|p| p.is_file())
    })
}

/// Separator for the `PYTHONPATH`-style search path variables.
pub fn path_sep() -> &'static str {
    if cfg!(windows) {
        ";"
    } else {
        ":"
    }
}

// ---------------------------------------------------------------- disk usage

/// Free / total bytes for the filesystem holding `path`.
///
/// Unix shells out to POSIX `df -Pk`. Windows has no `df`, so this
/// calls `GetDiskFreeSpaceExW` directly through a tiny `extern` block
/// rather than pulling in a system-info crate for two numbers.
#[cfg(windows)]
pub fn local_free_total(path: &str) -> Option<(u64, u64)> {
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "kernel32")]
    extern "system" {
        fn GetDiskFreeSpaceExW(
            directory: *const u16,
            free_to_caller: *mut u64,
            total: *mut u64,
            total_free: *mut u64,
        ) -> i32;
    }

    // The API wants a directory; a file path has to be trimmed back.
    let p = std::path::Path::new(path);
    let dir = if p.is_dir() {
        p.to_path_buf()
    } else {
        p.parent()?.to_path_buf()
    };
    let wide: Vec<u16> = std::ffi::OsStr::new(&dir)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    let (mut free, mut total, mut total_free) = (0u64, 0u64, 0u64);
    let ok = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut free, &mut total, &mut total_free) };
    (ok != 0).then_some((free, total))
}

/// Drive roots that currently exist, for the local pane's places menu.
#[cfg(windows)]
pub fn drives() -> Vec<String> {
    ('A'..='Z')
        .map(|c| format!("{c}:\\"))
        .filter(|d| std::path::Path::new(d).is_dir())
        .collect()
}

// ---------------------------------------------------------------- tar

/// Program used to unpack a downloaded `.tar.gz`.
///
/// Windows 10 1803 and later ship bsdtar as `tar.exe`, which reads
/// gzip archives, so the same invocation works everywhere.
pub fn tar_exe() -> &'static str {
    "tar"
}
