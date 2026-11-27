//! Server configuration: `%LOCALAPPDATA%\winmuse\config.toml`
//! (override with the WINMUSE_CONFIG environment variable).
//!
//! There is NO silent default profile: a missing file or an unset
//! `profile` means the server runs in `observe` and says so. The
//! file is watched by mtime and hot-reloaded; an invalid edit is
//! rejected and the previous configuration stays in force.
//! See the "Permission profiles" section of the README.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;

use crate::state::Shared;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Profile {
    Observe,
    Guided,
    Full,
}

impl Profile {
    pub fn as_str(&self) -> &'static str {
        match self {
            Profile::Observe => "observe",
            Profile::Guided => "guided",
            Profile::Full => "full",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Config {
    pub profile: Profile,
    /// False when the file is missing or `profile` is unset (the
    /// server is in observe because nobody chose otherwise).
    pub profile_chosen: bool,
    /// Lowercased process names input may land in (guided only).
    pub allow_processes: Vec<String>,
    pub block_win_keys: bool,
    pub require_local_arm: bool,
    pub audit_enabled: bool,
    /// Optional bearer token; when set, the HTTP server requires
    /// `Authorization: Bearer <token>` on every request.
    pub auth_token: Option<String>,
    /// [server] overrides; None = auto (tailnet address, port 18081).
    pub server_host: Option<String>,
    pub server_port: Option<u16>,
    /// [overlay] presentation settings.
    pub overlay_enabled: bool,
    /// "" = follow the OS language, otherwise "ja" or "en".
    pub overlay_lang: String,
    /// Custom operating-toast texts ("" = language preset).
    pub overlay_title: String,
    pub overlay_sub: String,
    /// Human-readable note about the config source, for status.
    pub source_note: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            profile: Profile::Observe,
            profile_chosen: false,
            allow_processes: Vec::new(),
            block_win_keys: true,
            require_local_arm: false,
            audit_enabled: true,
            auth_token: None,
            server_host: None,
            server_port: None,
            overlay_enabled: true,
            overlay_lang: String::new(),
            overlay_title: String::new(),
            overlay_sub: String::new(),
            source_note: "no config file; running in observe".to_string(),
        }
    }
}

impl Config {
    pub fn allows(&self, process_name: &str) -> bool {
        let name = process_name.to_lowercase();
        self.allow_processes.iter().any(|p| *p == name)
    }
}

#[derive(Deserialize, Default)]
struct RawConfig {
    profile: Option<String>,
    auth_token: Option<String>,
    guided: Option<RawGuided>,
    consent: Option<RawConsent>,
    audit: Option<RawAudit>,
    server: Option<RawServer>,
    overlay: Option<RawOverlay>,
}

#[derive(Deserialize, Default)]
struct RawServer {
    host: Option<String>,
    port: Option<u16>,
}

#[derive(Deserialize, Default)]
struct RawOverlay {
    enabled: Option<bool>,
    lang: Option<String>,
    title: Option<String>,
    sub: Option<String>,
}

#[derive(Deserialize, Default)]
struct RawGuided {
    allow_processes: Option<Vec<String>>,
    block_win_keys: Option<bool>,
}

#[derive(Deserialize, Default)]
struct RawConsent {
    require_local_arm: Option<bool>,
}

#[derive(Deserialize, Default)]
struct RawAudit {
    enabled: Option<bool>,
}

/// The winmuse data directory: %LOCALAPPDATA%\winmuse.
pub fn data_dir() -> PathBuf {
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        PathBuf::from(local).join("winmuse")
    } else if let Ok(home) = std::env::var("USERPROFILE") {
        PathBuf::from(home).join("AppData").join("Local").join("winmuse")
    } else {
        PathBuf::from("winmuse")
    }
}

pub fn config_path() -> PathBuf {
    if let Ok(p) = std::env::var("WINMUSE_CONFIG") {
        PathBuf::from(p)
    } else {
        data_dir().join("config.toml")
    }
}

pub fn audit_path() -> PathBuf {
    data_dir().join("audit.jsonl")
}

/// Load and parse the config file. A missing file is not an error:
/// it yields the observe default with an explanatory note.
pub fn load(path: &PathBuf) -> Result<Config, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Config::default());
        }
        Err(e) => return Err(format!("cannot read {}: {e}", path.display())),
    };
    let raw: RawConfig =
        toml::from_str(&text).map_err(|e| format!("invalid config {}: {e}", path.display()))?;
    let mut cfg = Config {
        source_note: format!("loaded from {}", path.display()),
        ..Default::default()
    };
    match raw.profile.as_deref() {
        None => {
            cfg.source_note = format!(
                "{} sets no profile; running in observe",
                path.display()
            );
        }
        Some("observe") => {
            cfg.profile = Profile::Observe;
            cfg.profile_chosen = true;
        }
        Some("guided") => {
            cfg.profile = Profile::Guided;
            cfg.profile_chosen = true;
        }
        Some("full") => {
            cfg.profile = Profile::Full;
            cfg.profile_chosen = true;
        }
        Some(other) => {
            return Err(format!(
                "unknown profile '{other}' in {} (expected observe|guided|full)",
                path.display()
            ));
        }
    }
    if let Some(g) = raw.guided {
        if let Some(list) = g.allow_processes {
            cfg.allow_processes =
                list.iter().map(|s| s.trim().to_lowercase()).collect();
        }
        if let Some(b) = g.block_win_keys {
            cfg.block_win_keys = b;
        }
    }
    if let Some(c) = raw.consent {
        if let Some(b) = c.require_local_arm {
            cfg.require_local_arm = b;
        }
    }
    if let Some(a) = raw.audit {
        if let Some(b) = a.enabled {
            cfg.audit_enabled = b;
        }
    }
    if let Some(t) = raw.auth_token {
        let t = t.trim().to_string();
        if !t.is_empty() {
            cfg.auth_token = Some(t);
        }
    }
    if let Some(sv) = raw.server {
        cfg.server_host = sv.host.filter(|h| !h.trim().is_empty());
        cfg.server_port = sv.port;
    }
    if let Some(ov) = raw.overlay {
        if let Some(b) = ov.enabled {
            cfg.overlay_enabled = b;
        }
        if let Some(l) = ov.lang {
            let l = l.trim().to_lowercase();
            if l == "ja" || l == "en" {
                cfg.overlay_lang = l;
            }
        }
        if let Some(t) = ov.title {
            cfg.overlay_title = t;
        }
        if let Some(t) = ov.sub {
            cfg.overlay_sub = t;
        }
    }
    Ok(cfg)
}

