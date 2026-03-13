use serde::{Deserialize, Serialize};

use crate::app::App;

/// A settings profile: a named collection of the 14 persisted display toggles.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettingsProfile {
    pub name: String,
    pub notify_direct: bool,
    pub notify_group: bool,
    pub desktop_notifications: bool,
    pub inline_images: bool,
    pub show_link_previews: bool,
    pub native_images: bool,
    pub date_separators: bool,
    pub show_receipts: bool,
    pub color_receipts: bool,
    pub nerd_fonts: bool,
    pub reaction_verbose: bool,
    pub send_read_receipts: bool,
    pub mouse_enabled: bool,
    pub sidebar_on_right: bool,
}

pub fn default_profile() -> SettingsProfile {
    SettingsProfile {
        name: "Default".to_string(),
        notify_direct: true,
        notify_group: true,
        desktop_notifications: false,
        inline_images: true,
        show_link_previews: true,
        native_images: false,
        date_separators: true,
        show_receipts: true,
        color_receipts: true,
        nerd_fonts: false,
        reaction_verbose: false,
        send_read_receipts: true,
        mouse_enabled: true,
        sidebar_on_right: false,
    }
}

pub fn minimal_profile() -> SettingsProfile {
    SettingsProfile {
        name: "Minimal".to_string(),
        notify_direct: false,
        notify_group: false,
        desktop_notifications: false,
        inline_images: false,
        show_link_previews: false,
        native_images: false,
        date_separators: false,
        show_receipts: false,
        color_receipts: false,
        nerd_fonts: false,
        reaction_verbose: false,
        send_read_receipts: false,
        mouse_enabled: true,
        sidebar_on_right: false,
    }
}

pub fn full_profile() -> SettingsProfile {
    SettingsProfile {
        name: "Full".to_string(),
        notify_direct: true,
        notify_group: true,
        desktop_notifications: true,
        inline_images: true,
        show_link_previews: true,
        native_images: true,
        date_separators: true,
        show_receipts: true,
        color_receipts: true,
        nerd_fonts: true,
        reaction_verbose: true,
        send_read_receipts: true,
        mouse_enabled: true,
        sidebar_on_right: false,
    }
}

pub fn builtin_profiles() -> Vec<SettingsProfile> {
    vec![default_profile(), minimal_profile(), full_profile()]
}

const BUILTIN_NAMES: &[&str] = &["Default", "Minimal", "Full"];

pub fn is_builtin(name: &str) -> bool {
    BUILTIN_NAMES.iter().any(|n| n.eq_ignore_ascii_case(name))
}

/// Load custom profiles from `~/.config/siggy/profiles/*.toml`.
pub fn load_custom_profiles() -> Vec<SettingsProfile> {
    let dir = match dirs::config_dir() {
        Some(d) => d.join("siggy").join("profiles"),
        None => return Vec::new(),
    };
    if !dir.is_dir() {
        return Vec::new();
    }
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(e) => {
            crate::debug_log::logf(format_args!("custom profiles dir read error: {e}"));
            return Vec::new();
        }
    };
    let mut profiles = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }
        match std::fs::read_to_string(&path) {
            Ok(contents) => match toml::from_str::<SettingsProfile>(&contents) {
                Ok(p) => profiles.push(p),
                Err(e) => {
                    crate::debug_log::logf(format_args!(
                        "custom profile parse error {}: {e}",
                        path.display()
                    ));
                }
            },
            Err(e) => {
                crate::debug_log::logf(format_args!(
                    "custom profile read error {}: {e}",
                    path.display()
                ));
            }
        }
    }
    profiles
}

/// All available profiles: built-ins followed by custom.
pub fn all_settings_profiles() -> Vec<SettingsProfile> {
    let mut profiles = builtin_profiles();
    profiles.extend(load_custom_profiles());
    profiles
}

/// Find a profile by name. Falls back to Default if not found.
/// Only used in tests, so marked with cfg(test).
#[cfg(test)]
pub fn find_settings_profile(name: &str) -> SettingsProfile {
    all_settings_profiles()
        .into_iter()
        .find(|p| p.name == name)
        .unwrap_or_else(default_profile)
}

/// Convert a profile name to a safe filename (lowercase, spaces to hyphens).
fn name_to_filename(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
        .collect()
}

/// Save a custom profile to `~/.config/siggy/profiles/<name>.toml`.
pub fn save_custom_profile(profile: &SettingsProfile) -> Result<(), String> {
    let dir = dirs::config_dir()
        .ok_or("no config dir")?
        .join("siggy")
        .join("profiles");
    std::fs::create_dir_all(&dir).map_err(|e| format!("mkdir: {e}"))?;
    let filename = format!("{}.toml", name_to_filename(&profile.name));
    let path = dir.join(filename);
    let contents = toml::to_string_pretty(profile).map_err(|e| format!("serialize: {e}"))?;
    std::fs::write(&path, contents).map_err(|e| format!("write: {e}"))?;
    Ok(())
}

