//! Configurable key bindings.
//!
//! [`KeyBindings`] resolves a (`KeyModifiers`, `KeyCode`, [`BindingMode`])
//! triple to a [`KeyAction`]. Built-in profiles ship with the binary;
//! [`KeyBindingOverrides`] is loaded from / saved to `keybindings.toml` so
//! users can rebind without recompiling.

use crossterm::event::{KeyCode, KeyModifiers};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Core types
// ---------------------------------------------------------------------------

/// All configurable key actions across the three binding modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyAction {
    // Global
    Quit,
    Lock,
    NextConversation,
    PrevConversation,
    ResizeSidebarLeft,
    ResizeSidebarRight,
    PageScrollUp,
    PageScrollDown,
    // Global: open overlays / toggles (unbound by default; bind in a profile).
    OpenContacts,
    OpenSettings,
    OpenHelp,
    ToggleSidebar,
    Attach,
    /// Fuzzy command palette (#614). Bound to Ctrl+P in the Default and
    /// Minimal profiles; unbound in Emacs, where Ctrl+P is line-up.
    CommandPalette,
    // Normal: scroll
    ScrollUp,
    ScrollDown,
    FocusNextMessage,
    FocusPrevMessage,
    HalfPageDown,
    HalfPageUp,
    ScrollToTop,
    ScrollToBottom,
    // Normal: edit/mode-switch
    InsertAtCursor,
    InsertAfterCursor,
    InsertLineStart,
    InsertLineEnd,
    OpenLineBelow,
    CursorLeft,
    CursorRight,
    LineStart,
    LineEnd,
    WordForward,
    WordBack,
    DeleteChar,
    DeleteToEnd,
    StartSearch,
    ClearInput,
    // Normal: actions
    CopyMessage,
    CopyAllMessages,
    React,
    Quote,
    EditMessage,
    ForwardMessage,
    DeleteMessage,
    NextSearchResult,
    PrevSearchResult,
    OpenActionMenu,
    PinMessage,
    JumpToQuote,
    JumpBack,
    SidebarSearch,
    // Insert
    ExitInsert,
    SendMessage,
    InsertNewline,
    DeleteWordBack,
}

/// Which mode a binding applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BindingMode {
    Global,
    Normal,
    Insert,
}

/// A key combination (modifier flags + key code).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeyCombo {
    pub modifiers: KeyModifiers,
    pub code: KeyCode,
}

/// The full set of keybindings for all modes.
#[derive(Debug, Clone)]
pub struct KeyBindings {
    pub profile_name: String,
    global: HashMap<KeyCombo, KeyAction>,
    normal: HashMap<KeyCombo, KeyAction>,
    insert: HashMap<KeyCombo, KeyAction>,
}

// ---------------------------------------------------------------------------
// KeyBindings methods
// ---------------------------------------------------------------------------

impl KeyBindings {
    /// Look up the action for a key press in the given mode.
    /// Global bindings are checked first, then mode-specific.
    pub fn resolve(
        &self,
        modifiers: KeyModifiers,
        code: KeyCode,
        mode: BindingMode,
    ) -> Option<KeyAction> {
        // For Char keys, strip SHIFT from modifiers since the case is already
        // encoded in the character itself (crossterm sends 'J' with SHIFT).
        let modifiers = if matches!(code, KeyCode::Char(_)) {
            modifiers - KeyModifiers::SHIFT
        } else {
            modifiers
        };
        let combo = KeyCombo { modifiers, code };
        // Global bindings always apply
        if let Some(action) = self.global.get(&combo) {
            return Some(*action);
        }
        let map = match mode {
            BindingMode::Global => &self.global,
            BindingMode::Normal => &self.normal,
            BindingMode::Insert => &self.insert,
        };
        map.get(&combo).copied()
    }

    /// Find all key combos that trigger the given action (across all modes).
    pub fn keys_for_action(&self, action: KeyAction) -> Vec<(BindingMode, KeyCombo)> {
        let mut result = Vec::new();
        for (combo, &a) in &self.global {
            if a == action {
                result.push((BindingMode::Global, combo.clone()));
            }
        }
        for (combo, &a) in &self.normal {
            if a == action {
                result.push((BindingMode::Normal, combo.clone()));
            }
        }
        for (combo, &a) in &self.insert {
            if a == action {
                result.push((BindingMode::Insert, combo.clone()));
            }
        }
        // Sort for deterministic ordering so `display_key` and help-overlay
        // snapshots aren't flaky when an action has multiple bindings.
        // Prefer simpler modifiers first (SHIFT < CONTROL < ALT by bit value),
        // breaking ties with the KeyCode's debug form.
        result.sort_by(|a, b| {
            let a_mod = a.1.modifiers.bits();
            let b_mod = b.1.modifiers.bits();
            a_mod
                .cmp(&b_mod)
                .then_with(|| format!("{:?}", a.1.code).cmp(&format!("{:?}", b.1.code)))
        });
        result
    }

    /// Human-readable display string for the first binding of an action.
    /// Falls back to "?" if unbound.
    pub fn display_key(&self, action: KeyAction) -> String {
        let bindings = self.keys_for_action(action);
        if let Some((_, combo)) = bindings.first() {
            format_key_combo(combo)
        } else {
            "?".to_string()
        }
    }

    /// Apply user overrides on top of the current bindings.
    pub fn apply_overrides(&mut self, overrides: &KeyBindingOverrides) {
        for (action, combos) in &overrides.global {
            // Remove old bindings for this action in global
            self.global.retain(|_, a| a != action);
            for combo in combos {
                self.global.insert(combo.clone(), *action);
            }
        }
        for (action, combos) in &overrides.normal {
            self.normal.retain(|_, a| a != action);
            for combo in combos {
                self.normal.insert(combo.clone(), *action);
            }
        }
        for (action, combos) in &overrides.insert {
            self.insert.retain(|_, a| a != action);
            for combo in combos {
                self.insert.insert(combo.clone(), *action);
            }
        }
    }

