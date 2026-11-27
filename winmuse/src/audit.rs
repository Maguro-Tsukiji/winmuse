//! Append-only audit log: one JSONL line per tool call at
//! `%LOCALAPPDATA%\winmuse\audit.jsonl` (never in temp — an audit
//! trail Disk Cleanup can erase is not an audit trail).
//!
//! `type_text` is logged as length + a short SHA-256 prefix, never
//! the raw text. Screenshots are logged as calls, never stored.

use std::io::Write;
use std::path::PathBuf;

use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

/// Current UTC time as ISO 8601 with milliseconds,
/// e.g. 2026-11-26T15:04:05.123Z. (Civil-from-days after
/// Howard Hinnant's algorithm; no date crate needed.)
pub fn now_iso8601() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs() as i64;
    let ms = now.subsec_millis();
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (hh, mm, ss) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let mut y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    if m <= 2 {
        y += 1;
    }
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}.{ms:03}Z")
}

/// The argument summary written to the log. Everything is logged as
/// given (coordinates, key combos) except type_text, which is
/// reduced to a character count and a SHA-256 prefix.
pub fn summarize_args(tool: &str, args: Option<&Map<String, Value>>) -> Value {
    if tool == "type_text" {
        let text = args
            .and_then(|a| a.get("text"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let digest = Sha256::digest(text.as_bytes());
        let prefix: String = digest.iter().take(4).map(|b| format!("{b:02x}")).collect();
        return json!({"chars": text.chars().count(), "sha256_8": prefix});
    }
    match args {
        Some(a) => Value::Object(a.clone()),
        None => json!({}),
    }
}

/// Append one line. Failures are swallowed: the audit log must
/// never break the tool call it records.
pub fn log(path: &PathBuf, tool: &str, profile: &str, decision: &str, args: &Value) {
    let line = json!({
        "ts": now_iso8601(),
        "tool": tool,
        "profile": profile,
        "decision": decision,
        "args": args,
    })
    .to_string();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(f, "{line}");
    }
}
