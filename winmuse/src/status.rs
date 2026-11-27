//! status.json: the resident process's heartbeat file at
//! `%LOCALAPPDATA%\winmuse\status.json`, read by the settings GUI
//! to show bridge state. Rewritten whenever anything it reports
//! changes, and at least every 30 s as a liveness signal.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;

use crate::state::Shared;

/// The operation state as one word, for status.json and the tray.
pub fn op_state_name(shared: &Shared) -> &'static str {
    let op = shared.op.lock().unwrap();
    if op.awaiting_arm {
        "waiting"
    } else if op
        .flash_until
        .map_or(false, |f| std::time::Instant::now() < f)
    {
        "cancelled"
    } else if op.active {
        "operating"
    } else {
        "idle"
    }
}

pub fn spawn(shared: Arc<Shared>) {
    std::thread::spawn(move || {
        let path = crate::config::data_dir().join("status.json");
        let mut last_key = String::new();
        let mut ticks: u64 = 0;
        loop {
            ticks += 1;
            let serving = shared.control.running.load(Ordering::Relaxed);
            let profile = shared.config.read().unwrap().profile.as_str();
            let state = op_state_name(&shared);
            let addr = shared.listen.read().unwrap().clone();
            let key = format!("{serving}|{profile}|{state}|{addr:?}");
            if key != last_key || ticks % 30 == 0 {
                last_key = key;
                let body = json!({
                    "pid": std::process::id(),
                    "version": env!("CARGO_PKG_VERSION"),
                    "addr": addr,
                    "serving": serving,
                    "profile": profile,
                    "op_state": state,
                    "updated": crate::audit::now_iso8601(),
                })
                .to_string();
                if let Some(parent) = path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let _ = std::fs::write(&path, body);
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    });
}
