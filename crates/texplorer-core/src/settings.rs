use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::entry::{KnownFolderKind, ViewMode};

/// Accent color presets for the Obsidian Dark flat theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum AccentPreset {
    #[default]
    Emerald,
    Violet,
    Monochrome,
}

impl AccentPreset {
    pub const ALL: [Self; 3] = [Self::Emerald, Self::Violet, Self::Monochrome];

    pub fn label(self) -> &'static str {
        match self {
            Self::Emerald => "Obsidian Emerald",
            Self::Violet => "Obsidian Violet",
            Self::Monochrome => "Obsidian Monochrome",
        }
    }

    pub fn primary_hex(self) -> u32 {
        match self {
            Self::Emerald => 0x34d399,
            Self::Violet => 0xa78bfa,
            Self::Monochrome => 0xe4e4e7,
        }
    }

    pub fn hover_hex(self) -> u32 {
        match self {
            Self::Emerald => 0x10b981,
            Self::Violet => 0x8b5cf6,
            Self::Monochrome => 0xf4f4f5,
        }
    }

    pub fn active_hex(self) -> u32 {
        match self {
            Self::Emerald => 0x059669,
            Self::Violet => 0x7c3aed,
            Self::Monochrome => 0xd4d4d8,
        }
    }

    pub fn text_on_accent_hex(self) -> u32 {
        0x09090b
    }
}

/// Byte size formatting standard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SizeUnit {
    #[default]
    Binary,
    Decimal,
}

impl SizeUnit {
    pub fn label(self) -> &'static str {
        match self {
            Self::Binary => "Binary (KiB / MiB / GiB)",
            Self::Decimal => "Decimal (KB / MB / GB)",
        }
    }
}

/// Persistent user configuration for T Explorer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub accent: AccentPreset,
    pub size_unit: SizeUnit,
    pub default_view_mode: ViewMode,
    pub show_tabs: bool,
    pub show_brand_title: bool,
    pub show_hidden_files: bool,
    pub show_file_extensions: bool,
    pub show_sidebar: bool,
    pub show_inspector: bool,
    pub calculate_folder_sizes: bool,
    pub use_mft_service: bool,
    pub auto_check_updates: bool,
    pub replace_default_explorer: bool,
    pub intercept_win_e: bool,
    pub restore_tabs_on_startup: bool,
    pub hidden_quick_access: Vec<KnownFolderKind>,
    pub pinned_folders: Vec<PathBuf>,
    pub terminal_command: String,
    pub editor_command: String,
    pub textree_path: Option<PathBuf>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            accent: AccentPreset::Emerald,
            size_unit: SizeUnit::Binary,
            default_view_mode: ViewMode::Details,
            show_tabs: true,
            show_brand_title: true,
            show_hidden_files: false,
            show_file_extensions: true,
            show_sidebar: true,
            show_inspector: false,
            calculate_folder_sizes: true,
            use_mft_service: true,
            auto_check_updates: true,
            replace_default_explorer: false,
            intercept_win_e: false,
            restore_tabs_on_startup: true,
            hidden_quick_access: Vec::new(),
            pinned_folders: Vec::new(),
            terminal_command: "wt.exe".to_string(),
            editor_command: "code".to_string(),
            textree_path: None,
        }
    }
}

impl AppSettings {
    /// Returns the platform config directory path for T Explorer.
    pub fn config_dir() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("TExplorer")
    }

    /// Returns the path to `settings.json`.
    pub fn config_path() -> PathBuf {
        Self::config_dir().join("settings.json")
    }

    /// Loads persisted settings from disk, falling back to defaults when missing or invalid.
    pub fn load() -> Self {
        let path = Self::config_path();
        let Ok(raw) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        serde_json::from_str(&raw).unwrap_or_default()
    }

    /// Saves settings atomically to disk.
    pub fn save(&self) -> Result<(), String> {
        let dir = Self::config_dir();
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("Failed to create settings directory: {e}"))?;
        let path = Self::config_path();
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize settings: {e}"))?;
        std::fs::write(&path, json).map_err(|e| format!("Failed to write settings file: {e}"))
    }

    /// Pins a folder to the sidebar if not already present.
    pub fn pin_folder(&mut self, path: PathBuf) -> bool {
        if self.pinned_folders.iter().any(|p| p == &path) {
            return false;
        }
        self.pinned_folders.push(path);
        true
    }

    /// Removes a pinned folder from the sidebar.
    pub fn unpin_folder(&mut self, path: &std::path::Path) -> bool {
        let before = self.pinned_folders.len();
        self.pinned_folders.retain(|p| p != path);
        self.pinned_folders.len() != before
    }

    /// Returns `true` when `kind` is visible in Quick Access.
    pub fn is_quick_access_visible(&self, kind: KnownFolderKind) -> bool {
        !self.hidden_quick_access.contains(&kind)
    }

    /// Hides a standard Quick Access folder (e.g. Pictures, Music, Videos).
    pub fn hide_quick_access(&mut self, kind: KnownFolderKind) -> bool {
        if self.hidden_quick_access.contains(&kind) {
            return false;
        }
        self.hidden_quick_access.push(kind);
        true
    }

    /// Toggles visibility of a standard Quick Access folder.
    pub fn toggle_quick_access(&mut self, kind: KnownFolderKind) {
        if let Some(pos) = self.hidden_quick_access.iter().position(|k| *k == kind) {
            self.hidden_quick_access.remove(pos);
        } else {
            self.hidden_quick_access.push(kind);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_settings_json() {
        let mut settings = AppSettings {
            accent: AccentPreset::Violet,
            show_hidden_files: true,
            ..Default::default()
        };
        settings.pin_folder(PathBuf::from(r"X:\TExplorer"));

        let json = serde_json::to_string(&settings).unwrap();
        let parsed: AppSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, settings);
    }
}
