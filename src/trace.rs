//! Opt-in timing trace. When `LAZYKASTEN_TRACE=1`, appends
//! `op <ms>` lines to `<vault>/.lazykasten/trace.log`. Release builds have
//! no console, hence the file. When unset the cost is a single branch:
//! no formatting, no I/O.

use std::io::Write as _;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

static ENABLED: OnceLock<bool> = OnceLock::new();
static PATH: OnceLock<PathBuf> = OnceLock::new();

pub fn init(vault_root: &std::path::Path) {
    let on = std::env::var("LAZYKASTEN_TRACE").map(|v| v == "1").unwrap_or(false);
    let _ = ENABLED.set(on);
    if on {
        let _ = PATH.set(vault_root.join(".lazykasten").join("trace.log"));
    }
}

pub fn enabled() -> bool {
    *ENABLED.get().unwrap_or(&false)
}

/// Append a plain note line (same off-cost as `log`).
pub fn info(msg: &str) {
    if !enabled() {
        return;
    }
    let Some(path) = PATH.get() else { return };
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{msg}");
    }
}

/// Append one timing line. Callers pass an already-measured duration; when
/// tracing is off this returns after one branch.
pub fn log(op: &str, dur: Duration) {
    if !enabled() {
        return;
    }
    let Some(path) = PATH.get() else { return };
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{op} {:.3}ms", dur.as_secs_f64() * 1000.0);
    }
}
