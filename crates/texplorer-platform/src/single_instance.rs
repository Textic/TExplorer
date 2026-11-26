use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};

use serde::{Deserialize, Serialize};

use crate::default_explorer::{is_home_or_this_pc_namespace, should_delegate_to_windows_explorer};

pub const SINGLE_INSTANCE_PIPE: &str = r"\\.\pipe\TExplorerSingleInstanceV1";

/// Resolved command-line startup or IPC handoff action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LaunchIntent {
    OpenDefault,
    OpenLocation(PathBuf),
    SelectFile(PathBuf),
    DelegateToWindowsExplorer(String),
    EnableDefaultExplorer { intercept_win_e: bool },
    RestoreDefaultExplorer,
    RunWinEDaemon,
    StopWinEDaemon,
}

/// Parses process arguments (excluding `argv[0]`) into a `LaunchIntent` and `force_new_window`.
pub fn parse_cli_args(args: &[String]) -> (LaunchIntent, bool) {
    let mut force_new_window = false;
    let mut select_mode = false;
    let mut positional: Option<String> = None;

    let mut i = 0usize;
    while i < args.len() {
        let arg = args[i].trim();
        if arg.eq_ignore_ascii_case("--win-e-daemon") {
            return (LaunchIntent::RunWinEDaemon, false);
        }
        if arg.eq_ignore_ascii_case("--stop-win-e-daemon") {
            return (LaunchIntent::StopWinEDaemon, false);
        }
        if arg.eq_ignore_ascii_case("--restore-explorer")
            || arg.eq_ignore_ascii_case("--restore-default-explorer")
        {
            return (LaunchIntent::RestoreDefaultExplorer, false);
        }
        if arg.eq_ignore_ascii_case("--enable-default-explorer-win-e") {
            return (
                LaunchIntent::EnableDefaultExplorer {
                    intercept_win_e: true,
                },
                false,
            );
        }
        if arg.eq_ignore_ascii_case("--enable-default-explorer") {
            return (
                LaunchIntent::EnableDefaultExplorer {
                    intercept_win_e: false,
                },
                false,
            );
        }
        if arg.eq_ignore_ascii_case("--new-window") || arg.eq_ignore_ascii_case("-n") {
            force_new_window = true;
            i += 1;
            continue;
        }
        if arg.eq_ignore_ascii_case("--select") || arg.eq_ignore_ascii_case("/select") {
            select_mode = true;
            if let Some(next) = args.get(i + 1) {
                positional = Some(next.clone());
                i += 2;
                continue;
            }
            i += 1;
            continue;
        }
        if let Some(rest) = arg
            .strip_prefix("/select,")
            .or_else(|| arg.strip_prefix("/SELECT,"))
        {
            select_mode = true;
            positional = Some(rest.trim_matches('"').to_string());
            i += 1;
            continue;
        }
        if !arg.starts_with('-') && !arg.starts_with('/') && positional.is_none() {
            positional = Some(arg.to_string());
        }
        i += 1;
    }

    let Some(raw) = positional else {
        return (LaunchIntent::OpenDefault, force_new_window);
    };

    let cleaned = raw.trim().trim_matches('"');
    if cleaned.is_empty() {
        return (LaunchIntent::OpenDefault, force_new_window);
    }

    if is_home_or_this_pc_namespace(cleaned) {
        return (LaunchIntent::OpenDefault, force_new_window);
    }

    if should_delegate_to_windows_explorer(cleaned) {
        return (
            LaunchIntent::DelegateToWindowsExplorer(cleaned.to_string()),
            false,
        );
    }

    let path = PathBuf::from(cleaned);
    if select_mode || is_existing_file(&path) {
        (LaunchIntent::SelectFile(path), force_new_window)
    } else {
        (LaunchIntent::OpenLocation(path), force_new_window)
    }
}

fn is_existing_file(path: &Path) -> bool {
    path.is_file()
}

/// Attempts to forward `intent` to an already running T Explorer instance via Named Pipe.
/// Returns `true` if a running instance accepted the request and this process can exit.
#[cfg(target_os = "windows")]
pub fn try_forward_to_running_instance(intent: &LaunchIntent) -> bool {
    use std::fs::OpenOptions;
    use std::io::Write;

    let Ok(mut pipe) = OpenOptions::new().write(true).open(SINGLE_INSTANCE_PIPE) else {
        return false;
    };
    let Ok(mut payload) = serde_json::to_vec(intent) else {
        return false;
    };
    payload.push(b'\n');
    pipe.write_all(&payload).is_ok() && pipe.flush().is_ok()
}

#[cfg(not(target_os = "windows"))]
pub fn try_forward_to_running_instance(_intent: &LaunchIntent) -> bool {
    false
}

