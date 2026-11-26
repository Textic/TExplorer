//! Safe, reversible, per-user (HKCU) default Windows File Manager registration.
//!
//! Never replaces `explorer.exe` or `Winlogon\Shell` so the Windows taskbar,
//! Start menu, and desktop continue operating normally. Only overrides folder-open
//! handlers under `HKCU\Software\Classes` with automatic JSON backup and one-click restore.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use texplorer_core::AppSettings;

/// Current registration state of T Explorer as the default Windows file manager.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DefaultExplorerStatus {
    Disabled,
    Enabled,
    /// Registered in HKCU, but pointing to an older/different executable path.
    OutdatedPath(PathBuf),
    UnsupportedPlatform,
}

/// Returns `true` when `arg` targets Windows' "This PC" / "Home" / File Explorer root
/// virtual namespace, which TExplorer can render natively on its `Location::Home` page.
pub fn is_home_or_this_pc_namespace(arg: &str) -> bool {
    let trimmed = arg.trim().trim_matches('"');
    let lower = trimmed.to_ascii_lowercase();
    let normalized = lower.strip_prefix("shell:").unwrap_or(&lower);
    matches!(
        normalized,
        "::{20d04fe0-3aea-1069-a2d8-08002b30309d}"
            | "::{f874310e-b6b7-47dc-bc84-b9e6b38f5903}"
            | "::{52205fd8-5dfb-447d-801a-d0b52f2e83e1}"
            | "mycomputerfolder"
            | "homefolder"
    )
}

/// Returns `true` when `arg` targets a Windows virtual Shell namespace (Control Panel,
/// Printers, Settings, CLSID GUID, etc.) that must be handed back to `explorer.exe`.
pub fn should_delegate_to_windows_explorer(arg: &str) -> bool {
    if is_home_or_this_pc_namespace(arg) {
        return false;
    }
    let trimmed = arg.trim().trim_matches('"');
    let lower = trimmed.to_ascii_lowercase();
    lower.starts_with("::{")
        || lower.starts_with("shell:")
        || lower.starts_with("ms-settings:")
        || lower.starts_with("search-ms:")
        || lower.starts_with("ftp://")
        || lower.contains("::{")
}

