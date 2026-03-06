//! Optional debug logger — writes to ~/.cache/siggy/debug.log when --debug is passed.
//! Rotates log file when it exceeds MAX_LOG_SIZE bytes.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

static ENABLED: AtomicBool = AtomicBool::new(false);
static FILE: Mutex<Option<File>> = Mutex::new(None);
static PATH: Mutex<Option<std::path::PathBuf>> = Mutex::new(None);

const MAX_LOG_SIZE: u64 = 10 * 1024 * 1024; // 10 MB

fn log_path() -> std::path::PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| std::path::PathBuf::from(".cache"))
        .join("siggy")
        .join("debug.log")
}

pub fn enable() {
    ENABLED.store(true, Ordering::Relaxed);
    let path = log_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
        }
    }
    // Rotate if existing log exceeds size limit
    if path.exists() {
        if let Ok(meta) = std::fs::metadata(&path) {
            if meta.len() > MAX_LOG_SIZE {
                let backup = path.with_extension("log.old");
                let _ = std::fs::rename(&path, &backup);
            }
        }
    }
    if let Ok(f) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }
        if let Ok(mut guard) = FILE.lock() {
            *guard = Some(f);
        }
        if let Ok(mut guard) = PATH.lock() {
            *guard = Some(path.clone());
        }
    }
    eprintln!("Debug logging enabled: {}", path.display());
}

pub fn log(msg: &str) {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    if let Ok(mut guard) = FILE.lock() {
        if let Some(ref mut f) = *guard {
            let now = chrono::Local::now().format("%H:%M:%S%.3f");
            let _ = writeln!(f, "[{now}] {msg}");
        }
    }
}

pub fn logf(args: std::fmt::Arguments<'_>) {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    log(&format!("{args}"));
}
