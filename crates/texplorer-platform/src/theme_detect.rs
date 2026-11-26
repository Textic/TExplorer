//! OS Light/Dark appearance detection for automatic System theme selection.

/// Returns `true` when the OS prefers dark mode for desktop applications.
#[cfg(target_os = "windows")]
pub fn system_prefers_dark_mode() -> bool {
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_READ, REG_DWORD, RegCloseKey, RegOpenKeyExW, RegQueryValueExW,
    };

    let subkey: Vec<u16> =
        "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize\0"
            .encode_utf16()
            .collect();
    let value_name: Vec<u16> = "AppsUseLightTheme\0".encode_utf16().collect();

    let mut key: HKEY = std::ptr::null_mut();
    let open_rc =
        unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, subkey.as_ptr(), 0, KEY_READ, &mut key) };
    if open_rc != 0 {
        return true;
    }

    let mut reg_type: u32 = 0;
    let mut data: u32 = 0;
    let mut data_len: u32 = std::mem::size_of::<u32>() as u32;
    let query_rc = unsafe {
        RegQueryValueExW(
            key,
            value_name.as_ptr(),
            std::ptr::null(),
            &mut reg_type,
            &mut data as *mut u32 as *mut u8,
            &mut data_len,
        )
    };
    unsafe {
        RegCloseKey(key);
    }

    if query_rc == 0 && reg_type == REG_DWORD {
        data == 0
    } else {
        true
    }
}

#[cfg(not(target_os = "windows"))]
pub fn system_prefers_dark_mode() -> bool {
    true
}
