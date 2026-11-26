//! `texplorer-service` — Dedicated read-only Windows background service maintaining an
//! in-RAM NTFS `$MFT` index updated live via the NTFS USN Change Journal.
//!
//! Security boundary:
//! - Strictly READ-ONLY: zero delete, write, rename, or process-execution endpoints exist.
//! - Local-only Named Pipe (`PIPE_REJECT_REMOTE_CLIENTS`) with DACL restricted to LocalSystem,
//!   Administrators, and authenticated local users (`SY`, `BA`, `AU`).

#[cfg(not(target_os = "windows"))]
fn main() {
    eprintln!("texplorer-service is only supported on Windows.");
}

#[cfg(target_os = "windows")]
fn main() {
    let arg = std::env::args().nth(1);
    let result = match arg.as_deref() {
        Some("--install") => win_service::install_and_start(),
        Some("--uninstall") => win_service::stop_and_uninstall(),
        Some("--start") => win_service::start_existing(),
        Some("--restart-if-installed") => win_service::restart_if_installed(),
        Some("--stop") => win_service::stop_existing(),
        Some("--status") => {
            println!("{}", win_service::query_status_label());
            Ok(())
        }
        Some("--console") => win_service::run_console_server(),
        Some(other) => Err(format!("Unknown argument: {other}")),
        None => win_service::run_scm_service(),
    };

    if let Err(err) = result {
        eprintln!("texplorer-service error: {err}");
        std::process::exit(1);
    }
}

