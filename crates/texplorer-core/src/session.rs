use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::entry::{SortSpec, ViewMode};
use crate::location::Location;
use crate::settings::AppSettings;
use crate::workspace_model::PaneSide;

const SESSION_SCHEMA_VERSION: u32 = 1;
const MAX_SAVED_TABS: usize = 32;

/// Persisted state of one explorer pane (location, layout mode, sort order, hidden visibility).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedPaneState {
    pub location: Location,
    #[serde(default)]
    pub view_mode: ViewMode,
    #[serde(default)]
    pub sort: SortSpec,
    #[serde(default)]
    pub show_hidden: bool,
}

/// Persisted state of one top-level tab (single or Dual-Pane split).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedTabState {
    #[serde(default)]
    pub split_enabled: bool,
    #[serde(default)]
    pub active_pane: PaneSide,
    pub left: SavedPaneState,
    #[serde(default)]
    pub right: Option<SavedPaneState>,
}

/// Persisted desktop window bounds, maximized state, and owning display identifier.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WindowGeometry {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    #[serde(default)]
    pub maximized: bool,
    #[serde(default)]
    pub display_uuid: Option<Uuid>,
}

/// Minimal descriptor of an attached OS display in logical pixels for multi-monitor window restore.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DisplayArea {
    pub uuid: Option<Uuid>,
    pub is_primary: bool,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub visible_x: f32,
    pub visible_y: f32,
    pub visible_width: f32,
    pub visible_height: f32,
}

impl DisplayArea {
    fn contains_point(self, px: f32, py: f32) -> bool {
        px >= self.x && px < self.x + self.width && py >= self.y && py < self.y + self.height
    }

    fn clamp_geometry(self, saved: WindowGeometry) -> WindowGeometry {
        let max_w = self.visible_width.max(640.0);
        let max_h = self.visible_height.max(420.0);
        let width = saved.width.clamp(640.0, max_w);
        let height = saved.height.clamp(420.0, max_h);
        let max_x = self.visible_x + (self.visible_width - width).max(0.0);
        let max_y = self.visible_y + (self.visible_height - height).max(0.0);
        let x = saved.x.clamp(self.visible_x, max_x);
        let y = saved.y.clamp(self.visible_y, max_y);
        WindowGeometry {
            x,
            y,
            width,
            height,
            maximized: saved.maximized,
            display_uuid: self.uuid,
        }
    }

    fn center_geometry(self, saved: WindowGeometry) -> WindowGeometry {
        let max_w = self.visible_width.max(640.0);
        let max_h = self.visible_height.max(420.0);
        let width = saved.width.clamp(640.0, max_w);
        let height = saved.height.clamp(420.0, max_h);
        let x = self.visible_x + ((self.visible_width - width) * 0.5).max(0.0);
        let y = self.visible_y + ((self.visible_height - height) * 0.5).max(0.0);
        WindowGeometry {
            x,
            y,
            width,
            height,
            maximized: saved.maximized,
            display_uuid: self.uuid,
        }
    }
}

/// Resolves which attached display should host a restored window and adjusts its geometry if the
/// original monitor was resized or disconnected so GPUI never rejects off-primary coordinates and
/// falls back to stretched `default_bounds()`.
pub fn resolve_window_restore(
    saved: WindowGeometry,
    displays: &[DisplayArea],
) -> Option<(usize, WindowGeometry)> {
    if !saved.is_valid() || displays.is_empty() {
        return None;
    }

    let center_x = saved.x + saved.width * 0.5;
    let center_y = saved.y + saved.height * 0.5;

    // 1. Exact UUID match when the saved center is also inside that display.
    if let Some(target_uuid) = saved.display_uuid
        && let Some(idx) = displays
            .iter()
            .position(|d| d.uuid == Some(target_uuid) && d.contains_point(center_x, center_y))
    {
        return Some((idx, saved));
    }

    // 2. For maximized windows moved between monitors (e.g. Win+Shift+Arrow), the OS may keep
    //    `rcNormalPosition` on the previous monitor while `display_uuid` tracks the active monitor.
    if saved.maximized
        && let Some(target_uuid) = saved.display_uuid
        && let Some(idx) = displays.iter().position(|d| d.uuid == Some(target_uuid))
    {
        return Some((idx, displays[idx].center_geometry(saved)));
    }

    // 3. Match whichever attached display contains the saved window center (handles secondary
    //    monitors with negative X/Y coordinates even for legacy session files without display_uuid).
    if let Some(idx) = displays
        .iter()
        .position(|d| d.contains_point(center_x, center_y))
    {
        return Some((idx, saved));
    }

    // 4. Saved display still exists, but window drifted outside its current bounds (e.g. resolution change).
    if let Some(target_uuid) = saved.display_uuid
        && let Some(idx) = displays.iter().position(|d| d.uuid == Some(target_uuid))
    {
        return Some((idx, displays[idx].clamp_geometry(saved)));
    }

    // 5. Original monitor was disconnected: re-center cleanly on the primary display preserving size.
    let primary_idx = displays.iter().position(|d| d.is_primary).unwrap_or(0);
    Some((primary_idx, displays[primary_idx].center_geometry(saved)))
}

