//! Native Windows UAC elevation fallback for protected filesystem paths (`C:\Program Files`,
//! `C:\Windows`, etc.) using Windows' official COM Elevation Moniker for `IFileOperation`.
//!
//! Keeps TExplorer running safely as a normal non-elevated desktop process (preserving UIPI
//! Drag & Drop with browsers/apps) while seamlessly showing the standard Windows UAC prompt
//! whenever a delete, copy, move, rename, or create operation hits `Access is denied (os error 5)`.

use std::path::{Path, PathBuf};

/// Outcome error for an elevated Windows `IFileOperation` request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ElevatedOpError {
    /// The user dismissed or declined the Windows UAC elevation prompt.
    Cancelled,
    /// The operation failed even after elevation (e.g. file locked or TrustedInstaller ACL).
    Failed(String),
}

impl ElevatedOpError {
    pub fn into_message(self) -> String {
        match self {
            Self::Cancelled => "Administrator permission was cancelled.".to_string(),
            Self::Failed(msg) => msg,
        }
    }
}

/// Normalizes a Windows path for `SHCreateItemFromParsingName` (strips `\\?\` extended prefixes
/// and normalizes forward slashes to backslashes).
pub fn normalize_shell_path(path: &Path) -> PathBuf {
    let raw = path.to_string_lossy();
    let stripped = if let Some(unc) = raw.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{unc}")
    } else if let Some(rest) = raw.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        raw.into_owned()
    };
    PathBuf::from(stripped.replace('/', "\\"))
}

/// Returns `true` when an error message from `std::fs` or the `trash` crate indicates Windows
/// `ERROR_ACCESS_DENIED` (`os error 5` / `0x80070005` / `0x80270001`).
pub fn is_access_denied_message(msg: &str) -> bool {
    let lower = msg.to_ascii_lowercase();
    lower.contains("access is denied")
        || lower.contains("permission denied")
        || lower.contains("os error 5")
        || lower.contains("error 5")
        || lower.contains("0x80070005")
        || lower.contains("0x80270001")
        || lower.contains("-2147024891")
        || lower.contains("e_accessdenied")
}

/// Deletes one or more local paths via an elevated Windows `IFileOperation` (single UAC prompt).
///
/// When `recycle` is `true`, items are sent to the Windows Recycle Bin (`FOFX_RECYCLEONDELETE`);
/// otherwise they are permanently deleted.
pub fn elevated_delete_paths(paths: &[PathBuf], recycle: bool) -> Result<(), ElevatedOpError> {
    if paths.is_empty() {
        return Ok(());
    }
    #[cfg(target_os = "windows")]
    {
        win_impl::elevated_delete_paths(paths, recycle)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (paths, recycle);
        Err(ElevatedOpError::Failed(
            "Elevated file operations are only supported on Windows.".to_string(),
        ))
    }
}

/// Copies (`is_move = false`) or moves (`is_move = true`) one or more `(source, final_dest)` pairs
/// in a single elevated Windows `IFileOperation` batch (one UAC prompt).
///
/// Overwrites existing targets without re-prompting because conflict resolution already ran in
/// TExplorer's transfer queue.
pub fn elevated_transfer_paths(
    is_move: bool,
    pairs: &[(PathBuf, PathBuf)],
) -> Result<(), ElevatedOpError> {
    if pairs.is_empty() {
        return Ok(());
    }
    #[cfg(target_os = "windows")]
    {
        win_impl::elevated_transfer_paths(is_move, pairs)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (is_move, pairs);
        Err(ElevatedOpError::Failed(
            "Elevated file operations are only supported on Windows.".to_string(),
        ))
    }
}

/// Renames `target` to `new_name` (leaf filename only) via an elevated Windows `IFileOperation`.
pub fn elevated_rename_item(target: &Path, new_name: &str) -> Result<PathBuf, ElevatedOpError> {
    let Some(parent) = target.parent() else {
        return Err(ElevatedOpError::Failed(
            "Cannot rename a root volume.".to_string(),
        ));
    };
    #[cfg(target_os = "windows")]
    {
        win_impl::elevated_rename_item(target, new_name)?;
        Ok(parent.join(new_name))
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (parent, new_name);
        Err(ElevatedOpError::Failed(
            "Elevated file operations are only supported on Windows.".to_string(),
        ))
    }
}

/// Creates a new folder or empty file inside `parent` via an elevated Windows `IFileOperation`.
pub fn elevated_new_item(
    parent: &Path,
    name: &str,
    is_dir: bool,
) -> Result<PathBuf, ElevatedOpError> {
    #[cfg(target_os = "windows")]
    {
        win_impl::elevated_new_item(parent, name, is_dir)?;
        Ok(parent.join(name))
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (parent, name, is_dir);
        Err(ElevatedOpError::Failed(
            "Elevated file operations are only supported on Windows.".to_string(),
        ))
    }
}

