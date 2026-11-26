//! Native Windows OLE outbound file drag-and-drop (`SHDoDragDrop` + `SHCreateShellItemArrayFromIDLists`).
//!
//! Allows users to drag local files and folders directly out of TExplorer into external Windows
//! applications such as Google Chrome, Edge, Discord, Telegram, VS Code, Desktop, and Windows
//! Explorer with full native Shell formats (`CF_HDROP`, Shell IDList Array, drag icons, and OS cursors).

/// Result of a native outbound Windows OLE drag operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeDragOutcome {
    Copied,
    Moved,
    Linked,
    Cancelled,
}

#[cfg(target_os = "windows")]
mod imp {
    use std::path::{Path, PathBuf};

    use windows::Win32::Foundation::{
        BOOL, DRAGDROP_S_CANCEL, DRAGDROP_S_DROP, DRAGDROP_S_USEDEFAULTCURSORS, HWND, S_OK,
    };
    use windows::Win32::System::Com::IDataObject;
    use windows::Win32::System::Ole::{
        DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_LINK, DROPEFFECT_MOVE, IDropSource,
        IDropSource_Impl,
    };
    use windows::Win32::System::SystemServices::{MK_LBUTTON, MODIFIERKEYS_FLAGS};
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, ReleaseCapture, VK_LBUTTON,
    };
    use windows::Win32::UI::Shell::Common::ITEMIDLIST;
    use windows::Win32::UI::Shell::{
        BHID_DataObject, ILCreateFromPathW, ILFree, SHCreateShellItemArrayFromIDLists,
        SHDoDragDrop,
    };
    use windows_core::{HRESULT, PCWSTR, implement};

    use super::NativeDragOutcome;

    #[implement(IDropSource)]
    struct NativeDropSource;

    #[allow(non_snake_case)]
    impl IDropSource_Impl for NativeDropSource_Impl {
        fn QueryContinueDrag(
            &self,
            fescapepressed: BOOL,
            grfkeystate: MODIFIERKEYS_FLAGS,
        ) -> HRESULT {
            if fescapepressed.as_bool() {
                return DRAGDROP_S_CANCEL;
            }
            if (grfkeystate.0 & MK_LBUTTON.0) == 0 {
                return DRAGDROP_S_DROP;
            }
            S_OK
        }

        fn GiveFeedback(&self, _dweffect: DROPEFFECT) -> HRESULT {
            DRAGDROP_S_USEDEFAULTCURSORS
        }
    }

    struct PidlList(Vec<*mut ITEMIDLIST>);

    impl Drop for PidlList {
        fn drop(&mut self) {
            for pidl in self.0.drain(..) {
                if !pidl.is_null() {
                    unsafe {
                        ILFree(Some(pidl as *const ITEMIDLIST));
                    }
                }
            }
        }
    }

    fn normalize_shell_path(path: &Path) -> String {
        let raw = path.to_string_lossy();
        if let Some(unc) = raw.strip_prefix(r"\\?\UNC\") {
            format!(r"\\{unc}")
        } else if let Some(stripped) = raw.strip_prefix(r"\\?\") {
            stripped.to_string()
        } else {
            raw.into_owned()
        }
    }

    /// Starts a native Windows OLE Shell drag operation for `paths` on the calling STA UI thread.
    ///
    /// Must be invoked when GPUI's `App` is NOT currently borrowed (for example from a foreground
    /// async task dispatched on the main thread) because `SHDoDragDrop` runs a nested Win32 modal
    /// message loop until the user drops or presses `Esc`.
    pub fn start_native_file_drag(
        hwnd_raw: isize,
        paths: &[PathBuf],
    ) -> Result<NativeDragOutcome, String> {
        if paths.is_empty() {
            return Ok(NativeDragOutcome::Cancelled);
        }

        // Bail cleanly if the user already released the left mouse button before the task ran.
        let lbutton_down = unsafe { GetAsyncKeyState(VK_LBUTTON.0 as i32) < 0 };
        if !lbutton_down {
            return Ok(NativeDragOutcome::Cancelled);
        }

        let mut pidls = PidlList(Vec::with_capacity(paths.len()));
        for path in paths {
            if !path.exists() {
                continue;
            }
            let normalized = normalize_shell_path(path);
            let wide: Vec<u16> = normalized
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();
            let pidl = unsafe { ILCreateFromPathW(PCWSTR(wide.as_ptr())) };
            if !pidl.is_null() {
                pidls.0.push(pidl);
            }
        }

        if pidls.0.is_empty() {
            return Ok(NativeDragOutcome::Cancelled);
        }

        let const_pidls: Vec<*const ITEMIDLIST> = pidls
            .0
            .iter()
            .map(|&p| p as *const ITEMIDLIST)
            .collect();
        let shell_array = unsafe { SHCreateShellItemArrayFromIDLists(&const_pidls) }
            .map_err(|e| format!("Failed creating Windows Shell item array for drag: {e}"))?;
        let data_object: IDataObject = unsafe { shell_array.BindToHandler(None, &BHID_DataObject) }
            .map_err(|e| format!("Failed binding Windows Shell IDataObject for drag: {e}"))?;

        let drop_source: IDropSource = NativeDropSource.into();
        let hwnd = HWND(hwnd_raw as *mut std::ffi::c_void);

        // Release GPUI's window mouse capture so OLE's modal drag tracker acquires capture cleanly.
        unsafe {
            let _ = ReleaseCapture();
        }

        let allowed = DROPEFFECT_COPY | DROPEFFECT_MOVE | DROPEFFECT_LINK;
        let effect = unsafe { SHDoDragDrop(hwnd, &data_object, &drop_source, allowed) }
            .map_err(|e| format!("Windows OLE drag-and-drop failed: {e}"))?;

        let outcome = if (effect.0 & DROPEFFECT_MOVE.0) != 0 {
            NativeDragOutcome::Moved
        } else if (effect.0 & DROPEFFECT_COPY.0) != 0 {
            NativeDragOutcome::Copied
        } else if (effect.0 & DROPEFFECT_LINK.0) != 0 {
            NativeDragOutcome::Linked
        } else {
            NativeDragOutcome::Cancelled
        };
        Ok(outcome)
    }
}

#[cfg(not(target_os = "windows"))]
mod imp {
    use std::path::PathBuf;

    use super::NativeDragOutcome;

    pub fn start_native_file_drag(
        _hwnd_raw: isize,
        _paths: &[PathBuf],
    ) -> Result<NativeDragOutcome, String> {
        Ok(NativeDragOutcome::Cancelled)
    }
}

pub use imp::start_native_file_drag;
