//! Free-function handlers extracted from `App`.
//!
//! Submodules implement what used to be `impl App` methods. Each handler
//! takes `&mut App` (and the parsed action it dispatches on) so that
//! per-arm logic lives outside `app.rs` while still operating on the
//! same global state.

pub(crate) mod input;
pub(crate) mod signal;