/// Serialize a Config back to TOML (used by the settings GUI).
/// Fields the GUI does not edit are written back unchanged.
pub fn save(path: &PathBuf, cfg: &Config) -> std::io::Result<()> {
    // Full TOML basic-string escaping: a value containing a
    // newline or other control character (a pasted token, a custom
    // toast) must still serialize to valid TOML.
    let esc = |s: &str| {
        let mut out = String::with_capacity(s.len());
        for c in s.chars() {
            match c {
                '\\' => out.push_str("\\\\"),
                '"' => out.push_str("\\\""),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                c if (c as u32) < 0x20 => {
                    out.push_str(&format!("\\u{:04X}", c as u32));
                }
                c => out.push(c),
            }
        }
        out
    };
    let mut out = String::new();
    out.push_str("# winmuse configuration\n");
    out.push_str("# Written by the settings GUI; the resident process hot-reloads this file.\n\n");
    out.push_str(&format!("profile = \"{}\"\n", cfg.profile.as_str()));
    if let Some(t) = &cfg.auth_token {
        out.push_str(&format!("auth_token = \"{}\"\n", esc(t)));
    }
    out.push_str("\n[guided]\nallow_processes = [");
    out.push_str(
        &cfg.allow_processes
            .iter()
            .map(|p| format!("\"{}\"", esc(p)))
            .collect::<Vec<_>>()
            .join(", "),
    );
    out.push_str("]\n");
    out.push_str(&format!("block_win_keys = {}\n", cfg.block_win_keys));
    out.push_str(&format!("\n[consent]\nrequire_local_arm = {}\n", cfg.require_local_arm));
    out.push_str(&format!("\n[audit]\nenabled = {}\n", cfg.audit_enabled));
    if cfg.server_host.is_some() || cfg.server_port.is_some() {
        out.push_str("\n[server]\n");
        if let Some(h) = &cfg.server_host {
            out.push_str(&format!("host = \"{}\"\n", esc(h)));
        }
        if let Some(p) = cfg.server_port {
            out.push_str(&format!("port = {p}\n"));
        }
    }
    out.push_str("\n[overlay]\n");
    out.push_str(&format!("enabled = {}\n", cfg.overlay_enabled));
    if !cfg.overlay_lang.is_empty() {
        out.push_str(&format!("lang = \"{}\"\n", cfg.overlay_lang));
    }
    if !cfg.overlay_title.is_empty() {
        out.push_str(&format!("title = \"{}\"\n", esc(&cfg.overlay_title)));
    }
    if !cfg.overlay_sub.is_empty() {
        out.push_str(&format!("sub = \"{}\"\n", esc(&cfg.overlay_sub)));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, out)
}

/// True when the Windows UI language is Japanese.
pub fn os_is_japanese() -> bool {
    // GetUserDefaultUILanguage: primary language id 0x11 = Japanese.
    let langid = unsafe { windows::Win32::Globalization::GetUserDefaultUILanguage() };
    langid & 0x3FF == 0x11
}

/// The overlay language in force: the config choice, or the OS
/// language when unset ("ja" on Japanese Windows, "en" elsewhere).
pub fn effective_lang(cfg: &Config) -> String {
    if cfg.overlay_lang == "ja" || cfg.overlay_lang == "en" {
        cfg.overlay_lang.clone()
    } else if os_is_japanese() {
        "ja".to_string()
    } else {
        "en".to_string()
    }
}

fn mtime_of(path: &PathBuf) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Poll the config file every 2 s and hot-reload on change. Invalid
/// edits are logged and rejected; the running config is kept.
pub fn spawn_watcher(shared: Arc<Shared>, path: PathBuf) {
    std::thread::spawn(move || {
        let mut last = mtime_of(&path);
        let mut attempts = 0;
        loop {
            std::thread::sleep(Duration::from_secs(2));
            let now = mtime_of(&path);
            if now != last {
                match load(&path) {
                    Ok(cfg) => {
                        tracing::info!(
                            "config reloaded: profile={} ({})",
                            cfg.profile.as_str(),
                            cfg.source_note
                        );
                        *shared.config.write().unwrap() = cfg;
                        last = now;
                        attempts = 0;
                    }
                    Err(e) => {
                        // A failed load must not consume the change:
                        // the file may only be momentarily locked by
                        // whatever wrote it (scp, an editor). Retry a
                        // few times, then give up on this version
                        // until it changes again.
                        attempts += 1;
                        if attempts <= 3 {
                            tracing::warn!("config reload failed, will retry: {e}");
                        }
                        if attempts >= 5 {
                            tracing::warn!("config reload rejected, keeping previous: {e}");
                            last = now;
                            attempts = 0;
                        }
                    }
                }
            }
        }
    });
}