    /// Rebind a single action in a specific mode to a new key combo.
    /// Removes the old binding(s) for this action and inserts the new one.
    /// Returns any action that was previously bound to the new combo (conflict).
    pub fn rebind(
        &mut self,
        mode: BindingMode,
        action: KeyAction,
        new_combo: KeyCombo,
    ) -> Option<KeyAction> {
        let map = match mode {
            BindingMode::Global => &mut self.global,
            BindingMode::Normal => &mut self.normal,
            BindingMode::Insert => &mut self.insert,
        };
        // Remove existing bindings for this action
        map.retain(|_, a| *a != action);
        // Check if the new combo already has a binding
        map.insert(new_combo, action)
    }

    /// Reset a single action in a specific mode to its default binding(s).
    pub fn reset_action(&mut self, mode: BindingMode, action: KeyAction) {
        let defaults = default_profile();
        let (src, dst) = match mode {
            BindingMode::Global => (&defaults.global, &mut self.global),
            BindingMode::Normal => (&defaults.normal, &mut self.normal),
            BindingMode::Insert => (&defaults.insert, &mut self.insert),
        };
        // Remove current bindings
        dst.retain(|_, a| *a != action);
        // Restore defaults
        for (combo, &a) in src {
            if a == action {
                dst.insert(combo.clone(), a);
            }
        }
    }

    /// Compute the difference between current bindings and the profile's defaults.
    /// Returns overrides that, when applied to the profile, reproduce the current state.
    pub fn diff_from_profile(&self) -> KeyBindingOverrides {
        let defaults = find_profile(&self.profile_name);
        fn diff_mode(
            current: &HashMap<KeyCombo, KeyAction>,
            default: &HashMap<KeyCombo, KeyAction>,
        ) -> Vec<(KeyAction, Vec<KeyCombo>)> {
            // Collect all actions that appear in either map
            let mut all_actions: std::collections::HashSet<KeyAction> =
                std::collections::HashSet::new();
            for action in current.values() {
                all_actions.insert(*action);
            }
            for action in default.values() {
                all_actions.insert(*action);
            }

            let mut result = Vec::new();
            for action in &all_actions {
                let current_combos: Vec<&KeyCombo> = current
                    .iter()
                    .filter(|(_, a)| *a == action)
                    .map(|(c, _)| c)
                    .collect();
                let default_combos: Vec<&KeyCombo> = default
                    .iter()
                    .filter(|(_, a)| *a == action)
                    .map(|(c, _)| c)
                    .collect();
                // Check if the bindings differ
                let mut cur_sorted: Vec<_> =
                    current_combos.iter().map(|c| format_key_combo(c)).collect();
                let mut def_sorted: Vec<_> =
                    default_combos.iter().map(|c| format_key_combo(c)).collect();
                cur_sorted.sort();
                def_sorted.sort();
                if cur_sorted != def_sorted {
                    result.push((*action, current_combos.into_iter().cloned().collect()));
                }
            }
            result
        }
        KeyBindingOverrides {
            global: diff_mode(&self.global, &defaults.global),
            normal: diff_mode(&self.normal, &defaults.normal),
            insert: diff_mode(&self.insert, &defaults.insert),
        }
    }
}

/// User overrides loaded from / saved to `keybindings.toml`.
#[derive(Debug, Default)]
pub struct KeyBindingOverrides {
    pub global: Vec<(KeyAction, Vec<KeyCombo>)>,
    pub normal: Vec<(KeyAction, Vec<KeyCombo>)>,
    pub insert: Vec<(KeyAction, Vec<KeyCombo>)>,
}

impl KeyBindingOverrides {
    pub fn is_empty(&self) -> bool {
        self.global.is_empty() && self.normal.is_empty() && self.insert.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Key combo formatting and parsing
// ---------------------------------------------------------------------------

/// Format a key combo for display (e.g., "Ctrl+D", "j", "Shift+Tab").
pub fn format_key_combo(combo: &KeyCombo) -> String {
    let mut parts = Vec::new();
    if combo.modifiers.contains(KeyModifiers::CONTROL) {
        parts.push("Ctrl".to_string());
    }
    if combo.modifiers.contains(KeyModifiers::ALT) {
        parts.push("Alt".to_string());
    }
    // Only show Shift for non-character keys or special chars
    let show_shift =
        combo.modifiers.contains(KeyModifiers::SHIFT) && !matches!(combo.code, KeyCode::Char(_));
    if show_shift {
        parts.push("Shift".to_string());
    }
    let key = match combo.code {
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Enter => "Enter".to_string(),
        KeyCode::Esc => "Esc".to_string(),
        KeyCode::Backspace => "Backspace".to_string(),
        KeyCode::Tab => "Tab".to_string(),
        KeyCode::BackTab => "Tab".to_string(), // BackTab is Shift+Tab
        KeyCode::Delete => "Delete".to_string(),
        KeyCode::Left => "Left".to_string(),
        KeyCode::Right => "Right".to_string(),
        KeyCode::Up => "Up".to_string(),
        KeyCode::Down => "Down".to_string(),
        KeyCode::PageUp => "PgUp".to_string(),
        KeyCode::PageDown => "PgDn".to_string(),
        KeyCode::Home => "Home".to_string(),
        KeyCode::End => "End".to_string(),
        KeyCode::F(n) => format!("F{n}"),
        _ => "?".to_string(),
    };
    parts.push(key);
    parts.join("+")
}

/// Parse a key combo string like "ctrl+d", "j", "shift+enter", "alt+enter".
pub fn parse_key_combo(s: &str) -> Result<KeyCombo, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("empty key string".into());
    }

    let parts: Vec<&str> = s.split('+').collect();
    let mut modifiers = KeyModifiers::NONE;
    let key_part = parts.last().ok_or("no key part")?;

    for &part in &parts[..parts.len() - 1] {
        match part.to_lowercase().as_str() {
            "ctrl" | "control" => modifiers |= KeyModifiers::CONTROL,
            "alt" => modifiers |= KeyModifiers::ALT,
            "shift" => modifiers |= KeyModifiers::SHIFT,
            _ => return Err(format!("unknown modifier: {part}")),
        }
    }