/// Delete a custom profile by name. Scans all .toml files and matches by parsed name field.
pub fn delete_custom_profile(name: &str) -> Result<(), String> {
    let dir = dirs::config_dir()
        .ok_or("no config dir")?
        .join("siggy")
        .join("profiles");
    if !dir.is_dir() {
        return Err("profiles dir not found".to_string());
    }
    let entries = std::fs::read_dir(&dir).map_err(|e| format!("read dir: {e}"))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }
        if let Ok(contents) = std::fs::read_to_string(&path) {
            if let Ok(p) = toml::from_str::<SettingsProfile>(&contents) {
                if p.name == name {
                    std::fs::remove_file(&path).map_err(|e| format!("delete: {e}"))?;
                    return Ok(());
                }
            }
        }
    }
    Err(format!("profile '{name}' not found"))
}

impl SettingsProfile {
    /// Create a profile from the current app settings.
    pub fn from_app(app: &App, name: String) -> Self {
        Self {
            name,
            notify_direct: app.notify_direct,
            notify_group: app.notify_group,
            desktop_notifications: app.desktop_notifications,
            inline_images: app.inline_images,
            show_link_previews: app.show_link_previews,
            native_images: app.native_images,
            date_separators: app.date_separators,
            show_receipts: app.show_receipts,
            color_receipts: app.color_receipts,
            nerd_fonts: app.nerd_fonts,
            reaction_verbose: app.reaction_verbose,
            send_read_receipts: app.send_read_receipts,
            mouse_enabled: app.mouse_enabled,
            sidebar_on_right: app.sidebar_on_right,
        }
    }

    /// Apply this profile to the app, setting all 14 toggle fields.
    pub fn apply_to(&self, app: &mut App) {
        app.notify_direct = self.notify_direct;
        app.notify_group = self.notify_group;
        app.desktop_notifications = self.desktop_notifications;
        app.inline_images = self.inline_images;
        app.show_link_previews = self.show_link_previews;
        app.native_images = self.native_images;
        app.date_separators = self.date_separators;
        app.show_receipts = self.show_receipts;
        app.color_receipts = self.color_receipts;
        app.nerd_fonts = self.nerd_fonts;
        app.reaction_verbose = self.reaction_verbose;
        app.send_read_receipts = self.send_read_receipts;
        app.mouse_enabled = self.mouse_enabled;
        app.sidebar_on_right = self.sidebar_on_right;
    }

    /// Check whether the app's current settings match this profile.
    pub fn matches_app(&self, app: &App) -> bool {
        self.notify_direct == app.notify_direct
            && self.notify_group == app.notify_group
            && self.desktop_notifications == app.desktop_notifications
            && self.inline_images == app.inline_images
            && self.show_link_previews == app.show_link_previews
            && self.native_images == app.native_images
            && self.date_separators == app.date_separators
            && self.show_receipts == app.show_receipts
            && self.color_receipts == app.color_receipts
            && self.nerd_fonts == app.nerd_fonts
            && self.reaction_verbose == app.reaction_verbose
            && self.send_read_receipts == app.send_read_receipts
            && self.mouse_enabled == app.mouse_enabled
            && self.sidebar_on_right == app.sidebar_on_right
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_profiles_have_correct_names() {
        let profiles = builtin_profiles();
        assert_eq!(profiles.len(), 3);
        assert_eq!(profiles[0].name, "Default");
        assert_eq!(profiles[1].name, "Minimal");
        assert_eq!(profiles[2].name, "Full");
    }

    #[test]
    fn is_builtin_check() {
        assert!(is_builtin("Default"));
        assert!(is_builtin("Minimal"));
        assert!(is_builtin("Full"));
        assert!(!is_builtin("My Custom"));
    }

    #[test]
    fn find_settings_profile_fallback() {
        let p = find_settings_profile("nonexistent");
        assert_eq!(p.name, "Default");
    }

    #[test]
    fn name_to_filename_converts() {
        assert_eq!(name_to_filename("My Custom Setup"), "my-custom-setup");
        assert_eq!(name_to_filename("Default"), "default");
    }

    #[test]
    fn minimal_profile_all_off_except_mouse() {
        let p = minimal_profile();
        assert!(!p.notify_direct);
        assert!(!p.notify_group);
        assert!(!p.desktop_notifications);
        assert!(!p.inline_images);
        assert!(!p.show_link_previews);
        assert!(!p.native_images);
        assert!(!p.date_separators);
        assert!(!p.show_receipts);
        assert!(!p.color_receipts);
        assert!(!p.nerd_fonts);
        assert!(!p.reaction_verbose);
        assert!(!p.send_read_receipts);
        assert!(p.mouse_enabled);
        assert!(!p.sidebar_on_right);
    }

    #[test]
    fn full_profile_all_on_except_sidebar_right() {
        let p = full_profile();
        assert!(p.notify_direct);
        assert!(p.notify_group);
        assert!(p.desktop_notifications);
        assert!(p.inline_images);
        assert!(p.show_link_previews);
        assert!(p.native_images);
        assert!(p.date_separators);
        assert!(p.show_receipts);
        assert!(p.color_receipts);
        assert!(p.nerd_fonts);
        assert!(p.reaction_verbose);
        assert!(p.send_read_receipts);
        assert!(p.mouse_enabled);
        assert!(!p.sidebar_on_right);
    }
}
