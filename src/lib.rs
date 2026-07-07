//! Library entrypoint for fuzz testing and external consumers.
//!
//! The main binary has its own module tree; this surface re-exports only
//! what fuzz harnesses need ([`config`], [`input`], [`keybindings`],
//! [`signal`], [`theme`]).

// Backend features are mutually exclusive (#640 U7, plan R2/KTD-1). The lib
// target carries the same guard as the binary so every compilation root
// rejects an impossible feature set.
#[cfg(all(feature = "signal-cli-backend", feature = "native-backend"))]
compile_error!(
    "features `signal-cli-backend` and `native-backend` are mutually exclusive; enable exactly one"
);
#[cfg(not(any(feature = "signal-cli-backend", feature = "native-backend")))]
compile_error!(
    "one backend feature is required: `signal-cli-backend` (default) or `native-backend`"
);

pub mod config;
#[allow(dead_code)]
mod debug_log;
pub mod fs_migrate;
pub mod input;
pub mod keybindings;
pub mod signal;
pub mod theme;
