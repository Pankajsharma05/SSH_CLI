use crate::ops;
use crate::target::Target;
use serde::Serialize;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tauri::{AppHandle, Emitter};

pub struct EditSession {
    pub stop: Arc<AtomicBool>,
}

#[derive(Clone, Serialize)]
struct EditStatus {
    id: u64,
    msg: String,
    alive: bool,
}

fn scp_run(args: &[String]) -> bool {
    Command::new("scp")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn mtime(p: &PathBuf) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// Download `remote_path` to a private temp dir, open it in the default
/// local editor, then watch the temp file and upload it back on every
/// save until stopped. Status flows through `edit-status` events.
pub fn open(
    app: AppHandle,
    id: u64,
    t: Target,
    remote_path: String,
) -> Result<(EditSession, String), String> {
    let fname = remote_path
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("file")
        .to_string();
    let dir = std::env::temp_dir().join("ssh_cli_edit").join(id.to_string());
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let local = dir.join(&fname);

    // Initial download.
    let mut args = ops::scp_args(&t)?;
    args.push(format!("{}:{}", t.destination, remote_path));
    args.push(local.to_string_lossy().into_owned());
    if !scp_run(&args) {
        return Err(format!(
            "could not download {remote_path} — is the connection up?"
        ));
    }

    // Open with the default app for this file type.
    let _ = Command::new(crate::ops::opener()).arg(&local).spawn();

    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = stop.clone();
    let label = format!("{} @ {}", fname, t.raw);
    let dest_spec = format!("{}:{}", t.destination, remote_path);
    let up_base = ops::scp_args(&t)?;
    let local2 = local.clone();

    std::thread::spawn(move || {
        let mut last = mtime(&local2);
        loop {
            if stop2.load(Ordering::Relaxed) {
                break;
            }
            std::thread::sleep(Duration::from_millis(1000));
            let now = mtime(&local2);
            if now.is_some() && now != last {
                last = now;
                let mut args = up_base.clone();
                args.push(local2.to_string_lossy().into_owned());
                args.push(dest_spec.clone());
                let ok = scp_run(&args);
                let _ = app.emit(
                    "edit-status",
                    EditStatus {
                        id,
                        msg: if ok { "saved → uploaded".into() } else { "upload FAILED".into() },
                        alive: true,
                    },
                );
            }
        }
        let _ = app.emit(
            "edit-status",
            EditStatus { id, msg: "watch stopped".into(), alive: false },
        );
    });

    Ok((EditSession { stop }, label))
}
