//! IPC client and Windows SCM lifecycle helper for `TExplorerIndexService` (`texplorer-service.exe`).

use std::path::PathBuf;

use crate::protocol::{
    FolderSizeEntry, IndexServiceStatus, SearchResponse,
};

/// Installation & runtime state of the optional Windows background index service.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IndexServiceInstallState {
    Running,
    Stopped,
    NotInstalled,
    #[default]
    Unavailable,
}

impl IndexServiceInstallState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Running => "Active",
            Self::Stopped => "Stopped",
            Self::NotInstalled => "Not Installed",
            Self::Unavailable => "Windows Only",
        }
    }
}

/// Locates `texplorer-service.exe` adjacent to the currently running executable.
pub fn find_service_exe() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        let current = std::env::current_exe().ok()?;
        let candidate = current.parent()?.join("texplorer-service.exe");
        if candidate.is_file() {
            return Some(candidate);
        }
        None
    }
    #[cfg(not(target_os = "windows"))]
    {
        None
    }
}

/// Queries the Windows Service Control Manager for `TExplorerIndexService` status without requiring Admin.
#[cfg(target_os = "windows")]
pub fn query_service_install_state() -> IndexServiceInstallState {
    use windows_sys::Win32::System::Services::{
        CloseServiceHandle, OpenSCManagerW, OpenServiceW, QueryServiceStatus, SC_MANAGER_CONNECT,
        SERVICE_QUERY_STATUS, SERVICE_RUNNING, SERVICE_STATUS,
    };

    let scm = unsafe { OpenSCManagerW(std::ptr::null(), std::ptr::null(), SC_MANAGER_CONNECT) };
    if scm.is_null() {
        return IndexServiceInstallState::Unavailable;
    }
    let name: Vec<u16> = crate::protocol::SERVICE_NAME
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let svc = unsafe { OpenServiceW(scm, name.as_ptr(), SERVICE_QUERY_STATUS) };
    if svc.is_null() {
        unsafe {
            CloseServiceHandle(scm);
        }
        return IndexServiceInstallState::NotInstalled;
    }

    let mut st: SERVICE_STATUS = unsafe { std::mem::zeroed() };
    let ok = unsafe { QueryServiceStatus(svc, &mut st) };
    unsafe {
        CloseServiceHandle(svc);
        CloseServiceHandle(scm);
    }
    if ok == 0 {
        IndexServiceInstallState::Stopped
    } else if st.dwCurrentState == SERVICE_RUNNING {
        IndexServiceInstallState::Running
    } else {
        IndexServiceInstallState::Stopped
    }
}

#[cfg(not(target_os = "windows"))]
pub fn query_service_install_state() -> IndexServiceInstallState {
    IndexServiceInstallState::Unavailable
}