impl WindowGeometry {
    /// Returns `true` when dimensions and coordinates are finite and usable on screen.
    pub fn is_valid(self) -> bool {
        self.x.is_finite()
            && self.y.is_finite()
            && self.width.is_finite()
            && self.height.is_finite()
            && self.width >= 640.0
            && self.height >= 420.0
            && self.width <= 16_384.0
            && self.height <= 16_384.0
            && self.x > -16_000.0
            && self.x < 16_000.0
            && self.y > -16_000.0
            && self.y < 16_000.0
    }
}

/// Full serialized workspace session stored at `%APPDATA%\TExplorer\session.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppSession {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub active_tab_index: usize,
    #[serde(default)]
    pub tabs: Vec<SavedTabState>,
    #[serde(default)]
    pub window: Option<WindowGeometry>,
}

fn default_version() -> u32 {
    SESSION_SCHEMA_VERSION
}

impl Default for AppSession {
    fn default() -> Self {
        Self {
            version: SESSION_SCHEMA_VERSION,
            active_tab_index: 0,
            tabs: Vec::new(),
            window: None,
        }
    }
}

impl AppSession {
    /// Returns `%APPDATA%\TExplorer\session.json`.
    pub fn session_path() -> PathBuf {
        AppSettings::config_dir().join("session.json")
    }

    /// Loads and sanitizes the persisted workspace session if present and valid.
    pub fn load() -> Option<Self> {
        let raw = std::fs::read_to_string(Self::session_path()).ok()?;
        let parsed: Self = serde_json::from_str(&raw).ok()?;
        let clean = parsed.sanitize();
        if clean.tabs.is_empty() && clean.window.is_none() {
            None
        } else {
            Some(clean)
        }
    }

    /// Writes the session atomically via a temporary sibling file so crashes never corrupt state.
    pub fn save(&self) -> Result<(), String> {
        let path = Self::session_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed creating session directory: {e}"))?;
        }
        let body = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Failed serializing session: {e}"))?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, &body).map_err(|e| format!("Failed writing session temp file: {e}"))?;
        if std::fs::rename(&tmp, &path).is_err() {
            let _ = std::fs::remove_file(&path);
            if let Err(err) = std::fs::rename(&tmp, &path) {
                let _ = std::fs::remove_file(&tmp);
                std::fs::write(&path, &body)
                    .map_err(|e| format!("Failed saving session ({err}): {e}"))?;
            }
        }
        Ok(())
    }

    /// Removes any saved session file (used when session restore is disabled).
    pub fn clear() {
        let _ = std::fs::remove_file(Self::session_path());
    }

    /// Filters invalid coordinates, ephemeral search views, and stale deleted paths.
    pub fn sanitize(mut self) -> Self {
        self.version = SESSION_SCHEMA_VERSION;
        self.window = self.window.filter(|g| g.is_valid());
        self.tabs.truncate(MAX_SAVED_TABS);

        for tab in &mut self.tabs {
            tab.left.location = sanitize_location(tab.left.location.clone());
            if let Some(right) = tab.right.as_mut() {
                right.location = sanitize_location(right.location.clone());
            } else {
                tab.split_enabled = false;
                tab.active_pane = PaneSide::Left;
            }
        }

        if self.tabs.is_empty() {
            self.active_tab_index = 0;
        } else if self.active_tab_index >= self.tabs.len() {
            self.active_tab_index = self.tabs.len() - 1;
        }
        self
    }
}