    let code = match key_part.to_lowercase().as_str() {
        "enter" | "return" => KeyCode::Enter,
        "esc" | "escape" => KeyCode::Esc,
        "backspace" | "bs" => KeyCode::Backspace,
        "tab" => {
            if modifiers.contains(KeyModifiers::SHIFT) {
                KeyCode::BackTab
            } else {
                KeyCode::Tab
            }
        }
        "backtab" => {
            modifiers |= KeyModifiers::SHIFT;
            KeyCode::BackTab
        }
        "delete" | "del" => KeyCode::Delete,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "pageup" | "pgup" => KeyCode::PageUp,
        "pagedown" | "pgdn" | "pgdown" => KeyCode::PageDown,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "space" => KeyCode::Char(' '),
        s if s.starts_with('f') && s.len() > 1 => {
            let n: u8 = s[1..].parse().map_err(|_| format!("bad F-key: {s}"))?;
            KeyCode::F(n)
        }
        s if s.chars().count() == 1 => {
            let c = s.chars().next().unwrap();
            // Uppercase char implies Shift modifier (for letters)
            if c.is_ascii_uppercase() && !modifiers.contains(KeyModifiers::SHIFT) {
                // Store as uppercase char — crossterm sends uppercase Char when Shift is held
                KeyCode::Char(c)
            } else {
                KeyCode::Char(c)
            }
        }
        _ => return Err(format!("unknown key: {key_part}")),
    };

    Ok(KeyCombo { modifiers, code })
}

// ---------------------------------------------------------------------------
// Ordered action list (for UI display)
// ---------------------------------------------------------------------------

/// Actions in display order for the keybindings overlay.
pub const GLOBAL_ACTIONS: &[KeyAction] = &[
    KeyAction::Quit,
    KeyAction::NextConversation,
    KeyAction::PrevConversation,
    KeyAction::ResizeSidebarLeft,
    KeyAction::ResizeSidebarRight,
    KeyAction::PageScrollUp,
    KeyAction::PageScrollDown,
    KeyAction::OpenContacts,
    KeyAction::OpenSettings,
    KeyAction::OpenHelp,
    KeyAction::ToggleSidebar,
    KeyAction::Attach,
    KeyAction::CommandPalette,
];

pub const NORMAL_ACTIONS: &[KeyAction] = &[
    KeyAction::ScrollUp,
    KeyAction::ScrollDown,
    KeyAction::FocusNextMessage,
    KeyAction::FocusPrevMessage,
    KeyAction::HalfPageDown,
    KeyAction::HalfPageUp,
    KeyAction::ScrollToTop,
    KeyAction::ScrollToBottom,
    KeyAction::InsertAtCursor,
    KeyAction::InsertAfterCursor,
    KeyAction::InsertLineStart,
    KeyAction::InsertLineEnd,
    KeyAction::OpenLineBelow,
    KeyAction::CursorLeft,
    KeyAction::CursorRight,
    KeyAction::LineStart,
    KeyAction::LineEnd,
    KeyAction::WordForward,
    KeyAction::WordBack,
    KeyAction::DeleteChar,
    KeyAction::DeleteToEnd,
    KeyAction::StartSearch,
    KeyAction::ClearInput,
    KeyAction::CopyMessage,
    KeyAction::CopyAllMessages,
    KeyAction::React,
    KeyAction::Quote,
    KeyAction::EditMessage,
    KeyAction::ForwardMessage,
    KeyAction::DeleteMessage,
    KeyAction::NextSearchResult,
    KeyAction::PrevSearchResult,
    KeyAction::OpenActionMenu,
    KeyAction::PinMessage,
    KeyAction::JumpToQuote,
    KeyAction::JumpBack,
    KeyAction::SidebarSearch,
];

pub const INSERT_ACTIONS: &[KeyAction] = &[
    KeyAction::ExitInsert,
    KeyAction::SendMessage,
    KeyAction::InsertNewline,
    KeyAction::DeleteWordBack,
];

/// Human-readable label for a KeyAction.
pub fn action_label(action: KeyAction) -> &'static str {
    match action {
        KeyAction::Quit => "Quit",
        KeyAction::Lock => "Lock session",
        KeyAction::NextConversation => "Next conversation",
        KeyAction::PrevConversation => "Previous conversation",
        KeyAction::ResizeSidebarLeft => "Shrink sidebar",
        KeyAction::ResizeSidebarRight => "Grow sidebar",
        KeyAction::PageScrollUp => "Page scroll up",
        KeyAction::PageScrollDown => "Page scroll down",
        KeyAction::OpenContacts => "Open contacts",
        KeyAction::OpenSettings => "Open settings",
        KeyAction::OpenHelp => "Open help",
        KeyAction::ToggleSidebar => "Toggle sidebar",
        KeyAction::Attach => "Attach file",
        KeyAction::CommandPalette => "Command palette",
        KeyAction::ScrollUp => "Scroll up",
        KeyAction::ScrollDown => "Scroll down",
        KeyAction::FocusNextMessage => "Focus next message",
        KeyAction::FocusPrevMessage => "Focus previous message",
        KeyAction::HalfPageDown => "Half-page down",
        KeyAction::HalfPageUp => "Half-page up",
        KeyAction::ScrollToTop => "Scroll to top",
        KeyAction::ScrollToBottom => "Scroll to bottom",
        KeyAction::InsertAtCursor => "Insert at cursor",
        KeyAction::InsertAfterCursor => "Insert after cursor",
        KeyAction::InsertLineStart => "Insert at line start",
        KeyAction::InsertLineEnd => "Insert at line end",
        KeyAction::OpenLineBelow => "Open line below",
        KeyAction::CursorLeft => "Cursor left",
        KeyAction::CursorRight => "Cursor right",
        KeyAction::LineStart => "Line start",
        KeyAction::LineEnd => "Line end",
        KeyAction::WordForward => "Word forward",
        KeyAction::WordBack => "Word back",
        KeyAction::DeleteChar => "Delete character",
        KeyAction::DeleteToEnd => "Delete to end",
        KeyAction::StartSearch => "Start command input",
        KeyAction::ClearInput => "Clear input",
        KeyAction::CopyMessage => "Copy message",
        KeyAction::CopyAllMessages => "Copy all messages",
        KeyAction::React => "React to message",
        KeyAction::Quote => "Reply/quote message",
        KeyAction::EditMessage => "Edit own message",
        KeyAction::ForwardMessage => "Forward message",
        KeyAction::DeleteMessage => "Delete message",
        KeyAction::NextSearchResult => "Next search match",
        KeyAction::PrevSearchResult => "Previous search match",
        KeyAction::OpenActionMenu => "Action menu",
        KeyAction::PinMessage => "Pin/unpin message",
        KeyAction::JumpToQuote => "Jump to quoted message",
        KeyAction::JumpBack => "Jump back",
        KeyAction::SidebarSearch => "Filter sidebar",
        KeyAction::ExitInsert => "Normal mode",
        KeyAction::SendMessage => "Send message",
        KeyAction::InsertNewline => "Insert newline",
        KeyAction::DeleteWordBack => "Delete word back",
    }
}

