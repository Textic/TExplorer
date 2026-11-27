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

/// Target for native Windows Shell icon extraction (icon-only, never file content thumbnails).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SystemIconRequest {
    /// Specific local file or folder path (used for `.exe`, `.lnk`, `.ico`, `.url`, and
    /// special/custom folders with `READONLY` or `SYSTEM` attributes).
    Path(PathBuf),
    /// Generic registered Windows file-type icon by extension (lowercased without leading dot;
    /// empty string = extensionless file). Uses `SHGFI_USEFILEATTRIBUTES` without touching disk,
    /// enabling instant caching across thousands of files and remote/archive listings.
    Extension(String),
    /// Generic Windows folder icon (`FILE_ATTRIBUTE_DIRECTORY` + `SHGFI_USEFILEATTRIBUTES`).
    GenericFolder,
}

type ThumbCallback = Box<dyn FnOnce(Option<RawBgraImage>) + Send>;
type MenuCallback = Box<dyn FnOnce(Result<(), String>) + Send>;

enum ShellJob {
    Thumbnail {
        path: PathBuf,
        size_px: i32,
        allow_icon_fallback: bool,
        on_ready: ThumbCallback,
    },
    SystemIcon {
        target: SystemIconRequest,
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
    OpenWith {
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
                            allow_icon_fallback,
                            on_ready,
                        } => {
                            let img = extract_shell_thumbnail(&path, size_px, allow_icon_fallback);
                            on_ready(img);
                        }
                        ShellJob::SystemIcon {
                            target,
                            size_px,
                            on_ready,
                        } => {
                            let img = extract_system_icon(&target, size_px);
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
                        ShellJob::OpenWith { path, on_done } => {
                            let res = crate::launch::open_with_dialog(&path);
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

    /// Queues an asynchronous native Shell thumbnail lookup for `path`
    /// (optionally falling back to the item's Shell icon when no content thumbnail exists).
    pub fn request_thumbnail(
        &self,
        path: PathBuf,
        size_px: i32,
        allow_icon_fallback: bool,
        on_ready: impl FnOnce(Option<RawBgraImage>) + Send + 'static,
    ) {
        let _ = self.tx.send(ShellJob::Thumbnail {
            path,
            size_px,
            allow_icon_fallback,
            on_ready: Box::new(on_ready),
        });
    }

    /// Queues an asynchronous native Windows system icon lookup (icon-only, never content thumbnail).
    pub fn request_system_icon(
        &self,
        target: SystemIconRequest,
        size_px: i32,
        on_ready: impl FnOnce(Option<RawBgraImage>) + Send + 'static,
    ) {
        let _ = self.tx.send(ShellJob::SystemIcon {
            target,
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

    /// Opens the native Windows "Open with -> Choose another app" dialog for `path` on the COM STA thread.
    pub fn open_with(
        &self,
        path: PathBuf,
        on_done: impl FnOnce(Result<(), String>) + Send + 'static,
    ) {
        let _ = self.tx.send(ShellJob::OpenWith {
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

/// Unpremultiplies 32-bit BGRA pixels in place when the bitmap carries valid premultiplied alpha
/// (`b <= a && g <= a && r <= a` for all pixels), matching GPUI's straight-alpha sprite blending.
fn unpremultiply_bgra_if_valid(bgra: &mut [u8]) {
    let mut has_semi_transparent = false;
    for px in bgra.as_chunks::<4>().0 {
        let a = px[3];
        if px[0] > a || px[1] > a || px[2] > a {
            return;
        }
        if a > 0 && a < 255 {
            has_semi_transparent = true;
        }
    }
    if !has_semi_transparent {
        return;
    }
    for px in bgra.as_chunks_mut::<4>().0 {
        let a = px[3] as u16;
        if a > 0 && a < 255 {
            let half = a / 2;
            px[0] = (((px[0] as u16) * 255 + half) / a).min(255) as u8;
            px[1] = (((px[1] as u16) * 255 + half) / a).min(255) as u8;
            px[2] = (((px[2] as u16) * 255 + half) / a).min(255) as u8;
        }
    }
}

#[cfg(target_os = "windows")]
struct BitmapGuard(windows::Win32::Graphics::Gdi::HBITMAP);

#[cfg(target_os = "windows")]
impl Drop for BitmapGuard {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            unsafe {
                let _ = windows::Win32::Graphics::Gdi::DeleteObject(self.0);
            }
        }
    }
}

#[cfg(target_os = "windows")]
struct IconGuard(windows::Win32::UI::WindowsAndMessaging::HICON);

#[cfg(target_os = "windows")]
impl Drop for IconGuard {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            unsafe {
                let _ = windows::Win32::UI::WindowsAndMessaging::DestroyIcon(self.0);
            }
        }
    }
}

#[cfg(target_os = "windows")]
fn hbitmap_to_bgra(
    hbitmap: windows::Win32::Graphics::Gdi::HBITMAP,
    unpremultiply_alpha: bool,
) -> Option<RawBgraImage> {
    use windows::Win32::Graphics::Gdi::{
        BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC,
        GetDIBits, GetObjectW,
    };

    if hbitmap.is_invalid() {
        return None;
    }

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
    } else if unpremultiply_alpha {
        unpremultiply_bgra_if_valid(&mut bgra);
    }

    Some(RawBgraImage {
        width,
        height,
        bgra,
    })
}

#[cfg(target_os = "windows")]
fn hicon_to_bgra(hicon: windows::Win32::UI::WindowsAndMessaging::HICON) -> Option<RawBgraImage> {
    use windows::Win32::Graphics::Gdi::{
        BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC,
        GetDIBits, GetObjectW,
    };
    use windows::Win32::UI::WindowsAndMessaging::{GetIconInfo, ICONINFO};

    if hicon.is_invalid() {
        return None;
    }

    let mut info = ICONINFO::default();
    unsafe { GetIconInfo(hicon, &mut info).ok()? };
    let _color_guard = BitmapGuard(info.hbmColor);
    let _mask_guard = BitmapGuard(info.hbmMask);

    if info.hbmColor.is_invalid() {
        return None;
    }

    let mut bm = BITMAP::default();
    let got = unsafe {
        GetObjectW(
            info.hbmColor,
            std::mem::size_of::<BITMAP>() as i32,
            Some(&mut bm as *mut BITMAP as *mut core::ffi::c_void),
        )
    };
    if got == 0 || bm.bmWidth <= 0 || bm.bmHeight <= 0 {
        return None;
    }

    let width = bm.bmWidth as u32;
    let height = bm.bmHeight as u32;
    let byte_len = (width as usize) * (height as usize) * 4;
    let mut bgra = vec![0u8; byte_len];

    let mut bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: bm.bmWidth,
            biHeight: -bm.bmHeight,
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
            info.hbmColor,
            0,
            height,
            Some(bgra.as_mut_ptr() as *mut core::ffi::c_void),
            &mut bmi,
            DIB_RGB_COLORS,
        )
    };
    if scanlines == 0 {
        unsafe {
            let _ = DeleteDC(hdc);
        }
        return None;
    }

    let any_alpha = bgra.as_chunks::<4>().0.iter().any(|px| px[3] != 0);
    if !any_alpha {
        // Legacy 24-bit/8-bit/4-bit icon: reconstruct transparency from the monochrome AND mask.
        let mut mask_applied = false;
        if !info.hbmMask.is_invalid() {
            let mut mask = vec![0u8; byte_len];
            let mask_lines = unsafe {
                GetDIBits(
                    hdc,
                    info.hbmMask,
                    0,
                    height,
                    Some(mask.as_mut_ptr() as *mut core::ffi::c_void),
                    &mut bmi,
                    DIB_RGB_COLORS,
                )
            };
            if mask_lines > 0 {
                for (px, m) in bgra.as_chunks_mut::<4>().0.iter_mut().zip(mask.as_chunks::<4>().0) {
                    if m[0] == 0 && m[1] == 0 && m[2] == 0 {
                        px[3] = 255;
                    } else {
                        *px = [0, 0, 0, 0];
                    }
                }
                mask_applied = true;
            }
        }
        if !mask_applied {
            for px in bgra.as_chunks_mut::<4>().0 {
                px[3] = 255;
            }
        }
    }

    unsafe {
        let _ = DeleteDC(hdc);
    }

    Some(RawBgraImage {
        width,
        height,
        bgra,
    })
}

#[cfg(target_os = "windows")]
fn extract_shell_thumbnail(
    path: &std::path::Path,
    size_px: i32,
    allow_icon_fallback: bool,
) -> Option<RawBgraImage> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Foundation::SIZE;
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

    let (hbitmap, is_icon_fallback) = match unsafe {
        factory.GetImage(target_size, SIIGBF_BIGGERSIZEOK | SIIGBF_THUMBNAILONLY)
    } {
        Ok(bmp) => (bmp, false),
        Err(_) if allow_icon_fallback => (
            unsafe {
                factory
                    .GetImage(target_size, SIIGBF_BIGGERSIZEOK | SIIGBF_ICONONLY)
                    .ok()?
            },
            true,
        ),
        Err(_) => return None,
    };
    let _bmp_guard = BitmapGuard(hbitmap);
    hbitmap_to_bgra(hbitmap, is_icon_fallback)
}

#[cfg(target_os = "windows")]
fn extract_shell_icon_by_attributes(
    virtual_name: &str,
    file_attrs: windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES,
    size_px: i32,
) -> Option<RawBgraImage> {
    use windows::Win32::UI::Controls::{IImageList, ILD_TRANSPARENT};
    use windows::Win32::UI::Shell::{
        SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON, SHGFI_SYSICONINDEX, SHGFI_USEFILEATTRIBUTES,
        SHGetFileInfoW, SHGetImageList, SHIL_EXTRALARGE, SHIL_LARGE,
    };
    use windows::core::PCWSTR;

    let wide: Vec<u16> = virtual_name
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    // Primary: fetch the exact resolution icon from the Windows Shell system image list
    // (`SHIL_LARGE` 32x32 for lists, `SHIL_EXTRALARGE` 48x48 for Grid/Inspector).
    let mut shfi = SHFILEINFOW::default();
    let idx_ok = unsafe {
        SHGetFileInfoW(
            PCWSTR(wide.as_ptr()),
            file_attrs,
            Some(&mut shfi),
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_USEFILEATTRIBUTES | SHGFI_SYSICONINDEX,
        )
    };
    if idx_ok != 0 {
        let shil = if size_px > 32 {
            SHIL_EXTRALARGE as i32
        } else {
            SHIL_LARGE as i32
        };
        if let Ok(image_list) = unsafe { SHGetImageList::<IImageList>(shil) }
            && let Ok(hicon) = unsafe { image_list.GetIcon(shfi.iIcon, ILD_TRANSPARENT.0) }
        {
            let _icon_guard = IconGuard(hicon);
            if let Some(img) = hicon_to_bgra(hicon) {
                return Some(img);
            }
        }
    }

    // Fallback: direct `SHGetFileInfoW` HICON handle.
    let mut shfi_icon = SHFILEINFOW::default();
    let icon_ok = unsafe {
        SHGetFileInfoW(
            PCWSTR(wide.as_ptr()),
            file_attrs,
            Some(&mut shfi_icon),
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_USEFILEATTRIBUTES | SHGFI_ICON | SHGFI_LARGEICON,
        )
    };
    if icon_ok == 0 || shfi_icon.hIcon.is_invalid() {
        return None;
    }
    let _icon_guard = IconGuard(shfi_icon.hIcon);
    hicon_to_bgra(shfi_icon.hIcon)
}

#[cfg(target_os = "windows")]
fn extract_shell_icon_for_path(path: &std::path::Path, size_px: i32) -> Option<RawBgraImage> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Foundation::SIZE;
    use windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES;
    use windows::Win32::UI::Shell::{
        IShellItemImageFactory, SHCreateItemFromParsingName, SHFILEINFOW, SHGFI_ICON,
        SHGFI_LARGEICON, SHGetFileInfoW, SIIGBF_BIGGERSIZEOK, SIIGBF_ICONONLY,
    };
    use windows::core::PCWSTR;

    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    if let Ok(factory) =
        unsafe { SHCreateItemFromParsingName::<_, _, IShellItemImageFactory>(PCWSTR(wide.as_ptr()), None) }
    {
        let target_size = SIZE {
            cx: size_px,
            cy: size_px,
        };
        if let Ok(hbitmap) =
            unsafe { factory.GetImage(target_size, SIIGBF_BIGGERSIZEOK | SIIGBF_ICONONLY) }
        {
            let _bmp_guard = BitmapGuard(hbitmap);
            if let Some(img) = hbitmap_to_bgra(hbitmap, true) {
                return Some(img);
            }
        }
    }

    let mut shfi = SHFILEINFOW::default();
    let ok = unsafe {
        SHGetFileInfoW(
            PCWSTR(wide.as_ptr()),
            FILE_FLAGS_AND_ATTRIBUTES(0),
            Some(&mut shfi),
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_ICON | SHGFI_LARGEICON,
        )
    };
    if ok == 0 || shfi.hIcon.is_invalid() {
        return None;
    }
    let _icon_guard = IconGuard(shfi.hIcon);
    hicon_to_bgra(shfi.hIcon)
}

#[cfg(target_os = "windows")]
fn extract_system_icon(target: &SystemIconRequest, size_px: i32) -> Option<RawBgraImage> {
    use windows::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL};

