//! Native Windows file and folder picker dialogs (`IFileOpenDialog`).
//!
//! Designed to be called from a background thread (`tokio::task::spawn_blocking`) so the modal
//! Win32 message loop never re-enters GPUI's main UI thread.

use std::path::{Path, PathBuf};

/// Opens a native folder selection dialog and returns the chosen directory (`Ok(None)` if canceled).
pub fn pick_folder_dialog(title: &str, initial_dir: Option<&Path>) -> Result<Option<PathBuf>, String> {
    #[cfg(target_os = "windows")]
    {
        imp::pick_folder(title, initial_dir)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (title, initial_dir);
        Err("Native folder picker is only supported on Windows.".to_string())
    }
}

/// Opens a native multi-file selection dialog and returns the chosen file paths (empty if canceled).
pub fn pick_files_dialog(title: &str, initial_dir: Option<&Path>) -> Result<Vec<PathBuf>, String> {
    #[cfg(target_os = "windows")]
    {
        imp::pick_files(title, initial_dir)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (title, initial_dir);
        Err("Native file picker is only supported on Windows.".to_string())
    }
}

#[cfg(target_os = "windows")]
mod imp {
    use std::path::{Path, PathBuf};

    use windows::Win32::Foundation::ERROR_CANCELLED;
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoCreateInstance,
        CoInitializeEx, CoTaskMemFree, CoUninitialize,
    };
    use windows::Win32::UI::Shell::{
        FOS_ALLOWMULTISELECT, FOS_FILEMUSTEXIST, FOS_FORCEFILESYSTEM, FOS_PATHMUSTEXIST,
        FOS_PICKFOLDERS, FileOpenDialog, IFileOpenDialog, IShellItem, SHCreateItemFromParsingName,
        SIGDN_FILESYSPATH,
    };
    use windows_core::{HSTRING, PCWSTR};

    struct ComStaGuard(bool);
    impl Drop for ComStaGuard {
        fn drop(&mut self) {
            if self.0 {
                unsafe {
                    CoUninitialize();
                }
            }
        }
    }

    fn init_com_sta() -> ComStaGuard {
        let ok = unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE).is_ok()
        };
        ComStaGuard(ok)
    }

    fn is_cancelled(err: &windows_core::Error) -> bool {
        err.code() == ERROR_CANCELLED.to_hresult()
    }

    fn shell_item_to_path(item: &IShellItem) -> Option<PathBuf> {
        let pwstr = unsafe { item.GetDisplayName(SIGDN_FILESYSPATH).ok()? };
        let result = unsafe { pwstr.to_string().ok() };
        unsafe {
            CoTaskMemFree(Some(pwstr.0.cast()));
        }
        result.filter(|s| !s.is_empty()).map(PathBuf::from)
    }

    fn apply_initial_dir(dialog: &IFileOpenDialog, initial_dir: Option<&Path>) {
        let Some(dir) = initial_dir.filter(|p| p.is_dir()) else {
            return;
        };
        let wide = HSTRING::from(dir.as_os_str());
        if let Ok(item) =
            unsafe { SHCreateItemFromParsingName::<_, _, IShellItem>(PCWSTR(wide.as_ptr()), None) }
        {
            unsafe {
                let _ = dialog.SetFolder(&item);
            }
        }
    }

    pub fn pick_folder(title: &str, initial_dir: Option<&Path>) -> Result<Option<PathBuf>, String> {
        let _guard = init_com_sta();
        let dialog: IFileOpenDialog =
            unsafe { CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER) }
                .map_err(|e| format!("Failed creating Windows folder dialog: {e}"))?;

        let opts = unsafe { dialog.GetOptions() }
            .map_err(|e| format!("Failed reading folder dialog options: {e}"))?;
        unsafe {
            dialog
                .SetOptions(opts | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST)
                .map_err(|e| format!("Failed configuring folder dialog: {e}"))?;
            let wide_title = HSTRING::from(title);
            let _ = dialog.SetTitle(PCWSTR(wide_title.as_ptr()));
        }
        apply_initial_dir(&dialog, initial_dir);

        if let Err(err) = unsafe { dialog.Show(None) } {
            if is_cancelled(&err) {
                return Ok(None);
            }
            return Err(format!("Folder selection dialog failed: {err}"));
        }

        let item = unsafe { dialog.GetResult() }
            .map_err(|e| format!("Failed reading selected folder: {e}"))?;
        Ok(shell_item_to_path(&item))
    }

    pub fn pick_files(title: &str, initial_dir: Option<&Path>) -> Result<Vec<PathBuf>, String> {
        let _guard = init_com_sta();
        let dialog: IFileOpenDialog =
            unsafe { CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER) }
                .map_err(|e| format!("Failed creating Windows file dialog: {e}"))?;

        let opts = unsafe { dialog.GetOptions() }
            .map_err(|e| format!("Failed reading file dialog options: {e}"))?;
        unsafe {
            dialog
                .SetOptions(opts | FOS_ALLOWMULTISELECT | FOS_FORCEFILESYSTEM | FOS_FILEMUSTEXIST)
                .map_err(|e| format!("Failed configuring file dialog: {e}"))?;
            let wide_title = HSTRING::from(title);
            let _ = dialog.SetTitle(PCWSTR(wide_title.as_ptr()));
        }
        apply_initial_dir(&dialog, initial_dir);

        if let Err(err) = unsafe { dialog.Show(None) } {
            if is_cancelled(&err) {
                return Ok(Vec::new());
            }
            return Err(format!("File selection dialog failed: {err}"));
        }

        let results = unsafe { dialog.GetResults() }
            .map_err(|e| format!("Failed reading selected files: {e}"))?;
        let count = unsafe { results.GetCount() }
            .map_err(|e| format!("Failed counting selected files: {e}"))?;
        let mut paths = Vec::with_capacity(count as usize);
        for i in 0..count {
            if let Ok(item) = unsafe { results.GetItemAt(i) }
                && let Some(path) = shell_item_to_path(&item)
            {
                paths.push(path);
            }
        }
        Ok(paths)
    }
}
