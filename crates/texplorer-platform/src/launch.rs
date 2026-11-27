use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Opens a file or folder with the OS default handler (safe to invoke from background threads).
pub fn open_item(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows::Win32::System::Com::{
            COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize,
        };
        use windows_sys::Win32::UI::Shell::ShellExecuteW;
        use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

        let com_inited = unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE).is_ok()
        };
        struct ComGuard(bool);
        impl Drop for ComGuard {
            fn drop(&mut self) {
                if self.0 {
                    unsafe {
                        CoUninitialize();
                    }
                }
            }
        }
        let _com_guard = ComGuard(com_inited);

        let verb: Vec<u16> = "open\0".encode_utf16().collect();
        let file: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let working_dir: Option<Vec<u16>> = path.parent().map(|p| {
            p.as_os_str()
                .encode_wide()
                .chain(std::iter::once(0))
                .collect()
        });

        let rc = unsafe {
            ShellExecuteW(
                std::ptr::null_mut(),
                verb.as_ptr(),
                file.as_ptr(),
                std::ptr::null(),
                working_dir
                    .as_ref()
                    .map_or(std::ptr::null(), |v| v.as_ptr()),
                SW_SHOWNORMAL,
            )
        } as isize;

        if rc <= 32 {
            Err(format!(
                "Failed to open \"{}\" (ShellExecute code {rc}).",
                path.display()
            ))
        } else {
            Ok(())
        }
    }
    #[cfg(target_os = "macos")]
    {
        Command::new("open")
            .arg(path)
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("Failed to open item: {e}"))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        Command::new("xdg-open")
            .arg(path)
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("Failed to open item: {e}"))
    }
}

/// Opens the native Windows "Open with -> Choose another app" dialog for `path`, enabling both
/// **Always** (change default app for extension) and **Just once**.
///
/// Invokes the canonical `"openas"` Shell context verb via `ShellExecuteExW(SEE_MASK_INVOKEIDLIST)`
/// — the exact path used by Windows Explorer's *"Choose another app"* — and falls back to
/// `SHOpenWithDialog(OAIF_ALLOW_REGISTRATION | OAIF_REGISTER_EXT | OAIF_EXEC)` if needed.
pub fn open_with_dialog(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows::Win32::Foundation::{ERROR_CANCELLED, HWND};
        use windows::Win32::UI::Shell::{
            OAIF_ALLOW_REGISTRATION, OAIF_EXEC, OAIF_REGISTER_EXT, OPENASINFO, SHOpenWithDialog,
        };
        use windows_sys::Win32::Foundation::GetLastError;
        use windows_sys::Win32::UI::Shell::{
            SEE_MASK_INVOKEIDLIST, SHELLEXECUTEINFOW, ShellExecuteExW,
        };
        use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
        use windows_core::{HSTRING, PCWSTR};

        let raw = path.to_string_lossy();
        let clean = if let Some(unc) = raw.strip_prefix(r"\\?\UNC\") {
            format!(r"\\{unc}")
        } else if let Some(rest) = raw.strip_prefix(r"\\?\") {
            rest.to_string()
        } else {
            raw.into_owned()
        };

        // 1. Primary path: canonical Shell `"openas"` verb (`SEE_MASK_INVOKEIDLIST`), identical to
        //    Windows Explorer's "Open with -> Choose another app" (shows Always & Just once).
        let verb: Vec<u16> = "openas\0".encode_utf16().collect();
        let file: Vec<u16> = std::ffi::OsStr::new(&clean)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut sei: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
        sei.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
        sei.fMask = SEE_MASK_INVOKEIDLIST;
        sei.lpVerb = verb.as_ptr();
        sei.lpFile = file.as_ptr();
        sei.nShow = SW_SHOWNORMAL;

        if unsafe { ShellExecuteExW(&mut sei) } != 0 {
            return Ok(());
        }
        let err_code = unsafe { GetLastError() };
        if err_code == ERROR_CANCELLED.0 {
            return Ok(());
        }

        // 2. Fallback: direct `SHOpenWithDialog` with `OAIF_REGISTER_EXT` enabled.
        let wide_path = HSTRING::from(clean);
        let info = OPENASINFO {
            pcszFile: PCWSTR(wide_path.as_ptr()),
            pcszClass: PCWSTR::null(),
            oaifInFlags: OAIF_ALLOW_REGISTRATION | OAIF_REGISTER_EXT | OAIF_EXEC,
        };

        match unsafe { SHOpenWithDialog(HWND::default(), &info) } {
            Ok(()) => Ok(()),
            Err(err) if err.code() == ERROR_CANCELLED.to_hresult() => Ok(()),
            Err(err) => Err(format!(
                "Failed to show Windows 'Open With' dialog for \"{}\": {err}",
                path.display()
            )),
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        open_item(path)
    }
}