// ---------------------------------------------------------------------------
// Built-in profiles
// ---------------------------------------------------------------------------

/// One row of a profile's binding table: (scope, modifier flags, key code,
/// action). Bundled into static slices below so each profile is a single
/// scannable list rather than 200+ lines of `bind(&mut map, ...)` calls.
type BindingRow = (BindingMode, KeyModifiers, KeyCode, KeyAction);

/// Build a `KeyBindings` instance from a static binding table. Sorts each
/// row into the global/normal/insert maps based on its declared scope.
fn build_profile(profile_name: &str, table: &[BindingRow]) -> KeyBindings {
    let mut global = HashMap::new();
    let mut normal = HashMap::new();
    let mut insert = HashMap::new();
    for &(scope, modifiers, code, action) in table {
        let target = match scope {
            BindingMode::Global => &mut global,
            BindingMode::Normal => &mut normal,
            BindingMode::Insert => &mut insert,
        };
        target.insert(KeyCombo { modifiers, code }, action);
    }
    KeyBindings {
        profile_name: profile_name.into(),
        global,
        normal,
        insert,
    }
}

/// The Default profile -- vim-style modal with j/k navigation and Normal-mode
/// actions on letter keys.
pub fn default_profile() -> KeyBindings {
    build_profile("Default", DEFAULT_BINDINGS)
}

const DEFAULT_BINDINGS: &[BindingRow] = {
    use BindingMode::{Global, Insert, Normal};
    use KeyAction::*;
    use KeyCode::{BackTab, Char, Enter, Esc, PageDown, PageUp, Tab};
    use KeyModifiers as M;

    &[
        // Global
        (Global, M::CONTROL, Char('c'), Quit),
        (Global, M::CONTROL, Char('l'), Lock),
        (Global, M::NONE, Tab, NextConversation),
        (Global, M::SHIFT, BackTab, PrevConversation),
        (Global, M::CONTROL, KeyCode::Left, ResizeSidebarLeft),
        (Global, M::CONTROL, KeyCode::Right, ResizeSidebarRight),
        (Global, M::NONE, PageUp, PageScrollUp),
        (Global, M::NONE, PageDown, PageScrollDown),
        (Global, M::CONTROL, Char('p'), CommandPalette),
        // Normal: scroll
        (Normal, M::NONE, Char('j'), FocusNextMessage),
        (Normal, M::NONE, Char('k'), FocusPrevMessage),
        (Normal, M::CONTROL, Char('d'), HalfPageDown),
        (Normal, M::CONTROL, Char('u'), HalfPageUp),
        (Normal, M::CONTROL, Char('e'), ScrollDown),
        (Normal, M::CONTROL, Char('y'), ScrollUp),
        (Normal, M::NONE, Char('G'), ScrollToBottom),
        // Normal: edit/mode-switch
        (Normal, M::NONE, Char('i'), InsertAtCursor),
        (Normal, M::NONE, Char('a'), InsertAfterCursor),
        (Normal, M::NONE, Char('I'), InsertLineStart),
        (Normal, M::NONE, Char('A'), InsertLineEnd),
        (Normal, M::NONE, Char('o'), OpenLineBelow),
        (Normal, M::NONE, Char('h'), CursorLeft),
        (Normal, M::NONE, Char('l'), CursorRight),
        (Normal, M::NONE, Char('0'), LineStart),
        (Normal, M::NONE, Char('$'), LineEnd),
        (Normal, M::NONE, Char('w'), WordForward),
        (Normal, M::NONE, Char('b'), WordBack),
        (Normal, M::NONE, Char('x'), DeleteChar),
        (Normal, M::NONE, Char('D'), DeleteToEnd),
        (Normal, M::NONE, Char('/'), StartSearch),
        (Normal, M::NONE, Esc, ClearInput),
        // Normal: actions
        (Normal, M::NONE, Char('y'), CopyMessage),
        (Normal, M::NONE, Char('Y'), CopyAllMessages),
        (Normal, M::NONE, Char('r'), React),
        (Normal, M::NONE, Char('q'), Quote),
        (Normal, M::NONE, Char('e'), EditMessage),
        (Normal, M::NONE, Char('f'), ForwardMessage),
        (Normal, M::NONE, Char('n'), NextSearchResult),
        (Normal, M::NONE, Char('N'), PrevSearchResult),
        (Normal, M::NONE, Enter, OpenActionMenu),
        (Normal, M::NONE, Char('p'), PinMessage),
        (Normal, M::NONE, Char('Q'), JumpToQuote),
        (Normal, M::CONTROL, Char('o'), JumpBack),
        (Normal, M::NONE, Char('s'), SidebarSearch),
        // Insert
        (Insert, M::NONE, Esc, ExitInsert),
        (Insert, M::NONE, Enter, SendMessage),
        (Insert, M::ALT, Enter, InsertNewline),
        (Insert, M::SHIFT, Enter, InsertNewline),
        (Insert, M::CONTROL, Char('w'), DeleteWordBack),
    ]
};

