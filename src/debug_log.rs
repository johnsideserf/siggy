//! Optional debug logger — writes to signal-tui-debug.log when --debug is passed.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

static ENABLED: AtomicBool = AtomicBool::new(false);
static FILE: Mutex<Option<File>> = Mutex::new(None);

pub fn enable() {
    ENABLED.store(true, Ordering::Relaxed);
    if let Ok(f) = OpenOptions::new()
        .create(true)
        .append(true)
        .open("signal-tui-debug.log")
    {
        if let Ok(mut guard) = FILE.lock() {
            *guard = Some(f);
        }
    }
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
