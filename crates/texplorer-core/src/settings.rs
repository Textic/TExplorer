use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::entry::{KnownFolderKind, ViewMode};

/// Base surface theme preset. Files palettes appear first, followed by Obsidian Dark.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ThemePreset {
    /// Automatically follows the OS light/dark theme using the Files palettes.
    #[default]
    FilesSystem,
    /// Fluent/WinUI 3 Files Dark palette.
    FilesDark,
    /// Fluent/WinUI 3 Files Light palette.
    FilesLight,
    /// Neutral Carbon Black flat-dark Obsidian palette.
    ObsidianDark,
}

impl ThemePreset {
    /// Ordered list for Settings UI: Files palettes first, then Obsidian.
    pub const ALL: [Self; 4] = [
        Self::FilesSystem,
        Self::FilesDark,
        Self::FilesLight,
        Self::ObsidianDark,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::FilesSystem => "System (Files)",
            Self::FilesDark => "Files Dark",
            Self::FilesLight => "Files Light",
            Self::ObsidianDark => "Obsidian Dark",
        }
    }

    pub fn short_label(self) -> &'static str {
        match self {
            Self::FilesSystem => "System",
            Self::FilesDark => "Files Dark",
            Self::FilesLight => "Files Light",
            Self::ObsidianDark => "Obsidian",
        }
    }

    /// Resolves whether the effective surface palette is dark given the OS preference.
    pub fn resolves_dark(self, system_is_dark: bool) -> bool {
        match self {
            Self::FilesSystem => system_is_dark,
            Self::FilesDark | Self::ObsidianDark => true,
            Self::FilesLight => false,
        }
    }
}

/// Accent color presets: Files accents first, followed by Obsidian accents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum AccentPreset {
    /// Files attention blue (`#50C0FF` dark / `#0070CB` light).
    #[default]
    FilesBlue,
    /// Files Windows blue (`#0078D7`).
    FilesCobalt,
    /// Files mint/teal (`#00B294`).
    FilesTeal,
    /// Obsidian Emerald (`#34D399`).
    Emerald,
    /// Obsidian Violet (`#A78BFA`).
    Violet,
    /// Obsidian Monochrome (`#E4E4E7` dark / `#27272A` light).
    Monochrome,
}

impl AccentPreset {
    /// Ordered list for Settings UI: Files colors first, then Obsidian colors.
    pub const ALL: [Self; 6] = [
        Self::FilesBlue,
        Self::FilesCobalt,
        Self::FilesTeal,
        Self::Emerald,
        Self::Violet,
        Self::Monochrome,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::FilesBlue => "Files Blue",
            Self::FilesCobalt => "Files Cobalt",
            Self::FilesTeal => "Files Mint",
            Self::Emerald => "Obsidian Emerald",
            Self::Violet => "Obsidian Violet",
            Self::Monochrome => "Obsidian Mono",
        }
    }

    pub fn short_label(self) -> &'static str {
        match self {
            Self::FilesBlue => "Files Blue",
            Self::FilesCobalt => "Cobalt",
            Self::FilesTeal => "Mint",
            Self::Emerald => "Emerald",
            Self::Violet => "Violet",
            Self::Monochrome => "Mono",
        }
    }

    /// Primary accent hex adapted for dark vs. light surfaces.
    pub fn primary_hex_for(self, is_dark: bool) -> u32 {
        match (self, is_dark) {
            (Self::FilesBlue, true) => 0x50c0ff,
            (Self::FilesBlue, false) => 0x0070cb,
            (Self::FilesCobalt, true) => 0x4cc2ff,
            (Self::FilesCobalt, false) => 0x0078d7,
            (Self::FilesTeal, true) => 0x00b7c3,
            (Self::FilesTeal, false) => 0x008b74,
            (Self::Emerald, true) => 0x34d399,
            (Self::Emerald, false) => 0x059669,
            (Self::Violet, true) => 0xa78bfa,
            (Self::Violet, false) => 0x7c3aed,
            (Self::Monochrome, true) => 0xe4e4e7,
            (Self::Monochrome, false) => 0x27272a,
        }
    }

    pub fn hover_hex_for(self, is_dark: bool) -> u32 {
        match (self, is_dark) {
            (Self::FilesBlue, true) => 0x6eccff,
            (Self::FilesBlue, false) => 0x005fb8,
            (Self::FilesCobalt, true) => 0x6bd0ff,
            (Self::FilesCobalt, false) => 0x0067c0,
            (Self::FilesTeal, true) => 0x24c9d4,
            (Self::FilesTeal, false) => 0x007561,
            (Self::Emerald, true) => 0x10b981,
            (Self::Emerald, false) => 0x047857,
            (Self::Violet, true) => 0x8b5cf6,
            (Self::Violet, false) => 0x6d28d9,
            (Self::Monochrome, true) => 0xf4f4f5,
            (Self::Monochrome, false) => 0x18181b,
        }
    }

    pub fn active_hex_for(self, is_dark: bool) -> u32 {
        match (self, is_dark) {
            (Self::FilesBlue, true) => 0x38b2f5,
            (Self::FilesBlue, false) => 0x00529e,
            (Self::FilesCobalt, true) => 0x2db3f5,
            (Self::FilesCobalt, false) => 0x0058a6,
            (Self::FilesTeal, true) => 0x009da8,
            (Self::FilesTeal, false) => 0x006150,
            (Self::Emerald, true) => 0x059669,
            (Self::Emerald, false) => 0x065f46,
            (Self::Violet, true) => 0x7c3aed,
            (Self::Violet, false) => 0x5b21b6,
            (Self::Monochrome, true) => 0xd4d4d8,
            (Self::Monochrome, false) => 0x09090b,
        }
    }

    pub fn text_on_accent_hex_for(self, is_dark: bool) -> u32 {
        if is_dark { 0x000000 } else { 0xffffff }
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

/// Persistent user configuration for TExplorer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub theme: ThemePreset,
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
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            theme: ThemePreset::FilesSystem,
            accent: AccentPreset::FilesBlue,
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
        }
    }
}