/// Emacs-style profile -- no Normal mode, Ctrl-based shortcuts.
pub fn emacs_profile() -> KeyBindings {
    build_profile("Emacs", EMACS_BINDINGS)
}

const EMACS_BINDINGS: &[BindingRow] = {
    use BindingMode::{Global, Insert, Normal};
    use KeyAction::*;
    use KeyCode::{BackTab, Char, Enter, Esc, PageDown, PageUp, Tab};
    use KeyModifiers as M;

    &[
        // Global
        (Global, M::CONTROL, Char('c'), Quit),
        (Global, M::CONTROL, Char('l'), Lock),
        (Global, M::NONE, Tab, NextConversation),
        (Global, M::SHIFT, BackTab, PrevConversation),
        (Global, M::CONTROL, KeyCode::Left, ResizeSidebarLeft),
        (Global, M::CONTROL, KeyCode::Right, ResizeSidebarRight),
        (Global, M::NONE, PageUp, PageScrollUp),
        (Global, M::NONE, PageDown, PageScrollDown),
        (Global, M::ALT, Char('s'), SidebarSearch),
        // Normal (essentially a stripped-down version)
        (Normal, M::NONE, Char('i'), InsertAtCursor),
        (Normal, M::NONE, Esc, ClearInput),
        // Insert (primary mode for Emacs)
        (Insert, M::NONE, Esc, ExitInsert),
        (Insert, M::NONE, Enter, SendMessage),
        (Insert, M::ALT, Enter, InsertNewline),
        (Insert, M::SHIFT, Enter, InsertNewline),
        (Insert, M::CONTROL, Char('w'), DeleteWordBack),
        // Emacs scroll
        (Insert, M::CONTROL, Char('p'), ScrollUp),
        (Insert, M::CONTROL, Char('n'), ScrollDown),
        // Emacs cursor
        (Insert, M::CONTROL, Char('a'), LineStart),
        (Insert, M::CONTROL, Char('e'), LineEnd),
        (Insert, M::CONTROL, Char('f'), CursorRight),
        (Insert, M::CONTROL, Char('b'), CursorLeft),
        (Insert, M::CONTROL, Char('d'), DeleteChar),
        (Insert, M::CONTROL, Char('k'), DeleteToEnd),
        // Emacs actions via Alt
        (Insert, M::ALT, Char('r'), React),
        (Insert, M::ALT, Char('q'), Quote),
        (Insert, M::ALT, Char('e'), EditMessage),
        (Insert, M::ALT, Char('f'), ForwardMessage),
        (Insert, M::ALT, Char('d'), DeleteMessage),
        (Insert, M::ALT, Char('y'), CopyMessage),
        (Insert, M::ALT, Char('n'), NextSearchResult),
        (Insert, M::ALT, Char('p'), PrevSearchResult),
        (Insert, M::ALT, Char('m'), OpenActionMenu),
        (Insert, M::ALT, Char('Q'), JumpToQuote),
        (Insert, M::CONTROL, Char('o'), JumpBack),
    ]
};

/// Minimal profile -- arrow-key centric, no modal concept needed.
pub fn minimal_profile() -> KeyBindings {
    build_profile("Minimal", MINIMAL_BINDINGS)
}

const MINIMAL_BINDINGS: &[BindingRow] = {
    use BindingMode::{Global, Insert, Normal};
    use KeyAction::*;
    use KeyCode::{BackTab, Char, Enter, Esc, F, PageDown, PageUp, Tab};
    use KeyModifiers as M;

    &[
        // Global
        (Global, M::CONTROL, Char('q'), Quit),
        (Global, M::CONTROL, Char('c'), Quit),
        (Global, M::CONTROL, Char('l'), Lock),
        (Global, M::CONTROL, Char('p'), CommandPalette),
        (Global, M::NONE, Tab, NextConversation),
        (Global, M::SHIFT, BackTab, PrevConversation),
        (Global, M::CONTROL, KeyCode::Left, ResizeSidebarLeft),
        (Global, M::CONTROL, KeyCode::Right, ResizeSidebarRight),
        (Global, M::NONE, PageUp, PageScrollUp),
        (Global, M::NONE, PageDown, PageScrollDown),
        (Global, M::CONTROL, Char('s'), SidebarSearch),
        // Normal (arrow-key navigation)
        (Normal, M::NONE, KeyCode::Up, ScrollUp),
        (Normal, M::NONE, KeyCode::Down, ScrollDown),
        (Normal, M::NONE, Char('i'), InsertAtCursor),
        (Normal, M::NONE, Esc, ClearInput),
        // Insert
        (Insert, M::NONE, Esc, ExitInsert),
        (Insert, M::NONE, Enter, SendMessage),
        (Insert, M::ALT, Enter, InsertNewline),
        (Insert, M::SHIFT, Enter, InsertNewline),
        (Insert, M::CONTROL, Char('w'), DeleteWordBack),
        // F-key actions
        (Insert, M::NONE, F(2), React),
        (Insert, M::NONE, F(3), Quote),
        (Insert, M::NONE, F(4), EditMessage),
        (Insert, M::NONE, F(5), CopyMessage),
        (Insert, M::NONE, F(6), DeleteMessage),
        (Insert, M::NONE, F(7), ForwardMessage),
        (Insert, M::NONE, F(8), OpenActionMenu),
        (Insert, M::NONE, F(9), JumpToQuote),
        (Insert, M::NONE, F(10), JumpBack),
    ]
};

// ---------------------------------------------------------------------------
// Profile discovery (mirrors theme.rs pattern)
// ---------------------------------------------------------------------------

fn builtin_profiles() -> Vec<KeyBindings> {
    vec![default_profile(), emacs_profile(), minimal_profile()]
}

