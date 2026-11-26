//! Native Windows `CF_HDROP` + `Preferred DropEffect` file clipboard interoperability.

use std::path::PathBuf;

/// File paths currently held on the OS clipboard together with Cut vs. Copy intent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardFiles {
    pub paths: Vec<PathBuf>,
    pub is_cut: bool,
}

#[cfg(target_os = "windows")]
const CF_HDROP_FORMAT: u32 = 15;
#[cfg(target_os = "windows")]
const DROPEFFECT_COPY: u32 = 1;
#[cfg(target_os = "windows")]
const DROPEFFECT_MOVE: u32 = 2;

#[cfg(target_os = "windows")]
#[repr(C)]
struct DropFilesHeader {
    p_files: u32,
    pt_x: i32,
    pt_y: i32,
    f_nc: i32,
    f_wide: i32,
}

/// Writes `paths` to the Windows clipboard as `CF_HDROP` plus `Preferred DropEffect`,
/// making Cut/Copy fully interoperable with Windows Explorer and all Windows apps.
#[cfg(target_os = "windows")]
pub fn set_clipboard_files(paths: &[PathBuf], is_cut: bool) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::GlobalFree;
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
    };
    use windows_sys::Win32::System::Memory::{GHND, GlobalAlloc, GlobalLock, GlobalUnlock};

    if paths.is_empty() {
        return Ok(());
    }

    let mut wide_paths: Vec<u16> = Vec::new();
    for p in paths {
        wide_paths.extend(p.as_os_str().encode_wide());
        wide_paths.push(0);
    }
    wide_paths.push(0); // Double-NUL terminator required by DROPFILES

    let header_size = std::mem::size_of::<DropFilesHeader>();
    let total_bytes = header_size + wide_paths.len() * 2;

    let hdrop_mem = unsafe { GlobalAlloc(GHND, total_bytes) };
    if hdrop_mem.is_null() {
        return Err("Failed allocating clipboard memory for CF_HDROP.".to_string());
    }

    unsafe {
        let ptr = GlobalLock(hdrop_mem) as *mut u8;
        if ptr.is_null() {
            GlobalFree(hdrop_mem);
            return Err("Failed locking CF_HDROP clipboard buffer.".to_string());
        }
        let header = DropFilesHeader {
            p_files: header_size as u32,
            pt_x: 0,
            pt_y: 0,
            f_nc: 0,
            f_wide: 1,
        };
        std::ptr::copy_nonoverlapping(
            &header as *const DropFilesHeader as *const u8,
            ptr,
            header_size,
        );
        std::ptr::copy_nonoverlapping(
            wide_paths.as_ptr() as *const u8,
            ptr.add(header_size),
            wide_paths.len() * 2,
        );
        GlobalUnlock(hdrop_mem);
    }

    let effect_mem = unsafe { GlobalAlloc(GHND, 4) };
    if effect_mem.is_null() {
        unsafe {
            GlobalFree(hdrop_mem);
        }
        return Err("Failed allocating clipboard memory for DropEffect.".to_string());
    }
    unsafe {
        let ptr = GlobalLock(effect_mem) as *mut u32;
        if !ptr.is_null() {
            *ptr = if is_cut {
                DROPEFFECT_MOVE
            } else {
                DROPEFFECT_COPY
            };
            GlobalUnlock(effect_mem);
        }
    }

    let opened = unsafe { OpenClipboard(std::ptr::null_mut()) };
    if opened == 0 {
        unsafe {
            GlobalFree(hdrop_mem);
            GlobalFree(effect_mem);
        }
        return Err("Could not open Windows clipboard (in use by another application).".to_string());
    }

    unsafe {
        EmptyClipboard();
        let set_drop = SetClipboardData(CF_HDROP_FORMAT, hdrop_mem);
        if set_drop.is_null() {
            GlobalFree(hdrop_mem);
            GlobalFree(effect_mem);
            CloseClipboard();
            return Err("Failed writing CF_HDROP data to Windows clipboard.".to_string());
        }

        let fmt_name: Vec<u16> = "Preferred DropEffect\0".encode_utf16().collect();
        let fmt_id = RegisterClipboardFormatW(fmt_name.as_ptr());
        if fmt_id != 0 {
            let set_eff = SetClipboardData(fmt_id, effect_mem);
            if set_eff.is_null() {
                GlobalFree(effect_mem);
            }
        } else {
            GlobalFree(effect_mem);
        }

        CloseClipboard();
    }

    Ok(())
}

#[cfg(not(target_os = "windows"))]
pub fn set_clipboard_files(_paths: &[PathBuf], _is_cut: bool) -> Result<(), String> {
    Err("Native CF_HDROP clipboard is only supported on Windows.".to_string())
}

/// Reads file paths and Cut/Copy intent currently placed on the Windows clipboard.
#[cfg(target_os = "windows")]
pub fn get_clipboard_files() -> Result<Option<ClipboardFiles>, String> {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
        RegisterClipboardFormatW,
    };
    use windows_sys::Win32::System::Memory::{GlobalLock, GlobalUnlock};
    use windows_sys::Win32::UI::Shell::DragQueryFileW;

    if unsafe { IsClipboardFormatAvailable(CF_HDROP_FORMAT) } == 0 {
        return Ok(None);
    }

    if unsafe { OpenClipboard(std::ptr::null_mut()) } == 0 {
        return Err("Could not open Windows clipboard.".to_string());
    }

    struct ClipboardGuard;
    impl Drop for ClipboardGuard {
        fn drop(&mut self) {
            unsafe {
                CloseClipboard();
            }
        }
    }
    let _guard = ClipboardGuard;

    let hdrop = unsafe { GetClipboardData(CF_HDROP_FORMAT) };
    if hdrop.is_null() {
        return Ok(None);
    }

    let count = unsafe { DragQueryFileW(hdrop, u32::MAX, std::ptr::null_mut(), 0) };
    let mut paths = Vec::with_capacity(count as usize);
    for idx in 0..count {
        let needed = unsafe { DragQueryFileW(hdrop, idx, std::ptr::null_mut(), 0) };
        if needed == 0 {
            continue;
        }
        let mut buf = vec![0u16; needed as usize + 1];
        let written = unsafe { DragQueryFileW(hdrop, idx, buf.as_mut_ptr(), buf.len() as u32) };
        if written > 0 {
            paths.push(PathBuf::from(OsString::from_wide(&buf[..written as usize])));
        }
    }

    if paths.is_empty() {
        return Ok(None);
    }

    let fmt_name: Vec<u16> = "Preferred DropEffect\0".encode_utf16().collect();
    let fmt_id = unsafe { RegisterClipboardFormatW(fmt_name.as_ptr()) };
    let mut is_cut = false;
    if fmt_id != 0 {
        let heff = unsafe { GetClipboardData(fmt_id) };
        if !heff.is_null() {
            unsafe {
                let ptr = GlobalLock(heff) as *const u32;
                if !ptr.is_null() {
                    is_cut = (*ptr & DROPEFFECT_MOVE) != 0;
                    GlobalUnlock(heff);
                }
            }
        }
    }

    Ok(Some(ClipboardFiles { paths, is_cut }))
}

#[cfg(not(target_os = "windows"))]
pub fn get_clipboard_files() -> Result<Option<ClipboardFiles>, String> {
    Ok(None)
}
