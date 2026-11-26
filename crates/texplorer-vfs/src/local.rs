use std::path::Path;

use texplorer_core::{
    EntryFlags, FileEntry, Location, classify_extension, location::normalize_local_path,
};

/// Fast local directory enumerator using `FindFirstFileExW` (`FIND_FIRST_EX_LARGE_FETCH`
/// + `FindExInfoBasic`) on Windows for high throughput on large folders.
#[cfg(target_os = "windows")]
pub fn list_local_dir(dir: &Path) -> Result<Vec<FileEntry>, String> {
    use std::ffi::OsString;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use windows_sys::Win32::Foundation::{
        ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND, ERROR_NO_MORE_FILES, ERROR_PATH_NOT_FOUND,
        GetLastError, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_ARCHIVE, FILE_ATTRIBUTE_COMPRESSED, FILE_ATTRIBUTE_DIRECTORY,
        FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_OFFLINE, FILE_ATTRIBUTE_READONLY,
        FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS, FILE_ATTRIBUTE_RECALL_ON_OPEN,
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_SYSTEM, FIND_FIRST_EX_LARGE_FETCH,
        FindClose, FindExInfoBasic, FindExSearchNameMatch, FindFirstFileExW, FindNextFileW,
        WIN32_FIND_DATAW,
    };

    // Normalize the parent directory once rather than re-normalizing every child path (100k+ files).
    let norm_dir = normalize_local_path(dir);
    let mut pattern_os = norm_dir.as_os_str().to_os_string();
    let dir_str = norm_dir.to_string_lossy();
    if !dir_str.ends_with('\\') && !dir_str.ends_with('/') {
        pattern_os.push("\\");
    }
    pattern_os.push("*");
    let wide_pattern: Vec<u16> = pattern_os
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    let mut data: WIN32_FIND_DATAW = unsafe { std::mem::zeroed() };
    let handle = unsafe {
        FindFirstFileExW(
            wide_pattern.as_ptr(),
            FindExInfoBasic,
            &mut data as *mut WIN32_FIND_DATAW as *mut core::ffi::c_void,
            FindExSearchNameMatch,
            std::ptr::null(),
            FIND_FIRST_EX_LARGE_FETCH,
        )
    };

    if handle == INVALID_HANDLE_VALUE {
        let err = unsafe { GetLastError() };
        return match err {
            ERROR_FILE_NOT_FOUND => Ok(Vec::new()),
            ERROR_ACCESS_DENIED => Err(format!("Access denied to \"{}\".", dir.display())),
            ERROR_PATH_NOT_FOUND => Err(format!("Path not found: \"{}\".", dir.display())),
            _ => Err(format!(
                "Failed to read directory \"{}\" (Win32 error {err}).",
                dir.display()
            )),
        };
    }

    struct FindGuard(*mut core::ffi::c_void);
    impl Drop for FindGuard {
        fn drop(&mut self) {
            unsafe {
                let _ = FindClose(self.0);
            }
        }
    }
    let _guard = FindGuard(handle);

    let mut entries = Vec::with_capacity(256);
    loop {
        let name_len = data
            .cFileName
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(data.cFileName.len());
        let name_slice = &data.cFileName[..name_len];
        let is_dot = name_slice == [b'.' as u16] || name_slice == [b'.' as u16, b'.' as u16];

        if !is_dot && !name_slice.is_empty() {
            let name_os = OsString::from_wide(name_slice);
            let name = name_os.to_string_lossy().into_owned();
            let attrs = data.dwFileAttributes;

            let mut flags = EntryFlags::empty();
            let is_dir = (attrs & FILE_ATTRIBUTE_DIRECTORY) != 0;
            let is_symlink = (attrs & FILE_ATTRIBUTE_REPARSE_POINT) != 0;
            if is_dir {
                flags |= EntryFlags::IS_DIR;
            }
            if (attrs & FILE_ATTRIBUTE_HIDDEN) != 0 {
                flags |= EntryFlags::IS_HIDDEN;
            }
            if (attrs & FILE_ATTRIBUTE_SYSTEM) != 0 {
                flags |= EntryFlags::IS_SYSTEM;
            }
            if (attrs & FILE_ATTRIBUTE_READONLY) != 0 {
                flags |= EntryFlags::IS_READONLY;
            }
            if is_symlink {
                flags |= EntryFlags::IS_SYMLINK;
            }
            if (attrs & FILE_ATTRIBUTE_ARCHIVE) != 0 {
                flags |= EntryFlags::IS_ARCHIVE;
            }
            if (attrs & FILE_ATTRIBUTE_COMPRESSED) != 0 {
                flags |= EntryFlags::IS_COMPRESSED;
            }
            if (attrs
                & (FILE_ATTRIBUTE_OFFLINE
                    | FILE_ATTRIBUTE_RECALL_ON_OPEN
                    | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS))
                != 0
            {
                flags |= EntryFlags::IS_CLOUD_ONLY;
            }

            let extension = if is_dir {
                String::new()
            } else {
                extract_extension(&name)
            };
            let (kind, type_label) = classify_extension(&extension, is_dir, is_symlink);
            let size = if is_dir {
                None
            } else {
                Some((u64::from(data.nFileSizeHigh) << 32) | u64::from(data.nFileSizeLow))
            };

            let modified_secs = filetime_to_unix_secs(data.ftLastWriteTime);
            let created_secs = filetime_to_unix_secs(data.ftCreationTime);

            entries.push(FileEntry {
                location: Location::Local(norm_dir.join(&name_os)),
                name,
                extension,
                size,
                modified_secs,
                created_secs,
                flags,
                kind,
                type_label,
            });
        }

        let has_next = unsafe { FindNextFileW(handle, &mut data) };
        if has_next == 0 {
            let err = unsafe { GetLastError() };
            if err != ERROR_NO_MORE_FILES && err != 0 {
                return Err(format!(
                    "Interrupted reading \"{}\" (Win32 error {err}).",
                    dir.display()
                ));
            }
            break;
        }
    }

    Ok(entries)
}

