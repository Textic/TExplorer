//! Pure-Rust read-only NTFS `$MFT` parser, live USN Change Journal updater,
//! compact in-RAM multi-volume index, Everything-style query engine, and IPC client.

pub mod client;
pub mod index;
pub mod mft_raw;
pub mod protocol;
pub mod query;
pub mod usn;

pub use client::{
    IndexServiceInstallState, export_volume_tree_via_service, find_service_exe,
    get_folder_sizes_via_service, install_service_elevated, ping_service,
    query_live_service_status, query_service_install_state, rebuild_index_via_service,
    search_via_service, start_service_elevated, stop_service_elevated, uninstall_service_elevated,
};
pub use index::{DirStats, MftNode, MultiVolumeIndex, VolumeIndex};
pub use protocol::{
    ExportedMftNode, FLAG_IS_COMPRESSED, FLAG_IS_DIR, FLAG_IS_HIDDEN, FLAG_IS_READONLY,
    FLAG_IS_SYMLINK, FLAG_IS_SYSTEM, FRAME_TREE_CHUNK, FRAME_TREE_DONE, FolderSizeEntry,
    IndexRequest, IndexResponse, IndexServiceStatus, IndexedVolumeStatus, PIPE_NAME,
    PROTOCOL_VERSION, SERVICE_DESCRIPTION, SERVICE_DISPLAY_NAME, SERVICE_NAME, SearchHit,
    SearchResponse, decode_tree_chunk, encode_tree_chunk,
};
pub use query::{ParsedQuery, QueryCandidate, SizeComparison, parse_query};
pub use usn::{UsnDelta, UsnJournalInfo};
