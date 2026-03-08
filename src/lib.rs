// Library entrypoint for fuzz testing and external consumers.
// The main binary (main.rs) has its own module tree; this re-exports
// only what fuzz harnesses need.

pub mod config;
#[allow(dead_code)]
mod debug_log;
pub mod input;
pub mod keybindings;
pub mod signal;
