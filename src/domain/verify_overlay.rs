//! Identity verification overlay state.
//!
//! `identities` is the filtered `IdentityInfo` list shown in the
//! overlay (one entry for a 1:1 chat, or one per group member);
//! `index` is the cursor; `confirming` gates the destructive
//! "verify identity" action behind a confirmation prompt.

use crate::signal::types::IdentityInfo;

/// State for the identity verification overlay.
#[derive(Default)]
pub struct VerifyOverlayState {
    /// Cursor position in verify overlay (for group member list)
    pub index: usize,
    /// Identity info entries filtered for the current overlay
    pub identities: Vec<IdentityInfo>,
    /// Confirmation pending for verify action
    pub confirming: bool,
}
