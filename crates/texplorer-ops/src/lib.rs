//! Asynchronous file transfer queue, conflict resolution, Bulk Rename, and reversible Undo/Redo engine.

pub mod bulk_rename;
pub mod conflict;
pub mod queue;
pub mod undo;

pub use bulk_rename::{
    BulkRenamePreviewRow, BulkRenameSpec, RenameCasing, execute_bulk_rename, preview_bulk_rename,
};
pub use conflict::{ConflictDecision, ConflictPrompt, ConflictResolution, unique_keep_both_path};
pub use queue::{
    TransferControl, TransferEvent, TransferKind, TransferSnapshot, TransferStatus,
    spawn_transfer_job,
};
pub use undo::{UndoAction, UndoHistory};
