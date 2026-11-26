//! Native Windows OLE `IDropTarget` with full WinRAR / 7-Zip / Built-in ZIP support.
//!
//! GPUI's default Windows drag-and-drop handler calls `IDataObject::GetData(CF_HDROP)` inside
//! `DragEnter` (before the user drops) and never queries `pdataobj` in `Drop`. Archive managers
//! like WinRAR and 7-Zip rely on delayed extraction (`GetData` invoked only inside `Drop`) and/or
//! virtual shell streams (`FileGroupDescriptorW` + `FileContents`).
//!
//! Replacing the window's `IDropTarget` with this implementation:
//! 1. Only calls `QueryGetData` in `DragEnter` (zero premature extraction).
//! 2. Reads `CF_HDROP` inside `Drop` and immediately preserves transient archiver `%TEMP%` folders
//!    (e.g. `Rar$DR...`, `7zO...`) using O(1) NTFS hard links (with copy fallback) before `Drop`
//!    returns so WinRAR's immediate post-drop cleanup cannot delete files mid-transfer.
//! 3. Falls back to `FileGroupDescriptorW` + `FileContents` (`IStream` / `HGLOBAL`) for virtual
//!    compressed folders (Windows Explorer `.zip`, WinRAR, 7-Zip, Outlook) with strict path
//!    sanitization against traversal attacks.

use std::path::{Component, Path, PathBuf};

/// Native external OLE drop events delivered to the application UI.
#[derive(Debug, Clone)]
pub enum NativeDropEvent {
    Entered {
        client_x: f32,
        client_y: f32,
    },
    Over {
        client_x: f32,
        client_y: f32,
    },
    Left,
    Dropped {
        client_x: f32,
        client_y: f32,
        paths: Vec<PathBuf>,
        /// Temporary staging folder owned by TExplorer that should be deleted after transfer finishes.
        stage_dir: Option<PathBuf>,
    },
}

/// Strictly sanitizes a relative path received from `FILEDESCRIPTORW::cFileName`.
/// Rejects `..`, root/drive prefixes, empty segments, NUL bytes, and illegal Windows characters.
pub fn sanitize_relative_drop_path(raw: &str) -> Option<PathBuf> {
    let trimmed = raw.trim().trim_matches(['/', '\\']);
    if trimmed.is_empty() || trimmed.contains('\0') {
        return None;
    }
    let normalized = trimmed.replace('\\', "/");
    let mut out = PathBuf::new();
    for seg in normalized.split('/') {
        if seg.is_empty() || seg == "." {
            continue;
        }
        if seg == ".." || seg.contains(':') {
            return None;
        }
        if seg
            .chars()
            .any(|c| matches!(c, '<' | '>' | '"' | '|' | '?' | '*') || (c as u32) < 0x20)
        {
            return None;
        }
        let clean = seg.trim_end_matches([' ', '.']);
        if clean.is_empty() {
            return None;
        }
        out.push(clean);
    }
    if out.as_os_str().is_empty() {
        return None;
    }
    for comp in out.components() {
        if !matches!(comp, Component::Normal(_)) {
            return None;
        }
    }
    Some(out)
}

/// Returns `true` when `path` lives inside the OS temporary directory and appears to belong to an
/// archive manager (WinRAR, 7-Zip, WinZip, Bandizip, PeaZip) that deletes extracted temp folders
/// right after `IDropTarget::Drop` returns.
pub fn is_transient_archiver_temp_path(path: &Path) -> bool {
    let temp_root = std::env::temp_dir();
    let Ok(rel) = path.strip_prefix(&temp_root) else {
        return false;
    };
    let Some(first) = rel.components().next() else {
        return false;
    };
    let name = first.as_os_str().to_string_lossy().to_ascii_lowercase();
    name.starts_with("rar$")
        || name.starts_with("7zo")
        || name.starts_with("7ze")
        || name.starts_with("7zs")
        || name.starts_with("wz")
        || name.starts_with("bandizip")
        || name.starts_with("peazip")
}

