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
    NativeBackgroundContextMenu {
        directory: PathBuf,
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
                        ShellJob::NativeBackgroundContextMenu {
                            directory,
                            screen_x,
                            screen_y,
                            on_done,
                        } => {
                            let res = invoke_native_background_context_menu(
                                &directory, screen_x, screen_y,
                            );
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

    /// Shows the native Win32 Explorer directory background context menu (empty folder space)
    /// for `directory` at `(screen_x, screen_y)`.
    pub fn show_native_background_context_menu(
        &self,
        directory: PathBuf,
        screen_x: i32,
        screen_y: i32,
        on_done: impl FnOnce(Result<(), String>) + Send + 'static,
    ) {
        let _ = self.tx.send(ShellJob::NativeBackgroundContextMenu {
            directory,
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
mod shell_menu_host {
    use std::cell::RefCell;

    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::UI::Shell::{
        CMF_EXPLORE, CMF_EXTENDEDVERBS, CMF_NORMAL, CMINVOKECOMMANDINFO, IContextMenu,
        IContextMenu2, IContextMenu3,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
        RegisterClassW, SetForegroundWindow, TPM_LEFTALIGN, TPM_RETURNCMD, TPM_RIGHTBUTTON,
        TrackPopupMenuEx, WINDOW_EX_STYLE, WM_DRAWITEM, WM_INITMENUPOPUP, WM_MEASUREITEM,
        WM_MENUCHAR, WNDCLASSW, WS_POPUP,
    };
    use windows::core::{Interface, PCSTR, PCWSTR};

    thread_local! {
        static ACTIVE_CTX_MENU2: RefCell<Option<IContextMenu2>> = const { RefCell::new(None) };
        static ACTIVE_CTX_MENU3: RefCell<Option<IContextMenu3>> = const { RefCell::new(None) };
    }

    unsafe extern "system" fn menu_host_wnd_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        let mut handled_res: Option<LRESULT> = None;
        if msg == WM_MENUCHAR || msg == WM_INITMENUPOPUP || msg == WM_DRAWITEM || msg == WM_MEASUREITEM
        {
            ACTIVE_CTX_MENU3.with(|cell| {
                if let Some(cm3) = cell.borrow().as_ref() {
                    let mut lres = LRESULT(0);
                    if unsafe { cm3.HandleMenuMsg2(msg, wparam, lparam, Some(&mut lres)) }.is_ok() {
                        handled_res = Some(lres);
                    }
                }
            });
            if handled_res.is_none() && msg != WM_MENUCHAR {
                ACTIVE_CTX_MENU2.with(|cell| {
                    if let Some(cm2) = cell.borrow().as_ref()
                        && unsafe { cm2.HandleMenuMsg(msg, wparam, lparam) }.is_ok()
                    {
                        handled_res = Some(LRESULT(0));
                    }
                });
            }
        }
        handled_res.unwrap_or_else(|| unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) })
    }

    pub struct HostWindowGuard(pub HWND);
    impl Drop for HostWindowGuard {
        fn drop(&mut self) {
            unsafe {
                let _ = DestroyWindow(self.0);
            }
        }
    }

    pub fn create_menu_host_window(screen_x: i32, screen_y: i32) -> Result<HostWindowGuard, String> {
        let class_name: Vec<u16> = "TExplorerShellMenuHostV1\0".encode_utf16().collect();
        let wc = WNDCLASSW {
            lpfnWndProc: Some(menu_host_wnd_proc),
            lpszClassName: PCWSTR(class_name.as_ptr()),
            ..Default::default()
        };
        unsafe {
            let _ = RegisterClassW(&wc);
        }
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                PCWSTR(class_name.as_ptr()),
                PCWSTR(class_name.as_ptr()),
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
        Ok(HostWindowGuard(hwnd))
    }

    pub fn run_context_menu_popup(
        hwnd: HWND,
        ctx_menu: &IContextMenu,
        screen_x: i32,
        screen_y: i32,
    ) -> Result<(), String> {
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

        // Bind IContextMenu2/3 on this STA thread so dynamic submenus (e.g. "New ->") render properly.
        ACTIVE_CTX_MENU3.with(|c| *c.borrow_mut() = ctx_menu.cast::<IContextMenu3>().ok());
        ACTIVE_CTX_MENU2.with(|c| *c.borrow_mut() = ctx_menu.cast::<IContextMenu2>().ok());
        struct ActiveMenuReset;
        impl Drop for ActiveMenuReset {
            fn drop(&mut self) {
                ACTIVE_CTX_MENU3.with(|c| *c.borrow_mut() = None);
                ACTIVE_CTX_MENU2.with(|c| *c.borrow_mut() = None);
            }
        }
        let _reset = ActiveMenuReset;

        const ID_CMD_FIRST: u32 = 1;
        const ID_CMD_LAST: u32 = 0x7FFF;
        unsafe {
            ctx_menu
                .QueryContextMenu(
                    hmenu,
                    0,
                    ID_CMD_FIRST,
                    ID_CMD_LAST,
                    CMF_NORMAL | CMF_EXPLORE | CMF_EXTENDEDVERBS,
                )
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
}

#[cfg(target_os = "windows")]
fn invoke_native_context_menu(
    paths: &[PathBuf],
    screen_x: i32,
    screen_y: i32,
) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::Common::ITEMIDLIST;
    use windows::Win32::UI::Shell::{
        IContextMenu, IShellFolder, SHBindToParent, SHParseDisplayName,
    };
    use windows::core::PCWSTR;

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
        let parse_res =
            unsafe { SHParseDisplayName(PCWSTR(wide.as_ptr()), None, &mut pidl, 0, None) };
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

    let host = shell_menu_host::create_menu_host_window(screen_x, screen_y)?;
    let ctx_menu: IContextMenu = unsafe {
        folder
            .GetUIObjectOf(host.0, &child_pidls, None)
            .map_err(|e| format!("Failed acquiring Windows IContextMenu: {e}"))?
    };

    shell_menu_host::run_context_menu_popup(host.0, &ctx_menu, screen_x, screen_y)
}

#[cfg(target_os = "windows")]
fn invoke_native_background_context_menu(
    directory: &std::path::Path,
    screen_x: i32,
    screen_y: i32,
) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::System::Com::{CoTaskMemFree, IBindCtx};
    use windows::Win32::UI::Shell::Common::ITEMIDLIST;
    use windows::Win32::UI::Shell::{
        IContextMenu, IShellFolder, SHBindToObject, SHParseDisplayName,
    };
    use windows::core::PCWSTR;

    let wide: Vec<u16> = directory
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
    unsafe {
        SHParseDisplayName(PCWSTR(wide.as_ptr()), None, &mut pidl, 0, None)
            .map_err(|e| format!("Failed parsing folder path for Windows menu: {e}"))?;
    }
    if pidl.is_null() {
        return Err("Failed resolving Windows Shell folder PIDL.".to_string());
    }

    struct SinglePidlGuard(*mut ITEMIDLIST);
    impl Drop for SinglePidlGuard {
        fn drop(&mut self) {
            unsafe {
                CoTaskMemFree(Some(self.0 as *const core::ffi::c_void));
            }
        }
    }
    let _pidl_guard = SinglePidlGuard(pidl);

    let folder: IShellFolder = unsafe {
        SHBindToObject::<Option<&IShellFolder>, Option<&IBindCtx>, IShellFolder>(None, pidl, None)
            .map_err(|e| format!("Failed binding Windows Shell folder: {e}"))?
    };

    let host = shell_menu_host::create_menu_host_window(screen_x, screen_y)?;
    let ctx_menu: IContextMenu = unsafe {
        folder
            .CreateViewObject(host.0)
            .map_err(|e| format!("Failed acquiring Windows folder background menu: {e}"))?
    };

    shell_menu_host::run_context_menu_popup(host.0, &ctx_menu, screen_x, screen_y)
}

#[cfg(not(target_os = "windows"))]
fn invoke_native_context_menu(
    _paths: &[PathBuf],
    _screen_x: i32,
    _screen_y: i32,
) -> Result<(), String> {
    Err("Native Windows context menu is only supported on Windows.".to_string())
}

#[cfg(not(target_os = "windows"))]
fn invoke_native_background_context_menu(
    _directory: &std::path::Path,
    _screen_x: i32,
    _screen_y: i32,
) -> Result<(), String> {
    Err("Native Windows context menu is only supported on Windows.".to_string())
}