/// Reveals `path` inside the original Windows Explorer (`explorer.exe`).
pub fn reveal_in_windows_explorer(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        let path_str = path.to_string_lossy();
        let arg = if path.is_dir() {
            format!("\"{path_str}\"")
        } else {
            format!("/select,\"{path_str}\"")
        };
        Command::new("explorer.exe")
            .raw_arg(arg)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("Failed to launch Windows Explorer: {e}"))
    }
    #[cfg(not(target_os = "windows"))]
    {
        open_item(path)
    }
}

/// Normalizes a filesystem directory path for passing to external Windows terminals (`wt.exe`,
/// `pwsh.exe`, `powershell.exe`): strips extended `\\?\` prefixes and trims redundant trailing
/// separators from non-root paths while preserving drive roots (`C:\`).
pub fn normalize_terminal_dir(path: &Path) -> PathBuf {
    let base = if path.is_file() {
        path.parent().unwrap_or(path)
    } else {
        path
    };
    let raw = base.to_string_lossy();
    let stripped = if let Some(unc) = raw.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{unc}")
    } else if let Some(rest) = raw.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        raw.into_owned()
    };
    let bytes = stripped.as_bytes();
    if bytes.len() == 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && (bytes[2] == b'\\' || bytes[2] == b'/') {
        return PathBuf::from(format!("{}:\\", bytes[0] as char));
    }
    if bytes.len() == 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return PathBuf::from(format!("{}:\\", bytes[0] as char));
    }
    let trimmed = stripped.trim_end_matches(['\\', '/']);
    if trimmed.is_empty() {
        base.to_path_buf()
    } else {
        PathBuf::from(trimmed)
    }
}

/// Formats `-d "<dir>"` for Windows Terminal (`wt.exe`), safely escaping drive roots (`X:\`) so
/// `CommandLineToArgvW` never treats `\"` as an escaped quote.
pub fn format_wt_dir_args(dir: &Path) -> String {
    let raw = dir.to_string_lossy();
    let escaped = if raw.ends_with('\\') {
        format!("{raw}\\")
    } else {
        raw.into_owned()
    };
    format!("-d \"{}\"", escaped.replace('"', "\\\""))
}

