//! Read-only NTFS USN Change Journal reader & platform-independent delta model.
//!
//! Enables real-time incremental index updates (`create`, `rename`, `move`, `delete`, and
//! `size`/`mtime` updates on close) without re-scanning `$MFT`.

use crate::mft_raw::{apply_dos_flags, ntfs_filetime_to_unix_secs};
use crate::protocol::FLAG_IS_DIR;

pub const USN_REASON_DATA_OVERWRITE: u32 = 0x0000_0001;
pub const USN_REASON_DATA_EXTEND: u32 = 0x0000_0002;
pub const USN_REASON_DATA_TRUNCATION: u32 = 0x0000_0004;
pub const USN_REASON_FILE_CREATE: u32 = 0x0000_0100;
pub const USN_REASON_FILE_DELETE: u32 = 0x0000_0200;
pub const USN_REASON_RENAME_NEW_NAME: u32 = 0x0000_2000;
pub const USN_REASON_CLOSE: u32 = 0x8000_0000;

const DOS_DIRECTORY: u32 = 0x0010;

/// Metadata snapshot of an active NTFS USN Change Journal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UsnJournalInfo {
    pub journal_id: u64,
    pub next_usn: i64,
}

/// Incremental change event emitted by the USN Change Journal and applied to `VolumeIndex`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UsnDelta {
    Upsert {
        file_ref: u64,
        parent_ref: u64,
        name: String,
        size: Option<u64>,
        modified_secs: i64,
        flags: u16,
    },
    Delete {
        file_ref: u64,
    },
    UpdateMetadata {
        file_ref: u64,
        size: u64,
        modified_secs: i64,
    },
}

#[inline]
fn read_u16(buf: &[u8], off: usize) -> Option<u16> {
    let s = buf.get(off..off + 2)?;
    Some(u16::from_le_bytes([s[0], s[1]]))
}

#[inline]
fn read_u32(buf: &[u8], off: usize) -> Option<u32> {
    let s = buf.get(off..off + 4)?;
    Some(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

#[inline]
fn read_u64(buf: &[u8], off: usize) -> Option<u64> {
    let s = buf.get(off..off + 8)?;
    Some(u64::from_le_bytes([
        s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7],
    ]))
}

#[inline]
fn read_i64(buf: &[u8], off: usize) -> Option<i64> {
    let s = buf.get(off..off + 8)?;
    Some(i64::from_le_bytes([
        s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7],
    ]))
}

