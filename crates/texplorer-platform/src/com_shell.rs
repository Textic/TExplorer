//! Dedicated COM STA worker thread for native Windows Shell thumbnails/icons
//! (`IShellItemImageFactory`) and on-demand Win32 Shell context menus (`IContextMenu`).

use std::path::PathBuf;
use std::sync::mpsc::{Sender, channel};
use std::thread;

/// Raw top-down 32-bit BGRA bitmap ready for GPU upload as a `gpui::RenderImage`.
#[derive(Debug, Clone)]
pub struct RawBgraImage {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

type ThumbCallback = Box<dyn FnOnce(Option<RawBgraImage>) + Send>;
type MenuCallback = Box<dyn FnOnce(Result<(), String>) + Send>;

enum ShellJob {
    Thumbnail {
        path: PathBuf,
        size_px: i32,
        on_ready: ThumbCallback,
    },
    NativeContextMenu {
        paths: Vec<PathBuf>,
        screen_x: i32,
        screen_y: i32,
        on_done: MenuCallback,
    },
    ShowProperties {
        path: PathBuf,
        on_done: MenuCallback,
    },
}

/// Handle to the background COM STA worker thread.
#[derive(Clone)]
pub struct ShellWorker {
    tx: Sender<ShellJob>,
}

impl ShellWorker {
    /// Spawns the dedicated COM STA worker thread.
    pub fn spawn() -> Self {
        let (tx, rx) = channel::<ShellJob>();
        let _ = thread::Builder::new()
            .name("texplorer-com-sta".to_string())
            .spawn(move || {
                #[cfg(target_os = "windows")]
                unsafe {
                    use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
                    let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
                }

                while let Ok(job) = rx.recv() {
                    match job {
                        ShellJob::Thumbnail {
                            path,
                            size_px,
                            on_ready,
                        } => {
                            let img = extract_shell_thumbnail(&path, size_px);
                            on_ready(img);
                        }
                        ShellJob::NativeContextMenu {
                            paths,
                            screen_x,
                            screen_y,
                            on_done,
                        } => {
                            let res = invoke_native_context_menu(&paths, screen_x, screen_y);
                            on_done(res);
                        }
                        ShellJob::ShowProperties { path, on_done } => {
                            let res = crate::launch::show_properties_dialog(&path);
                            on_done(res);
                        }
                    }
                }

                #[cfg(target_os = "windows")]
                unsafe {
                    windows::Win32::System::Com::CoUninitialize();
                }
            });

        Self { tx }
    }

    /// Queues an asynchronous native Shell thumbnail/icon lookup for `path`.
    pub fn request_thumbnail(
        &self,
        path: PathBuf,
        size_px: i32,
        on_ready: impl FnOnce(Option<RawBgraImage>) + Send + 'static,
    ) {
        let _ = self.tx.send(ShellJob::Thumbnail {
            path,
            size_px,
            on_ready: Box::new(on_ready),
        });
    }

    /// Shows the native Win32 Explorer context menu for `paths` at `(screen_x, screen_y)`.
    pub fn show_native_context_menu(
        &self,
        paths: Vec<PathBuf>,
        screen_x: i32,
        screen_y: i32,
        on_done: impl FnOnce(Result<(), String>) + Send + 'static,
    ) {
        let _ = self.tx.send(ShellJob::NativeContextMenu {
            paths,
            screen_x,
            screen_y,
            on_done: Box::new(on_done),
        });
    }