/// Delegates a system shell namespace or virtual folder argument back to `explorer.exe`.
pub fn delegate_to_windows_explorer(raw_arg: &str) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        use std::process::Command;

        Command::new("explorer.exe")
            .raw_arg(raw_arg)
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("Failed to delegate \"{raw_arg}\" to Windows Explorer: {e}"))
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = raw_arg;
        Err("Delegating to Windows Explorer is only supported on Windows.".to_string())
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct RegistryKeyBackup {
    subkey: String,
    existed: bool,
    default_value: Option<String>,
    delegate_execute: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ShellBackup {
    entries: Vec<RegistryKeyBackup>,
}

fn backup_file_path() -> PathBuf {
    AppSettings::config_dir().join("shell_backup.json")
}

#[cfg(target_os = "windows")]
const FOLDER_COMMAND_KEYS: [&str; 4] = [
    r"Software\Classes\Folder\shell\open\command",
    r"Software\Classes\Folder\shell\explore\command",
    r"Software\Classes\Directory\shell\open\command",
    r"Software\Classes\Drive\shell\open\command",
];

/// Optional `Win+E` handler CLSID verbs used safely in HKCU (same non-hook mechanism as Files App).
#[cfg(target_os = "windows")]
const WIN_E_COMMAND_KEYS: [&str; 2] = [
    r"Software\Classes\CLSID\{52205fd8-5dfb-447d-801a-d0b52f2e83e1}\shell\opennewwindow\command",
    r"Software\Classes\CLSID\{52205fd8-5dfb-447d-801a-d0b52f2e83e1}\shell\open\command",
];

/// Checks whether T Explorer is currently registered as the default Windows file manager.
#[cfg(target_os = "windows")]
pub fn query_default_explorer_status() -> DefaultExplorerStatus {
    let Ok(current_exe) = std::env::current_exe() else {
        return DefaultExplorerStatus::Disabled;
    };
    let expected_cmd = format!("\"{}\" \"%1\"", current_exe.display());

    let Some(actual) = read_hkcu_string(FOLDER_COMMAND_KEYS[0], "") else {
        return DefaultExplorerStatus::Disabled;
    };

    if actual.eq_ignore_ascii_case(&expected_cmd) {
        return DefaultExplorerStatus::Enabled;
    }

    if actual.to_ascii_lowercase().contains("texplorer.exe") {
        let extracted = extract_quoted_exe(&actual).unwrap_or_else(|| PathBuf::from(actual));
        return DefaultExplorerStatus::OutdatedPath(extracted);
    }

    DefaultExplorerStatus::Disabled
}

#[cfg(not(target_os = "windows"))]
pub fn query_default_explorer_status() -> DefaultExplorerStatus {
    DefaultExplorerStatus::UnsupportedPlatform
}

/// Registers T Explorer in `HKCU\Software\Classes` as the default Windows file manager.
#[cfg(target_os = "windows")]
pub fn enable_default_explorer(intercept_win_e: bool) -> Result<(), String> {
    let exe = std::env::current_exe()
        .map_err(|e| format!("Failed resolving current T Explorer executable path: {e}"))?;
    let exe_str = exe.display().to_string();

    // Only write a fresh backup if we are not already registered, preserving pre-T-Explorer values.
    if query_default_explorer_status() == DefaultExplorerStatus::Disabled {
        let mut all_keys: Vec<&str> = FOLDER_COMMAND_KEYS.to_vec();
        all_keys.extend_from_slice(&WIN_E_COMMAND_KEYS);
        let mut backup = ShellBackup::default();
        for key in all_keys {
            let existed = hkcu_key_exists(key);
            let default_value = read_hkcu_string(key, "");
            let delegate_execute = read_hkcu_string(key, "DelegateExecute");
            backup.entries.push(RegistryKeyBackup {
                subkey: key.to_string(),
                existed,
                default_value,
                delegate_execute,
            });
        }
        let dir = AppSettings::config_dir();
        let _ = std::fs::create_dir_all(&dir);
        if let Ok(json) = serde_json::to_string_pretty(&backup) {
            let _ = std::fs::write(backup_file_path(), json);
        }
    }

    let folder_cmd = format!("\"{exe_str}\" \"%1\"");
    for key in FOLDER_COMMAND_KEYS {
        write_hkcu_string(key, "", &folder_cmd)?;
        write_hkcu_string(key, "DelegateExecute", "")?;
    }

    if intercept_win_e {
        let win_e_cmd = format!("\"{exe_str}\"");
        for key in WIN_E_COMMAND_KEYS {
            write_hkcu_string(key, "", &win_e_cmd)?;
            write_hkcu_string(key, "DelegateExecute", "")?;
        }
    } else {
        delete_hkcu_tree(r"Software\Classes\CLSID\{52205fd8-5dfb-447d-801a-d0b52f2e83e1}");
    }

    notify_shell_assoc_changed();
    Ok(())
}

#[cfg(not(target_os = "windows"))]
pub fn enable_default_explorer(_intercept_win_e: bool) -> Result<(), String> {
    Err("Replacing the default file manager is only supported on Windows.".to_string())
}

/// Cleanly restores Windows Explorer as the default file manager using the saved backup
/// (or removing T Explorer's HKCU overrides if no backup exists).
#[cfg(target_os = "windows")]
pub fn restore_windows_explorer() -> Result<(), String> {
    let backup_path = backup_file_path();
    if let Ok(raw) = std::fs::read_to_string(&backup_path)
        && let Ok(backup) = serde_json::from_str::<ShellBackup>(&raw)
    {
        let mut restored_keys = std::collections::HashSet::new();
        for entry in backup.entries {
            restored_keys.insert(entry.subkey.to_ascii_lowercase());
            if !entry.existed {
                delete_hkcu_tree(&entry.subkey);
            } else {
                if let Some(val) = entry.default_value {
                    let _ = write_hkcu_string(&entry.subkey, "", &val);
                } else {
                    delete_hkcu_value(&entry.subkey, "");
                }
                if let Some(del) = entry.delegate_execute {
                    let _ = write_hkcu_string(&entry.subkey, "DelegateExecute", &del);
                } else {
                    delete_hkcu_value(&entry.subkey, "DelegateExecute");
                }
            }
        }
        // Clean up any newly added keys (e.g. Folder\shell\explore\command) absent from older backups.
        for key in FOLDER_COMMAND_KEYS {
            if !restored_keys.contains(&key.to_ascii_lowercase()) {
                delete_hkcu_tree(key);
            }
        }
        let _ = std::fs::remove_file(&backup_path);
    } else {
        for key in FOLDER_COMMAND_KEYS {
            delete_hkcu_tree(key);
        }
    }

    delete_hkcu_tree(r"Software\Classes\CLSID\{52205fd8-5dfb-447d-801a-d0b52f2e83e1}");
    notify_shell_assoc_changed();
    Ok(())
}

#[cfg(not(target_os = "windows"))]
pub fn restore_windows_explorer() -> Result<(), String> {
    Ok(())
}

#[cfg(target_os = "windows")]
fn extract_quoted_exe(cmd: &str) -> Option<PathBuf> {
    let trimmed = cmd.trim();
    if let Some(rest) = trimmed.strip_prefix('"') {
        let end = rest.find('"')?;
        return Some(PathBuf::from(&rest[..end]));
    }
    trimmed.split_whitespace().next().map(PathBuf::from)
}

#[cfg(target_os = "windows")]
fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(target_os = "windows")]
fn hkcu_key_exists(subkey: &str) -> bool {
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_READ, RegCloseKey, RegOpenKeyExW,
    };
    let wide = to_wide(subkey);
    let mut key: HKEY = std::ptr::null_mut();
    let rc = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, wide.as_ptr(), 0, KEY_READ, &mut key) };
    if rc == 0 {
        unsafe {
            RegCloseKey(key);
        }
        true
    } else {
        false
    }
}

