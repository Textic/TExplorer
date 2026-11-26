use std::path::{Path, PathBuf};
use std::process::Command;

/// Opens a file or folder with the OS default handler.
pub fn open_item(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::UI::Shell::ShellExecuteW;
        use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

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
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("Failed to launch Windows Explorer: {e}"))
    }
    #[cfg(not(target_os = "windows"))]
    {
        open_item(path)
    }
}

/// Opens a native terminal in `path` (or its parent directory if `path` is a file).
pub fn open_terminal_here(path: &Path, preferred_cmd: &str) -> Result<(), String> {
    let dir = if path.is_dir() {
        path
    } else {
        path.parent().unwrap_or(path)
    };

    #[cfg(target_os = "windows")]
    {
        let preferred = preferred_cmd.trim();
        if (preferred.is_empty() || preferred.eq_ignore_ascii_case("wt.exe") || preferred.eq_ignore_ascii_case("wt"))
            && Command::new("wt.exe").arg("-d").arg(dir).spawn().is_ok()
        {
            return Ok(());
        }
        if !preferred.is_empty()
            && !preferred.eq_ignore_ascii_case("wt.exe")
            && !preferred.eq_ignore_ascii_case("wt")
            && Command::new(preferred).current_dir(dir).spawn().is_ok()
        {
            return Ok(());
        }
        let escaped = dir.to_string_lossy().replace('\'', "''");
        Command::new("powershell.exe")
            .arg("-NoExit")
            .arg("-Command")
            .arg(format!("Set-Location -LiteralPath '{escaped}'"))
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
        if Command::new(cmd).arg(path).spawn().is_ok() {
            return Ok(());
        }
        // Handle `.cmd` shims such as VS Code's `code.cmd`.
        Command::new("cmd.exe")
            .args(["/C", cmd])
            .arg(path)
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
pub fn move_to_trash(paths: &[PathBuf]) -> Result<(), String> {
    if paths.is_empty() {
        return Ok(());
    }
    trash::delete_all(paths).map_err(|e| format!("Failed to move items to Recycle Bin: {e}"))
}

/// Permanently removes a file, symlink, or directory iteratively without following symlinks/junctions.
pub fn delete_permanently(path: &Path) -> Result<(), String> {
    let mut stack: Vec<(PathBuf, bool)> = vec![(path.to_path_buf(), false)];
    while let Some((cur, ready)) = stack.pop() {
        if ready {
            remove_dir_readonly(&cur)?;
            continue;
        }
        let meta = std::fs::symlink_metadata(&cur)
            .map_err(|e| format!("Failed to inspect \"{}\": {e}", cur.display()))?;
        let ft = meta.file_type();
        if ft.is_symlink() {
            if std::fs::remove_file(&cur).is_err() {
                remove_dir_readonly(&cur)?;
            }
        } else if ft.is_dir() {
            stack.push((cur.clone(), true));
            for entry in std::fs::read_dir(&cur)
                .map_err(|e| format!("Failed to read \"{}\": {e}", cur.display()))?
            {
                let entry = entry.map_err(|e| format!("Failed enumerating directory: {e}"))?;
                stack.push((entry.path(), false));
            }
        } else {
            remove_file_readonly(&cur)?;
        }
    }
    Ok(())
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

fn remove_file_readonly(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            clear_readonly(path);
            std::fs::remove_file(path)
                .map_err(|err| format!("Failed to delete \"{}\": {err}", path.display()))
        }
        Err(e) => Err(format!("Failed to delete \"{}\": {e}", path.display())),
    }
}

fn remove_dir_readonly(path: &Path) -> Result<(), String> {
    match std::fs::remove_dir(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            clear_readonly(path);
            std::fs::remove_dir(path)
                .map_err(|err| format!("Failed to delete folder \"{}\": {err}", path.display()))
        }
        Err(e) => Err(format!("Failed to delete folder \"{}\": {e}", path.display())),
    }
}

/// Locates `textree.exe` either from user settings or common sibling/install locations.
pub fn find_textree_exe(configured: Option<&Path>) -> Option<PathBuf> {
    if let Some(p) = configured
        && p.is_file()
    {
        return Some(p.to_path_buf());
    }

    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        candidates.push(dir.join("textree.exe"));
    }
    if let Some(local) = dirs::data_local_dir() {
        candidates.push(local.join("Programs").join("TexTree").join("textree.exe"));
    }
    candidates.push(PathBuf::from(r"X:\TexTree\target\release\textree.exe"));
    candidates.push(PathBuf::from(r"X:\TexTree\target\debug\textree.exe"));

    candidates.into_iter().find(|p| p.is_file())
}
