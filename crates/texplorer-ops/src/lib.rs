//! Asynchronous file transfer queue, conflict resolution, and reversible Undo/Redo engine.

pub mod conflict;
pub mod queue;
pub mod undo;

pub use conflict::{ConflictDecision, ConflictPrompt, ConflictResolution, unique_keep_both_path};
pub use queue::{
    TransferControl, TransferEvent, TransferKind, TransferSnapshot, TransferStatus,
    spawn_transfer_job,
};
pub use undo::{UndoAction, UndoHistory};
