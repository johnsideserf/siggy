/// State for the message action menu overlay.
#[derive(Default)]
pub struct ActionMenuState {
    /// Action menu overlay visible
    pub show: bool,
    /// Cursor position in action menu
    pub index: usize,
}