impl AppSettings {
    /// Returns the platform config directory path for TExplorer.
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
    /// Migrates pre-v0.3 configs (which lacked the `theme` field and stored the old `Emerald` default)
    /// to the new `FilesSystem` + `FilesBlue` defaults once.
    pub fn load() -> Self {
        let path = Self::config_path();
        let Ok(raw) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        let (parsed, migrated) = Self::parse_with_migration(&raw);
        if migrated {
            let _ = parsed.save();
        }
        parsed
    }

    fn parse_with_migration(raw: &str) -> (Self, bool) {
        let Ok(val) = serde_json::from_str::<serde_json::Value>(raw) else {
            return (Self::default(), false);
        };
        let is_legacy_pre_theme = val.get("theme").is_none();
        let Ok(mut parsed) = serde_json::from_value::<Self>(val) else {
            return (Self::default(), false);
        };
        if is_legacy_pre_theme && parsed.accent == AccentPreset::Emerald {
            parsed.accent = AccentPreset::FilesBlue;
            return (parsed, true);
        }
        (parsed, false)
    }

    /// Saves settings to disk.
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
            theme: ThemePreset::FilesLight,
            accent: AccentPreset::FilesBlue,
            show_hidden_files: true,
            ..Default::default()
        };
        settings.pin_folder(PathBuf::from(r"X:\TExplorer"));

        let json = serde_json::to_string(&settings).unwrap();
        let parsed: AppSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, settings);
    }

    #[test]
    fn theme_preset_orders_files_before_obsidian() {
        assert_eq!(ThemePreset::default(), ThemePreset::FilesSystem);
        assert_eq!(ThemePreset::ALL[0], ThemePreset::FilesSystem);
        assert_eq!(ThemePreset::ALL[1], ThemePreset::FilesDark);
        assert_eq!(ThemePreset::ALL[2], ThemePreset::FilesLight);
        assert_eq!(ThemePreset::ALL[3], ThemePreset::ObsidianDark);

        assert_eq!(AccentPreset::default(), AccentPreset::FilesBlue);
        assert_eq!(AccentPreset::ALL[0], AccentPreset::FilesBlue);
    }

    #[test]
    fn migrates_legacy_emerald_default_to_files_blue_without_overwriting_explicit_choice() {
        let legacy_json = r#"{"accent":"Emerald","show_tabs":true}"#;
        let (migrated_settings, did_migrate) = AppSettings::parse_with_migration(legacy_json);
        assert!(did_migrate);
        assert_eq!(migrated_settings.theme, ThemePreset::FilesSystem);
        assert_eq!(migrated_settings.accent, AccentPreset::FilesBlue);

        let explicit_json = r#"{"theme":"ObsidianDark","accent":"Emerald"}"#;
        let (explicit_settings, did_migrate_explicit) =
            AppSettings::parse_with_migration(explicit_json);
        assert!(!did_migrate_explicit);
        assert_eq!(explicit_settings.theme, ThemePreset::ObsidianDark);
        assert_eq!(explicit_settings.accent, AccentPreset::Emerald);
    }
}
