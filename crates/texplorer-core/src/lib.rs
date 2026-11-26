//! Platform-agnostic domain models for T Explorer.

pub mod entry;
pub mod location;
pub mod network;
pub mod session;
pub mod settings;
pub mod workspace_model;

pub use entry::{
    DriveKind, EntryFlags, FileEntry, FileKind, KnownFolder, KnownFolderKind, SortColumn, SortSpec,
    ViewMode, VolumeInfo, classify_extension, sort_entries,
};
pub use location::{BreadcrumbSegment, Location, SearchScope};
pub use network::{
    NetworkConnection, NetworkProtocol, ParsedNetworkUri, SshAuthMode, join_remote_path,
    normalize_remote_path, normalize_smb_share_path, parse_network_uri,
};
pub use session::{AppSession, SavedPaneState, SavedTabState, WindowGeometry};
pub use settings::{AccentPreset, AppSettings, SidebarSection, SizeUnit, ThemePreset};
pub use workspace_model::{PaneModel, PaneSide, SelectionSet, TabModel};