#[cfg(target_os = "windows")]
fn run_service_admin_command(flag: &str) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};
    use windows_sys::Win32::UI::Shell::{
        SEE_MASK_NO_CONSOLE, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;

    let exe = find_service_exe().ok_or_else(|| {
        "texplorer-service.exe was not found next to texplorer.exe. Build or install the service binary first."
            .to_string()
    })?;

    let verb: Vec<u16> = "runas\0".encode_utf16().collect();
    let file: Vec<u16> = exe
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let params: Vec<u16> = flag.encode_utf16().chain(std::iter::once(0)).collect();

    let mut sei: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    sei.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    sei.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NO_CONSOLE;
    sei.lpVerb = verb.as_ptr();
    sei.lpFile = file.as_ptr();
    sei.lpParameters = params.as_ptr();
    sei.nShow = SW_HIDE;

    let ok = unsafe { ShellExecuteExW(&mut sei) };
    if ok == 0 || sei.hProcess.is_null() {
        return Err("Service action was cancelled or failed UAC elevation.".to_string());
    }

    let wait = unsafe { WaitForSingleObject(sei.hProcess, 20_000) };
    let mut exit_code: u32 = 1;
    unsafe {
        let _ = GetExitCodeProcess(sei.hProcess, &mut exit_code);
        CloseHandle(sei.hProcess);
    }
    if wait != WAIT_OBJECT_0 {
        return Err("Timed out waiting for TExplorer Index Service command.".to_string());
    }
    if exit_code != 0 {
        return Err(format!(
            "texplorer-service.exe {flag} failed with exit code {exit_code}."
        ));
    }
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn run_service_admin_command(_flag: &str) -> Result<(), String> {
    Err("TExplorer Index Service is only available on Windows.".to_string())
}

pub fn install_service_elevated() -> Result<(), String> {
    run_service_admin_command("--install")
}

pub fn start_service_elevated() -> Result<(), String> {
    run_service_admin_command("--start")
}

pub fn stop_service_elevated() -> Result<(), String> {
    run_service_admin_command("--stop")
}

pub fn uninstall_service_elevated() -> Result<(), String> {
    run_service_admin_command("--uninstall")
}

#[cfg(target_os = "windows")]
fn send_request(req: &crate::protocol::IndexRequest) -> Result<crate::protocol::IndexResponse, String> {
    use std::fs::OpenOptions;
    use std::io::{BufReader, BufWriter};
    use windows_sys::Win32::Foundation::ERROR_PIPE_BUSY;
    use windows_sys::Win32::System::Pipes::WaitNamedPipeW;

    use crate::protocol::{FRAME_JSON_REQ, FRAME_JSON_RESP, PIPE_NAME, read_frame, write_json_frame};

    let wide_pipe: Vec<u16> = PIPE_NAME.encode_utf16().chain(std::iter::once(0)).collect();
    let mut pipe_file = None;
    for attempt in 0..8 {
        match OpenOptions::new().read(true).write(true).open(PIPE_NAME) {
            Ok(f) => {
                pipe_file = Some(f);
                break;
            }
            Err(err) => {
                if err.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) && attempt < 7 {
                    unsafe {
                        let _ = WaitNamedPipeW(wide_pipe.as_ptr(), 800);
                    }
                    continue;
                }
                return Err(format!("TExplorer Index Service unavailable: {err}"));
            }
        }
    }
    let pipe = pipe_file.ok_or_else(|| "TExplorer Index Service pipe busy.".to_string())?;
    let pipe_read = pipe
        .try_clone()
        .map_err(|e| format!("Failed cloning IPC pipe handle: {e}"))?;
    let mut writer = BufWriter::new(pipe);
    let mut reader = BufReader::with_capacity(64 * 1024, pipe_read);

    write_json_frame(&mut writer, FRAME_JSON_REQ, req)?;
    let (kind, payload) = read_frame(&mut reader)?;
    if kind != FRAME_JSON_RESP {
        return Err("Unexpected response frame kind from TExplorer Index Service.".to_string());
    }
    serde_json::from_slice(&payload)
        .map_err(|e| format!("Invalid JSON response from TExplorer Index Service: {e}"))
}

#[cfg(not(target_os = "windows"))]
fn send_request(_req: &crate::protocol::IndexRequest) -> Result<crate::protocol::IndexResponse, String> {
    Err("TExplorer Index Service is only available on Windows.".to_string())
}

pub fn ping_service() -> bool {
    matches!(
        send_request(&crate::protocol::IndexRequest::Ping),
        Ok(crate::protocol::IndexResponse::Pong { .. })
    )
}

pub fn query_live_service_status() -> Option<IndexServiceStatus> {
    match send_request(&crate::protocol::IndexRequest::Status) {
        Ok(crate::protocol::IndexResponse::Status(st)) => Some(st),
        _ => None,
    }
}

pub fn search_via_service(
    query: &str,
    scope_path: Option<String>,
    limit: usize,
) -> Result<SearchResponse, String> {
    match send_request(&crate::protocol::IndexRequest::Search {
        query: query.to_string(),
        scope_path,
        limit,
    })? {
        crate::protocol::IndexResponse::SearchResults(resp) => Ok(resp),
        crate::protocol::IndexResponse::Error(err) => Err(err),
        _ => Err("Unexpected search response from TExplorer Index Service.".to_string()),
    }
}

pub fn get_folder_sizes_via_service(paths: Vec<String>) -> Result<Vec<FolderSizeEntry>, String> {
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    match send_request(&crate::protocol::IndexRequest::GetFolderSizes { paths })? {
        crate::protocol::IndexResponse::FolderSizes(entries) => Ok(entries),
        crate::protocol::IndexResponse::Error(err) => Err(err),
        _ => Err("Unexpected folder-size response from TExplorer Index Service.".to_string()),
    }
}

pub fn rebuild_index_via_service() -> Result<(), String> {
    match send_request(&crate::protocol::IndexRequest::RebuildIndex)? {
        crate::protocol::IndexResponse::Ack => Ok(()),
        crate::protocol::IndexResponse::Error(err) => Err(err),
        _ => Err("Unexpected rebuild response from TExplorer Index Service.".to_string()),
    }
}