/// Decodes a raw `FSCTL_READ_USN_JOURNAL` response buffer (leading 8-byte `NextUsn` + `USN_RECORD_V2`/`V3` entries).
pub fn decode_usn_buffer(buf: &[u8]) -> Option<(i64, Vec<UsnDelta>)> {
    if buf.len() < 8 {
        return None;
    }
    let next_usn = read_i64(buf, 0)?;
    let mut deltas = Vec::new();
    let mut pos = 8usize;

    while pos + 8 <= buf.len() {
        let rec_len = read_u32(buf, pos)? as usize;
        if rec_len < 60 || pos + rec_len > buf.len() {
            break;
        }
        let rec = &buf[pos..pos + rec_len];
        pos += rec_len;

        let major = read_u16(rec, 4).unwrap_or(0);
        let parsed = if major == 2 && rec_len >= 60 {
            let file_ref = read_u64(rec, 8)? & 0x0000_FFFF_FFFF_FFFF;
            let parent_ref = read_u64(rec, 16)? & 0x0000_FFFF_FFFF_FFFF;
            let timestamp = read_u64(rec, 32).unwrap_or(0);
            let reason = read_u32(rec, 40).unwrap_or(0);
            let attrs = read_u32(rec, 52).unwrap_or(0);
            let name_len = read_u16(rec, 56).unwrap_or(0) as usize;
            let name_off = read_u16(rec, 58).unwrap_or(0) as usize;
            Some((
                file_ref, parent_ref, timestamp, reason, attrs, name_len, name_off,
            ))
        } else if major == 3 && rec_len >= 76 {
            let file_ref = read_u64(rec, 8)? & 0x0000_FFFF_FFFF_FFFF;
            let parent_ref = read_u64(rec, 24)? & 0x0000_FFFF_FFFF_FFFF;
            let timestamp = read_u64(rec, 48).unwrap_or(0);
            let reason = read_u32(rec, 56).unwrap_or(0);
            let attrs = read_u32(rec, 68).unwrap_or(0);
            let name_len = read_u16(rec, 72).unwrap_or(0) as usize;
            let name_off = read_u16(rec, 74).unwrap_or(0) as usize;
            Some((
                file_ref, parent_ref, timestamp, reason, attrs, name_len, name_off,
            ))
        } else {
            None
        };

        let Some((file_ref, parent_ref, timestamp, reason, attrs, name_len, name_off)) = parsed
        else {
            continue;
        };

        if file_ref < 16 {
            continue;
        }

        if (reason & USN_REASON_FILE_DELETE) != 0 {
            deltas.push(UsnDelta::Delete { file_ref });
            continue;
        }

        let is_upsert = (reason & (USN_REASON_FILE_CREATE | USN_REASON_RENAME_NEW_NAME)) != 0;
        let is_data_close = (reason & USN_REASON_CLOSE) != 0
            && (reason
                & (USN_REASON_DATA_EXTEND
                    | USN_REASON_DATA_OVERWRITE
                    | USN_REASON_DATA_TRUNCATION))
                != 0;

        if !is_upsert && !is_data_close {
            continue;
        }

        if name_off + name_len > rec.len() {
            continue;
        }
        let utf16: Vec<u16> = rec[name_off..name_off + name_len]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .collect();
        let name = String::from_utf16_lossy(&utf16);
        if name.is_empty() {
            continue;
        }

        let mut flags = 0u16;
        if (attrs & DOS_DIRECTORY) != 0 {
            flags |= FLAG_IS_DIR;
        }
        apply_dos_flags(attrs, &mut flags);
        let modified_secs = ntfs_filetime_to_unix_secs(timestamp);

        deltas.push(UsnDelta::Upsert {
            file_ref,
            parent_ref,
            name,
            size: None,
            modified_secs,
            flags,
        });
    }

    Some((next_usn, deltas))
}