#[cfg(target_os = "windows")]
#[inline]
fn filetime_to_unix_secs(ft: windows_sys::Win32::Foundation::FILETIME) -> i64 {
    const WINDOWS_TO_UNIX_EPOCH_SECS: i64 = 11_644_473_600;
    let ticks = (u64::from(ft.dwHighDateTime) << 32) | u64::from(ft.dwLowDateTime);
    if ticks == 0 {
        return 0;
    }
    ((ticks / 10_000_000) as i64).saturating_sub(WINDOWS_TO_UNIX_EPOCH_SECS)
}

#[cfg(not(target_os = "windows"))]
pub fn list_local_dir(dir: &Path) -> Result<Vec<FileEntry>, String> {
    use std::time::UNIX_EPOCH;

    let read_dir = std::fs::read_dir(dir)
        .map_err(|e| format!("Failed to read directory \"{}\": {e}", dir.display()))?;
    let mut entries = Vec::new();

    for item in read_dir {
        let Ok(item) = item else { continue };
        let path = item.path();
        let name = item.file_name().to_string_lossy().into_owned();
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        let is_dir = meta.is_dir();
        let is_symlink = meta.file_type().is_symlink();
        let mut flags = EntryFlags::empty();
        if is_dir {
            flags |= EntryFlags::IS_DIR;
        }
        if is_symlink {
            flags |= EntryFlags::IS_SYMLINK;
        }
        if name.starts_with('.') {
            flags |= EntryFlags::IS_HIDDEN;
        }
        let extension = if is_dir {
            String::new()
        } else {
            extract_extension(&name)
        };
        let (kind, type_label) = classify_extension(&extension, is_dir, is_symlink);
        let modified_secs = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let created_secs = meta
            .created()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(modified_secs);

        entries.push(FileEntry {
            location: Location::local(path),
            name,
            extension,
            size: if is_dir { None } else { Some(meta.len()) },
            modified_secs,
            created_secs,
            flags,
            kind,
            type_label,
        });
    }

    Ok(entries)
}

fn extract_extension(name: &str) -> String {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && !ext.is_empty() => ext.to_ascii_lowercase(),
        _ => String::new(),
    }
}