/// Launches `file` via the desktop `explorer.exe` shell (`IShellDispatch2::ShellExecute`) so child
/// processes run under the normal non-elevated desktop user token and user environment even when
/// TExplorer itself was launched elevated.
pub fn spawn_as_desktop_user(file: &str, args: &str, working_dir: &Path) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::System::Com::{
            CLSCTX_LOCAL_SERVER, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
            CoCreateInstance, CoInitializeEx, CoUninitialize, IDispatch, IServiceProvider,
        };
        use windows::Win32::UI::Shell::{
            CSIDL_DESKTOP, IShellBrowser, IShellDispatch2, IShellFolderViewDual, IShellView,
            IShellWindows, SID_STopLevelBrowser, SVGIO_BACKGROUND, SWC_DESKTOP, SWFO_NEEDDISPATCH,
            ShellWindows,
        };
        use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
        use windows_core::{BSTR, Interface, VARIANT};

        let com_inited = unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE).is_ok()
        };
        struct ComGuard(bool);
        impl Drop for ComGuard {
            fn drop(&mut self) {
                if self.0 {
                    unsafe {
                        CoUninitialize();
                    }
                }
            }
        }
        let _com_guard = ComGuard(com_inited);

        let shell_windows: IShellWindows =
            unsafe { CoCreateInstance(&ShellWindows, None, CLSCTX_LOCAL_SERVER) }
                .map_err(|e| format!("Failed connecting to Windows Shell desktop: {e}"))?;
        let loc = VARIANT::from(CSIDL_DESKTOP as i32);
        let empty = VARIANT::default();
        let mut hwnd = 0i32;
        let desktop_disp: IDispatch = unsafe {
            shell_windows.FindWindowSW(&loc, &empty, SWC_DESKTOP, &mut hwnd, SWFO_NEEDDISPATCH)
        }
        .map_err(|e| format!("Failed locating desktop shell window: {e}"))?;

        let service_provider: IServiceProvider = desktop_disp
            .cast()
            .map_err(|e| format!("Failed querying IServiceProvider: {e}"))?;
        let browser: IShellBrowser =
            unsafe { service_provider.QueryService(&SID_STopLevelBrowser) }
                .map_err(|e| format!("Failed querying desktop IShellBrowser: {e}"))?;
        let view: IShellView = unsafe { browser.QueryActiveShellView() }
            .map_err(|e| format!("Failed querying active desktop IShellView: {e}"))?;
        let bg_disp: IDispatch = unsafe { view.GetItemObject(SVGIO_BACKGROUND) }
            .map_err(|e| format!("Failed obtaining desktop background object: {e}"))?;
        let folder_view: IShellFolderViewDual = bg_disp
            .cast()
            .map_err(|e| format!("Failed querying IShellFolderViewDual: {e}"))?;
        let app_disp: IDispatch = unsafe { folder_view.Application() }
            .map_err(|e| format!("Failed obtaining Shell application object: {e}"))?;
        let dispatch: IShellDispatch2 = app_disp
            .cast()
            .map_err(|e| format!("Failed querying IShellDispatch2: {e}"))?;

        let clean_dir = normalize_terminal_dir(working_dir);
        let dir_str = clean_dir.to_string_lossy().into_owned();
        unsafe {
            dispatch.ShellExecute(
                &BSTR::from(file),
                &VARIANT::from(BSTR::from(args)),
                &VARIANT::from(BSTR::from(dir_str)),
                &VARIANT::from(BSTR::from("open")),
                &VARIANT::from(SW_SHOWNORMAL.0),
            )
        }
        .map_err(|e| format!("Desktop shell failed launching \"{file}\": {e}"))?;
        Ok(())
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (file, args, working_dir);
        Err("Desktop shell launch is only supported on Windows.".to_string())
    }
}

#[cfg(target_os = "windows")]
fn windows_terminal_available() -> bool {
    if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
        let alias = PathBuf::from(local_app_data)
            .join("Microsoft")
            .join("WindowsApps")
            .join("wt.exe");
        if alias.exists() {
            return true;
        }
    }
    if let Some(path_var) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path_var) {
            if dir.join("wt.exe").exists() {
                return true;
            }
        }
    }
    false
}

/// Opens a native terminal in `path` (or its parent directory if `path` is a file).
/// Always spawns as the normal desktop user even if TExplorer is running elevated.
pub fn open_terminal_here(path: &Path, preferred_cmd: &str) -> Result<(), String> {
    let dir = normalize_terminal_dir(path);

    #[cfg(target_os = "windows")]
    {
        let preferred = preferred_cmd.trim();
        let wants_wt = preferred.is_empty()
            || preferred.eq_ignore_ascii_case("wt.exe")
            || preferred.eq_ignore_ascii_case("wt");
        let elevated = crate::elevation::is_elevated();

        if wants_wt && windows_terminal_available() {
            if elevated {
                let wt_args = format_wt_dir_args(&dir);
                if spawn_as_desktop_user("wt.exe", &wt_args, &dir).is_ok() {
                    return Ok(());
                }
            }
            if Command::new("wt.exe")
                .arg("-d")
                .arg(&dir)
                .current_dir(&dir)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .is_ok()
            {
                return Ok(());
            }
        }

        if !preferred.is_empty() && !wants_wt {
            if elevated && spawn_as_desktop_user(preferred, "", &dir).is_ok() {
                return Ok(());
            }
            if Command::new(preferred)
                .current_dir(&dir)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .is_ok()
            {
                return Ok(());
            }
        }

        let escaped = dir.to_string_lossy().replace('\'', "''");
        let ps_args = format!("-NoExit -Command \"Set-Location -LiteralPath '{escaped}'\"");
        if elevated && spawn_as_desktop_user("powershell.exe", &ps_args, &dir).is_ok() {
            return Ok(());
        }
        Command::new("powershell.exe")
            .arg("-NoExit")
            .arg("-Command")
            .arg(format!("Set-Location -LiteralPath '{escaped}'"))
            .current_dir(&dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("Failed to launch terminal: {e}"))
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = preferred_cmd;
        Command::new("x-terminal-emulator")
            .arg("--working-directory")
            .arg(dir)
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("Failed to launch terminal: {e}"))
    }
}