#[cfg(target_os = "windows")]
mod imp {
    use std::cell::Cell;
    use std::fs;
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use windows::Win32::Foundation::{E_INVALIDARG, HWND, POINT, POINTL, S_OK};
    use windows::Win32::Graphics::Gdi::ScreenToClient;
    use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_DIRECTORY;
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, CoCreateInstance, DVASPECT_CONTENT, FORMATETC, IDataObject, IStream,
        STGMEDIUM, TYMED_HGLOBAL, TYMED_ISTREAM,
    };
    use windows::Win32::System::DataExchange::RegisterClipboardFormatW;
    use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
    use windows::Win32::System::Ole::{
        CF_HDROP, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_NONE, IDropTarget, IDropTarget_Impl,
        RegisterDragDrop, ReleaseStgMedium, RevokeDragDrop,
    };
    use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
    use windows::Win32::UI::Shell::{
        CLSID_DragDropHelper, DragQueryFileW, FD_ATTRIBUTES, FILEDESCRIPTORW, FILEGROUPDESCRIPTORW,
        HDROP, IDropTargetHelper,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        ChangeWindowMessageFilterEx, MSGFLT_ALLOW, WM_COPYDATA, WM_DROPFILES,
    };
    use windows_core::{Error, implement, w};

    use super::{NativeDropEvent, is_transient_archiver_temp_path, sanitize_relative_drop_path};

    const WM_COPYGLOBALDATA: u32 = 0x0049;
    static STAGE_SEQ: AtomicU64 = AtomicU64::new(1);

    #[implement(IDropTarget)]
    struct NativeDropTarget {
        hwnd: HWND,
        drop_target_helper: Option<IDropTargetHelper>,
        accepted: Cell<bool>,
        cf_file_descriptor_w: u16,
        cf_file_contents: u16,
        on_event: Arc<dyn Fn(NativeDropEvent) + Send + Sync>,
    }

    impl NativeDropTarget {
        fn query_supported(&self, data_obj: &IDataObject) -> bool {
            let hdrop_fmt = FORMATETC {
                cfFormat: CF_HDROP.0,
                ptd: std::ptr::null_mut(),
                dwAspect: DVASPECT_CONTENT.0,
                lindex: -1,
                tymed: TYMED_HGLOBAL.0 as u32,
            };
            if unsafe { data_obj.QueryGetData(&hdrop_fmt) } == S_OK {
                return true;
            }

            if self.cf_file_descriptor_w != 0 {
                let desc_fmt = FORMATETC {
                    cfFormat: self.cf_file_descriptor_w,
                    ptd: std::ptr::null_mut(),
                    dwAspect: DVASPECT_CONTENT.0,
                    lindex: -1,
                    tymed: TYMED_HGLOBAL.0 as u32,
                };
                if unsafe { data_obj.QueryGetData(&desc_fmt) } == S_OK {
                    return true;
                }
            }
            false
        }

        fn to_client_xy(&self, pt: &POINTL) -> (f32, f32) {
            let mut client = POINT { x: pt.x, y: pt.y };
            unsafe {
                let _ = ScreenToClient(self.hwnd, &mut client);
            }
            (client.x as f32, client.y as f32)
        }
    }

    #[allow(non_snake_case)]
    impl IDropTarget_Impl for NativeDropTarget_Impl {
        fn DragEnter(
            &self,
            pdataobj: Option<&IDataObject>,
            _grfkeystate: MODIFIERKEYS_FLAGS,
            pt: &POINTL,
            pdweffect: *mut DROPEFFECT,
        ) -> windows_core::Result<()> {
            let Some(data_obj) = pdataobj else {
                return Err(Error::from(E_INVALIDARG));
            };
            let can_accept = self.query_supported(data_obj);
            self.accepted.set(can_accept);
            let effect = if can_accept {
                DROPEFFECT_COPY
            } else {
                DROPEFFECT_NONE
            };
            if !pdweffect.is_null() {
                unsafe {
                    *pdweffect = effect;
                }
            }
            let screen_pt = POINT { x: pt.x, y: pt.y };
            if let Some(helper) = &self.drop_target_helper {
                unsafe {
                    let _ = helper.DragEnter(self.hwnd, data_obj, &screen_pt, effect);
                }
            }
            if can_accept {
                let (client_x, client_y) = self.to_client_xy(pt);
                (self.on_event)(NativeDropEvent::Entered { client_x, client_y });
            }
            Ok(())
        }

        fn DragOver(
            &self,
            _grfkeystate: MODIFIERKEYS_FLAGS,
            pt: &POINTL,
            pdweffect: *mut DROPEFFECT,
        ) -> windows_core::Result<()> {
            let can_accept = self.accepted.get();
            let effect = if can_accept {
                DROPEFFECT_COPY
            } else {
                DROPEFFECT_NONE
            };
            if !pdweffect.is_null() {
                unsafe {
                    *pdweffect = effect;
                }
            }
            let screen_pt = POINT { x: pt.x, y: pt.y };
            if let Some(helper) = &self.drop_target_helper {
                unsafe {
                    let _ = helper.DragOver(&screen_pt, effect);
                }
            }
            if can_accept {
                let (client_x, client_y) = self.to_client_xy(pt);
                (self.on_event)(NativeDropEvent::Over { client_x, client_y });
            }
            Ok(())
        }

        fn DragLeave(&self) -> windows_core::Result<()> {
            self.accepted.set(false);
            if let Some(helper) = &self.drop_target_helper {
                unsafe {
                    let _ = helper.DragLeave();
                }
            }
            (self.on_event)(NativeDropEvent::Left);
            Ok(())
        }

        fn Drop(
            &self,
            pdataobj: Option<&IDataObject>,
            _grfkeystate: MODIFIERKEYS_FLAGS,
            pt: &POINTL,
            pdweffect: *mut DROPEFFECT,
        ) -> windows_core::Result<()> {
            let was_accepted = self.accepted.replace(false);
            let Some(data_obj) = pdataobj else {
                return Err(Error::from(E_INVALIDARG));
            };
            let screen_pt = POINT { x: pt.x, y: pt.y };
            if !was_accepted {
                if !pdweffect.is_null() {
                    unsafe {
                        *pdweffect = DROPEFFECT_NONE;
                    }
                }
                if let Some(helper) = &self.drop_target_helper {
                    unsafe {
                        let _ = helper.Drop(data_obj, &screen_pt, DROPEFFECT_NONE);
                    }
                }
                (self.on_event)(NativeDropEvent::Left);
                return Ok(());
            }

            let extracted = extract_cf_hdrop(data_obj).or_else(|| {
                if self.cf_file_descriptor_w != 0 && self.cf_file_contents != 0 {
                    extract_virtual_descriptors(
                        data_obj,
                        self.cf_file_descriptor_w,
                        self.cf_file_contents,
                    )
                } else {
                    None
                }
            });

            let effect = if extracted.is_some() {
                DROPEFFECT_COPY
            } else {
                DROPEFFECT_NONE
            };
            if !pdweffect.is_null() {
                unsafe {
                    *pdweffect = effect;
                }
            }
            if let Some(helper) = &self.drop_target_helper {
                unsafe {
                    let _ = helper.Drop(data_obj, &screen_pt, effect);
                }
            }

            if let Some((paths, stage_dir)) = extracted {
                let (client_x, client_y) = self.to_client_xy(pt);
                (self.on_event)(NativeDropEvent::Dropped {
                    client_x,
                    client_y,
                    paths,
                    stage_dir,
                });
            } else {
                (self.on_event)(NativeDropEvent::Left);
            }
            Ok(())
        }
    }

    fn create_stage_dir() -> Option<PathBuf> {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let seq = STAGE_SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir()
            .join("TExplorer")
            .join("drop-stage")
            .join(format!("drop-{}-{stamp}-{seq}", std::process::id()));
        fs::create_dir_all(&dir).ok()?;
        Some(dir)
    }

    fn stage_entry_fast(src: &Path, dst: &Path) -> std::io::Result<()> {
        let meta = fs::symlink_metadata(src)?;
        if meta.is_dir() {
            fs::create_dir_all(dst)?;
            for entry in fs::read_dir(src)? {
                let entry = entry?;
                let child_src = entry.path();
                let child_dst = dst.join(entry.file_name());
                stage_entry_fast(&child_src, &child_dst)?;
            }
            Ok(())
        } else {
            if let Some(parent) = dst.parent() {
                fs::create_dir_all(parent)?;
            }
            // Instant zero-copy NTFS hard link inside %TEMP% so WinRAR's cleanup cannot delete it.
            if fs::hard_link(src, dst).is_ok() {
                return Ok(());
            }
            fs::copy(src, dst).map(|_| ())
        }
    }

    fn extract_cf_hdrop(data_obj: &IDataObject) -> Option<(Vec<PathBuf>, Option<PathBuf>)> {
        let fmt = FORMATETC {
            cfFormat: CF_HDROP.0,
            ptd: std::ptr::null_mut(),
            dwAspect: DVASPECT_CONTENT.0,
            lindex: -1,
            tymed: TYMED_HGLOBAL.0 as u32,
        };
        let mut medium: STGMEDIUM = unsafe { data_obj.GetData(&fmt).ok()? };
        let hglobal = unsafe { medium.u.hGlobal };
        if hglobal.is_invalid() {
            unsafe { ReleaseStgMedium(&mut medium) };
            return None;
        }

        let hdrop = HDROP(hglobal.0);
        let count = unsafe { DragQueryFileW(hdrop, 0xFFFF_FFFF, None) };
        let mut paths = Vec::with_capacity(count as usize);
        for i in 0..count {
            let len = unsafe { DragQueryFileW(hdrop, i, None) };
            if len == 0 {
                continue;
            }
            let mut buf = vec![0u16; (len as usize) + 1];
            let copied = unsafe { DragQueryFileW(hdrop, i, Some(&mut buf)) };
            if copied > 0 {
                let s = String::from_utf16_lossy(&buf[..copied as usize]);
                if !s.is_empty() {
                    paths.push(PathBuf::from(s));
                }
            }
        }
        unsafe { ReleaseStgMedium(&mut medium) };

        if paths.is_empty() {
            return None;
        }

        // If WinRAR / 7-Zip extracted these into a transient %TEMP% folder, immediately preserve
        // them before `Drop()` returns so post-drop cleanup does not race with our transfer queue.
        let needs_staging = paths.iter().any(|p| is_transient_archiver_temp_path(p));
        if needs_staging && let Some(stage_dir) = create_stage_dir() {
            let mut staged_paths = Vec::with_capacity(paths.len());
            for src in &paths {
                let Some(name) = src.file_name() else {
                    continue;
                };
                let dst = stage_dir.join(name);
                if stage_entry_fast(src, &dst).is_ok() {
                    staged_paths.push(dst);
                }
            }
            if !staged_paths.is_empty() {
                return Some((staged_paths, Some(stage_dir)));
            }
            let _ = fs::remove_dir_all(&stage_dir);
        }

        Some((paths, None))
    }

    fn extract_virtual_descriptors(
        data_obj: &IDataObject,
        cf_file_descriptor_w: u16,
        cf_file_contents: u16,
    ) -> Option<(Vec<PathBuf>, Option<PathBuf>)> {
        let desc_fmt = FORMATETC {
            cfFormat: cf_file_descriptor_w,
            ptd: std::ptr::null_mut(),
            dwAspect: DVASPECT_CONTENT.0,
            lindex: -1,
            tymed: TYMED_HGLOBAL.0 as u32,
        };
        let mut desc_medium = unsafe { data_obj.GetData(&desc_fmt).ok()? };
        let hglobal = unsafe { desc_medium.u.hGlobal };
        if hglobal.is_invalid() {
            unsafe { ReleaseStgMedium(&mut desc_medium) };
            return None;
        }

        let ptr = unsafe { GlobalLock(hglobal) };
        if ptr.is_null() {
            unsafe { ReleaseStgMedium(&mut desc_medium) };
            return None;
        }

        let mut descriptors: Vec<(PathBuf, bool)> = Vec::new();
        unsafe {
            let total_bytes = GlobalSize(hglobal);
            if total_bytes >= std::mem::size_of::<u32>() {
                let group = ptr as *const FILEGROUPDESCRIPTORW;
                let count = std::ptr::addr_of!((*group).cItems).read_unaligned() as usize;
                let fd_base = std::ptr::addr_of!((*group).fgd) as *const FILEDESCRIPTORW;
                let fd_size = std::mem::size_of::<FILEDESCRIPTORW>();
                let max_items = total_bytes
                    .saturating_sub(std::mem::size_of::<u32>())
                    / fd_size.max(1);
                let safe_count = count.min(max_items);

                for i in 0..safe_count {
                    let fd = fd_base.add(i).read_unaligned();
                    let c_file_name = fd.cFileName;
                    let nul = c_file_name
                        .iter()
                        .position(|&c| c == 0)
                        .unwrap_or(c_file_name.len());
                    let raw_name = String::from_utf16_lossy(&c_file_name[..nul]);
                    let dw_flags = fd.dwFlags;
                    let dw_attrs = fd.dwFileAttributes;
                    let is_dir = (dw_flags & (FD_ATTRIBUTES.0 as u32)) != 0
                        && (dw_attrs & FILE_ATTRIBUTE_DIRECTORY.0) != 0;
                    if let Some(rel) = sanitize_relative_drop_path(&raw_name) {
                        descriptors.push((rel, is_dir));
                    } else {
                        // Preserve index alignment for `lindex` even if an entry is skipped.
                        descriptors.push((PathBuf::new(), true));
                    }
                }
            }
            let _ = GlobalUnlock(hglobal);
            ReleaseStgMedium(&mut desc_medium);
        }

        if descriptors.iter().all(|(p, _)| p.as_os_str().is_empty()) {
            return None;
        }

        let stage_dir = create_stage_dir()?;
        let mut materialized_any = false;
        for (idx, (rel, is_dir)) in descriptors.iter().enumerate() {
            if rel.as_os_str().is_empty() {
                continue;
            }
            let full_dest = stage_dir.join(rel);
            if *is_dir {
                if fs::create_dir_all(&full_dest).is_ok() {
                    materialized_any = true;
                }
                continue;
            }

            if let Some(parent) = full_dest.parent() {
                let _ = fs::create_dir_all(parent);
            }
            let contents_fmt = FORMATETC {
                cfFormat: cf_file_contents,
                ptd: std::ptr::null_mut(),
                dwAspect: DVASPECT_CONTENT.0,
                lindex: idx as i32,
                tymed: (TYMED_ISTREAM.0 | TYMED_HGLOBAL.0) as u32,
            };
            let Ok(mut content_med) = (unsafe { data_obj.GetData(&contents_fmt) }) else {
                // Some shell namespaces omit `FILE_ATTRIBUTE_DIRECTORY` on folder descriptors
                // and refuse `CFSTR_FILECONTENTS` for them.
                continue;
            };
            let wrote = write_stg_medium_to_file(&content_med, &full_dest);
            unsafe { ReleaseStgMedium(&mut content_med) };
            if wrote {
                materialized_any = true;
            }
        }

        if !materialized_any {
            let _ = fs::remove_dir_all(&stage_dir);
            return None;
        }

        let mut top_level = Vec::new();
        if let Ok(rd) = fs::read_dir(&stage_dir) {
            for entry in rd.flatten() {
                top_level.push(entry.path());
            }
        }
        if top_level.is_empty() {
            let _ = fs::remove_dir_all(&stage_dir);
            return None;
        }
        Some((top_level, Some(stage_dir)))
    }

    fn write_stg_medium_to_file(medium: &STGMEDIUM, dest: &Path) -> bool {
        let Ok(mut file) = fs::File::create(dest) else {
            return false;
        };
        if medium.tymed == TYMED_ISTREAM.0 as u32 {
            let stream_opt: Option<IStream> = unsafe {
                medium
                    .u
                    .pstm
                    .as_ref()
                    .cloned()
            };
            let Some(stream) = stream_opt else {
                return false;
            };
            let mut buf = [0u8; 64 * 1024];
            loop {
                let mut read_bytes = 0u32;
                let hr = unsafe {
                    stream.Read(
                        buf.as_mut_ptr().cast(),
                        buf.len() as u32,
                        Some(&mut read_bytes),
                    )
                };
                if read_bytes > 0
                    && file.write_all(&buf[..read_bytes as usize]).is_err()
                {
                    return false;
                }
                if hr.is_err() || read_bytes == 0 {
                    break;
                }
            }
            true
        } else if medium.tymed == TYMED_HGLOBAL.0 as u32 {
            let hglobal = unsafe { medium.u.hGlobal };
            if hglobal.is_invalid() {
                return false;
            }
            let ptr = unsafe { GlobalLock(hglobal) };
            if ptr.is_null() {
                return false;
            }
            let len = unsafe { GlobalSize(hglobal) };
            let slice = unsafe { std::slice::from_raw_parts(ptr as *const u8, len) };
            let ok = file.write_all(slice).is_ok();
            unsafe {
                let _ = GlobalUnlock(hglobal);
            }
            ok
        } else {
            false
        }
    }

    pub fn install_native_drop_target<F>(hwnd_raw: isize, on_event: F) -> Result<(), String>
    where
        F: Fn(NativeDropEvent) + Send + Sync + 'static,
    {
        if hwnd_raw == 0 {
            return Err("Invalid HWND for native drop target.".to_string());
        }
        let hwnd = HWND(hwnd_raw as *mut core::ffi::c_void);
        unsafe {
            let _ = ChangeWindowMessageFilterEx(hwnd, WM_DROPFILES, MSGFLT_ALLOW, None);
            let _ = ChangeWindowMessageFilterEx(hwnd, WM_COPYDATA, MSGFLT_ALLOW, None);
            let _ = ChangeWindowMessageFilterEx(hwnd, WM_COPYGLOBALDATA, MSGFLT_ALLOW, None);
        }

        let helper: Option<IDropTargetHelper> = unsafe {
            CoCreateInstance(&CLSID_DragDropHelper, None, CLSCTX_INPROC_SERVER).ok()
        };
        let cf_file_descriptor_w =
            unsafe { RegisterClipboardFormatW(w!("FileGroupDescriptorW")) as u16 };
        let cf_file_contents = unsafe { RegisterClipboardFormatW(w!("FileContents")) as u16 };

        let target: IDropTarget = NativeDropTarget {
            hwnd,
            drop_target_helper: helper,
            accepted: Cell::new(false),
            cf_file_descriptor_w,
            cf_file_contents,
            on_event: Arc::new(on_event),
        }
        .into();

        unsafe {
            let _ = RevokeDragDrop(hwnd);
            RegisterDragDrop(hwnd, &target)
                .map_err(|e| format!("RegisterDragDrop failed: {e}"))?;
        }
        Ok(())
    }
}

