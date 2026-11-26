use std::path::PathBuf;

use texplorer_core::{DriveKind, KnownFolder, KnownFolderKind, VolumeInfo};

/// Returns the list of standard user Quick Access folders available on this machine.
pub fn list_known_folders() -> Vec<KnownFolder> {
    let candidates: [(KnownFolderKind, &str, Option<PathBuf>); 7] = [
        (KnownFolderKind::Desktop, "Desktop", dirs::desktop_dir()),
        (KnownFolderKind::Downloads, "Downloads", dirs::download_dir()),
        (KnownFolderKind::Documents, "Documents", dirs::document_dir()),
        (KnownFolderKind::Pictures, "Pictures", dirs::picture_dir()),
        (KnownFolderKind::Music, "Music", dirs::audio_dir()),
        (KnownFolderKind::Videos, "Videos", dirs::video_dir()),
        (KnownFolderKind::UserHome, "User Home", dirs::home_dir()),
    ];

    candidates
        .into_iter()
        .filter_map(|(kind, name, path_opt)| {
            let path = path_opt?;
            if path.exists() {
                Some(KnownFolder {
                    kind,
                    name: name.to_string(),
                    path,
                })
            } else {
                None
            }
        })
        .collect()
}

/// Enumerates mounted storage volumes with capacity, label, and filesystem metadata.
#[cfg(target_os = "windows")]
pub fn list_volumes() -> Vec<VolumeInfo> {
    use std::ffi::OsString;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use windows_sys::Win32::Storage::FileSystem::{
        GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDriveStringsW, GetVolumeInformationW,
    };

    const DRIVE_REMOVABLE: u32 = 2;
    const DRIVE_FIXED: u32 = 3;
    const DRIVE_REMOTE: u32 = 4;
    const DRIVE_CDROM: u32 = 5;
    const DRIVE_RAMDISK: u32 = 6;

    let mut buf = vec![0u16; 512];
    let len = unsafe { GetLogicalDriveStringsW(buf.len() as u32, buf.as_mut_ptr()) } as usize;
    if len == 0 {
        return Vec::new();
    }
    if len > buf.len() {
        buf.resize(len + 2, 0);
        let len2 = unsafe { GetLogicalDriveStringsW(buf.len() as u32, buf.as_mut_ptr()) } as usize;
        buf.truncate(len2);
    } else {
        buf.truncate(len);
    }

    let mut volumes = Vec::new();
    for slice in buf.split(|&c| c == 0) {
        if slice.is_empty() {
            continue;
        }
        let root_os = OsString::from_wide(slice);
        let root_path = PathBuf::from(&root_os);
        let wide_root: Vec<u16> = root_os.encode_wide().chain(std::iter::once(0)).collect();

        let drive_type = unsafe { GetDriveTypeW(wide_root.as_ptr()) };
        let kind = match drive_type {
            DRIVE_FIXED => DriveKind::Fixed,
            DRIVE_REMOVABLE => DriveKind::Removable,
            DRIVE_REMOTE => DriveKind::Network,
            DRIVE_CDROM => DriveKind::Optical,
            DRIVE_RAMDISK => DriveKind::RamDisk,
            _ => DriveKind::Unknown,
        };

        let mut free_avail: u64 = 0;
        let mut total_bytes: u64 = 0;
        let mut total_free: u64 = 0;
        let ok_space = unsafe {
            GetDiskFreeSpaceExW(
                wide_root.as_ptr(),
                &mut free_avail,
                &mut total_bytes,
                &mut total_free,
            )
        };
        if ok_space == 0 || total_bytes == 0 {
            continue;
        }

        let mut vol_name = [0u16; 260];
        let mut fs_name = [0u16; 64];
        let ok_info = unsafe {
            GetVolumeInformationW(
                wide_root.as_ptr(),
                vol_name.as_mut_ptr(),
                vol_name.len() as u32,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                fs_name.as_mut_ptr(),
                fs_name.len() as u32,
            )
        };

        let (label, file_system) = if ok_info != 0 {
            (wide_nul_to_string(&vol_name), wide_nul_to_string(&fs_name))
        } else {
            (String::new(), String::new())
        };

        let short_name = root_path
            .to_string_lossy()
            .trim_end_matches(['\\', '/'])
            .to_string();

        volumes.push(VolumeInfo {
            root_path,
            short_name,
            label,
            file_system,
            kind,
            total_bytes,
            free_bytes: total_free,
        });
    }

    volumes.sort_by(|a, b| a.short_name.cmp(&b.short_name));
    volumes
}