    /// Shows the native Windows Properties dialog for `path` on the COM STA thread.
    pub fn show_properties(
        &self,
        path: PathBuf,
        on_done: impl FnOnce(Result<(), String>) + Send + 'static,
    ) {
        let _ = self.tx.send(ShellJob::ShowProperties {
            path,
            on_done: Box::new(on_done),
        });
    }
}

/// Returns the current OS cursor position in physical screen coordinates.
pub fn cursor_screen_pos() -> (i32, i32) {
    #[cfg(target_os = "windows")]
    unsafe {
        use windows::Win32::Foundation::POINT;
        use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;
        let mut pt = POINT::default();
        if GetCursorPos(&mut pt).is_ok() {
            return (pt.x, pt.y);
        }
        (100, 100)
    }
    #[cfg(not(target_os = "windows"))]
    {
        (100, 100)
    }
}

#[cfg(target_os = "windows")]
fn extract_shell_thumbnail(path: &std::path::Path, size_px: i32) -> Option<RawBgraImage> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Foundation::SIZE;
    use windows::Win32::Graphics::Gdi::{
        BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, DIB_RGB_COLORS,
        DeleteDC, DeleteObject, GetDIBits, GetObjectW, HBITMAP,
    };
    use windows::Win32::UI::Shell::{
        IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_BIGGERSIZEOK, SIIGBF_ICONONLY,
        SIIGBF_THUMBNAILONLY,
    };
    use windows::core::PCWSTR;

    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    let factory: IShellItemImageFactory =
        unsafe { SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None).ok()? };
    let target_size = SIZE {
        cx: size_px,
        cy: size_px,
    };

    // Try rich thumbnail first (videos, photos, PDFs), then fall back to crisp Shell icon.
    let hbitmap: HBITMAP = unsafe {
        factory
            .GetImage(target_size, SIIGBF_BIGGERSIZEOK | SIIGBF_THUMBNAILONLY)
            .or_else(|_| factory.GetImage(target_size, SIIGBF_BIGGERSIZEOK | SIIGBF_ICONONLY))
            .ok()?
    };

    struct BitmapGuard(HBITMAP);
    impl Drop for BitmapGuard {
        fn drop(&mut self) {
            unsafe {
                let _ = DeleteObject(self.0);
            }
        }
    }
    let _bmp_guard = BitmapGuard(hbitmap);

    let mut bm = BITMAP::default();
    let got = unsafe {
        GetObjectW(
            hbitmap,
            std::mem::size_of::<BITMAP>() as i32,
            Some(&mut bm as *mut BITMAP as *mut core::ffi::c_void),
        )
    };
    if got == 0 || bm.bmWidth <= 0 || bm.bmHeight <= 0 {
        return None;
    }

    let width = bm.bmWidth as u32;
    let height = bm.bmHeight as u32;
    let mut bgra = vec![0u8; (width as usize) * (height as usize) * 4];

    let mut bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: bm.bmWidth,
            biHeight: -bm.bmHeight, // Negative height = top-down DIB
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };

    let hdc = unsafe { CreateCompatibleDC(None) };
    if hdc.is_invalid() {
        return None;
    }
    let scanlines = unsafe {
        GetDIBits(
            hdc,
            hbitmap,
            0,
            height,
            Some(bgra.as_mut_ptr() as *mut core::ffi::c_void),
            &mut bmi,
            DIB_RGB_COLORS,
        )
    };
    unsafe {
        let _ = DeleteDC(hdc);
    }

    if scanlines == 0 {
        return None;
    }

    // Some GDI 32-bit icons/thumbnails leave the alpha byte zeroed across all pixels;
    // normalize to opaque alpha when all alpha bytes are zero so GPUI renders them cleanly.
    let any_alpha = bgra.as_chunks::<4>().0.iter().any(|px| px[3] != 0);
    if !any_alpha {
        for px in bgra.as_chunks_mut::<4>().0 {
            px[3] = 255;
        }
    }

    Some(RawBgraImage {
        width,
        height,
        bgra,
    })
}

#[cfg(not(target_os = "windows"))]
fn extract_shell_thumbnail(_path: &std::path::Path, _size_px: i32) -> Option<RawBgraImage> {
    None
}