#[cfg(target_os = "windows")]
mod win_impl {
    use std::os::windows::ffi::OsStrExt;
    use std::path::{Path, PathBuf};

    use windows::Win32::Foundation::ERROR_CANCELLED;
    use windows::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL};
    use windows::Win32::System::Com::{
        BIND_OPTS, BIND_OPTS3, CLSCTX_LOCAL_SERVER, COINIT_APARTMENTTHREADED,
        COINIT_DISABLE_OLE1DDE, CoGetObject, CoInitializeEx, CoUninitialize,
    };
    use windows::Win32::UI::Shell::{
        FILEOPERATION_FLAGS, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOCONFIRMMKDIR, FOF_NOERRORUI,
        FOF_SILENT, FOFX_NOCOPYHOOKS, FOFX_RECYCLEONDELETE, IFileOperation,
        IFileOperationProgressSink, IShellItem, SHCreateItemFromParsingName,
    };
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
    use windows::core::{HRESULT, PCWSTR};

    use super::{ElevatedOpError, normalize_shell_path};

    const FILE_OPERATION_ELEVATION_MONIKER: &str =
        "Elevation:Administrator!new:{3ad05575-8857-4850-9277-11b85bdb8e09}";

    /// Shell CopyEngine user-cancelled HRESULT (`COPYENGINE_E_USER_CANCELLED`).
    const COPYENGINE_E_USER_CANCELLED: i32 = 0x8027_0000_u32 as i32;

    struct ComStaGuard(bool);

    impl ComStaGuard {
        fn enter() -> Self {
            let ok = unsafe {
                CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE).is_ok()
            };
            Self(ok)
        }
    }

    impl Drop for ComStaGuard {
        fn drop(&mut self) {
            if self.0 {
                unsafe {
                    CoUninitialize();
                }
            }
        }
    }

    fn is_cancelled_hresult(hr: HRESULT) -> bool {
        hr == HRESULT::from_win32(ERROR_CANCELLED.0) || hr.0 == COPYENGINE_E_USER_CANCELLED
    }

    fn map_com_error(context: &str, err: windows::core::Error) -> ElevatedOpError {
        if is_cancelled_hresult(err.code()) {
            ElevatedOpError::Cancelled
        } else {
            ElevatedOpError::Failed(format!("{context}: {err}"))
        }
    }

    fn to_wide_null(s: &std::ffi::OsStr) -> Vec<u16> {
        s.encode_wide().chain(std::iter::once(0)).collect()
    }

    fn shell_item_from_path(path: &Path) -> Result<IShellItem, ElevatedOpError> {
        let clean = normalize_shell_path(path);
        let wide = to_wide_null(clean.as_os_str());
        unsafe { SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None) }
            .map_err(|e| map_com_error(&format!("Failed resolving \"{}\"", clean.display()), e))
    }

    fn create_elevated_file_operation(
        flags: FILEOPERATION_FLAGS,
    ) -> Result<IFileOperation, ElevatedOpError> {
        let owner_hwnd = unsafe { GetForegroundWindow() };
        let mut opts = BIND_OPTS3::default();
        opts.Base.Base.cbStruct = std::mem::size_of::<BIND_OPTS3>() as u32;
        opts.Base.dwClassContext = CLSCTX_LOCAL_SERVER.0;
        opts.hwnd = owner_hwnd;

        let moniker: Vec<u16> = FILE_OPERATION_ELEVATION_MONIKER
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();

        let op: IFileOperation = unsafe {
            CoGetObject(
                PCWSTR(moniker.as_ptr()),
                Some(&opts as *const BIND_OPTS3 as *const BIND_OPTS),
            )
        }
        .map_err(|e| map_com_error("Administrator elevation failed", e))?;

        unsafe {
            if !owner_hwnd.is_invalid() {
                let _ = op.SetOwnerWindow(owner_hwnd);
            }
            op.SetOperationFlags(flags)
                .map_err(|e| map_com_error("Failed configuring elevated file operation", e))?;
        }
        Ok(op)
    }

    fn run_and_check(op: &IFileOperation, context: &str) -> Result<(), ElevatedOpError> {
        unsafe {
            op.PerformOperations()
                .map_err(|e| map_com_error(context, e))?;
            if op
                .GetAnyOperationsAborted()
                .is_ok_and(|aborted| aborted.as_bool())
            {
                return Err(ElevatedOpError::Cancelled);
            }
        }
        Ok(())
    }

    fn base_elevated_flags() -> FILEOPERATION_FLAGS {
        FOF_NOCONFIRMATION | FOF_NOCONFIRMMKDIR | FOF_NOERRORUI | FOF_SILENT | FOFX_NOCOPYHOOKS
    }

    pub fn elevated_delete_paths(paths: &[PathBuf], recycle: bool) -> Result<(), ElevatedOpError> {
        let _com = ComStaGuard::enter();
        let flags = if recycle {
            base_elevated_flags() | FOF_ALLOWUNDO | FOFX_RECYCLEONDELETE
        } else {
            base_elevated_flags()
        };
        let op = create_elevated_file_operation(flags)?;

        for path in paths {
            let item = shell_item_from_path(path)?;
            unsafe {
                op.DeleteItem(&item, None::<&IFileOperationProgressSink>)
                    .map_err(|e| {
                        map_com_error(&format!("Failed queueing delete for \"{}\"", path.display()), e)
                    })?;
            }
        }

        let verb = if recycle {
            "Elevated Recycle Bin delete failed"
        } else {
            "Elevated permanent delete failed"
        };
        run_and_check(&op, verb)
    }

    pub fn elevated_transfer_paths(
        is_move: bool,
        pairs: &[(PathBuf, PathBuf)],
    ) -> Result<(), ElevatedOpError> {
        let _com = ComStaGuard::enter();
        let op = create_elevated_file_operation(base_elevated_flags())?;

        for (src, final_dest) in pairs {
            let Some(dest_parent) = final_dest.parent() else {
                return Err(ElevatedOpError::Failed(format!(
                    "Destination \"{}\" has no parent folder.",
                    final_dest.display()
                )));
            };
            let Some(dest_name) = final_dest.file_name() else {
                return Err(ElevatedOpError::Failed(format!(
                    "Destination \"{}\" has no filename.",
                    final_dest.display()
                )));
            };

            let src_item = shell_item_from_path(src)?;
            let dest_dir_item = shell_item_from_path(dest_parent)?;
            let new_name_wide = to_wide_null(dest_name);

            unsafe {
                if is_move {
                    op.MoveItem(
                        &src_item,
                        &dest_dir_item,
                        PCWSTR(new_name_wide.as_ptr()),
                        None::<&IFileOperationProgressSink>,
                    )
                    .map_err(|e| {
                        map_com_error(&format!("Failed queueing move for \"{}\"", src.display()), e)
                    })?;
                } else {
                    op.CopyItem(
                        &src_item,
                        &dest_dir_item,
                        PCWSTR(new_name_wide.as_ptr()),
                        None::<&IFileOperationProgressSink>,
                    )
                    .map_err(|e| {
                        map_com_error(&format!("Failed queueing copy for \"{}\"", src.display()), e)
                    })?;
                }
            }
        }

        let verb = if is_move {
            "Elevated move failed"
        } else {
            "Elevated copy failed"
        };
        run_and_check(&op, verb)
    }

    pub fn elevated_rename_item(target: &Path, new_name: &str) -> Result<(), ElevatedOpError> {
        let _com = ComStaGuard::enter();
        let op = create_elevated_file_operation(base_elevated_flags())?;
        let item = shell_item_from_path(target)?;
        let wide_name: Vec<u16> = new_name.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe {
            op.RenameItem(
                &item,
                PCWSTR(wide_name.as_ptr()),
                None::<&IFileOperationProgressSink>,
            )
            .map_err(|e| {
                map_com_error(
                    &format!("Failed queueing rename for \"{}\"", target.display()),
                    e,
                )
            })?;
        }
        run_and_check(&op, "Elevated rename failed")
    }

    pub fn elevated_new_item(
        parent: &Path,
        name: &str,
        is_dir: bool,
    ) -> Result<(), ElevatedOpError> {
        let _com = ComStaGuard::enter();
        let op = create_elevated_file_operation(base_elevated_flags())?;
        let parent_item = shell_item_from_path(parent)?;
        let wide_name: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        let attrs = if is_dir {
            FILE_ATTRIBUTE_DIRECTORY.0
        } else {
            FILE_ATTRIBUTE_NORMAL.0
        };
        unsafe {
            op.NewItem(
                &parent_item,
                attrs,
                PCWSTR(wide_name.as_ptr()),
                PCWSTR::null(),
                None::<&IFileOperationProgressSink>,
            )
            .map_err(|e| map_com_error(&format!("Failed queueing creation of \"{name}\""), e))?;
        }
        let verb = if is_dir {
            "Elevated folder creation failed"
        } else {
            "Elevated file creation failed"
        };
        run_and_check(&op, verb)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_shell_paths_and_detects_access_denied_messages() {
        assert_eq!(
            normalize_shell_path(Path::new(r"\\?\C:\Program Files/App\file.dll")),
            PathBuf::from(r"C:\Program Files\App\file.dll")
        );
        assert_eq!(
            normalize_shell_path(Path::new(r"\\?\UNC\server\share/sub")),
            PathBuf::from(r"\\server\share\sub")
        );
        assert!(is_access_denied_message(
            "Failed to delete \"C:\\Program Files\\a.sig\": Access is denied. (os error 5)"
        ));
        assert!(is_access_denied_message("COM error 0x80070005"));
        assert!(is_access_denied_message("Os { code: -2147024891 }"));
        assert!(!is_access_denied_message("The system cannot find the file specified. (os error 2)"));
    }
}
