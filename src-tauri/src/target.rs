use crate::config::{self, Config, Session};
use anyhow::Result;

/// A resolved connection target: either a saved session or a raw
/// ssh destination (which may itself be an alias from ~/.ssh/config).
#[derive(Debug, Clone)]
pub struct Target {
    /// What the user typed (saved-session name or user@host).
    pub raw: String,
    /// "user@host" handed to ssh/scp.
    pub destination: String,
    pub port: Option<u16>,
    pub identity: Option<String>,
    pub jump: Option<String>,
    pub startup: Option<String>,
}

impl Target {
    pub fn resolve(cfg: &Config, raw: &str) -> Target {
        if let Some(s) = cfg.sessions.get(raw) {
            Target::from_session(raw, s)
        } else {
            Target {
                raw: raw.to_string(),
                destination: raw.to_string(),
                port: None,
                identity: None,
                jump: None,
                startup: None,
            }
        }
    }

    fn from_session(name: &str, s: &Session) -> Target {
        Target {
            raw: name.to_string(),
            destination: s.destination(),
            port: s.port,
            identity: s.identity.clone(),
            jump: s.jump.clone(),
            startup: s.startup.clone(),
        }
    }

    /// Multiplexing + keepalive options shared by ssh and scp.
    ///
    /// On macOS and Linux these set up the shared ControlMaster socket:
    /// one TCP connection and one authentication per host, with every
    /// subsequent command attaching to the live master.
    ///
    /// Windows OpenSSH has no multiplexing, so there the same call
    /// returns an explicit opt-out instead (see `plat::mux_opts` for
    /// why silence is not good enough) and connection reuse is handled
    /// by `mux.rs` at a different layer.
    #[cfg(not(windows))]
    pub fn control_opts(&self) -> Result<Vec<String>> {
        let sockdir = config::socket_dir()?;
        Ok(vec![
            "-o".into(),
            "ControlMaster=auto".into(),
            "-o".into(),
            format!("ControlPath={}/%C", sockdir.display()),
            "-o".into(),
            "ControlPersist=600".into(),
            "-o".into(),
            "ServerAliveInterval=30".into(),
            "-o".into(),
            "ServerAliveCountMax=4".into(),
        ])
    }

    #[cfg(windows)]
    pub fn control_opts(&self) -> Result<Vec<String>> {
        Ok(crate::plat::mux_opts())
    }

    /// Options specific to this target (port flag differs: ssh -p, scp -P).
    pub fn host_opts(&self, port_flag: &str) -> Vec<String> {
        let mut v = Vec::new();
        if let Some(p) = self.port {
            v.push(port_flag.to_string());
            v.push(p.to_string());
        }
        if let Some(id) = &self.identity {
            v.push("-i".into());
            v.push(config::expand_tilde(id));
        }
        if let Some(j) = &self.jump {
            v.push("-J".into());
            v.push(j.clone());
        }
        v
    }
}