#[cfg(not(target_os = "windows"))]
pub fn list_volumes() -> Vec<VolumeInfo> {
    vec![VolumeInfo {
        root_path: PathBuf::from("/"),
        short_name: "/".to_string(),
        label: "Root".to_string(),
        file_system: "POSIX".to_string(),
        kind: DriveKind::Fixed,
        total_bytes: 0,
        free_bytes: 0,
    }]
}

/// Discovers installed WSL distributions via `HKCU\Software\Microsoft\Windows\CurrentVersion\Lxss`.
#[cfg(target_os = "windows")]
pub fn list_wsl_distros() -> Vec<(String, PathBuf)> {
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_READ, REG_SZ, RegCloseKey, RegEnumKeyExW, RegOpenKeyExW,
        RegQueryValueExW,
    };

    let subkey: Vec<u16> = "Software\\Microsoft\\Windows\\CurrentVersion\\Lxss\0"
        .encode_utf16()
        .collect();
    let mut lxss_key: HKEY = std::ptr::null_mut();
    let rc = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            0,
            KEY_READ,
            &mut lxss_key,
        )
    };
    if rc != 0 {
        return Vec::new();
    }

    let mut distros = Vec::new();
    let mut index: u32 = 0;
    let value_name: Vec<u16> = "DistributionName\0".encode_utf16().collect();

    loop {
        let mut name_buf = [0u16; 256];
        let mut name_len = name_buf.len() as u32;
        let enum_rc = unsafe {
            RegEnumKeyExW(
                lxss_key,
                index,
                name_buf.as_mut_ptr(),
                &mut name_len,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if enum_rc != 0 {
            break;
        }
        index += 1;

        let mut child_key: HKEY = std::ptr::null_mut();
        let open_rc =
            unsafe { RegOpenKeyExW(lxss_key, name_buf.as_ptr(), 0, KEY_READ, &mut child_key) };
        if open_rc != 0 {
            continue;
        }

        let mut reg_type: u32 = 0;
        let mut data_buf = [0u16; 256];
        let mut data_bytes = (data_buf.len() * 2) as u32;
        let q_rc = unsafe {
            RegQueryValueExW(
                child_key,
                value_name.as_ptr(),
                std::ptr::null(),
                &mut reg_type,
                data_buf.as_mut_ptr() as *mut u8,
                &mut data_bytes,
            )
        };
        unsafe {
            RegCloseKey(child_key);
        }

        if q_rc == 0 && reg_type == REG_SZ && data_bytes >= 2 {
            let distro = wide_nul_to_string(&data_buf);
            if !distro.is_empty() && !distro.starts_with("docker-desktop") {
                let unc = PathBuf::from(format!(r"\\wsl.localhost\{distro}"));
                distros.push((distro, unc));
            }
        }
    }

    unsafe {
        RegCloseKey(lxss_key);
    }

    distros.sort_by(|a, b| a.0.cmp(&b.0));
    distros
}

#[cfg(not(target_os = "windows"))]
pub fn list_wsl_distros() -> Vec<(String, PathBuf)> {
    Vec::new()
}

#[cfg(target_os = "windows")]
fn wide_nul_to_string(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end]).trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_at_least_one_volume() {
        let vols = list_volumes();
        assert!(!vols.is_empty());
    }
}