/// Load custom keybinding profiles from `~/.config/siggy/keybindings/*.toml`.
pub fn load_custom_profiles() -> Vec<KeyBindings> {
    let dir = match dirs::config_dir() {
        Some(d) => d.join("siggy").join("keybindings"),
        None => return Vec::new(),
    };
    if !dir.is_dir() {
        return Vec::new();
    }
    let mut profiles = Vec::new();
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(e) => {
            crate::debug_log::logf(format_args!("custom keybindings dir read error: {e}"));
            return Vec::new();
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }
        match std::fs::read_to_string(&path) {
            Ok(contents) => match parse_profile_toml(&contents) {
                Ok(profile) => profiles.push(profile),
                Err(e) => {
                    crate::debug_log::logf(format_args!(
                        "custom keybinding profile parse error {}: {e}",
                        path.display()
                    ));
                }
            },
            Err(e) => {
                crate::debug_log::logf(format_args!(
                    "custom keybinding profile read error {}: {e}",
                    path.display()
                ));
            }
        }
    }
    profiles
}

/// All available keybinding profiles: built-ins followed by custom profiles.
pub fn all_profiles() -> Vec<KeyBindings> {
    let mut profiles = builtin_profiles();
    profiles.extend(load_custom_profiles());
    profiles
}

/// All available profile names.
pub fn all_profile_names() -> Vec<String> {
    all_profiles().into_iter().map(|p| p.profile_name).collect()
}

/// Find a profile by name. Falls back to Default if not found.
pub fn find_profile(name: &str) -> KeyBindings {
    all_profiles()
        .into_iter()
        .find(|p| p.profile_name == name)
        .unwrap_or_else(default_profile)
}

// ---------------------------------------------------------------------------
// TOML parsing
// ---------------------------------------------------------------------------

/// TOML structure for a custom profile file.
#[derive(Deserialize)]
struct ProfileToml {
    name: String,
    #[serde(default)]
    global: HashMap<String, TomlKeyValue>,
    #[serde(default)]
    normal: HashMap<String, TomlKeyValue>,
    #[serde(default)]
    insert: HashMap<String, TomlKeyValue>,
}

/// A key binding value: either a single string or an array of strings.
#[derive(Deserialize)]
#[serde(untagged)]
enum TomlKeyValue {
    Single(String),
    Multiple(Vec<String>),
}

/// Parse a custom profile from TOML.
fn parse_profile_toml(contents: &str) -> Result<KeyBindings, String> {
    let toml: ProfileToml =
        toml::from_str(contents).map_err(|e| format!("TOML parse error: {e}"))?;

    let mut global = HashMap::new();
    let mut normal = HashMap::new();
    let mut insert = HashMap::new();

    parse_toml_section(&toml.global, &mut global)?;
    parse_toml_section(&toml.normal, &mut normal)?;
    parse_toml_section(&toml.insert, &mut insert)?;

    Ok(KeyBindings {
        profile_name: toml.name,
        global,
        normal,
        insert,
    })
}

fn parse_toml_section(
    section: &HashMap<String, TomlKeyValue>,
    map: &mut HashMap<KeyCombo, KeyAction>,
) -> Result<(), String> {
    for (action_str, key_val) in section {
        let action: KeyAction = serde_json::from_str(&format!("\"{action_str}\""))
            .map_err(|_| format!("unknown action: {action_str}"))?;
        let keys = match key_val {
            TomlKeyValue::Single(s) => vec![s.as_str()],
            TomlKeyValue::Multiple(v) => v.iter().map(|s| s.as_str()).collect(),
        };
        for key_str in keys {
            let combo = parse_key_combo(key_str)?;
            map.insert(combo, action);
        }
    }
    Ok(())
}

/// Load overrides from `~/.config/siggy/keybindings.toml`.
pub fn load_overrides() -> KeyBindingOverrides {
    let path = match dirs::config_dir() {
        Some(d) => d.join("siggy").join("keybindings.toml"),
        None => return KeyBindingOverrides::default(),
    };
    if !path.exists() {
        return KeyBindingOverrides::default();
    }
    match std::fs::read_to_string(&path) {
        Ok(contents) => parse_overrides_toml(&contents).unwrap_or_else(|e| {
            crate::debug_log::logf(format_args!("keybindings.toml parse error: {e}"));
            KeyBindingOverrides::default()
        }),
        Err(e) => {
            crate::debug_log::logf(format_args!("keybindings.toml read error: {e}"));
            KeyBindingOverrides::default()
        }
    }
}

