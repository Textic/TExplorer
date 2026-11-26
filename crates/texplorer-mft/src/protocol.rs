//! Shared read-only IPC wire protocol between `texplorer.exe` and `texplorer-service.exe`.
//!
//! Strictly read-only: zero delete, write, rename, or process-execution requests exist.

use std::io::{Read, Write};

use serde::{Deserialize, Serialize};

pub const SERVICE_NAME: &str = "TExplorerIndexService";
pub const SERVICE_DISPLAY_NAME: &str = "TExplorer Read-Only MFT & USN Index Service";
pub const SERVICE_DESCRIPTION: &str = "Provides read-only NTFS Master File Table indexing, live USN Change Journal updates, instant global search, and live folder sizes for TExplorer without UAC prompts.";
pub const PIPE_NAME: &str = r"\\.\pipe\TExplorerIndexV1";
pub const PROTOCOL_VERSION: u32 = 1;

// Compact bitflags aligned with `texplorer_core::EntryFlags` semantics.
pub const FLAG_IS_DIR: u16 = 1 << 0;
pub const FLAG_IS_HIDDEN: u16 = 1 << 1;
pub const FLAG_IS_SYSTEM: u16 = 1 << 2;
pub const FLAG_IS_READONLY: u16 = 1 << 3;
pub const FLAG_IS_SYMLINK: u16 = 1 << 4;
pub const FLAG_IS_COMPRESSED: u16 = 1 << 5;

pub const FRAME_JSON_REQ: u8 = 1;
pub const FRAME_JSON_RESP: u8 = 2;

const MAX_FRAME_BYTES: usize = 32 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum IndexRequest {
    Ping,
    Status,
    Search {
        query: String,
        scope_path: Option<String>,
        limit: usize,
    },
    GetFolderSizes {
        paths: Vec<String>,
    },
    RebuildIndex,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexedVolumeStatus {
    pub drive_letter: char,
    pub total_entries: u64,
    pub usn_active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct IndexServiceStatus {
    pub ready: bool,
    pub indexing: bool,
    pub total_entries: u64,
    pub volumes: Vec<IndexedVolumeStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchHit {
    pub name: String,
    pub full_path: String,
    pub size: u64,
    pub modified_secs: i64,
    pub flags: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SearchResponse {
    pub hits: Vec<SearchHit>,
    pub total_matches: usize,
    pub elapsed_ms: u64,
    pub from_mft: bool,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderSizeEntry {
    pub path: String,
    pub bytes: Option<u64>,
    pub files: u64,
    pub folders: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum IndexResponse {
    Pong {
        protocol_version: u32,
        service_version: String,
    },
    Status(IndexServiceStatus),
    SearchResults(SearchResponse),
    FolderSizes(Vec<FolderSizeEntry>),
    Ack,
    Error(String),
}

pub fn write_json_frame<W: Write, T: Serialize>(
    writer: &mut W,
    kind: u8,
    value: &T,
) -> Result<(), String> {
    let payload =
        serde_json::to_vec(value).map_err(|e| format!("Failed serializing IPC frame: {e}"))?;
    write_raw_frame(writer, kind, &payload)
}

pub fn write_raw_frame<W: Write>(writer: &mut W, kind: u8, payload: &[u8]) -> Result<(), String> {
    if payload.len() > MAX_FRAME_BYTES {
        return Err(format!(
            "IPC payload exceeded maximum limit ({} bytes).",
            payload.len()
        ));
    }
    let len = payload.len() as u32;
    let mut hdr = [0u8; 5];
    hdr[0] = kind;
    hdr[1..5].copy_from_slice(&len.to_le_bytes());
    writer
        .write_all(&hdr)
        .map_err(|e| format!("Failed writing IPC header: {e}"))?;
    writer
        .write_all(payload)
        .map_err(|e| format!("Failed writing IPC payload: {e}"))?;
    writer
        .flush()
        .map_err(|e| format!("Failed flushing IPC stream: {e}"))?;
    Ok(())
}

pub fn read_frame<R: Read>(reader: &mut R) -> Result<(u8, Vec<u8>), String> {
    let mut hdr = [0u8; 5];
    reader
        .read_exact(&mut hdr)
        .map_err(|e| format!("Failed reading IPC header: {e}"))?;
    let kind = hdr[0];
    let len = u32::from_le_bytes([hdr[1], hdr[2], hdr[3], hdr[4]]) as usize;
    if len > MAX_FRAME_BYTES {
        return Err(format!("IPC frame too large ({len} bytes)."));
    }
    let mut buf = vec![0u8; len];
    reader
        .read_exact(&mut buf)
        .map_err(|e| format!("Failed reading IPC payload: {e}"))?;
    Ok((kind, buf))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn round_trips_index_ipc_frames() {
        let mut wire = Vec::new();
        let req = IndexRequest::Search {
            query: "ext:rs size:>1kb".to_string(),
            scope_path: Some(r"X:\TExplorer".to_string()),
            limit: 200,
        };
        write_json_frame(&mut wire, FRAME_JSON_REQ, &req).unwrap();

        let resp = IndexResponse::SearchResults(SearchResponse {
            hits: vec![SearchHit {
                name: "main.rs".to_string(),
                full_path: r"X:\TExplorer\crates\texplorer-app\src\main.rs".to_string(),
                size: 4096,
                modified_secs: 1_700_000_000,
                flags: 0,
            }],
            total_matches: 1,
            elapsed_ms: 2,
            from_mft: true,
            truncated: false,
        });
        write_json_frame(&mut wire, FRAME_JSON_RESP, &resp).unwrap();

        let mut cur = Cursor::new(wire);
        let (k1, b1) = read_frame(&mut cur).unwrap();
        assert_eq!(k1, FRAME_JSON_REQ);
        assert_eq!(serde_json::from_slice::<IndexRequest>(&b1).unwrap(), req);

        let (k2, b2) = read_frame(&mut cur).unwrap();
        assert_eq!(k2, FRAME_JSON_RESP);
        assert_eq!(serde_json::from_slice::<IndexResponse>(&b2).unwrap(), resp);
    }
}