#[cfg(target_os = "windows")]
mod win_service {
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU64, Ordering};
    use std::sync::{Arc, RwLock};
    use std::time::{Duration, Instant};

    use rayon::prelude::*;
    use texplorer_mft::mft_raw::scan_volume_mft_entries;
    use texplorer_mft::protocol::{
        FLAG_IS_DIR, FRAME_JSON_REQ, FRAME_JSON_RESP, IndexRequest, IndexResponse, PIPE_NAME,
        PROTOCOL_VERSION, SERVICE_DESCRIPTION, SERVICE_DISPLAY_NAME, SERVICE_NAME, read_frame,
        write_json_frame,
    };
    use texplorer_mft::usn::{UsnDelta, poll_usn_changes, query_usn_journal};
    use texplorer_mft::{MultiVolumeIndex, VolumeIndex};
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_IO_PENDING, ERROR_PIPE_CONNECTED, ERROR_SERVICE_ALREADY_RUNNING,
        ERROR_SERVICE_DOES_NOT_EXIST, ERROR_SERVICE_EXISTS, ERROR_SERVICE_NOT_ACTIVE,
        GetLastError, HANDLE, INVALID_HANDLE_VALUE, LocalFree, WAIT_OBJECT_0,
    };
    use windows_sys::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };
    use windows_sys::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_OVERLAPPED, FlushFileBuffers, GetDriveTypeW, GetLogicalDriveStringsW,
        GetVolumeInformationW, PIPE_ACCESS_DUPLEX, ReadFile, WriteFile,
    };
    use windows_sys::Win32::System::IO::{GetOverlappedResult, OVERLAPPED};
    use windows_sys::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE,
        PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
    };
    use windows_sys::Win32::System::Services::{
        ChangeServiceConfig2W, ChangeServiceConfigW, CloseServiceHandle, ControlService,
        CreateServiceW, DeleteService, OpenSCManagerW, OpenServiceW, QueryServiceStatus,
        RegisterServiceCtrlHandlerExW, SC_HANDLE, SC_MANAGER_ALL_ACCESS, SC_MANAGER_CONNECT,
        SERVICE_ACCEPT_SHUTDOWN, SERVICE_ACCEPT_STOP, SERVICE_ALL_ACCESS, SERVICE_AUTO_START,
        SERVICE_CONFIG_DESCRIPTION, SERVICE_CONTROL_INTERROGATE, SERVICE_CONTROL_SHUTDOWN,
        SERVICE_CONTROL_STOP, SERVICE_DESCRIPTIONW, SERVICE_ERROR_NORMAL, SERVICE_QUERY_STATUS,
        SERVICE_RUNNING, SERVICE_START_PENDING, SERVICE_STATUS, SERVICE_STATUS_HANDLE,
        SERVICE_STOP_PENDING, SERVICE_STOPPED, SERVICE_TABLE_ENTRYW, SERVICE_WIN32_OWN_PROCESS,
        SetServiceStatus, StartServiceCtrlDispatcherW, StartServiceW,
    };
    use windows_sys::Win32::System::Threading::{
        CreateEventW, INFINITE, SetEvent, WaitForMultipleObjects, WaitForSingleObject,
    };

    const DRIVE_FIXED_TYPE: u32 = 3;

    static STOP_FLAG: AtomicBool = AtomicBool::new(false);
    static STOP_EVENT_HANDLE: AtomicIsize = AtomicIsize::new(0);
    static STATUS_HANDLE_RAW: AtomicIsize = AtomicIsize::new(0);
    static INDEX_GENERATION: AtomicU64 = AtomicU64::new(0);

    fn to_wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    struct ScHandleGuard(SC_HANDLE);
    impl Drop for ScHandleGuard {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    CloseServiceHandle(self.0);
                }
            }
        }
    }

    struct HandleGuard(HANDLE);
    impl Drop for HandleGuard {
        fn drop(&mut self) {
            if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
                unsafe {
                    CloseHandle(self.0);
                }
            }
        }
    }

    pub fn query_status_label() -> &'static str {
        let scm = unsafe { OpenSCManagerW(std::ptr::null(), std::ptr::null(), SC_MANAGER_CONNECT) };
        if scm.is_null() {
            return "unknown";
        }
        let _scm = ScHandleGuard(scm);
        let name = to_wide(SERVICE_NAME);
        let svc = unsafe { OpenServiceW(scm, name.as_ptr(), SERVICE_QUERY_STATUS) };
        if svc.is_null() {
            return "not_installed";
        }
        let _svc = ScHandleGuard(svc);
        let mut status: SERVICE_STATUS = unsafe { std::mem::zeroed() };
        if unsafe { QueryServiceStatus(svc, &mut status) } == 0 {
            return "unknown";
        }
        if status.dwCurrentState == SERVICE_RUNNING {
            "running"
        } else {
            "stopped"
        }
    }

    pub fn install_and_start() -> Result<(), String> {
        let exe = std::env::current_exe()
            .map_err(|e| format!("Failed resolving service executable path: {e}"))?;
        let quoted_path = format!("\"{}\"", exe.to_string_lossy());
        let wide_name = to_wide(SERVICE_NAME);
        let wide_display = to_wide(SERVICE_DISPLAY_NAME);
        let wide_bin = to_wide(&quoted_path);

        let scm =
            unsafe { OpenSCManagerW(std::ptr::null(), std::ptr::null(), SC_MANAGER_ALL_ACCESS) };
        if scm.is_null() {
            return Err(format!(
                "OpenSCManagerW failed (error {}). Run as Administrator.",
                unsafe { GetLastError() }
            ));
        }
        let _scm = ScHandleGuard(scm);

        let svc = unsafe {
            CreateServiceW(
                scm,
                wide_name.as_ptr(),
                wide_display.as_ptr(),
                SERVICE_ALL_ACCESS,
                SERVICE_WIN32_OWN_PROCESS,
                SERVICE_AUTO_START,
                SERVICE_ERROR_NORMAL,
                wide_bin.as_ptr(),
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(), // LocalSystem
                std::ptr::null(),
            )
        };

        let svc = if svc.is_null() {
            let err = unsafe { GetLastError() };
            if err != ERROR_SERVICE_EXISTS {
                return Err(format!("CreateServiceW failed with Win32 error {err}."));
            }
            let existing = unsafe { OpenServiceW(scm, wide_name.as_ptr(), SERVICE_ALL_ACCESS) };
            if existing.is_null() {
                return Err(format!(
                    "OpenServiceW failed with Win32 error {}.",
                    unsafe { GetLastError() }
                ));
            }
            let cfg_ok = unsafe {
                ChangeServiceConfigW(
                    existing,
                    SERVICE_WIN32_OWN_PROCESS,
                    SERVICE_AUTO_START,
                    SERVICE_ERROR_NORMAL,
                    wide_bin.as_ptr(),
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    std::ptr::null(),
                    std::ptr::null(),
                    wide_display.as_ptr(),
                )
            };
            if cfg_ok == 0 {
                let _g = ScHandleGuard(existing);
                return Err(format!(
                    "ChangeServiceConfigW failed with Win32 error {}.",
                    unsafe { GetLastError() }
                ));
            }
            existing
        } else {
            svc
        };
        let _svc = ScHandleGuard(svc);

        let mut wide_desc = to_wide(SERVICE_DESCRIPTION);
        let desc = SERVICE_DESCRIPTIONW {
            lpDescription: wide_desc.as_mut_ptr(),
        };
        unsafe {
            let _ = ChangeServiceConfig2W(
                svc,
                SERVICE_CONFIG_DESCRIPTION,
                (&desc as *const SERVICE_DESCRIPTIONW).cast(),
            );
        }

        start_service_handle(svc)
    }

    pub fn start_existing() -> Result<(), String> {
        let scm =
            unsafe { OpenSCManagerW(std::ptr::null(), std::ptr::null(), SC_MANAGER_ALL_ACCESS) };
        if scm.is_null() {
            return Err(format!(
                "OpenSCManagerW failed with Win32 error {}.",
                unsafe { GetLastError() }
            ));
        }
        let _scm = ScHandleGuard(scm);
        let wide_name = to_wide(SERVICE_NAME);
        let svc = unsafe { OpenServiceW(scm, wide_name.as_ptr(), SERVICE_ALL_ACCESS) };
        if svc.is_null() {
            return Err("TExplorerIndexService is not installed.".to_string());
        }
        let _svc = ScHandleGuard(svc);
        start_service_handle(svc)
    }

    pub fn restart_if_installed() -> Result<(), String> {
        let scm =
            unsafe { OpenSCManagerW(std::ptr::null(), std::ptr::null(), SC_MANAGER_ALL_ACCESS) };
        if scm.is_null() {
            return Ok(());
        }
        let _scm = ScHandleGuard(scm);
        let wide_name = to_wide(SERVICE_NAME);
        let svc = unsafe { OpenServiceW(scm, wide_name.as_ptr(), SERVICE_ALL_ACCESS) };
        if svc.is_null() {
            return Ok(());
        }
        let _svc = ScHandleGuard(svc);
        let _ = stop_service_handle(svc);
        start_service_handle(svc)
    }

    pub fn stop_existing() -> Result<(), String> {
        let scm =
            unsafe { OpenSCManagerW(std::ptr::null(), std::ptr::null(), SC_MANAGER_ALL_ACCESS) };
        if scm.is_null() {
            return Err(format!(
                "OpenSCManagerW failed with Win32 error {}.",
                unsafe { GetLastError() }
            ));
        }
        let _scm = ScHandleGuard(scm);
        let wide_name = to_wide(SERVICE_NAME);
        let svc = unsafe { OpenServiceW(scm, wide_name.as_ptr(), SERVICE_ALL_ACCESS) };
        if svc.is_null() {
            let err = unsafe { GetLastError() };
            if err == ERROR_SERVICE_DOES_NOT_EXIST {
                return Ok(());
            }
            return Err(format!("OpenServiceW failed with Win32 error {err}."));
        }
        let _svc = ScHandleGuard(svc);
        stop_service_handle(svc)
    }

    pub fn stop_and_uninstall() -> Result<(), String> {
        let scm =
            unsafe { OpenSCManagerW(std::ptr::null(), std::ptr::null(), SC_MANAGER_ALL_ACCESS) };
        if scm.is_null() {
            return Err(format!(
                "OpenSCManagerW failed with Win32 error {}.",
                unsafe { GetLastError() }
            ));
        }
        let _scm = ScHandleGuard(scm);
        let wide_name = to_wide(SERVICE_NAME);
        let svc = unsafe { OpenServiceW(scm, wide_name.as_ptr(), SERVICE_ALL_ACCESS) };
        if svc.is_null() {
            let err = unsafe { GetLastError() };
            if err == ERROR_SERVICE_DOES_NOT_EXIST {
                return Ok(());
            }
            return Err(format!("OpenServiceW failed with Win32 error {err}."));
        }
        let _svc = ScHandleGuard(svc);
        let _ = stop_service_handle(svc);
        let del_ok = unsafe { DeleteService(svc) };
        if del_ok == 0 {
            let err = unsafe { GetLastError() };
            return Err(format!("DeleteService failed with Win32 error {err}."));
        }
        Ok(())
    }

    fn start_service_handle(svc: SC_HANDLE) -> Result<(), String> {
        let rc = unsafe { StartServiceW(svc, 0, std::ptr::null()) };
        if rc == 0 {
            let err = unsafe { GetLastError() };
            if err != ERROR_SERVICE_ALREADY_RUNNING {
                return Err(format!("StartServiceW failed with Win32 error {err}."));
            }
            return Ok(());
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            let mut st: SERVICE_STATUS = unsafe { std::mem::zeroed() };
            if unsafe { QueryServiceStatus(svc, &mut st) } != 0
                && st.dwCurrentState == SERVICE_RUNNING
            {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Ok(())
    }

    fn stop_service_handle(svc: SC_HANDLE) -> Result<(), String> {
        let mut st: SERVICE_STATUS = unsafe { std::mem::zeroed() };
        let rc = unsafe { ControlService(svc, SERVICE_CONTROL_STOP, &mut st) };
        if rc == 0 {
            let err = unsafe { GetLastError() };
            if err == ERROR_SERVICE_NOT_ACTIVE {
                return Ok(());
            }
            return Err(format!("ControlService(STOP) failed with Win32 error {err}."));
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if unsafe { QueryServiceStatus(svc, &mut st) } != 0
                && st.dwCurrentState == SERVICE_STOPPED
            {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Ok(())
    }

    pub fn run_console_server() -> Result<(), String> {
        let stop_evt = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
        if stop_evt.is_null() {
            return Err("Failed to create stop event.".to_string());
        }
        STOP_FLAG.store(false, Ordering::SeqCst);
        STOP_EVENT_HANDLE.store(stop_evt as isize, Ordering::SeqCst);
        let _guard = HandleGuard(stop_evt);

        let index = Arc::new(RwLock::new(MultiVolumeIndex::default()));
        spawn_full_index_build(Arc::clone(&index));
        serve_named_pipe_loop(stop_evt, index);
        Ok(())
    }

    pub fn run_scm_service() -> Result<(), String> {
        let mut wide_name = to_wide(SERVICE_NAME);
        let table = [
            SERVICE_TABLE_ENTRYW {
                lpServiceName: wide_name.as_mut_ptr(),
                lpServiceProc: Some(service_main),
            },
            SERVICE_TABLE_ENTRYW {
                lpServiceName: std::ptr::null_mut(),
                lpServiceProc: None,
            },
        ];
        let ok = unsafe { StartServiceCtrlDispatcherW(table.as_ptr()) };
        if ok == 0 {
            return Err(format!(
                "StartServiceCtrlDispatcherW failed (error {}). Pass --console or --install to run interactively.",
                unsafe { GetLastError() }
            ));
        }
        Ok(())
    }

    fn report_status(state: u32, exit_code: u32, wait_hint: u32) {
        let raw = STATUS_HANDLE_RAW.load(Ordering::SeqCst);
        if raw == 0 {
            return;
        }
        let handle = raw as SERVICE_STATUS_HANDLE;
        let controls = if state == SERVICE_RUNNING {
            SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SHUTDOWN
        } else {
            0
        };
        let status = SERVICE_STATUS {
            dwServiceType: SERVICE_WIN32_OWN_PROCESS,
            dwCurrentState: state,
            dwControlsAccepted: controls,
            dwWin32ExitCode: exit_code,
            dwServiceSpecificExitCode: 0,
            dwCheckPoint: 0,
            dwWaitHint: wait_hint,
        };
        unsafe {
            let _ = SetServiceStatus(handle, &status);
        }
    }

    unsafe extern "system" fn service_ctrl_handler(
        control: u32,
        _event_type: u32,
        _event_data: *mut core::ffi::c_void,
        _context: *mut core::ffi::c_void,
    ) -> u32 {
        match control {
            SERVICE_CONTROL_STOP | SERVICE_CONTROL_SHUTDOWN => {
                STOP_FLAG.store(true, Ordering::SeqCst);
                report_status(SERVICE_STOP_PENDING, 0, 3000);
                let evt = STOP_EVENT_HANDLE.load(Ordering::SeqCst) as HANDLE;
                if !evt.is_null() {
                    unsafe {
                        let _ = SetEvent(evt);
                    }
                }
                0
            }
            SERVICE_CONTROL_INTERROGATE => 0,
            _ => 120,
        }
    }

    unsafe extern "system" fn service_main(_argc: u32, _argv: *mut *mut u16) {
        let wide_name = to_wide(SERVICE_NAME);
        let status_handle = unsafe {
            RegisterServiceCtrlHandlerExW(
                wide_name.as_ptr(),
                Some(service_ctrl_handler),
                std::ptr::null_mut(),
            )
        };
        if status_handle.is_null() {
            return;
        }
        STATUS_HANDLE_RAW.store(status_handle as isize, Ordering::SeqCst);
        report_status(SERVICE_START_PENDING, 0, 3000);

        let stop_evt = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
        if stop_evt.is_null() {
            report_status(SERVICE_STOPPED, 1, 0);
            return;
        }
        STOP_FLAG.store(false, Ordering::SeqCst);
        STOP_EVENT_HANDLE.store(stop_evt as isize, Ordering::SeqCst);
        let _guard = HandleGuard(stop_evt);

        let index = Arc::new(RwLock::new(MultiVolumeIndex::default()));
        spawn_full_index_build(Arc::clone(&index));

        report_status(SERVICE_RUNNING, 0, 0);
        serve_named_pipe_loop(stop_evt, index);
        report_status(SERVICE_STOPPED, 0, 0);
    }

    /// Enumerates all fixed local NTFS drive letters (`C`, `D`, `X`, ...).
    fn enumerate_fixed_ntfs_drives() -> Vec<char> {
        let mut buf = vec![0u16; 512];
        let len = unsafe { GetLogicalDriveStringsW(buf.len() as u32, buf.as_mut_ptr()) } as usize;
        if len == 0 || len > buf.len() {
            return Vec::new();
        }
        let mut out = Vec::new();
        for chunk in buf[..len].split(|&c| c == 0) {
            if chunk.len() < 2 {
                continue;
            }
            let root_str = String::from_utf16_lossy(chunk);
            let wide_root = to_wide(&root_str);
            if unsafe { GetDriveTypeW(wide_root.as_ptr()) } != DRIVE_FIXED_TYPE {
                continue;
            }
            let mut fs_buf = [0u16; 64];
            let ok = unsafe {
                GetVolumeInformationW(
                    wide_root.as_ptr(),
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    fs_buf.as_mut_ptr(),
                    fs_buf.len() as u32,
                )
            };
            if ok != 0 {
                let fs_len = fs_buf.iter().position(|&c| c == 0).unwrap_or(fs_buf.len());
                let fs_name = String::from_utf16_lossy(&fs_buf[..fs_len]);
                if fs_name.eq_ignore_ascii_case("NTFS")
                    && let Some(drive) = root_str.chars().next()
                    && drive.is_ascii_alphabetic()
                {
                    out.push(drive.to_ascii_uppercase());
                }
            }
        }
        out
    }

    /// Scans all fixed NTFS volumes in parallel and starts a live USN Change Journal watcher per volume.
    fn spawn_full_index_build(index: Arc<RwLock<MultiVolumeIndex>>) {
        let generation = INDEX_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
        if let Ok(mut guard) = index.write() {
            guard.indexing = true;
        }

        std::thread::spawn(move || {
            struct BuiltVolume {
                drive: char,
                volume: VolumeIndex,
                usn_checkpoint: Option<(u64, i64)>,
            }
            let drives = enumerate_fixed_ntfs_drives();
            let built: Vec<BuiltVolume> = drives
                .into_par_iter()
                .filter_map(|drive| {
                    if STOP_FLAG.load(Ordering::Relaxed)
                        || INDEX_GENERATION.load(Ordering::SeqCst) != generation
                    {
                        return None;
                    }
                    // Capture USN checkpoint BEFORE scanning $MFT so no changes are lost during load.
                    let usn_checkpoint = query_usn_journal(drive)
                        .ok()
                        .map(|info| (info.journal_id, info.next_usn));

                    let entries = scan_volume_mft_entries(drive, &STOP_FLAG).ok()?;
                    let mut vol = VolumeIndex::from_mft_entries(drive, entries);
                    if let Some((jid, next_usn)) = usn_checkpoint {
                        vol.usn_journal_id = jid;
                        vol.next_usn = next_usn;
                        vol.usn_active = true;
                    }
                    Some(BuiltVolume {
                        drive,
                        volume: vol,
                        usn_checkpoint,
                    })
                })
                .collect();

            if STOP_FLAG.load(Ordering::Relaxed)
                || INDEX_GENERATION.load(Ordering::SeqCst) != generation
            {
                return;
            }

            let mut watchers = Vec::new();
            if let Ok(mut guard) = index.write() {
                guard.volumes.clear();
                for item in built {
                    guard.insert_volume(item.volume);
                    if let Some((jid, next_usn)) = item.usn_checkpoint {
                        watchers.push((item.drive, jid, next_usn));
                    }
                }
                guard.indexing = false;
                guard.ready = true;
            }

            for (drive, journal_id, next_usn) in watchers {
                spawn_usn_watcher(Arc::clone(&index), drive, journal_id, next_usn, generation);
            }
        });
    }

    /// Lightweight per-volume USN Change Journal loop keeping the RAM index & folder sizes live.
    fn spawn_usn_watcher(
        index: Arc<RwLock<MultiVolumeIndex>>,
        drive: char,
        journal_id: u64,
        mut next_usn: i64,
        generation: u64,
    ) {
        std::thread::spawn(move || {
            while !STOP_FLAG.load(Ordering::Relaxed)
                && INDEX_GENERATION.load(Ordering::SeqCst) == generation
            {
                std::thread::sleep(Duration::from_millis(350));
                if STOP_FLAG.load(Ordering::Relaxed)
                    || INDEX_GENERATION.load(Ordering::SeqCst) != generation
                {
                    break;
                }

                match poll_usn_changes(drive, journal_id, next_usn) {
                    Ok((new_next, deltas)) => {
                        next_usn = new_next;
                        if deltas.is_empty() {
                            continue;
                        }
                        if let Ok(mut guard) = index.write()
                            && let Some(vol) = guard.volumes.get_mut(&drive)
                        {
                            vol.next_usn = new_next;
                            let mut size_refresh_refs: Vec<u64> = Vec::new();
                            for delta in deltas {
                                if let UsnDelta::Upsert {
                                    file_ref, flags, ..
                                } = &delta
                                    && (*flags & FLAG_IS_DIR) == 0
                                {
                                    size_refresh_refs.push(*file_ref);
                                }
                                vol.apply_usn_delta(delta);
                            }

                            // Refresh file sizes for newly created/modified files while keeping lock brief.
                            let paths_to_stat: Vec<(u64, String)> = size_refresh_refs
                                .into_iter()
                                .take(256)
                                .filter_map(|r| Some((r, vol.reconstruct_path(r)?)))
                                .collect();
                            drop(guard);

                            let mut meta_updates = Vec::new();
                            for (fref, path) in paths_to_stat {
                                if let Ok(meta) = std::fs::metadata(&path)
                                    && meta.is_file()
                                {
                                    let mtime = meta
                                        .modified()
                                        .ok()
                                        .and_then(|t| {
                                            t.duration_since(std::time::UNIX_EPOCH).ok()
                                        })
                                        .map(|d| d.as_secs() as i64)
                                        .unwrap_or(0);
                                    meta_updates.push((fref, meta.len(), mtime));
                                }
                            }
                            if !meta_updates.is_empty()
                                && let Ok(mut guard) = index.write()
                                && let Some(vol) = guard.volumes.get_mut(&drive)
                            {
                                for (file_ref, size, modified_secs) in meta_updates {
                                    vol.apply_usn_delta(UsnDelta::UpdateMetadata {
                                        file_ref,
                                        size,
                                        modified_secs,
                                    });
                                }
                            }
                        }
                    }
                    Err(_) => {
                        if let Ok(mut guard) = index.write()
                            && let Some(vol) = guard.volumes.get_mut(&drive)
                        {
                            vol.usn_active = false;
                        }
                        break;
                    }
                }
            }
        });
    }

    fn create_secure_pipe_instance() -> Option<HANDLE> {
        let sddl = to_wide("D:(A;;GA;;;SY)(A;;GA;;;BA)(A;;GRGW;;;AU)");
        let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        let sd_ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut sd,
                std::ptr::null_mut(),
            )
        };
        if sd_ok == 0 || sd.is_null() {
            return None;
        }

        let sa = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: sd,
            bInheritHandle: 0,
        };
        let wide_pipe = to_wide(PIPE_NAME);
        let pipe = unsafe {
            CreateNamedPipeW(
                wide_pipe.as_ptr(),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                256 * 1024,
                64 * 1024,
                0,
                &sa,
            )
        };
        unsafe {
            LocalFree(sd);
        }
        if pipe == INVALID_HANDLE_VALUE || pipe.is_null() {
            None
        } else {
            Some(pipe)
        }
    }

    fn serve_named_pipe_loop(stop_evt: HANDLE, index: Arc<RwLock<MultiVolumeIndex>>) {
        let connect_evt = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
        if connect_evt.is_null() {
            return;
        }
        let _connect_guard = HandleGuard(connect_evt);

        while !STOP_FLAG.load(Ordering::SeqCst) {
            let Some(pipe) = create_secure_pipe_instance() else {
                std::thread::sleep(Duration::from_millis(200));
                continue;
            };

            let mut ov: OVERLAPPED = unsafe { std::mem::zeroed() };
            ov.hEvent = connect_evt;

            let connected = unsafe { ConnectNamedPipe(pipe, &mut ov) };
            let client_ready = if connected != 0 {
                true
            } else {
                let err = unsafe { GetLastError() };
                if err == ERROR_PIPE_CONNECTED {
                    true
                } else if err == ERROR_IO_PENDING {
                    let handles = [stop_evt, connect_evt];
                    let wait = unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, INFINITE) };
                    if wait == WAIT_OBJECT_0 {
                        unsafe {
                            CloseHandle(pipe);
                        }
                        break;
                    }
                    let mut transferred: u32 = 0;
                    unsafe { GetOverlappedResult(pipe, &ov, &mut transferred, 0) != 0 }
                } else {
                    false
                }
            };

            if !client_ready {
                unsafe {
                    CloseHandle(pipe);
                }
                continue;
            }

            let pipe_raw = pipe as usize;
            let idx_clone = Arc::clone(&index);
            std::thread::spawn(move || {
                handle_client(pipe_raw as HANDLE, idx_clone);
            });
        }
    }

    struct OverlappedPipe {
        handle: HANDLE,
        io_event: HANDLE,
    }

    impl OverlappedPipe {
        fn new(handle: HANDLE) -> Option<Self> {
            let io_event = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
            if io_event.is_null() {
                None
            } else {
                Some(Self { handle, io_event })
            }
        }
    }

    impl Drop for OverlappedPipe {
        fn drop(&mut self) {
            unsafe {
                let _ = FlushFileBuffers(self.handle);
                let _ = DisconnectNamedPipe(self.handle);
                CloseHandle(self.io_event);
                CloseHandle(self.handle);
            }
        }
    }

    impl Read for OverlappedPipe {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let mut ov: OVERLAPPED = unsafe { std::mem::zeroed() };
            ov.hEvent = self.io_event;
            let mut read_bytes: u32 = 0;
            let ok = unsafe {
                ReadFile(
                    self.handle,
                    buf.as_mut_ptr(),
                    buf.len().min(u32::MAX as usize) as u32,
                    &mut read_bytes,
                    &mut ov,
                )
            };
            if ok != 0 {
                return Ok(read_bytes as usize);
            }
            let err = unsafe { GetLastError() };
            if err == ERROR_IO_PENDING {
                let wait = unsafe { WaitForSingleObject(self.io_event, INFINITE) };
                if wait != WAIT_OBJECT_0 {
                    return Err(std::io::Error::other("Overlapped pipe read wait failed"));
                }
                if unsafe { GetOverlappedResult(self.handle, &ov, &mut read_bytes, 0) } != 0 {
                    return Ok(read_bytes as usize);
                }
                return Err(std::io::Error::from_raw_os_error(
                    unsafe { GetLastError() } as i32,
                ));
            }
            Err(std::io::Error::from_raw_os_error(err as i32))
        }
    }

    impl Write for OverlappedPipe {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            let mut ov: OVERLAPPED = unsafe { std::mem::zeroed() };
            ov.hEvent = self.io_event;
            let mut written: u32 = 0;
            let ok = unsafe {
                WriteFile(
                    self.handle,
                    buf.as_ptr(),
                    buf.len().min(u32::MAX as usize) as u32,
                    &mut written,
                    &mut ov,
                )
            };
            if ok != 0 {
                return Ok(written as usize);
            }
            let err = unsafe { GetLastError() };
            if err == ERROR_IO_PENDING {
                let wait = unsafe { WaitForSingleObject(self.io_event, INFINITE) };
                if wait != WAIT_OBJECT_0 {
                    return Err(std::io::Error::other("Overlapped pipe write wait failed"));
                }
                if unsafe { GetOverlappedResult(self.handle, &ov, &mut written, 0) } != 0 {
                    return Ok(written as usize);
                }
                return Err(std::io::Error::from_raw_os_error(
                    unsafe { GetLastError() } as i32,
                ));
            }
            Err(std::io::Error::from_raw_os_error(err as i32))
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn handle_client(pipe_handle: HANDLE, index: Arc<RwLock<MultiVolumeIndex>>) {
        let Some(mut stream) = OverlappedPipe::new(pipe_handle) else {
            unsafe {
                CloseHandle(pipe_handle);
            }
            return;
        };

        let Ok((kind, payload)) = read_frame(&mut stream) else {
            return;
        };
        if kind != FRAME_JSON_REQ {
            let _ = write_json_frame(
                &mut stream,
                FRAME_JSON_RESP,
                &IndexResponse::Error("Unexpected IPC request frame kind.".to_string()),
            );
            return;
        }
        let Ok(req) = serde_json::from_slice::<IndexRequest>(&payload) else {
            let _ = write_json_frame(
                &mut stream,
                FRAME_JSON_RESP,
                &IndexResponse::Error("Malformed IPC request payload.".to_string()),
            );
            return;
        };

        let response = match req {
            IndexRequest::Ping => IndexResponse::Pong {
                protocol_version: PROTOCOL_VERSION,
                service_version: env!("CARGO_PKG_VERSION").to_string(),
            },
            IndexRequest::Status => match index.read() {
                Ok(guard) => IndexResponse::Status(guard.status()),
                Err(_) => IndexResponse::Error("Index lock poisoned.".to_string()),
            },
            IndexRequest::Search {
                query,
                scope_path,
                limit,
            } => match index.read() {
                Ok(guard) => {
                    if !guard.ready && guard.volumes.is_empty() {
                        IndexResponse::Error(
                            "NTFS MFT index is still initializing...".to_string(),
                        )
                    } else {
                        IndexResponse::SearchResults(guard.search(
                            &query,
                            scope_path.as_deref(),
                            limit,
                        ))
                    }
                }
                Err(_) => IndexResponse::Error("Index lock poisoned.".to_string()),
            },
            IndexRequest::GetFolderSizes { paths } => match index.read() {
                Ok(guard) => IndexResponse::FolderSizes(guard.folder_sizes(&paths)),
                Err(_) => IndexResponse::Error("Index lock poisoned.".to_string()),
            },
            IndexRequest::RebuildIndex => {
                spawn_full_index_build(index);
                IndexResponse::Ack
            }
        };

        let _ = write_json_frame(&mut stream, FRAME_JSON_RESP, &response);
    }
}
