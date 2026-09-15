use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Session {
    pub host: String,
    #[serde(default)]
    pub user: Option<String>,
    #[serde(default)]
    pub port: Option<u16>,
    /// Path to a private key, `~` allowed.
    #[serde(default)]
    pub identity: Option<String>,
    /// ProxyJump host (bastion), e.g. "user@gateway.example.com".
    #[serde(default)]
    pub jump: Option<String>,
    /// Command(s) run on terminal open, then an interactive shell.
    #[serde(default)]
    pub startup: Option<String>,
}

impl Session {
    /// "user@host" (or just "host" if no user saved).
    pub fn destination(&self) -> String {
        match &self.user {
            Some(u) => format!("{}@{}", u, self.host),
            None => self.host.clone(),
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub sessions: BTreeMap<String, Session>,
}

pub fn config_path() -> Result<PathBuf> {
    let base = dirs::config_dir().ok_or_else(|| anyhow!("cannot determine config directory"))?;
    Ok(base.join("ssh_cli").join("sessions.toml"))
}

pub fn socket_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| anyhow!("cannot determine home directory"))?;
    let dir = home.join(".ssh").join("ssh_cli_sockets");
    if !dir.exists() {
        fs::create_dir_all(&dir).context("creating control-socket directory")?;
        // Restrictive permissions like ~/.ssh itself.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(0o700));
        }
    }
    Ok(dir)
}

pub fn load() -> Result<Config> {
    let path = config_path()?;
    if !path.exists() {
        return Ok(Config::default());
    }
    let text = fs::read_to_string(&path)
        .with_context(|| format!("reading {}", path.display()))?;
    let cfg: Config = toml::from_str(&text)
        .with_context(|| format!("parsing {}", path.display()))?;
    Ok(cfg)
}

pub fn save(cfg: &Config) -> Result<()> {
    let path = config_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).context("creating config directory")?;
    }
    let text = toml::to_string_pretty(cfg).context("serializing config")?;
    fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// Expand a leading `~` to the home directory.
pub fn expand_tilde(p: &str) -> String {
    if let Some(rest) = p.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest).to_string_lossy().into_owned();
        }
    }
    p.to_string()
}

pub fn ui_state_path() -> Result<PathBuf> {
    let base = dirs::config_dir().ok_or_else(|| anyhow!("cannot determine config directory"))?;
    Ok(base.join("ssh_cli").join("uistate.json"))
}

#[derive(Debug, serde::Serialize)]
pub struct ImportedHost {
    pub name: String,
    pub host: String,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub identity: Option<String>,
    pub jump: Option<String>,
}

/// Parse ~/.ssh/config into importable host entries (concrete aliases only).
pub fn parse_ssh_config() -> Vec<ImportedHost> {
    let Some(home) = dirs::home_dir() else { return Vec::new() };
    let Ok(text) = fs::read_to_string(home.join(".ssh").join("config")) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut aliases: Vec<String> = Vec::new();
    let mut props: std::collections::HashMap<String, String> = Default::default();

    let flush = |aliases: &mut Vec<String>,
                     props: &mut std::collections::HashMap<String, String>,
                     out: &mut Vec<ImportedHost>| {
        for a in aliases.drain(..) {
            if a.contains('*') || a.contains('?') || a == "localhost" {
                continue;
            }
            out.push(ImportedHost {
                host: props.get("hostname").cloned().unwrap_or_else(|| a.clone()),
                name: a,
                user: props.get("user").cloned(),
                port: props.get("port").and_then(|p| p.parse().ok()),
                identity: props.get("identityfile").cloned(),
                jump: props.get("proxyjump").cloned(),
            });
        }
        props.clear();
    };

    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut it = line.splitn(2, |c: char| c.is_whitespace() || c == '=');
        let key = it.next().unwrap_or("").to_lowercase();
        let val = it.next().unwrap_or("").trim().trim_matches('"').to_string();
        match key.as_str() {
            "host" => {
                flush(&mut aliases, &mut props, &mut out);
                aliases = val.split_whitespace().map(|s| s.to_string()).collect();
            }
            "match" => {
                flush(&mut aliases, &mut props, &mut out);
                aliases.clear();
            }
            _ if !aliases.is_empty() => {
                props.entry(key).or_insert(val);
            }
            _ => {}
        }
    }
    flush(&mut aliases, &mut props, &mut out);
    out
}