/// Queries the current USN Change Journal metadata for NTFS `drive_letter`.
#[cfg(target_os = "windows")]
pub fn query_usn_journal(drive_letter: char) -> Result<UsnJournalInfo, String> {
    use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_READ, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::IO::DeviceIoControl;
    use windows_sys::Win32::System::Ioctl::{FSCTL_QUERY_USN_JOURNAL, USN_JOURNAL_DATA_V0};

    let upper = drive_letter.to_ascii_uppercase();
    let dev_path = format!(r"\\.\{upper}:");
    let wide_dev: Vec<u16> = dev_path.encode_utf16().chain(std::iter::once(0)).collect();

    let handle: HANDLE = unsafe {
        CreateFileW(
            wide_dev.as_ptr(),
            GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            0,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE || handle.is_null() {
        return Err(format!("Failed opening volume {upper}: for USN query."));
    }

    let mut data: USN_JOURNAL_DATA_V0 = unsafe { std::mem::zeroed() };
    let mut returned: u32 = 0;
    let ok = unsafe {
        DeviceIoControl(
            handle,
            FSCTL_QUERY_USN_JOURNAL,
            std::ptr::null(),
            0,
            (&mut data as *mut USN_JOURNAL_DATA_V0).cast(),
            std::mem::size_of::<USN_JOURNAL_DATA_V0>() as u32,
            &mut returned,
            std::ptr::null_mut(),
        )
    };
    unsafe {
        CloseHandle(handle);
    }

    if ok == 0 {
        return Err(format!("FSCTL_QUERY_USN_JOURNAL unavailable on {upper}:."));
    }
    Ok(UsnJournalInfo {
        journal_id: data.UsnJournalID,
        next_usn: data.NextUsn,
    })
}

/// Polls incremental USN Journal changes since `next_usn` (non-blocking read).
#[cfg(target_os = "windows")]
pub fn poll_usn_changes(
    drive_letter: char,
    journal_id: u64,
    next_usn: i64,
) -> Result<(i64, Vec<UsnDelta>), String> {
    use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_READ, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::IO::DeviceIoControl;
    use windows_sys::Win32::System::Ioctl::{FSCTL_READ_USN_JOURNAL, READ_USN_JOURNAL_DATA_V0};

    let upper = drive_letter.to_ascii_uppercase();
    let dev_path = format!(r"\\.\{upper}:");
    let wide_dev: Vec<u16> = dev_path.encode_utf16().chain(std::iter::once(0)).collect();

    let handle: HANDLE = unsafe {
        CreateFileW(
            wide_dev.as_ptr(),
            GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            0,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE || handle.is_null() {
        return Err(format!("Failed opening volume {upper}: for USN poll."));
    }

    let read_data = READ_USN_JOURNAL_DATA_V0 {
        StartUsn: next_usn,
        ReasonMask: USN_REASON_FILE_CREATE
            | USN_REASON_FILE_DELETE
            | USN_REASON_RENAME_NEW_NAME
            | USN_REASON_DATA_EXTEND
            | USN_REASON_DATA_OVERWRITE
            | USN_REASON_DATA_TRUNCATION
            | USN_REASON_CLOSE,
        ReturnOnlyOnClose: 0,
        Timeout: 0,
        BytesToWaitFor: 0,
        UsnJournalID: journal_id,
    };

    let mut buf = vec![0u8; 64 * 1024];
    let mut returned: u32 = 0;
    let ok = unsafe {
        DeviceIoControl(
            handle,
            FSCTL_READ_USN_JOURNAL,
            (&read_data as *const READ_USN_JOURNAL_DATA_V0).cast(),
            std::mem::size_of::<READ_USN_JOURNAL_DATA_V0>() as u32,
            buf.as_mut_ptr().cast(),
            buf.len() as u32,
            &mut returned,
            std::ptr::null_mut(),
        )
    };
    unsafe {
        CloseHandle(handle);
    }

    if ok == 0 {
        return Err(format!("FSCTL_READ_USN_JOURNAL failed on {upper}:."));
    }
    let slice = &buf[..(returned as usize).min(buf.len())];
    decode_usn_buffer(slice).ok_or_else(|| "Truncated USN buffer.".to_string())
}

#[cfg(not(target_os = "windows"))]
pub fn query_usn_journal(_drive_letter: char) -> Result<UsnJournalInfo, String> {
    Err("USN Change Journal is only supported on Windows.".to_string())
}

#[cfg(not(target_os = "windows"))]
pub fn poll_usn_changes(
    _drive_letter: char,
    _journal_id: u64,
    next_usn: i64,
) -> Result<(i64, Vec<UsnDelta>), String> {
    Ok((next_usn, Vec::new()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_usn_v2_record(
        file_ref: u64,
        parent_ref: u64,
        reason: u32,
        attrs: u32,
        name: &str,
    ) -> Vec<u8> {
        let utf16: Vec<u16> = name.encode_utf16().collect();
        let name_bytes = utf16.len() * 2;
        let rec_len = (60 + name_bytes + 7) & !7;
        let mut rec = vec![0u8; rec_len];
        rec[0..4].copy_from_slice(&(rec_len as u32).to_le_bytes());
        rec[4..6].copy_from_slice(&2u16.to_le_bytes()); // V2
        rec[8..16].copy_from_slice(&file_ref.to_le_bytes());
        rec[16..24].copy_from_slice(&parent_ref.to_le_bytes());
        rec[40..44].copy_from_slice(&reason.to_le_bytes());
        rec[52..56].copy_from_slice(&attrs.to_le_bytes());
        rec[56..58].copy_from_slice(&(name_bytes as u16).to_le_bytes());
        rec[58..60].copy_from_slice(&60u16.to_le_bytes());
        for (i, &u) in utf16.iter().enumerate() {
            rec[60 + i * 2..62 + i * 2].copy_from_slice(&u.to_le_bytes());
        }
        rec
    }

    #[test]
    fn decodes_synthetic_usn_v2_stream() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&9999i64.to_le_bytes());
        buf.extend_from_slice(&build_usn_v2_record(100, 5, USN_REASON_FILE_CREATE, 0, "hello.rs"));
        buf.extend_from_slice(&build_usn_v2_record(101, 5, USN_REASON_FILE_DELETE, 0, "old.rs"));

        let (next_usn, deltas) = decode_usn_buffer(&buf).unwrap();
        assert_eq!(next_usn, 9999);
        assert_eq!(deltas.len(), 2);
        assert!(matches!(
            &deltas[0],
            UsnDelta::Upsert { file_ref: 100, parent_ref: 5, name, .. } if name == "hello.rs"
        ));
        assert_eq!(deltas[1], UsnDelta::Delete { file_ref: 101 });
    }
}
