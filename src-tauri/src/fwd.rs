use crate::target::Target;
use serde::Serialize;
use std::process::{Child, Command, Stdio};

pub struct Forward {
    pub child: Child,
    pub label: String,
}

#[derive(Clone, Serialize)]
pub struct ForwardInfo {
    pub id: u64,
    pub label: String,
}

/// Spawn `ssh -N` with a single forward.
/// kind: "L" (local), "R" (remote), or "D" (dynamic SOCKS).
/// spec: "8888:localhost:8888" for L/R, or a port like "1080" for D.
pub fn start(t: &Target, kind: &str, spec: &str) -> Result<Forward, String> {
    let flag = match kind {
        "L" => "-L",
        "R" => "-R",
        "D" => "-D",
        _ => return Err("kind must be L, R, or D".into()),
    };
    if spec.is_empty() || spec.contains(char::is_whitespace) {
        return Err("forward spec must not contain spaces".into());
    }
    let mut args = t.control_opts().map_err(|e| e.to_string())?;
    args.push("-o".into());
    args.push("ExitOnForwardFailure=yes".into());
    args.push("-o".into());
    args.push("ConnectTimeout=10".into());
    args.extend(t.host_opts("-p"));
    args.push("-N".into());
    args.push(flag.into());
    args.push(spec.into());
    args.push(t.destination.clone());

    let child = Command::new("ssh")
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("spawn ssh: {e}"))?;

    Ok(Forward {
        child,
        label: format!("{} {} → {}", flag, spec, t.raw),
    })
}