#[cfg(target_os = "windows")]
fn read_hkcu_string(subkey: &str, value_name: &str) -> Option<String> {
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_READ, REG_EXPAND_SZ, REG_SZ, RegCloseKey, RegOpenKeyExW,
        RegQueryValueExW,
    };

    let wide_sub = to_wide(subkey);
    let mut key: HKEY = std::ptr::null_mut();
    if unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, wide_sub.as_ptr(), 0, KEY_READ, &mut key) } != 0 {
        return None;
    }

    let wide_val = to_wide(value_name);
    let mut reg_type: u32 = 0;
    let mut byte_len: u32 = 0;
    let probe = unsafe {
        RegQueryValueExW(
            key,
            wide_val.as_ptr(),
            std::ptr::null(),
            &mut reg_type,
            std::ptr::null_mut(),
            &mut byte_len,
        )
    };
    if probe != 0 || (reg_type != REG_SZ && reg_type != REG_EXPAND_SZ) {
        unsafe {
            RegCloseKey(key);
        }
        return None;
    }

    let mut buf = vec![0u16; (byte_len as usize / 2).saturating_add(1)];
    let mut actual_bytes = (buf.len() * 2) as u32;
    let rc = unsafe {
        RegQueryValueExW(
            key,
            wide_val.as_ptr(),
            std::ptr::null(),
            &mut reg_type,
            buf.as_mut_ptr() as *mut u8,
            &mut actual_bytes,
        )
    };
    unsafe {
        RegCloseKey(key);
    }

    if rc != 0 {
        return None;
    }
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..end]))
}

#[cfg(target_os = "windows")]
fn write_hkcu_string(subkey: &str, value_name: &str, value: &str) -> Result<(), String> {
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_WRITE, REG_SZ, RegCloseKey, RegCreateKeyExW, RegSetValueExW,
    };

    let wide_sub = to_wide(subkey);
    let mut key: HKEY = std::ptr::null_mut();
    let create_rc = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            wide_sub.as_ptr(),
            0,
            std::ptr::null(),
            0,
            KEY_WRITE,
            std::ptr::null(),
            &mut key,
            std::ptr::null_mut(),
        )
    };
    if create_rc != 0 {
        return Err(format!(
            "Failed creating registry key HKCU\\{subkey} (code {create_rc})."
        ));
    }

    let wide_name = to_wide(value_name);
    let wide_data = to_wide(value);
    let set_rc = unsafe {
        RegSetValueExW(
            key,
            wide_name.as_ptr(),
            0,
            REG_SZ,
            wide_data.as_ptr() as *const u8,
            (wide_data.len() * 2) as u32,
        )
    };
    unsafe {
        RegCloseKey(key);
    }

    if set_rc != 0 {
        return Err(format!(
            "Failed writing registry value \"{value_name}\" under HKCU\\{subkey} (code {set_rc})."
        ));
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn delete_hkcu_value(subkey: &str, value_name: &str) {
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_WRITE, RegCloseKey, RegDeleteValueW, RegOpenKeyExW,
    };
    let wide_sub = to_wide(subkey);
    let mut key: HKEY = std::ptr::null_mut();
    if unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, wide_sub.as_ptr(), 0, KEY_WRITE, &mut key) } == 0 {
        let wide_val = to_wide(value_name);
        unsafe {
            let _ = RegDeleteValueW(key, wide_val.as_ptr());
            RegCloseKey(key);
        }
    }
}

#[cfg(target_os = "windows")]
fn delete_hkcu_tree(subkey: &str) {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RegDeleteTreeW};
    let wide_sub = to_wide(subkey);
    unsafe {
        let _ = RegDeleteTreeW(HKEY_CURRENT_USER, wide_sub.as_ptr());
    }
}

#[cfg(target_os = "windows")]
fn notify_shell_assoc_changed() {
    use windows_sys::Win32::UI::Shell::{SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify};
    unsafe {
        SHChangeNotify(
            SHCNE_ASSOCCHANGED as i32,
            SHCNF_IDLIST,
            std::ptr::null(),
            std::ptr::null(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifies_system_namespaces_to_delegate() {
        assert!(should_delegate_to_windows_explorer(
            "::{26EE0668-A00A-44D7-9371-BEB064C98683}"
        ));
        assert!(should_delegate_to_windows_explorer("shell:ControlPanelFolder"));
        assert!(should_delegate_to_windows_explorer("ms-settings:storage"));
        assert!(!should_delegate_to_windows_explorer(
            "::{20D04FE0-3AEA-1069-A2D8-08002B30309D}"
        ));
        assert!(is_home_or_this_pc_namespace(
            "shell:::{f874310e-b6b7-47dc-bc84-b9e6b38f5903}"
        ));
        assert!(!should_delegate_to_windows_explorer(r"C:\Users\Admin"));
        assert!(!should_delegate_to_windows_explorer(r"X:\TExplorer"));
        assert!(!should_delegate_to_windows_explorer(r"\\wsl.localhost\Ubuntu"));
    }
}
