//! Optional debug logger — writes to ~/.cache/siggy/debug.log when --debug is passed.
//! Rotates log file when it exceeds MAX_LOG_SIZE bytes.
//!
//! `--debug` enables logging with PII redaction (phone numbers masked, message bodies omitted).
//! `--debug-full` enables logging with full unredacted output.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

static ENABLED: AtomicBool = AtomicBool::new(false);
static REDACT: AtomicBool = AtomicBool::new(true);
static FILE: Mutex<Option<File>> = Mutex::new(None);
static PATH: Mutex<Option<std::path::PathBuf>> = Mutex::new(None);

const MAX_LOG_SIZE: u64 = 10 * 1024 * 1024; // 10 MB

fn log_path() -> std::path::PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| std::path::PathBuf::from(".cache"))
        .join("siggy")
        .join("debug.log")
}

fn setup_file() {
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
    if path.exists()
        && let Ok(meta) = std::fs::metadata(&path)
        && meta.len() > MAX_LOG_SIZE
    {
        let backup = path.with_extension("log.old");
        let _ = std::fs::rename(&path, &backup);
    }
    if let Ok(f) = OpenOptions::new().create(true).append(true).open(&path) {
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

/// Enable debug logging with PII redaction (--debug).
pub fn enable() {
    ENABLED.store(true, Ordering::Relaxed);
    REDACT.store(true, Ordering::Relaxed);
    setup_file();
}

/// Enable debug logging without PII redaction (--debug-full).
pub fn enable_full() {
    ENABLED.store(true, Ordering::Relaxed);
    REDACT.store(false, Ordering::Relaxed);
    setup_file();
}

/// Whether PII should be redacted in log output.
pub fn redact() -> bool {
    REDACT.load(Ordering::Relaxed)
}

/// Mask a phone number for redacted logging: "+15551234567" → "+1***...567"
pub fn mask_phone(phone: &str) -> String {
    if !redact() {
        return phone.to_string();
    }
    if phone.starts_with('+') && phone.len() > 5 {
        let suffix = &phone[phone.len() - 3..];
        format!("+{}***...{}", &phone[1..2], suffix)
    } else if phone.len() > 8 {
        // Group IDs or other long identifiers
        format!("{}...{}", &phone[..4], &phone[phone.len() - 4..])
    } else {
        "[redacted]".to_string()
    }
}

/// Summarize a message body for redacted logging: "hello world" → "[msg: 11 chars]"
pub fn mask_body(body: &str) -> String {
    if !redact() {
        return body.to_string();
    }
    format!("[msg: {} chars]", body.len())
}

pub fn log(msg: &str) {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    if let Ok(mut guard) = FILE.lock()
        && let Some(ref mut f) = *guard
    {
        let now = chrono::Local::now().format("%H:%M:%S%.3f");
        let _ = writeln!(f, "[{now}] {msg}");
    }
}

pub fn logf(args: std::fmt::Arguments<'_>) {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    log(&format!("{args}"));
}