/// Keeps the single-instance IPC server alive for the lifetime of the application.
pub struct SingleInstanceGuard {
    stop_flag: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for SingleInstanceGuard {
    fn drop(&mut self) {
        self.stop_flag.store(true, Ordering::Release);
        #[cfg(target_os = "windows")]
        {
            // Wake the blocking ConnectNamedPipe loop cleanly with a no-op connection.
            let _ = std::fs::OpenOptions::new()
                .write(true)
                .open(SINGLE_INSTANCE_PIPE);
        }
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

/// Starts listening on the single-instance Named Pipe so external folder launches open tabs
/// inside the active T Explorer window.
#[cfg(target_os = "windows")]
pub fn start_single_instance_server(
    on_intent: impl Fn(LaunchIntent) + Send + 'static,
) -> SingleInstanceGuard {
    use std::fs::File;
    use std::io::{BufRead, BufReader};
    use std::os::windows::io::FromRawHandle;
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_PIPE_CONNECTED, GetLastError, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::PIPE_ACCESS_INBOUND;
    use windows_sys::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE,
        PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT,
    };

    let stop_flag = Arc::new(AtomicBool::new(false));
    let stop_bg = stop_flag.clone();

    let thread = thread::Builder::new()
        .name("texplorer-single-instance".to_string())
        .spawn(move || {
            let pipe_name: Vec<u16> = SINGLE_INSTANCE_PIPE
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();

            while !stop_bg.load(Ordering::Acquire) {
                let handle = unsafe {
                    CreateNamedPipeW(
                        pipe_name.as_ptr(),
                        PIPE_ACCESS_INBOUND,
                        PIPE_TYPE_BYTE
                            | PIPE_READMODE_BYTE
                            | PIPE_WAIT
                            | PIPE_REJECT_REMOTE_CLIENTS,
                        8,
                        4096,
                        4096,
                        0,
                        std::ptr::null(),
                    )
                };
                if handle == INVALID_HANDLE_VALUE {
                    thread::sleep(std::time::Duration::from_millis(250));
                    continue;
                }

                let connected = unsafe { ConnectNamedPipe(handle, std::ptr::null_mut()) };
                let ok = connected != 0 || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED;
                if ok && !stop_bg.load(Ordering::Acquire) {
                    // Wrap raw handle temporarily to read the single JSON line safely.
                    let file = unsafe { File::from_raw_handle(handle) };
                    let mut reader = BufReader::new(file);
                    let mut line = String::new();
                    if reader.read_line(&mut line).is_ok()
                        && let Ok(intent) = serde_json::from_str::<LaunchIntent>(line.trim())
                    {
                        on_intent(intent);
                    }
                    // `file` owns and closes `handle` when dropped; disconnect first for cleanliness.
                    let raw = std::os::windows::io::IntoRawHandle::into_raw_handle(reader.into_inner());
                    unsafe {
                        DisconnectNamedPipe(raw);
                        CloseHandle(raw);
                    }
                } else {
                    unsafe {
                        CloseHandle(handle);
                    }
                }
            }
        })
        .ok();

    SingleInstanceGuard { stop_flag, thread }
}

#[cfg(not(target_os = "windows"))]
pub fn start_single_instance_server(
    _on_intent: impl Fn(LaunchIntent) + Send + 'static,
) -> SingleInstanceGuard {
    SingleInstanceGuard {
        stop_flag: Arc::new(AtomicBool::new(false)),
        thread: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_standard_select_restore_and_clsid_cli_args() {
        assert_eq!(parse_cli_args(&[]), (LaunchIntent::OpenDefault, false));

        assert_eq!(
            parse_cli_args(&["--win-e-daemon".to_string()]),
            (LaunchIntent::RunWinEDaemon, false)
        );
        assert_eq!(
            parse_cli_args(&["--stop-win-e-daemon".to_string()]),
            (LaunchIntent::StopWinEDaemon, false)
        );
        assert_eq!(
            parse_cli_args(&["--restore-explorer".to_string()]),
            (LaunchIntent::RestoreDefaultExplorer, false)
        );
        assert_eq!(
            parse_cli_args(&["--enable-default-explorer".to_string()]),
            (
                LaunchIntent::EnableDefaultExplorer {
                    intercept_win_e: false
                },
                false
            )
        );
        assert_eq!(
            parse_cli_args(&["--enable-default-explorer-win-e".to_string()]),
            (
                LaunchIntent::EnableDefaultExplorer {
                    intercept_win_e: true
                },
                false
            )
        );

        assert_eq!(
            parse_cli_args(&["--new-window".to_string(), r"X:\TExplorer".to_string()]),
            (LaunchIntent::OpenLocation(PathBuf::from(r"X:\TExplorer")), true)
        );

        assert_eq!(
            parse_cli_args(&[r"/select,X:\TExplorer\Cargo.toml".to_string()]),
            (
                LaunchIntent::SelectFile(PathBuf::from(r"X:\TExplorer\Cargo.toml")),
                false
            )
        );

        assert_eq!(
            parse_cli_args(&["::{20D04FE0-3AEA-1069-A2D8-08002B30309D}".to_string()]),
            (LaunchIntent::OpenDefault, false)
        );
        assert_eq!(
            parse_cli_args(&["::{26EE0668-A00A-44D7-9371-BEB064C98683}".to_string()]),
            (
                LaunchIntent::DelegateToWindowsExplorer(
                    "::{26EE0668-A00A-44D7-9371-BEB064C98683}".to_string()
                ),
                false
            )
        );
    }
}