fn sanitize_location(loc: Location) -> Location {
    match loc {
        Location::Home => Location::Home,
        Location::Local(path) => {
            if path.is_dir() {
                return Location::Local(path);
            }
            let mut cur = path.as_path();
            while let Some(parent) = cur.parent() {
                if parent.is_dir() {
                    return Location::local(parent);
                }
                cur = parent;
            }
            Location::Home
        }
        Location::Archive {
            archive_path,
            inner_path,
        } => {
            if archive_path.is_file() {
                Location::Archive {
                    archive_path,
                    inner_path,
                }
            } else if let Some(parent) = archive_path.parent()
                && parent.is_dir()
            {
                Location::local(parent)
            } else {
                Location::Home
            }
        }
        Location::Remote { .. } | Location::Search { .. } | Location::DiskAnalyzer => {
            Location::Home
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::location::SearchScope;

    #[test]
    fn round_trips_and_sanitizes_session_state() {
        let existing_dir = std::env::temp_dir();
        let stale_dir = existing_dir.join("__texplorer_nonexistent_folder_9999__");

        let session = AppSession {
            version: 1,
            active_tab_index: 5,
            tabs: vec![
                SavedTabState {
                    split_enabled: true,
                    active_pane: PaneSide::Right,
                    left: SavedPaneState {
                        location: Location::local(&existing_dir),
                        view_mode: ViewMode::Grid,
                        sort: SortSpec::default(),
                        show_hidden: true,
                    },
                    right: Some(SavedPaneState {
                        location: Location::local(&stale_dir),
                        view_mode: ViewMode::Compact,
                        sort: SortSpec::default(),
                        show_hidden: false,
                    }),
                },
                SavedTabState {
                    split_enabled: false,
                    active_pane: PaneSide::Left,
                    left: SavedPaneState {
                        location: Location::Search {
                            query: "ext:rs".to_string(),
                            scope: SearchScope::Global,
                        },
                        view_mode: ViewMode::Details,
                        sort: SortSpec::default(),
                        show_hidden: false,
                    },
                    right: None,
                },
            ],
            window: Some(WindowGeometry {
                x: 120.0,
                y: 80.0,
                width: 1440.0,
                height: 900.0,
                maximized: false,
                display_uuid: None,
            }),
        };

        let json = serde_json::to_string(&session).unwrap();
        let parsed = serde_json::from_str::<AppSession>(&json).unwrap().sanitize();
        assert_eq!(parsed.active_tab_index, 1);
        assert_eq!(parsed.tabs[0].left.view_mode, ViewMode::Grid);
        // Stale right path falls back cleanly to its existing parent (`existing_dir`).
        assert_eq!(
            parsed.tabs[0].right.as_ref().unwrap().location,
            Location::local(&existing_dir)
        );
        // Ephemeral search tab sanitizes back to Home.
        assert_eq!(parsed.tabs[1].left.location, Location::Home);
        assert!(parsed.window.unwrap().is_valid());

        let bad_geom = WindowGeometry {
            x: 0.0,
            y: 0.0,
            width: 100.0,
            height: 100.0,
            maximized: false,
            display_uuid: None,
        };
        assert!(!bad_geom.is_valid());
    }

    #[test]
    fn resolves_multi_monitor_restore_including_negative_secondary_and_disconnect() {
        let id_primary = Uuid::from_u128(1);
        let id_left = Uuid::from_u128(2);
        let primary = DisplayArea {
            uuid: Some(id_primary),
            is_primary: true,
            x: 0.0,
            y: 0.0,
            width: 1920.0,
            height: 1080.0,
            visible_x: 0.0,
            visible_y: 0.0,
            visible_width: 1920.0,
            visible_height: 1040.0,
        };
        let left_monitor = DisplayArea {
            uuid: Some(id_left),
            is_primary: false,
            x: -1920.0,
            y: 0.0,
            width: 1920.0,
            height: 1080.0,
            visible_x: -1920.0,
            visible_y: 0.0,
            visible_width: 1920.0,
            visible_height: 1040.0,
        };
        let dual = [primary, left_monitor];

        // Window closed on left secondary monitor (negative X), even without display_uuid saved.
        let on_left = WindowGeometry {
            x: -1700.0,
            y: 90.0,
            width: 1360.0,
            height: 860.0,
            maximized: false,
            display_uuid: None,
        };
        let (idx, restored) = resolve_window_restore(on_left, &dual).unwrap();
        assert_eq!(idx, 1);
        assert_eq!(restored.x, -1700.0);
        assert_eq!(restored.width, 1360.0);
        assert_eq!(restored.height, 860.0);

        // Maximized window shifted to left monitor whose normal rect still points to primary.
        let max_on_left = WindowGeometry {
            x: 180.0,
            y: 80.0,
            width: 1400.0,
            height: 860.0,
            maximized: true,
            display_uuid: Some(id_left),
        };
        let (idx_max, restored_max) = resolve_window_restore(max_on_left, &dual).unwrap();
        assert_eq!(idx_max, 1);
        assert!(restored_max.maximized);
        assert!(restored_max.x < 0.0);

        // Secondary monitor unplugged: re-centers on primary keeping user's 1360x860 size.
        let (idx_fb, restored_fb) = resolve_window_restore(on_left, &[primary]).unwrap();
        assert_eq!(idx_fb, 0);
        assert_eq!(restored_fb.width, 1360.0);
        assert_eq!(restored_fb.height, 860.0);
        assert_eq!(restored_fb.x, 280.0);
        assert_eq!(restored_fb.y, 90.0);
    }
}
