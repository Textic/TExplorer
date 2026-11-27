//! Disk Space Analyzer engine for TExplorer.
//!
//! Provides an arena-backed `DiskTree` with instant bottom-up size rollups and soft-delete,
//! a squarified `TreemapLayout` engine, and a hybrid scanner combining `TExplorerIndexService`
//! binary MFT streaming, direct elevated `$MFT` parsing, and parallel Win32 directory walking.

pub mod scanner;
pub mod tree;
pub mod treemap;

pub use scanner::{
    DiskScanEvent, DiskScanProgress, DiskScanTarget, cluster_size_for_path, spawn_disk_scan,
};
pub use tree::{
    DiskSortColumn, DiskSortSpec, DiskTree, ExtInterner, ExtensionStat, FileNode, NodeFlags,
    NodeId, ScanEngineKind, VisibleRow,
};
pub use treemap::{
    TREEMAP_LEGEND_ITEMS, TreemapHeader, TreemapLayout, TreemapRect, color_for_extension,
    color_for_node,
};