#[cfg(not(target_os = "windows"))]
mod imp {
    use super::NativeDropEvent;

    pub fn install_native_drop_target<F>(_hwnd_raw: isize, _on_event: F) -> Result<(), String>
    where
        F: Fn(NativeDropEvent) + Send + Sync + 'static,
    {
        Ok(())
    }
}

pub use imp::install_native_drop_target;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_virtual_drop_paths_strictly() {
        assert_eq!(
            sanitize_relative_drop_path(r"folder\sub\file.txt"),
            Some(PathBuf::from("folder").join("sub").join("file.txt"))
        );
        assert_eq!(sanitize_relative_drop_path(r"..\evil.exe"), None);
        assert_eq!(sanitize_relative_drop_path(r"a\..\..\evil.exe"), None);
        assert_eq!(sanitize_relative_drop_path(r"C:\Windows\system.ini"), None);
        assert_eq!(sanitize_relative_drop_path(""), None);
    }

    #[test]
    fn detects_transient_archiver_temp_folders() {
        let temp = std::env::temp_dir();
        assert!(is_transient_archiver_temp_path(
            &temp.join("Rar$DRa1234.5678").join("MyFolder")
        ));
        assert!(is_transient_archiver_temp_path(
            &temp.join("7zO9F12.tmp").join("ArchiveDir")
        ));
        assert!(!is_transient_archiver_temp_path(Path::new(
            r"C:\Users\danie\Downloads\MyFolder"
        )));
    }
}
