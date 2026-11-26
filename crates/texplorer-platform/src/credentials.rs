//! Native OS encrypted credential storage backed by Windows Credential Manager
//! (`CredWriteW` / `CredReadW` / `CredDeleteW`) so network passwords are never stored in plaintext.

const TARGET_PREFIX: &str = "TExplorer:network:";

fn target_name(conn_id: &str) -> String {
    format!("{TARGET_PREFIX}{conn_id}")
}

/// Saves or updates an encrypted credential for `conn_id` in Windows Credential Manager.
pub fn save_network_password(conn_id: &str, username: &str, password: &str) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        windows_impl::write_cred(&target_name(conn_id), username, password)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (conn_id, username, password);
        Ok(())
    }
}

/// Loads a previously stored password for `conn_id` from Windows Credential Manager.
pub fn load_network_password(conn_id: &str) -> Option<String> {
    #[cfg(target_os = "windows")]
    {
        windows_impl::read_cred(&target_name(conn_id))
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = conn_id;
        None
    }
}

/// Deletes any stored credential for `conn_id` from Windows Credential Manager.
pub fn delete_network_password(conn_id: &str) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        windows_impl::delete_cred(&target_name(conn_id))
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = conn_id;
        Ok(())
    }
}

#[cfg(target_os = "windows")]
mod windows_impl {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use std::ptr;

    use windows_sys::Win32::Foundation::{ERROR_NOT_FOUND, GetLastError};
    use windows_sys::Win32::Security::Credentials::{
        CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW, CredDeleteW, CredFree,
        CredReadW, CredWriteW,
    };

    fn to_wide(s: &str) -> Vec<u16> {
        OsStr::new(s)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    pub fn write_cred(target: &str, username: &str, password: &str) -> Result<(), String> {
        let mut target_w = to_wide(target);
        let mut user_w = to_wide(username);
        let mut secret_bytes = password.as_bytes().to_vec();

        let cred = CREDENTIALW {
            Flags: 0,
            Type: CRED_TYPE_GENERIC,
            TargetName: target_w.as_mut_ptr(),
            Comment: ptr::null_mut(),
            LastWritten: unsafe { std::mem::zeroed() },
            CredentialBlobSize: secret_bytes.len() as u32,
            CredentialBlob: if secret_bytes.is_empty() {
                ptr::null_mut()
            } else {
                secret_bytes.as_mut_ptr()
            },
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            AttributeCount: 0,
            Attributes: ptr::null_mut(),
            TargetAlias: ptr::null_mut(),
            UserName: if username.is_empty() {
                ptr::null_mut()
            } else {
                user_w.as_mut_ptr()
            },
        };

        let ok = unsafe { CredWriteW(&cred, 0) };
        // Wipe temporary secret buffer in memory.
        secret_bytes.fill(0);
        if ok == 0 {
            let err = unsafe { GetLastError() };
            return Err(format!(
                "Failed to save credential in Windows Credential Manager (Win32 error {err})."
            ));
        }
        Ok(())
    }

    pub fn read_cred(target: &str) -> Option<String> {
        let target_w = to_wide(target);
        let mut cred_ptr: *mut CREDENTIALW = ptr::null_mut();
        let ok = unsafe { CredReadW(target_w.as_ptr(), CRED_TYPE_GENERIC, 0, &mut cred_ptr) };
        if ok == 0 || cred_ptr.is_null() {
            return None;
        }
        let result = unsafe {
            let cred = &*cred_ptr;
            if cred.CredentialBlob.is_null() || cred.CredentialBlobSize == 0 {
                Some(String::new())
            } else {
                let slice =
                    std::slice::from_raw_parts(cred.CredentialBlob, cred.CredentialBlobSize as usize);
                String::from_utf8(slice.to_vec()).ok()
            }
        };
        unsafe { CredFree(cred_ptr.cast()) };
        result
    }

    pub fn delete_cred(target: &str) -> Result<(), String> {
        let target_w = to_wide(target);
        let ok = unsafe { CredDeleteW(target_w.as_ptr(), CRED_TYPE_GENERIC, 0) };
        if ok == 0 {
            let err = unsafe { GetLastError() };
            if err != ERROR_NOT_FOUND {
                return Err(format!(
                    "Failed to delete credential from Windows Credential Manager (Win32 error {err})."
                ));
            }
        }
        Ok(())
    }
}
