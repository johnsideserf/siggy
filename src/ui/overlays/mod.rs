//! Modal overlays rendered on top of the main UI.
//!
//! One submodule per `OverlayKind`, each exporting a `draw_*`
//! function. All overlays share `centered_popup` (defined in
//! `ui/mod.rs`) for the standard "clear area + bordered popup"
//! frame, plus per-overlay popup-width constants for sizing.

pub(super) mod about;
pub(super) mod delete_confirm;
