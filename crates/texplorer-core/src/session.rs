use std::path::PathBuf;

use serde::{Deserialize, Serialize};

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

/// Persisted desktop window bounds and maximized state.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WindowGeometry {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    #[serde(default)]
    pub maximized: bool,
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
        Location::Search { .. } => Location::Home,
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
        };
        assert!(!bad_geom.is_valid());
    }
}
