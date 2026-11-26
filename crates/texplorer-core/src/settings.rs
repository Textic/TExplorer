use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::entry::{KnownFolder, KnownFolderKind, ViewMode};
use crate::network::NetworkConnection;

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

/// Configurable top-level navigation categories rendered in the left sidebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SidebarSection {
    QuickAccess,
    Pinned,
    Drives,
    Network,
    LinuxWsl,
}

impl SidebarSection {
    /// Default vertical order for left sidebar categories.
    pub const DEFAULT_ORDER: [Self; 5] = [
        Self::QuickAccess,
        Self::Pinned,
        Self::Drives,
        Self::Network,
        Self::LinuxWsl,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::QuickAccess => "Quick Access",
            Self::Pinned => "Pinned",
            Self::Drives => "Drives",
            Self::Network => "Network",
            Self::LinuxWsl => "Linux / WSL",
        }
    }

    pub fn header_label(self) -> &'static str {
        match self {
            Self::QuickAccess => "QUICK ACCESS",
            Self::Pinned => "PINNED",
            Self::Drives => "DRIVES",
            Self::Network => "NETWORK",
            Self::LinuxWsl => "LINUX / WSL",
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
    pub win_e_new_window: bool,
    pub restore_tabs_on_startup: bool,
    pub sidebar_section_order: Vec<SidebarSection>,
    pub hidden_sidebar_sections: Vec<SidebarSection>,
    pub collapsed_sidebar_sections: Vec<SidebarSection>,
    pub quick_access_order: Vec<KnownFolderKind>,
    pub hidden_quick_access: Vec<KnownFolderKind>,
    pub pinned_folders: Vec<PathBuf>,
    pub network_connections: Vec<NetworkConnection>,
    pub show_sidebar_p2p_row: bool,
    pub details_col_modified: u16,
    pub details_col_type: u16,
    pub details_col_size: u16,
    pub quicklook_video_muted: bool,
    pub quicklook_video_volume: u8,
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
            win_e_new_window: true,
            restore_tabs_on_startup: true,
            sidebar_section_order: SidebarSection::DEFAULT_ORDER.to_vec(),
            hidden_sidebar_sections: Vec::new(),
            collapsed_sidebar_sections: Vec::new(),
            quick_access_order: Vec::new(),
            hidden_quick_access: Vec::new(),
            pinned_folders: Vec::new(),
            network_connections: Vec::new(),
            show_sidebar_p2p_row: false,
            details_col_modified: 164,
            details_col_type: 144,
            details_col_size: 106,
            quicklook_video_muted: true,
            quicklook_video_volume: 70,
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

    /// Returns `true` when pressing `Win+E` while TExplorer is already open should open
    /// a separate window instead of adding a new tab to the active window.
    /// Always opens a separate window when the top tab bar is hidden.
    pub fn should_win_e_open_new_window(&self) -> bool {
        !self.show_tabs || self.win_e_new_window
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

    /// Returns `available` Quick Access folders ordered according to `quick_access_order`,
    /// appending any remaining folders in their natural discovery order.
    pub fn ordered_quick_access(&self, available: &[KnownFolder]) -> Vec<KnownFolder> {
        let mut out = Vec::with_capacity(available.len());
        for &kind in &self.quick_access_order {
            if let Some(folder) = available.iter().find(|f| f.kind == kind)
                && !out.iter().any(|existing: &KnownFolder| existing.kind == kind)
            {
                out.push(folder.clone());
            }
        }
        for folder in available {
            if !out.iter().any(|existing| existing.kind == folder.kind) {
                out.push(folder.clone());
            }
        }
        out
    }

    /// Moves a Quick Access folder up (`delta = -1`) or down (`delta = 1`) relative to `available`.
    pub fn move_quick_access(
        &mut self,
        kind: KnownFolderKind,
        delta: isize,
        available: &[KnownFolder],
    ) -> bool {
        let mut order: Vec<KnownFolderKind> = self
            .ordered_quick_access(available)
            .into_iter()
            .map(|f| f.kind)
            .collect();
        let Some(idx) = order.iter().position(|k| *k == kind) else {
            return false;
        };
        let target = idx as isize + delta;
        if target < 0 || target as usize >= order.len() {
            return false;
        }
        order.swap(idx, target as usize);
        self.quick_access_order = order;
        true
    }

    /// Reorders Quick Access by dragging `from` onto `to`.
    pub fn reorder_quick_access(
        &mut self,
        from: KnownFolderKind,
        to: KnownFolderKind,
        available: &[KnownFolder],
    ) -> bool {
        if from == to {
            return false;
        }
        let mut order: Vec<KnownFolderKind> = self
            .ordered_quick_access(available)
            .into_iter()
            .map(|f| f.kind)
            .collect();
        let (Some(from_idx), Some(to_idx)) = (
            order.iter().position(|k| *k == from),
            order.iter().position(|k| *k == to),
        ) else {
            return false;
        };
        let item = order.remove(from_idx);
        order.insert(to_idx, item);
        self.quick_access_order = order;
        true
    }

    /// Returns `true` when Quick Access folder order or visibility differs from defaults.
    pub fn has_custom_quick_access(&self, available: &[KnownFolder]) -> bool {
        if !self.hidden_quick_access.is_empty() {
            return true;
        }
        let default_kinds: Vec<KnownFolderKind> = available.iter().map(|f| f.kind).collect();
        let current_kinds: Vec<KnownFolderKind> = self
            .ordered_quick_access(available)
            .into_iter()
            .map(|f| f.kind)
            .collect();
        current_kinds != default_kinds
    }

    /// Restores default Quick Access folder ordering and visibility.
    pub fn reset_quick_access(&mut self) {
        self.quick_access_order.clear();
        self.hidden_quick_access.clear();
    }

    /// Returns all sidebar sections deduplicated in the user's configured order,
    /// appending any newly introduced sections in their default relative order.
    pub fn normalized_sidebar_order(&self) -> Vec<SidebarSection> {
        let mut out = Vec::with_capacity(SidebarSection::DEFAULT_ORDER.len());
        for &sec in &self.sidebar_section_order {
            if !out.contains(&sec) {
                out.push(sec);
            }
        }
        for sec in SidebarSection::DEFAULT_ORDER {
            if !out.contains(&sec) {
                out.push(sec);
            }
        }
        out
    }

    /// Returns `true` when `section` is enabled to appear in the left sidebar.
    pub fn is_sidebar_section_visible(&self, section: SidebarSection) -> bool {
        !self.hidden_sidebar_sections.contains(&section)
    }

    /// Toggles whether `section` is shown in the left sidebar.
    pub fn toggle_sidebar_section_visible(&mut self, section: SidebarSection) {
        if let Some(pos) = self
            .hidden_sidebar_sections
            .iter()
            .position(|s| *s == section)
        {
            self.hidden_sidebar_sections.remove(pos);
        } else {
            self.hidden_sidebar_sections.push(section);
        }
    }

    /// Returns `true` when `section` is currently collapsed in the left sidebar.
    pub fn is_sidebar_section_collapsed(&self, section: SidebarSection) -> bool {
        self.collapsed_sidebar_sections.contains(&section)
    }

    /// Toggles collapsed/expanded state of `section` in the left sidebar.
    pub fn toggle_sidebar_section_collapsed(&mut self, section: SidebarSection) {
        if let Some(pos) = self
            .collapsed_sidebar_sections
            .iter()
            .position(|s| *s == section)
        {
            self.collapsed_sidebar_sections.remove(pos);
        } else {
            self.collapsed_sidebar_sections.push(section);
        }
    }

    /// Moves `section` up (`delta = -1`) or down (`delta = 1`) in `sidebar_section_order`.
    pub fn move_sidebar_section(&mut self, section: SidebarSection, delta: isize) -> bool {
        let mut order = self.normalized_sidebar_order();
        let Some(idx) = order.iter().position(|s| *s == section) else {
            return false;
        };
        let target = idx as isize + delta;
        if target < 0 || target as usize >= order.len() {
            return false;
        }
        order.swap(idx, target as usize);
        self.sidebar_section_order = order;
        true
    }

    /// Reorders sidebar sections by dragging `from` onto `to`.
    pub fn reorder_sidebar_section(&mut self, from: SidebarSection, to: SidebarSection) -> bool {
        if from == to {
            return false;
        }
        let mut order = self.normalized_sidebar_order();
        let (Some(from_idx), Some(to_idx)) = (
            order.iter().position(|s| *s == from),
            order.iter().position(|s| *s == to),
        ) else {
            return false;
        };
        let item = order.remove(from_idx);
        order.insert(to_idx, item);
        self.sidebar_section_order = order;
        true
    }

    /// Returns `true` when sidebar section order or visibility differs from defaults.
    pub fn has_custom_sidebar_sections(&self) -> bool {
        self.normalized_sidebar_order().as_slice() != SidebarSection::DEFAULT_ORDER
            || !self.hidden_sidebar_sections.is_empty()
    }

    /// Restores default sidebar section ordering and visibility.
    pub fn reset_sidebar_sections(&mut self) {
        self.sidebar_section_order = SidebarSection::DEFAULT_ORDER.to_vec();
        self.hidden_sidebar_sections.clear();
        self.collapsed_sidebar_sections.clear();
    }

    /// Reorders pinned sidebar folders by dragging `from` onto `to`.
    pub fn reorder_pinned_folder(&mut self, from: &Path, to: &Path) -> bool {
        let same = |a: &Path, b: &Path| {
            #[cfg(target_os = "windows")]
            {
                a.to_string_lossy()
                    .eq_ignore_ascii_case(&b.to_string_lossy())
            }
            #[cfg(not(target_os = "windows"))]
            {
                a == b
            }
        };
        if same(from, to) {
            return false;
        }
        let (Some(from_idx), Some(to_idx)) = (
            self.pinned_folders.iter().position(|p| same(p, from)),
            self.pinned_folders.iter().position(|p| same(p, to)),
        ) else {
            return false;
        };
        let item = self.pinned_folders.remove(from_idx);
        self.pinned_folders.insert(to_idx, item);
        true
    }

    /// Inserts or updates a saved network connection profile by `id`.
    pub fn upsert_network_connection(&mut self, conn: NetworkConnection) {
        if let Some(existing) = self
            .network_connections
            .iter_mut()
            .find(|c| c.id == conn.id)
        {
            *existing = conn;
        } else {
            self.network_connections.push(conn);
        }
    }

    /// Removes a saved network connection profile by `id`.
    pub fn remove_network_connection(&mut self, id: Uuid) -> bool {
        let before = self.network_connections.len();
        self.network_connections.retain(|c| c.id != id);
        self.network_connections.len() != before
    }

    /// Looks up a saved network connection profile by `id`.
    pub fn network_connection(&self, id: Uuid) -> Option<&NetworkConnection> {
        self.network_connections.iter().find(|c| c.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidebar_sections_reorder_hide_collapse_and_normalize() {
        let mut s = AppSettings {
            sidebar_section_order: vec![SidebarSection::Drives, SidebarSection::Drives],
            ..Default::default()
        };
        // Normalizing deduplicates and appends missing sections in default order.
        assert_eq!(
            s.normalized_sidebar_order(),
            vec![
                SidebarSection::Drives,
                SidebarSection::QuickAccess,
                SidebarSection::Pinned,
                SidebarSection::Network,
                SidebarSection::LinuxWsl,
            ]
        );
        // Boundary checks & swapping.
        assert!(!s.move_sidebar_section(SidebarSection::Drives, -1));
        assert!(s.move_sidebar_section(SidebarSection::Drives, 1));
        assert_eq!(s.normalized_sidebar_order()[0], SidebarSection::QuickAccess);
        assert_eq!(s.normalized_sidebar_order()[1], SidebarSection::Drives);

        // Drag-and-drop reorder inserts `from` at `to`'s position.
        assert!(s.reorder_sidebar_section(SidebarSection::LinuxWsl, SidebarSection::QuickAccess));
        assert_eq!(s.normalized_sidebar_order()[0], SidebarSection::LinuxWsl);
        assert_eq!(s.normalized_sidebar_order()[1], SidebarSection::QuickAccess);

        // Visibility & collapse toggles.
        assert!(s.is_sidebar_section_visible(SidebarSection::LinuxWsl));
        s.toggle_sidebar_section_visible(SidebarSection::LinuxWsl);
        assert!(!s.is_sidebar_section_visible(SidebarSection::LinuxWsl));

        assert!(!s.is_sidebar_section_collapsed(SidebarSection::Drives));
        s.toggle_sidebar_section_collapsed(SidebarSection::Drives);
        assert!(s.is_sidebar_section_collapsed(SidebarSection::Drives));

        assert!(s.has_custom_sidebar_sections());
        s.reset_sidebar_sections();
        assert!(!s.has_custom_sidebar_sections());
        assert!(s.is_sidebar_section_visible(SidebarSection::LinuxWsl));
        assert!(!s.is_sidebar_section_collapsed(SidebarSection::Drives));
    }

    #[test]
    fn quick_access_items_reorder_and_reset() {
        let available = vec![
            KnownFolder {
                kind: KnownFolderKind::Desktop,
                name: "Desktop".to_string(),
                path: PathBuf::from(r"C:\Users\Demo\Desktop"),
            },
            KnownFolder {
                kind: KnownFolderKind::Downloads,
                name: "Downloads".to_string(),
                path: PathBuf::from(r"C:\Users\Demo\Downloads"),
            },
            KnownFolder {
                kind: KnownFolderKind::Documents,
                name: "Documents".to_string(),
                path: PathBuf::from(r"C:\Users\Demo\Documents"),
            },
        ];

        let mut s = AppSettings::default();
        assert!(!s.has_custom_quick_access(&available));
        assert!(!s.move_quick_access(KnownFolderKind::Desktop, -1, &available));
        assert!(s.move_quick_access(KnownFolderKind::Documents, -1, &available));
        assert!(s.has_custom_quick_access(&available));
        assert!(s.reorder_quick_access(
            KnownFolderKind::Downloads,
            KnownFolderKind::Desktop,
            &available
        ));
        assert_eq!(
            s.ordered_quick_access(&available)[0].kind,
            KnownFolderKind::Downloads
        );
        s.reset_quick_access();
        assert!(s.move_quick_access(KnownFolderKind::Documents, -1, &available));

        let ordered: Vec<KnownFolderKind> = s
            .ordered_quick_access(&available)
            .into_iter()
            .map(|f| f.kind)
            .collect();
        assert_eq!(
            ordered,
            vec![
                KnownFolderKind::Desktop,
                KnownFolderKind::Documents,
                KnownFolderKind::Downloads,
            ]
        );

        s.reset_quick_access();
        assert!(!s.has_custom_quick_access(&available));
    }

    #[test]
    fn round_trips_settings_json() {
        let mut settings = AppSettings {
            theme: ThemePreset::FilesLight,
            accent: AccentPreset::FilesBlue,
            show_hidden_files: true,
            ..Default::default()
        };
        assert!(settings.quicklook_video_muted);
        assert_eq!(settings.quicklook_video_volume, 70);
        settings.quicklook_video_muted = false;
        settings.quicklook_video_volume = 45;
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

    #[test]
    fn reorders_pinned_folders_and_hides_sidebar_p2p_row_by_default() {
        let mut s = AppSettings::default();
        assert!(!s.show_sidebar_p2p_row);
        assert!(s.win_e_new_window);
        assert!(s.should_win_e_open_new_window());

        s.win_e_new_window = false;
        assert!(!s.should_win_e_open_new_window());
        s.show_tabs = false;
        assert!(s.should_win_e_open_new_window());

        let a = PathBuf::from(r"C:\Alpha");
        let b = PathBuf::from(r"C:\Beta");
        let c = PathBuf::from(r"C:\Gamma");
        s.pin_folder(a.clone());
        s.pin_folder(b.clone());
        s.pin_folder(c.clone());

        assert!(s.reorder_pinned_folder(&c, &a));
        assert_eq!(s.pinned_folders, vec![c.clone(), a.clone(), b.clone()]);
        assert!(!s.reorder_pinned_folder(&a, &a));
    }
}