/// Opens `path` in the configured external code editor (e.g. `code`).
pub fn open_in_editor(path: &Path, editor_cmd: &str) -> Result<(), String> {
    let cmd = if editor_cmd.trim().is_empty() {
        "code"
    } else {
        editor_cmd.trim()
    };

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let working_dir = normalize_terminal_dir(path);
        if crate::elevation::is_elevated() {
            let quoted_target = format!("\"{}\"", path.to_string_lossy().replace('"', "\\\""));
            if spawn_as_desktop_user(cmd, &quoted_target, &working_dir).is_ok() {
                return Ok(());
            }
        }
        if Command::new(cmd)
            .arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .is_ok()
        {
            return Ok(());
        }
        // Handle `.cmd` shims such as VS Code's `code.cmd` without leaking console output.
        Command::new("cmd.exe")
            .args(["/C", cmd])
            .arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("Failed to launch editor \"{cmd}\": {e}"))
    }
    #[cfg(not(target_os = "windows"))]
    {
        Command::new(cmd)
            .arg(path)
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("Failed to launch editor \"{cmd}\": {e}"))
    }
}

/// Opens the native Windows Properties dialog for `path`.
pub fn show_properties_dialog(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::UI::Shell::{
            SEE_MASK_INVOKEIDLIST, SHELLEXECUTEINFOW, ShellExecuteExW,
        };
        use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOW;

        let verb: Vec<u16> = "properties\0".encode_utf16().collect();
        let file: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();

        let mut sei: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
        sei.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
        sei.fMask = SEE_MASK_INVOKEIDLIST;
        sei.lpVerb = verb.as_ptr();
        sei.lpFile = file.as_ptr();
        sei.nShow = SW_SHOW;

        let ok = unsafe { ShellExecuteExW(&mut sei) };
        if ok == 0 {
            Err("Failed to open Windows Properties dialog.".to_string())
        } else {
            Ok(())
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = path;
        Err("Properties dialog is only supported on Windows.".to_string())
    }
}

/// Moves one or more files/folders to the OS Recycle Bin / Trash.
///
/// On Windows, if the standard non-elevated deletion fails with `Access is denied` (e.g. inside
/// `C:\Program Files`), automatically retries remaining items via an elevated `IFileOperation`
/// triggering a single native Windows UAC prompt.
pub fn move_to_trash(paths: &[PathBuf]) -> Result<(), String> {
    if paths.is_empty() {
        return Ok(());
    }
    match trash::delete_all(paths) {
        Ok(()) => Ok(()),
        Err(err) => {
            let err_str = err.to_string();
            if !crate::is_elevated() && crate::is_access_denied_message(&err_str) {
                let remaining: Vec<PathBuf> = paths
                    .iter()
                    .filter(|p| p.symlink_metadata().is_ok())
                    .cloned()
                    .collect();
                if remaining.is_empty() {
                    return Ok(());
                }
                return crate::elevated_delete_paths(&remaining, true)
                    .map_err(crate::ElevatedOpError::into_message);
            }
            Err(format!("Failed to move items to Recycle Bin: {err_str}"))
        }
    }
}

enum DeleteRawError {
    PermissionDenied(String),
    Other(String),
}

/// Permanently removes a single file, symlink, or directory iteratively without following symlinks,
/// falling back to an elevated Windows UAC `IFileOperation` on `Access is denied`.
pub fn delete_permanently(path: &Path) -> Result<(), String> {
    delete_permanently_batch(std::slice::from_ref(&path.to_path_buf()))
}

