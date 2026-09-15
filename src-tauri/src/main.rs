#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod edit;
mod fwd;
#[cfg(windows)]
mod mux;
mod ops;
mod plat;
mod state;
mod target;
mod term;

use config::Session;
use ops::{Entry, Side};
use serde::{Deserialize, Serialize};
use state::AppState;
#[cfg(not(windows))]
use std::process::Stdio;
use target::Target;

#[derive(Serialize)]
struct SessionInfo {
    name: String,
    destination: String,
    port: Option<u16>,
    jump: Option<String>,
    live: bool,
}

#[derive(Deserialize)]
struct SideSpec {
    /// Saved session name / user@host, or null for the local machine.
    target: Option<String>,
    path: String,
}

fn resolve(name: &str) -> Result<Target, String> {
    let cfg = config::load().map_err(|e| e.to_string())?;
    Ok(Target::resolve(&cfg, name))
}

/// Is a reusable, already-authenticated connection to this host up?
#[cfg(not(windows))]
fn master_alive(t: &Target) -> bool {
    let Ok(mut args) = t.control_opts() else { return false };
    args.push("-O".into());
    args.push("check".into());
    args.extend(t.host_opts("-p"));
    args.push(t.destination.clone());
    plat::cmd(&plat::ssh_exe())
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Windows has no control socket to interrogate, so "alive" means the
/// persistent shell in `mux.rs` answers.
#[cfg(windows)]
fn master_alive(t: &Target) -> bool {
    mux::alive(t)
}

// ---------- sessions ----------

#[tauri::command]
async fn sessions_list() -> Result<Vec<SessionInfo>, String> {
    let cfg = config::load().map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for (name, s) in &cfg.sessions {
        let t = Target::resolve(&cfg, name);
        // On Unix this is a cheap socket poke. On Windows "is it live"
        // can only be answered by *using* the connection, which would
        // mean dialling every saved host every time the list is drawn —
        // so report only connections already established.
        #[cfg(windows)]
        let live = mux::established(&t);
        #[cfg(not(windows))]
        let live = master_alive(&t);
        out.push(SessionInfo {
            name: name.clone(),
            destination: s.destination(),
            port: s.port,
            jump: s.jump.clone(),
            live,
        });
    }
    Ok(out)
}

#[tauri::command]
async fn session_add(
    name: String,
    destination: String,
    port: Option<u16>,
    identity: Option<String>,
    jump: Option<String>,
    startup: Option<String>,
) -> Result<(), String> {
    let mut cfg = config::load().map_err(|e| e.to_string())?;
    let (user, host) = match destination.split_once('@') {
        Some((u, h)) => (Some(u.to_string()), h.to_string()),
        None => (None, destination),
    };
    let clean = |o: Option<String>| o.filter(|s| !s.trim().is_empty());
    cfg.sessions.insert(
        name,
        Session {
            host,
            user,
            port,
            identity: clean(identity),
            jump: clean(jump),
            startup: clean(startup),
        },
    );
    config::save(&cfg).map_err(|e| e.to_string())
}

#[tauri::command]
async fn session_remove(name: String) -> Result<(), String> {
    let mut cfg = config::load().map_err(|e| e.to_string())?;
    cfg.sessions
        .remove(&name)
        .ok_or_else(|| format!("no saved session '{name}'"))?;
    config::save(&cfg).map_err(|e| e.to_string())
}

#[tauri::command]
async fn master_close(target: String) -> Result<(), String> {
    let t = resolve(&target)?;
    #[cfg(windows)]
    {
        mux::close(&t);
    }
    #[cfg(not(windows))]
    {
        let mut args = t.control_opts().map_err(|e| e.to_string())?;
        args.push("-O".into());
        args.push("exit".into());
        args.extend(t.host_opts("-p"));
        args.push(t.destination.clone());
        let _ = plat::cmd(&plat::ssh_exe())
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    Ok(())
}

// ---------- terminals ----------

/// Open a terminal tab. `target` is a saved session / user@host, or null
/// for a shell on this machine. `cwd` starts the session in a folder —
/// the pane you opened it from.
#[tauri::command]
#[allow(clippy::too_many_arguments)]   // it is an IPC entry point, not an API
async fn term_open(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    target: Option<String>,
    cwd: Option<String>,
    rows: u16,
    cols: u16,
    prelude: Option<String>,
    plots: Option<bool>,
) -> Result<u64, String> {
    match target {
        None => term::open_local(
            app,
            &state,
            cwd.as_deref(),
            rows,
            cols,
            plots.unwrap_or(false),
        ),
        Some(name) => {
            let t = resolve(&name)?;
            term::open_ssh(app, &state, &t, cwd.as_deref(), rows, cols, prelude.as_deref())
        }
    }
}

#[tauri::command]
async fn term_write(
    state: tauri::State<'_, AppState>,
    id: u64,
    data: String,
) -> Result<(), String> {
    term::write(&state, id, &data)
}

#[tauri::command]
async fn term_resize(
    state: tauri::State<'_, AppState>,
    id: u64,
    rows: u16,
    cols: u16,
) -> Result<(), String> {
    term::resize(&state, id, rows, cols)
}

#[tauri::command]
async fn term_close(state: tauri::State<'_, AppState>, id: u64) -> Result<(), String> {
    term::close(&state, id)
}

/// Is the multiplexed master connection to this host up? Polled after a
/// terminal login so the file panes can jump to the cluster's home
/// directory the moment authentication succeeds.
#[tauri::command]
async fn host_connected(target: String) -> Result<bool, String> {
    Ok(master_alive(&resolve(&target)?))
}

#[tauri::command]
async fn host_info(target: String) -> Result<serde_json::Value, String> {
    ops::host_info(&resolve(&target)?)
}

/// Named folders for a pane's places menu: $HOME plus whatever
/// scratch/work/project space this cluster actually has.
#[tauri::command]
async fn places(target: Option<String>) -> Result<Vec<ops::Place>, String> {
    match target {
        None => Ok(ops::local_places()),
        Some(name) => ops::remote_places(&resolve(&name)?),
    }
}

// ---------- browsing ----------

#[tauri::command]
async fn local_home() -> Result<String, String> {
    dirs::home_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .ok_or_else(|| "no home directory".into())
}

#[tauri::command]
async fn remote_home(target: String) -> Result<String, String> {
    ops::remote_home(&resolve(&target)?)
}

#[tauri::command]
async fn list_local(path: String) -> Result<Vec<Entry>, String> {
    ops::local_list(&path)
}

#[tauri::command]
async fn list_remote(target: String, path: String) -> Result<Vec<Entry>, String> {
    ops::remote_list(&resolve(&target)?, &path)
}

// ---------- file operations ----------

#[tauri::command]
async fn fs_mkdir(target: Option<String>, path: String) -> Result<(), String> {
    match target {
        None => std::fs::create_dir_all(&path).map_err(|e| e.to_string()),
        Some(name) => ops::remote_mkdir(&resolve(&name)?, &path),
    }
}

#[tauri::command]
async fn fs_delete(target: Option<String>, path: String) -> Result<(), String> {
    match target {
        None => {
            let meta = std::fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
            if meta.is_dir() {
                std::fs::remove_dir_all(&path).map_err(|e| e.to_string())
            } else {
                std::fs::remove_file(&path).map_err(|e| e.to_string())
            }
        }
        Some(name) => ops::remote_delete(&resolve(&name)?, &path),
    }
}

#[tauri::command]
async fn fs_rename(
    target: Option<String>,
    from: String,
    to: String,
) -> Result<(), String> {
    match target {
        None => std::fs::rename(&from, &to).map_err(|e| e.to_string()),
        Some(name) => ops::remote_rename(&resolve(&name)?, &from, &to),
    }
}

// ---------- transfers ----------

#[tauri::command]
async fn start_transfer(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    id: u64,
    sources: Vec<SideSpec>,
    dest: SideSpec,
    engine: Option<String>,
) -> Result<(), String> {
    let to_side = |s: &SideSpec| -> Result<Side, String> {
        Ok(match &s.target {
            None => Side::Local(s.path.clone()),
            Some(name) => Side::Remote(resolve(name)?, s.path.clone()),
        })
    };

    if sources.is_empty() {
        return Err("no sources selected".into());
    }

    // Same-host remote copy never leaves the server.
    if let (1, Some(st), Some(dt)) = (
        sources.len(),
        sources[0].target.as_deref(),
        dest.target.as_deref(),
    ) {
        if st == dt {
            let t = resolve(st)?;
            let (from, to) = (sources[0].path.clone(), dest.path.clone());
            let app2 = app.clone();
            std::thread::spawn(move || {
                let ok = ops::remote_copy_same_host(&t, &from, &to).is_ok();
                let _ = tauri::Emitter::emit(
                    &app2,
                    "xfer-done",
                    serde_json::json!({ "id": id, "ok": ok }),
                );
            });
            return Ok(());
        }
    }

    let mut side_sources = Vec::new();
    for s in &sources {
        side_sources.push(to_side(s)?);
    }
    let handle: ops::XferHandle = std::sync::Arc::new(std::sync::Mutex::new(None));
    state.transfers.lock().unwrap().insert(id, handle.clone());
    ops::transfer(
        app,
        id,
        side_sources,
        to_side(&dest)?,
        engine.as_deref().unwrap_or("scp"),
        handle,
    )
}

#[tauri::command]
async fn xfer_cancel(state: tauri::State<'_, AppState>, id: u64) -> Result<(), String> {
    #[cfg(windows)]
    mux::cancel(id);
    if let Some(h) = state.transfers.lock().unwrap().get(&id) {
        if let Some(c) = h.lock().unwrap().as_mut() {
            let _ = c.kill();
        }
    }
    Ok(())
}

#[tauri::command]
async fn compress_download(
    app: tauri::AppHandle,
    id: u64,
    target: String,
    remote_dir: String,
    names: Vec<String>,
    local_dir: String,
) -> Result<(), String> {
    let t = resolve(&target)?;
    ops::compress_download(app, id, t, remote_dir, names, local_dir)
}

// ---------- misc host tools ----------

#[tauri::command]
async fn remote_exec(target: String, cmd: String) -> Result<String, String> {
    ops::remote_exec(&resolve(&target)?, &cmd)
}

#[tauri::command]
async fn remote_search(target: String, base: String, query: String) -> Result<Vec<String>, String> {
    ops::remote_search(&resolve(&target)?, &base, &query)
}

#[tauri::command]
async fn fs_disk(target: Option<String>, path: String) -> Result<String, String> {
    match target {
        None => ops::disk_usage(None, &path),
        Some(name) => ops::disk_usage(Some(&resolve(&name)?), &path),
    }
}

#[tauri::command]
async fn fs_chmod(target: Option<String>, path: String, mode: String) -> Result<(), String> {
    match target {
        None => ops::chmod_local(&path, &mode),
        Some(name) => ops::chmod_remote(&resolve(&name)?, &path, &mode),
    }
}

#[tauri::command]
async fn file_read(target: Option<String>, path: String) -> Result<String, String> {
    match target {
        None => ops::local_read_text(&path),
        Some(name) => ops::remote_read_text(&resolve(&name)?, &path),
    }
}

#[tauri::command]
async fn file_write(target: Option<String>, path: String, data: String) -> Result<(), String> {
    match target {
        None => ops::local_write_text(&path, &data),
        Some(name) => ops::remote_write_text(&resolve(&name)?, &path, &data),
    }
}

#[tauri::command]
async fn file_read_b64(target: Option<String>, path: String) -> Result<String, String> {
    let bytes = match target {
        None => ops::local_read_bytes(&path)?,
        Some(name) => ops::remote_read_bytes(&resolve(&name)?, &path)?,
    };
    Ok(ops::base64_encode(&bytes))
}

#[tauri::command]
async fn file_stat(target: Option<String>, path: String) -> Result<serde_json::Value, String> {
    let (mtime, size) = match target {
        None => {
            let meta = std::fs::metadata(&path).map_err(|e| e.to_string())?;
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            (mtime, meta.len())
        }
        Some(name) => ops::remote_stat(&resolve(&name)?, &path)?,
    };
    Ok(serde_json::json!({ "mtime": mtime, "size": size }))
}

/// The matplotlib backend shipped inside the binary, installed on demand.
const MPL_SHIM: &str = include_str!("ssh_cli_mpl.py");

/// Install the matplotlib shim so `plt.show()` renders into a directory the
/// app watches. Idempotent — safe to call before every terminal.
#[tauri::command]
async fn plots_enable(target: Option<String>) -> Result<serde_json::Value, String> {
    let (base, dir) = match target {
        None => {
            let home = dirs::home_dir().ok_or("no home directory")?;
            let base = home.join(".ssh_cli");
            let dir = base.join("plots");
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            std::fs::write(base.join("ssh_cli_mpl.py"), MPL_SHIM).map_err(|e| e.to_string())?;
            (
                base.to_string_lossy().into_owned(),
                dir.to_string_lossy().into_owned(),
            )
        }
        Some(name) => {
            let t = resolve(&name)?;
            let home = ops::remote_home(&t)?;
            let base = format!("{}/.ssh_cli", home.trim_end_matches('/'));
            let dir = format!("{base}/plots");
            ops::remote_exec(&t, &format!("mkdir -p {}", ops::sh_quote(&dir)))?;
            ops::remote_write_text(&t, &format!("{base}/ssh_cli_mpl.py"), MPL_SHIM)?;
            (base, dir)
        }
    };
    Ok(serde_json::json!({ "base": base, "dir": dir }))
}

#[tauri::command]
async fn local_open(path: String) -> Result<(), String> {
    ops::open_path(&path)
}

// ---------- remote editing ----------

#[tauri::command]
async fn edit_open(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    target: String,
    path: String,
) -> Result<serde_json::Value, String> {
    let t = resolve(&target)?;
    let id = state.next_id.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let (sess, label) = edit::open(app, id, t, path)?;
    state.edits.lock().unwrap().insert(id, sess);
    Ok(serde_json::json!({ "id": id, "label": label }))
}

#[tauri::command]
async fn edit_stop(state: tauri::State<'_, AppState>, id: u64) -> Result<(), String> {
    if let Some(s) = state.edits.lock().unwrap().remove(&id) {
        s.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    Ok(())
}

// ---------- port forwarding ----------

#[tauri::command]
async fn fwd_start(
    state: tauri::State<'_, AppState>,
    target: String,
    kind: String,
    spec: String,
) -> Result<u64, String> {
    let t = resolve(&target)?;
    let f = fwd::start(&t, &kind, &spec)?;
    let id = state.next_id.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    state.fwds.lock().unwrap().insert(id, f);
    Ok(id)
}

#[tauri::command]
async fn fwd_stop(state: tauri::State<'_, AppState>, id: u64) -> Result<(), String> {
    if let Some(mut f) = state.fwds.lock().unwrap().remove(&id) {
        let _ = f.child.kill();
    }
    Ok(())
}

#[tauri::command]
async fn fwd_list(state: tauri::State<'_, AppState>) -> Result<Vec<fwd::ForwardInfo>, String> {
    let mut fwds = state.fwds.lock().unwrap();
    // prune forwards whose ssh process died
    fwds.retain(|_, f| matches!(f.child.try_wait(), Ok(None)));
    Ok(fwds
        .iter()
        .map(|(id, f)| fwd::ForwardInfo { id: *id, label: f.label.clone() })
        .collect())
}

/// Answer a password / passphrase / 2FA challenge raised by the Windows
/// transport. `text` of null means the user cancelled.
///
/// Sync on purpose: async commands share the tokio worker pool with the
/// operation that is blocked waiting for this reply, and on a busy pool
/// that is a deadlock waiting to happen. Sync commands run on the main
/// thread and always get through.
#[tauri::command]
#[cfg_attr(not(windows), allow(unused_variables))]
fn auth_reply(id: u64, text: Option<String>) {
    #[cfg(windows)]
    mux::answer(id, text);
}

// ---------- ui state + ssh config import ----------

#[tauri::command]
async fn ui_load() -> Result<String, String> {
    let path = config::ui_state_path().map_err(|e| e.to_string())?;
    Ok(std::fs::read_to_string(path).unwrap_or_else(|_| "{}".into()))
}

#[tauri::command]
async fn ui_save(data: String) -> Result<(), String> {
    let path = config::ui_state_path().map_err(|e| e.to_string())?;
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(path, data).map_err(|e| e.to_string())
}

#[tauri::command]
async fn ssh_config_import() -> Result<Vec<config::ImportedHost>, String> {
    Ok(config::parse_ssh_config())
}

#[tauri::command]
async fn forget_host_key(target: String) -> Result<String, String> {
    let t = resolve(&target)?;
    let host = t
        .destination
        .split('@')
        .next_back()
        .unwrap_or(t.destination.as_str())
        .to_string();
    let pattern = match t.port {
        Some(p) if p != 22 => format!("[{host}]:{p}"),
        _ => host,
    };
    // A stale key also means the multiplexed connection is bound to a
    // host that no longer answers as itself — drop it so the next
    // operation redials.
    #[cfg(windows)]
    mux::close(&t);
    let out = plat::cmd(&plat::ssh_keygen_exe())
        .args(["-R", &pattern])
        .output()
        .map_err(|e| format!("running ssh-keygen: {e}"))?;
    let mut log = String::from_utf8_lossy(&out.stdout).into_owned();
    log.push_str(&String::from_utf8_lossy(&out.stderr));
    Ok(log)
}

/// Hand the in-process transport an `AppHandle`, so it can raise auth
/// prompts from worker threads that have none of their own.
#[cfg(windows)]
fn init_transport(app: &mut tauri::App) {
    use tauri::Manager;
    mux::init(app.handle().clone());
}

#[cfg(not(windows))]
fn init_transport(_app: &mut tauri::App) {}

fn main() {
    tauri::Builder::default()
        .manage(AppState::default())
        .setup(|app| {
            init_transport(app);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            sessions_list,
            session_add,
            session_remove,
            master_close,
            host_connected,
            host_info,
            places,
            term_open,
            term_write,
            term_resize,
            term_close,
            local_home,
            remote_home,
            list_local,
            list_remote,
            fs_mkdir,
            fs_delete,
            fs_rename,
            fs_chmod,
            fs_disk,
            forget_host_key,
            start_transfer,
            xfer_cancel,
            compress_download,
            remote_exec,
            remote_search,
            file_read,
            file_write,
            file_read_b64,
            file_stat,
            plots_enable,
            local_open,
            edit_open,
            edit_stop,
            fwd_start,
            fwd_stop,
            fwd_list,
            auth_reply,
            ui_load,
            ui_save,
            ssh_config_import
        ])
        .run(tauri::generate_context!())
        .expect("error while running SSH_CLI");
}
