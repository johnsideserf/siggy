use crate::signal::types::IdentityInfo;

/// State for the identity verification overlay.
#[derive(Default)]
pub struct VerifyOverlayState {
    /// Verify identity overlay visible
    pub show: bool,
    /// Cursor position in verify overlay (for group member list)
    pub index: usize,
    /// Identity info entries filtered for the current overlay
    pub identities: Vec<IdentityInfo>,
    /// Confirmation pending for verify action
    pub confirming: bool,
}
