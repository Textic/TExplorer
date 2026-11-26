//! Native Windows SMB / UNC shared folder authentication via `WNetAddConnection2W`.

use texplorer_core::normalize_smb_share_path;

/// Authenticates and connects a Windows SMB/UNC shared folder (e.g. `\\192.168.1.10\Share`).
/// Returns the normalized UNC path ready to navigate via `Location::Local`.
pub fn connect_smb_share(
    raw_path: &str,
    username: Option<&str>,
    password: Option<&str>,
) -> Result<String, String> {
    let unc = normalize_smb_share_path(raw_path)
        .ok_or_else(|| "Invalid SMB share path (expected \\\\server\\share).".to_string())?;

    #[cfg(target_os = "windows")]
    {
        windows_impl::wnet_connect(&unc, username, password)?;
        Ok(unc)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (username, password);
        Ok(unc)
    }
}

/// Disconnects an active Windows SMB/UNC share session.
pub fn disconnect_smb_share(raw_path: &str) -> Result<(), String> {
    let unc = normalize_smb_share_path(raw_path)
        .ok_or_else(|| "Invalid SMB share path.".to_string())?;

    #[cfg(target_os = "windows")]
    {
        windows_impl::wnet_disconnect(&unc)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = unc;
        Ok(())
    }
}

#[cfg(target_os = "windows")]
mod windows_impl {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use std::ptr;

    use windows_sys::Win32::Foundation::{
        ERROR_ACCESS_DENIED, ERROR_ALREADY_ASSIGNED, ERROR_BAD_NET_NAME, ERROR_BAD_NETPATH,
        ERROR_INVALID_PASSWORD, ERROR_LOGON_FAILURE, ERROR_NOT_CONNECTED,
        ERROR_SESSION_CREDENTIAL_CONFLICT, NO_ERROR,
    };
    use windows_sys::Win32::NetworkManagement::WNet::{
        NETRESOURCEW, RESOURCETYPE_DISK, WNetAddConnection2W, WNetCancelConnection2W,
    };

    fn to_wide(s: &str) -> Vec<u16> {
        OsStr::new(s)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    pub fn wnet_connect(
        unc: &str,
        username: Option<&str>,
        password: Option<&str>,
    ) -> Result<(), String> {
        let mut remote_w = to_wide(unc);
        let user_w = username
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(to_wide);
        let pass_w = password.filter(|s| !s.is_empty()).map(to_wide);

        let nr = NETRESOURCEW {
            dwScope: 0,
            dwType: RESOURCETYPE_DISK,
            dwDisplayType: 0,
            dwUsage: 0,
            lpLocalName: ptr::null_mut(),
            lpRemoteName: remote_w.as_mut_ptr(),
            lpComment: ptr::null_mut(),
            lpProvider: ptr::null_mut(),
        };

        let user_ptr = user_w.as_ref().map_or(ptr::null(), |v| v.as_ptr());
        let pass_ptr = pass_w.as_ref().map_or(ptr::null(), |v| v.as_ptr());

        let mut code = unsafe { WNetAddConnection2W(&nr, pass_ptr, user_ptr, 0) };
        if code == ERROR_SESSION_CREDENTIAL_CONFLICT {
            // If an existing session to the same host exists with different credentials,
            // verify if the share is already reachable; otherwise cancel and retry once.
            if std::fs::read_dir(unc).is_ok() {
                return Ok(());
            }
            let _ = unsafe { WNetCancelConnection2W(remote_w.as_ptr(), 0, 1) };
            code = unsafe { WNetAddConnection2W(&nr, pass_ptr, user_ptr, 0) };
        }

        match code {
            NO_ERROR | ERROR_ALREADY_ASSIGNED => Ok(()),
            ERROR_LOGON_FAILURE | ERROR_INVALID_PASSWORD | ERROR_ACCESS_DENIED => {
                Err(format!("Access denied connecting to {unc} — check username and password."))
            }
            ERROR_BAD_NETPATH | ERROR_BAD_NET_NAME => {
                Err(format!("Network share {unc} was not found or is unreachable."))
            }
            ERROR_SESSION_CREDENTIAL_CONFLICT => Err(format!(
                "Windows already has an active session to this server with different credentials ({unc})."
            )),
            other => Err(format!("Could not connect to {unc} (Win32 error {other}).")),
        }
    }

    pub fn wnet_disconnect(unc: &str) -> Result<(), String> {
        let remote_w = to_wide(unc);
        let code = unsafe { WNetCancelConnection2W(remote_w.as_ptr(), 0, 0) };
        match code {
            NO_ERROR | ERROR_NOT_CONNECTED => Ok(()),
            other => Err(format!("Failed to disconnect {unc} (Win32 error {other}).")),
        }
    }
}