#[cfg(target_os = "windows")]
fn invoke_native_context_menu(
    paths: &[PathBuf],
    screen_x: i32,
    screen_y: i32,
) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::Common::ITEMIDLIST;
    use windows::Win32::UI::Shell::{
        CMF_EXPLORE, CMF_NORMAL, CMINVOKECOMMANDINFO, IContextMenu, IShellFolder, SHBindToParent,
        SHParseDisplayName,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CreatePopupMenu, CreateWindowExW, DestroyMenu, DestroyWindow, SetForegroundWindow,
        TPM_LEFTALIGN, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenuEx, WINDOW_EX_STYLE,
        WS_POPUP,
    };
    use windows::core::{PCSTR, PCWSTR};

    if paths.is_empty() {
        return Ok(());
    }

    let mut full_pidls: Vec<*mut ITEMIDLIST> = Vec::with_capacity(paths.len());
    let mut child_pidls: Vec<*const ITEMIDLIST> = Vec::with_capacity(paths.len());
    let mut parent_folder: Option<IShellFolder> = None;

    struct PidlListGuard(Vec<*mut ITEMIDLIST>);
    impl Drop for PidlListGuard {
        fn drop(&mut self) {
            for pidl in self.0.drain(..) {
                unsafe {
                    CoTaskMemFree(Some(pidl as *const core::ffi::c_void));
                }
            }
        }
    }

    for p in paths {
        let wide: Vec<u16> = p
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
        let parse_res = unsafe { SHParseDisplayName(PCWSTR(wide.as_ptr()), None, &mut pidl, 0, None) };
        if parse_res.is_err() || pidl.is_null() {
            continue;
        }
        full_pidls.push(pidl);

        let mut child: *mut ITEMIDLIST = std::ptr::null_mut();
        let folder_res: windows::core::Result<IShellFolder> =
            unsafe { SHBindToParent(pidl, Some(&mut child)) };
        if let Ok(folder) = folder_res
            && !child.is_null()
        {
            if parent_folder.is_none() {
                parent_folder = Some(folder);
            }
            child_pidls.push(child as *const ITEMIDLIST);
        }
    }

    let _pidl_guard = PidlListGuard(full_pidls);
    let Some(folder) = parent_folder else {
        return Err("Failed binding parent Windows Shell folder.".to_string());
    };
    if child_pidls.is_empty() {
        return Err("No valid Windows Shell items selected.".to_string());
    }

    let class_name: Vec<u16> = "STATIC\0".encode_utf16().collect();
    let title: Vec<u16> = "TExplorerShellMenuHost\0".encode_utf16().collect();
    let hwnd: HWND = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(class_name.as_ptr()),
            PCWSTR(title.as_ptr()),
            WS_POPUP,
            screen_x,
            screen_y,
            1,
            1,
            None,
            None,
            None,
            None,
        )
        .map_err(|e| format!("Failed creating Win32 menu host window: {e}"))?
    };

    struct WindowGuard(HWND);
    impl Drop for WindowGuard {
        fn drop(&mut self) {
            unsafe {
                let _ = DestroyWindow(self.0);
            }
        }
    }
    let _win_guard = WindowGuard(hwnd);

    let ctx_menu: IContextMenu = unsafe {
        folder
            .GetUIObjectOf(hwnd, &child_pidls, None)
            .map_err(|e| format!("Failed acquiring Windows IContextMenu: {e}"))?
    };

    let hmenu = unsafe {
        CreatePopupMenu().map_err(|e| format!("Failed creating native popup menu: {e}"))?
    };

    struct MenuGuard(windows::Win32::UI::WindowsAndMessaging::HMENU);
    impl Drop for MenuGuard {
        fn drop(&mut self) {
            unsafe {
                let _ = DestroyMenu(self.0);
            }
        }
    }
    let _menu_guard = MenuGuard(hmenu);

    const ID_CMD_FIRST: u32 = 1;
    const ID_CMD_LAST: u32 = 0x7FFF;
    unsafe {
        ctx_menu
            .QueryContextMenu(hmenu, 0, ID_CMD_FIRST, ID_CMD_LAST, CMF_NORMAL | CMF_EXPLORE)
            .map_err(|e| format!("Failed populating Windows context menu: {e}"))?;
        let _ = SetForegroundWindow(hwnd);
    }

    let cmd = unsafe {
        TrackPopupMenuEx(
            hmenu,
            (TPM_LEFTALIGN | TPM_RETURNCMD | TPM_RIGHTBUTTON).0,
            screen_x,
            screen_y,
            hwnd,
            None,
        )
    };

    if cmd.as_bool() {
        let cmd_id = cmd.0 as u32;
        if cmd_id >= ID_CMD_FIRST {
            let offset = (cmd_id - ID_CMD_FIRST) as usize;
            let info = CMINVOKECOMMANDINFO {
                cbSize: std::mem::size_of::<CMINVOKECOMMANDINFO>() as u32,
                fMask: 0,
                hwnd,
                lpVerb: PCSTR(offset as *const u8),
                lpParameters: PCSTR::null(),
                lpDirectory: PCSTR::null(),
                nShow: 1, // SW_SHOWNORMAL
                dwHotKey: 0,
                hIcon: Default::default(),
            };
            unsafe {
                ctx_menu
                    .InvokeCommand(&info)
                    .map_err(|e| format!("Windows shell command failed: {e}"))?;
            }
        }
    }

    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn invoke_native_context_menu(
    _paths: &[PathBuf],
    _screen_x: i32,
    _screen_y: i32,
) -> Result<(), String> {
    Err("Native Windows context menu is only supported on Windows.".to_string())
}