    match target {
        SystemIconRequest::Path(path) => extract_shell_icon_for_path(path, size_px),
        SystemIconRequest::Extension(ext) => {
            let clean: String = ext
                .trim()
                .trim_start_matches('.')
                .chars()
                .filter(|c| {
                    !matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0')
                        && !c.is_control()
                })
                .collect();
            let virtual_name = if clean.is_empty() {
                "file".to_string()
            } else {
                format!("file.{clean}")
            };
            extract_shell_icon_by_attributes(&virtual_name, FILE_ATTRIBUTE_NORMAL, size_px)
        }
        SystemIconRequest::GenericFolder => {
            extract_shell_icon_by_attributes("folder", FILE_ATTRIBUTE_DIRECTORY, size_px)
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn extract_shell_thumbnail(
    _path: &std::path::Path,
    _size_px: i32,
    _allow_icon_fallback: bool,
) -> Option<RawBgraImage> {
    None
}

#[cfg(not(target_os = "windows"))]
fn extract_system_icon(_target: &SystemIconRequest, _size_px: i32) -> Option<RawBgraImage> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unpremultiplies_valid_premultiplied_bgra_and_leaves_straight_alpha_untouched() {
        // Premultiplied 50% white (128, 128, 128, 128) -> straight white (255, 255, 255, 128).
        let mut premul = vec![128, 128, 128, 128, 0, 0, 0, 0, 200, 100, 50, 255];
        unpremultiply_bgra_if_valid(&mut premul);
        assert_eq!(premul, vec![255, 255, 255, 128, 0, 0, 0, 0, 200, 100, 50, 255]);

        // Straight alpha pixel with channel > alpha must NOT be modified.
        let mut straight = vec![255, 200, 100, 128];
        unpremultiply_bgra_if_valid(&mut straight);
        assert_eq!(straight, vec![255, 200, 100, 128]);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn extracts_native_windows_system_icons_for_dll_folder_and_exe() {
        let worker = ShellWorker::spawn();

        let (tx_dll, rx_dll) = channel();
        worker.request_system_icon(SystemIconRequest::Extension("dll".to_string()), 32, move |img| {
            let _ = tx_dll.send(img);
        });
        let dll_img = rx_dll
            .recv()
            .expect("worker response")
            .expect("Windows DLL icon should resolve");
        assert!(dll_img.width >= 16 && dll_img.height >= 16);
        assert_eq!(
            dll_img.bgra.len(),
            (dll_img.width as usize) * (dll_img.height as usize) * 4
        );
        assert!(dll_img.bgra.as_chunks::<4>().0.iter().any(|px| px[3] > 0));

        let (tx_dir, rx_dir) = channel();
        worker.request_system_icon(SystemIconRequest::GenericFolder, 48, move |img| {
            let _ = tx_dir.send(img);
        });
        let dir_img = rx_dir
            .recv()
            .expect("worker response")
            .expect("Windows generic folder icon should resolve");
        assert!(dir_img.width >= 32 && dir_img.height >= 32);
        assert!(dir_img.bgra.as_chunks::<4>().0.iter().any(|px| px[3] > 0));

        if let Ok(exe) = std::env::current_exe() {
            let (tx_exe, rx_exe) = channel();
            worker.request_system_icon(SystemIconRequest::Path(exe), 32, move |img| {
                let _ = tx_exe.send(img);
            });
            let exe_img = rx_exe
                .recv()
                .expect("worker response")
                .expect("Windows per-file executable icon should resolve");
            assert!(exe_img.width >= 16 && exe_img.height >= 16);
        }
    }
}