/// Save overrides to `~/.config/siggy/keybindings.toml`.
/// If there are no overrides, removes the file.
pub fn save_overrides(overrides: &KeyBindingOverrides) {
    let path = match dirs::config_dir() {
        Some(d) => d.join("siggy").join("keybindings.toml"),
        None => return,
    };
    if overrides.is_empty() {
        // No overrides — remove the file if it exists
        let _ = std::fs::remove_file(&path);
        return;
    }
    fn section_to_toml(entries: &[(KeyAction, Vec<KeyCombo>)]) -> String {
        let mut lines = Vec::new();
        for (action, combos) in entries {
            let action_str = serde_json::to_string(action)
                .unwrap_or_default()
                .trim_matches('"')
                .to_string();
            if combos.len() == 1 {
                lines.push(format!(
                    "{} = \"{}\"",
                    action_str,
                    format_key_combo(&combos[0]).to_lowercase()
                ));
            } else {
                let keys: Vec<String> = combos
                    .iter()
                    .map(|c| format!("\"{}\"", format_key_combo(c).to_lowercase()))
                    .collect();
                lines.push(format!("{} = [{}]", action_str, keys.join(", ")));
            }
        }
        lines.join("\n")
    }
    let mut content = String::new();
    if !overrides.global.is_empty() {
        content.push_str("[global]\n");
        content.push_str(&section_to_toml(&overrides.global));
        content.push('\n');
    }
    if !overrides.normal.is_empty() {
        if !content.is_empty() {
            content.push('\n');
        }
        content.push_str("[normal]\n");
        content.push_str(&section_to_toml(&overrides.normal));
        content.push('\n');
    }
    if !overrides.insert.is_empty() {
        if !content.is_empty() {
            content.push('\n');
        }
        content.push_str("[insert]\n");
        content.push_str(&section_to_toml(&overrides.insert));
        content.push('\n');
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = std::fs::write(&path, content) {
        crate::debug_log::logf(format_args!("keybindings.toml write error: {e}"));
    }
}

/// TOML structure for overrides file.
#[derive(Deserialize)]
struct OverridesToml {
    #[serde(default)]
    global: HashMap<String, TomlKeyValue>,
    #[serde(default)]
    normal: HashMap<String, TomlKeyValue>,
    #[serde(default)]
    insert: HashMap<String, TomlKeyValue>,
}

fn parse_overrides_toml(contents: &str) -> Result<KeyBindingOverrides, String> {
    let toml: OverridesToml =
        toml::from_str(contents).map_err(|e| format!("TOML parse error: {e}"))?;

    let parse_section = |section: &HashMap<String, TomlKeyValue>| -> Result<Vec<(KeyAction, Vec<KeyCombo>)>, String> {
        let mut result = Vec::new();
        for (action_str, key_val) in section {
            let action: KeyAction = serde_json::from_str(&format!("\"{action_str}\""))
                .map_err(|_| format!("unknown action: {action_str}"))?;
            let keys = match key_val {
                TomlKeyValue::Single(s) => vec![parse_key_combo(s)?],
                TomlKeyValue::Multiple(v) => v.iter().map(|s| parse_key_combo(s)).collect::<Result<Vec<_>, _>>()?,
            };
            result.push((action, keys));
        }
        Ok(result)
    };

    Ok(KeyBindingOverrides {
        global: parse_section(&toml.global)?,
        normal: parse_section(&toml.normal)?,
        insert: parse_section(&toml.insert)?,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_profile_resolves_ctrl_c_as_quit() {
        let kb = default_profile();
        assert_eq!(
            kb.resolve(
                KeyModifiers::CONTROL,
                KeyCode::Char('c'),
                BindingMode::Normal
            ),
            Some(KeyAction::Quit)
        );
    }

    #[test]
    fn default_profile_resolves_j_in_normal() {
        let kb = default_profile();
        assert_eq!(
            kb.resolve(KeyModifiers::NONE, KeyCode::Char('j'), BindingMode::Normal),
            Some(KeyAction::FocusNextMessage)
        );
    }

    #[test]
    fn default_profile_j_not_in_insert() {
        let kb = default_profile();
        assert_eq!(
            kb.resolve(KeyModifiers::NONE, KeyCode::Char('j'), BindingMode::Insert),
            None
        );
    }

    #[test]
    fn default_profile_esc_in_insert() {
        let kb = default_profile();
        assert_eq!(
            kb.resolve(KeyModifiers::NONE, KeyCode::Esc, BindingMode::Insert),
            Some(KeyAction::ExitInsert)
        );
    }

    #[test]
    fn default_profile_enter_in_insert() {
        let kb = default_profile();
        assert_eq!(
            kb.resolve(KeyModifiers::NONE, KeyCode::Enter, BindingMode::Insert),
            Some(KeyAction::SendMessage)
        );
    }

    #[test]
    fn default_profile_alt_enter_in_insert() {
        let kb = default_profile();
        assert_eq!(
            kb.resolve(KeyModifiers::ALT, KeyCode::Enter, BindingMode::Insert),
            Some(KeyAction::InsertNewline)
        );
    }

    #[test]
    fn parse_simple_key() {
        let combo = parse_key_combo("j").unwrap();
        assert_eq!(combo.modifiers, KeyModifiers::NONE);
        assert_eq!(combo.code, KeyCode::Char('j'));
    }

    #[test]
    fn parse_ctrl_key() {
        let combo = parse_key_combo("ctrl+d").unwrap();
        assert_eq!(combo.modifiers, KeyModifiers::CONTROL);
        assert_eq!(combo.code, KeyCode::Char('d'));
    }

    #[test]
    fn parse_alt_enter() {
        let combo = parse_key_combo("alt+enter").unwrap();
        assert_eq!(combo.modifiers, KeyModifiers::ALT);
        assert_eq!(combo.code, KeyCode::Enter);
    }

    #[test]
    fn parse_shift_tab() {
        let combo = parse_key_combo("shift+tab").unwrap();
        assert_eq!(combo.modifiers, KeyModifiers::SHIFT);
        assert_eq!(combo.code, KeyCode::BackTab);
    }

    #[test]
    fn parse_pageup() {
        let combo = parse_key_combo("pageup").unwrap();
        assert_eq!(combo.modifiers, KeyModifiers::NONE);
        assert_eq!(combo.code, KeyCode::PageUp);
    }

    #[test]
    fn format_roundtrip() {
        let cases = vec![
            ("j", KeyModifiers::NONE, KeyCode::Char('j')),
            ("Ctrl+d", KeyModifiers::CONTROL, KeyCode::Char('d')),
            ("Esc", KeyModifiers::NONE, KeyCode::Esc),
            ("Enter", KeyModifiers::NONE, KeyCode::Enter),
            ("Shift+Tab", KeyModifiers::SHIFT, KeyCode::BackTab),
        ];
        for (expected, mods, code) in cases {
            let combo = KeyCombo {
                modifiers: mods,
                code,
            };
            assert_eq!(format_key_combo(&combo), expected);
        }
    }

    #[test]
    fn keys_for_action_finds_binding() {
        let kb = default_profile();
        let keys = kb.keys_for_action(KeyAction::Quit);
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].0, BindingMode::Global);
        assert_eq!(keys[0].1.code, KeyCode::Char('c'));
        assert!(keys[0].1.modifiers.contains(KeyModifiers::CONTROL));
    }

    #[test]
    fn display_key_for_action() {
        let kb = default_profile();
        assert_eq!(kb.display_key(KeyAction::Quit), "Ctrl+c");
        assert_eq!(kb.display_key(KeyAction::ScrollDown), "Ctrl+e");
    }

    #[test]
    fn rebind_works() {
        let mut kb = default_profile();
        let new_combo = parse_key_combo("ctrl+j").unwrap();
        let displaced = kb.rebind(
            BindingMode::Normal,
            KeyAction::ScrollDown,
            new_combo.clone(),
        );
        assert!(displaced.is_none());
        assert_eq!(
            kb.resolve(
                KeyModifiers::CONTROL,
                KeyCode::Char('j'),
                BindingMode::Normal
            ),
            Some(KeyAction::ScrollDown)
        );
        // Old ScrollDown binding (ctrl+e) should be gone, but j is now FocusNextMessage
        assert_eq!(
            kb.resolve(KeyModifiers::NONE, KeyCode::Char('j'), BindingMode::Normal),
            Some(KeyAction::FocusNextMessage)
        );
    }

    #[test]
    fn rebind_detects_conflict() {
        let mut kb = default_profile();
        // Rebind ScrollDown to 'k' which is already FocusPrevMessage
        let new_combo = parse_key_combo("k").unwrap();
        let displaced = kb.rebind(BindingMode::Normal, KeyAction::ScrollDown, new_combo);
        assert_eq!(displaced, Some(KeyAction::FocusPrevMessage));
    }

    #[test]
    fn reset_action_restores_default() {
        let mut kb = default_profile();
        // Change ctrl+e binding (ScrollDown)
        let new_combo = parse_key_combo("ctrl+j").unwrap();
        kb.rebind(BindingMode::Normal, KeyAction::ScrollDown, new_combo);
        // Now ctrl+e shouldn't resolve to ScrollDown
        assert_eq!(
            kb.resolve(
                KeyModifiers::CONTROL,
                KeyCode::Char('e'),
                BindingMode::Normal
            ),
            None
        );
        // Reset
        kb.reset_action(BindingMode::Normal, KeyAction::ScrollDown);
        assert_eq!(
            kb.resolve(
                KeyModifiers::CONTROL,
                KeyCode::Char('e'),
                BindingMode::Normal
            ),
            Some(KeyAction::ScrollDown)
        );
    }

    #[test]
    fn overrides_apply() {
        let mut kb = default_profile();
        let overrides = KeyBindingOverrides {
            global: vec![(KeyAction::Quit, vec![parse_key_combo("ctrl+q").unwrap()])],
            normal: vec![],
            insert: vec![],
        };
        kb.apply_overrides(&overrides);
        // Old Ctrl+C should be gone
        assert_eq!(
            kb.resolve(
                KeyModifiers::CONTROL,
                KeyCode::Char('c'),
                BindingMode::Normal
            ),
            None
        );
        // New Ctrl+Q should work
        assert_eq!(
            kb.resolve(
                KeyModifiers::CONTROL,
                KeyCode::Char('q'),
                BindingMode::Normal
            ),
            Some(KeyAction::Quit)
        );
    }

    #[test]
    fn find_profile_falls_back_to_default() {
        let kb = find_profile("nonexistent");
        assert_eq!(kb.profile_name, "Default");
    }

    #[test]
    fn all_builtin_profiles_have_unique_names() {
        let profiles = builtin_profiles();
        let mut names: Vec<&str> = profiles.iter().map(|p| p.profile_name.as_str()).collect();
        let len = names.len();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), len, "duplicate profile names found");
    }

    #[test]
    fn emacs_profile_has_ctrl_n_scroll() {
        let kb = emacs_profile();
        assert_eq!(
            kb.resolve(
                KeyModifiers::CONTROL,
                KeyCode::Char('n'),
                BindingMode::Insert
            ),
            Some(KeyAction::ScrollDown)
        );
    }

    #[test]
    fn parse_overrides_toml_basic() {
        let toml = r#"
[global]
quit = "ctrl+q"

[normal]
scroll_up = "ctrl+k"
scroll_down = ["ctrl+j", "down"]

[insert]
"#;
        let overrides = parse_overrides_toml(toml).unwrap();
        assert_eq!(overrides.global.len(), 1);
        assert_eq!(overrides.global[0].0, KeyAction::Quit);
        assert_eq!(overrides.normal.len(), 2);
    }

    #[test]
    fn parse_profile_toml_basic() {
        let toml = r#"
name = "Test"

[global]
quit = "ctrl+q"

[normal]
scroll_down = "j"

[insert]
exit_insert = "esc"
send_message = "enter"
"#;
        let profile = parse_profile_toml(toml).unwrap();
        assert_eq!(profile.profile_name, "Test");
        assert_eq!(
            profile.resolve(
                KeyModifiers::CONTROL,
                KeyCode::Char('q'),
                BindingMode::Normal
            ),
            Some(KeyAction::Quit)
        );
        assert_eq!(
            profile.resolve(KeyModifiers::NONE, KeyCode::Char('j'), BindingMode::Normal),
            Some(KeyAction::ScrollDown)
        );
    }

    #[test]
    fn tab_global_binding_not_conflicted_with_autocomplete() {
        // Tab is a global binding for NextConversation but handle_global_key
        // has a guard `if !self.autocomplete.visible`. The keybinding system
        // must let the caller handle this guard — resolve() just returns the action.
        let kb = default_profile();
        assert_eq!(
            kb.resolve(KeyModifiers::NONE, KeyCode::Tab, BindingMode::Insert),
            Some(KeyAction::NextConversation)
        );
    }

    #[test]
    fn insert_newline_both_alt_and_shift() {
        let kb = default_profile();
        assert_eq!(
            kb.resolve(KeyModifiers::ALT, KeyCode::Enter, BindingMode::Insert),
            Some(KeyAction::InsertNewline)
        );
        assert_eq!(
            kb.resolve(KeyModifiers::SHIFT, KeyCode::Enter, BindingMode::Insert),
            Some(KeyAction::InsertNewline)
        );
    }
}
