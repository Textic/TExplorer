//! Platform-agnostic domain models for T Explorer.

pub mod entry;
pub mod location;
pub mod session;
pub mod settings;
pub mod workspace_model;

pub use entry::{
    DriveKind, EntryFlags, FileEntry, FileKind, KnownFolder, KnownFolderKind, SortColumn, SortSpec,
    ViewMode, VolumeInfo, classify_extension, sort_entries,
};
pub use location::{BreadcrumbSegment, Location, SearchScope};
pub use session::{AppSession, SavedPaneState, SavedTabState, WindowGeometry};
pub use settings::{AccentPreset, AppSettings, SidebarSection, SizeUnit, ThemePreset};
pub use workspace_model::{PaneModel, PaneSide, SelectionSet, TabModel};