/// Permanently removes a batch of local paths, consolidating any `Access is denied` items into a
/// single elevated Windows UAC `IFileOperation` prompt.
pub fn delete_permanently_batch(paths: &[PathBuf]) -> Result<(), String> {
    let mut denied_paths: Vec<PathBuf> = Vec::new();
    let mut first_denied_msg: Option<String> = None;
    let mut first_other_err: Option<String> = None;

    for path in paths {
        match delete_permanently_local_raw(path) {
            Ok(()) => {}
            Err(DeleteRawError::PermissionDenied(msg)) => {
                if first_denied_msg.is_none() {
                    first_denied_msg = Some(msg);
                }
                denied_paths.push(path.clone());
            }
            Err(DeleteRawError::Other(msg)) => {
                if first_other_err.is_none() {
                    first_other_err = Some(msg);
                }
            }
        }
    }

    if !denied_paths.is_empty() {
        if !crate::is_elevated() {
            let remaining: Vec<PathBuf> = denied_paths
                .into_iter()
                .filter(|p| p.symlink_metadata().is_ok())
                .collect();
            if !remaining.is_empty() {
                crate::elevated_delete_paths(&remaining, false)
                    .map_err(crate::ElevatedOpError::into_message)?;
            }
        } else if let Some(msg) = first_denied_msg {
            return Err(msg);
        }
    }

    match first_other_err {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

fn delete_permanently_local_raw(path: &Path) -> Result<(), DeleteRawError> {
    let mut stack: Vec<(PathBuf, bool)> = vec![(path.to_path_buf(), false)];
    while let Some((cur, ready)) = stack.pop() {
        if ready {
            remove_dir_readonly(&cur)?;
            continue;
        }
        let meta = std::fs::symlink_metadata(&cur).map_err(|e| {
            classify_io_delete_err(format!("Failed to inspect \"{}\": {e}", cur.display()), &e)
        })?;
        let ft = meta.file_type();
        if ft.is_symlink() {
            if let Err(file_err) = std::fs::remove_file(&cur)
                && remove_dir_readonly(&cur).is_err()
            {
                return Err(classify_io_delete_err(
                    format!("Failed to delete \"{}\": {file_err}", cur.display()),
                    &file_err,
                ));
            }
        } else if ft.is_dir() {
            stack.push((cur.clone(), true));
            for entry in std::fs::read_dir(&cur).map_err(|e| {
                classify_io_delete_err(format!("Failed to read \"{}\": {e}", cur.display()), &e)
            })? {
                let entry = entry.map_err(|e| {
                    classify_io_delete_err(format!("Failed enumerating directory: {e}"), &e)
                })?;
                stack.push((entry.path(), false));
            }
        } else {
            remove_file_readonly(&cur)?;
        }
    }
    Ok(())
}

fn classify_io_delete_err(msg: String, err: &std::io::Error) -> DeleteRawError {
    if err.kind() == std::io::ErrorKind::PermissionDenied || err.raw_os_error() == Some(5) {
        DeleteRawError::PermissionDenied(msg)
    } else {
        DeleteRawError::Other(msg)
    }
}

fn clear_readonly(path: &Path) {
    if let Ok(meta) = std::fs::symlink_metadata(path) {
        let mut perms = meta.permissions();
        if perms.readonly() {
            #[allow(clippy::permissions_set_readonly_false)]
            perms.set_readonly(false);
            let _ = std::fs::set_permissions(path, perms);
        }
    }
}

fn remove_file_readonly(path: &Path) -> Result<(), DeleteRawError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            clear_readonly(path);
            std::fs::remove_file(path).map_err(|err| {
                classify_io_delete_err(format!("Failed to delete \"{}\": {err}", path.display()), &err)
            })
        }
        Err(e) => Err(classify_io_delete_err(
            format!("Failed to delete \"{}\": {e}", path.display()),
            &e,
        )),
    }
}

fn remove_dir_readonly(path: &Path) -> Result<(), DeleteRawError> {
    match std::fs::remove_dir(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            clear_readonly(path);
            std::fs::remove_dir(path).map_err(|err| {
                classify_io_delete_err(
                    format!("Failed to delete folder \"{}\": {err}", path.display()),
                    &err,
                )
            })
        }
        Err(e) => Err(classify_io_delete_err(
            format!("Failed to delete folder \"{}\": {e}", path.display()),
            &e,
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_terminal_dir_and_wt_quotes() {
        assert_eq!(
            normalize_terminal_dir(Path::new(r"\\?\Z:\NonExistent\Project\")),
            PathBuf::from(r"Z:\NonExistent\Project")
        );
        assert_eq!(
            normalize_terminal_dir(Path::new(r"\\?\C:\")),
            PathBuf::from(r"C:\")
        );
        let temp_file = std::env::temp_dir().join(format!(
            "texplorer_term_test_{}.txt",
            std::process::id()
        ));
        let _ = std::fs::write(&temp_file, b"ok");
        assert_eq!(
            normalize_terminal_dir(&temp_file),
            normalize_terminal_dir(&std::env::temp_dir())
        );
        let _ = std::fs::remove_file(&temp_file);
        assert_eq!(
            format_wt_dir_args(Path::new(r"X:\")),
            r#"-d "X:\\""#
        );
        assert_eq!(
            format_wt_dir_args(Path::new(r"C:\Users\My Folder")),
            r#"-d "C:\Users\My Folder""#
        );
    }
}
