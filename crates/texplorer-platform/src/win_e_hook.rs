//! Universal Windows `Win+E` shortcut interceptor (`WH_KEYBOARD_LL` lightweight daemon).
//!
//! Windows 11 26H2 (Build 26300+) introduced `ExplorerDirectLaunch` (Feature ID `63202390`),
//! which bypasses the traditional `HKCU\Software\Classes\CLSID\{52205fd8-...}` registry override
//! on `Win+E`. Running this ultra-lightweight message-loop daemon (`texplorer.exe --win-e-daemon`,
//! ~2 MB RAM, 0% idle CPU, no GPU/GPUI) intercepts `Win+E` reliably across every Windows 10 and
//! Windows 11 version without requiring Administrator privileges, reboots, or third-party tools.

use std::path::Path;

#[cfg(target_os = "windows")]
mod imp {
    use std::path::Path;
    use std::process::{Command, Stdio};
    use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
    use std::time::{Duration, Instant};

    use windows_sys::Win32::Foundation::{
        BOOL, CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HWND, LPARAM, LRESULT, WPARAM,
    };
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::System::Threading::{CreateMutexW, GetCurrentProcessId};
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput,
        VK_CONTROL, VK_LWIN, VK_MENU, VK_NONAME, VK_RWIN, VK_SHIFT,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        ASFW_ANY, AllowSetForegroundWindow, CallNextHookEx, CreateWindowExW, DefWindowProcW,
        DestroyWindow, DispatchMessageW, EnumWindows, FindWindowExW, GetMessageW,
        GetWindowThreadProcessId, HHOOK, HWND_MESSAGE, IsIconic, IsWindowVisible, KBDLLHOOKSTRUCT,
        MSG, PostMessageW, PostQuitMessage, RegisterClassW, SW_RESTORE, SetForegroundWindow,
        SetWindowsHookExW, ShowWindow, TranslateMessage, UnhookWindowsHookEx, WH_KEYBOARD_LL,
        WM_APP, WM_CLOSE, WM_ENDSESSION, WM_KEYDOWN, WM_KEYUP, WM_QUERYENDSESSION, WM_SYSKEYDOWN,
        WM_SYSKEYUP, WNDCLASSW,
    };

    use crate::single_instance::{LaunchIntent, try_forward_to_running_instance};

    const DAEMON_MUTEX_NAME: &str = "Local\\TExplorerWinEDaemonV1";
    const DAEMON_WINDOW_CLASS: &str = "TExplorerWinEDaemonClassV1";
    const WM_APP_LAUNCH_TEXPLORER: u32 = WM_APP + 1;
    const VK_E: u32 = 0x45;
    /// Marker tag placed on synthetic Start-Menu-masking input so the hook ignores its own events.
    const TEXPLORER_SYNTH_EXTRA_INFO: usize = 0x5445_5850;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const DETACHED_PROCESS: u32 = 0x0000_0008;

    static DAEMON_HWND: AtomicIsize = AtomicIsize::new(0);
    static SWALLOWED_E: AtomicBool = AtomicBool::new(false);

    fn to_wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn find_daemon_window() -> HWND {
        let class = to_wide(DAEMON_WINDOW_CLASS);
        unsafe { FindWindowExW(HWND_MESSAGE, std::ptr::null_mut(), class.as_ptr(), std::ptr::null()) }
    }

    /// Returns `true` if a `TExplorer` `Win+E` background daemon is currently active.
    pub fn is_win_e_daemon_running() -> bool {
        !find_daemon_window().is_null()
    }

    /// Gracefully stops any running `TExplorer` `Win+E` background daemon.
    pub fn stop_win_e_daemon() {
        let deadline = Instant::now() + Duration::from_millis(750);
        loop {
            let hwnd = find_daemon_window();
            if hwnd.is_null() {
                return;
            }
            unsafe {
                let _ = PostMessageW(hwnd, WM_CLOSE, 0, 0);
            }
            if Instant::now() >= deadline {
                return;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    /// Restarts the detached `Win+E` daemon using `exe_path` so changes take effect immediately.
    pub fn start_win_e_daemon(exe_path: &Path) -> Result<(), String> {
        use std::os::windows::process::CommandExt;

        stop_win_e_daemon();
        Command::new(exe_path)
            .arg("--win-e-daemon")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS)
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("Failed starting Win+E shortcut daemon: {e}"))
    }

    /// Brings the current process's main visible top-level window to the foreground (restoring it
    /// if minimized), working reliably when woken from a background process via Named Pipe.
    pub fn bring_current_process_window_to_front() {
        unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
            let target_pid = lparam as u32;
            let mut pid: u32 = 0;
            unsafe {
                GetWindowThreadProcessId(hwnd, &mut pid);
                if pid == target_pid && IsWindowVisible(hwnd) != 0 {
                    if IsIconic(hwnd) != 0 {
                        let _ = ShowWindow(hwnd, SW_RESTORE);
                    }
                    let _ = SetForegroundWindow(hwnd);
                    return 0;
                }
            }
            1
        }

        unsafe {
            let pid = GetCurrentProcessId();
            let _ = EnumWindows(Some(enum_proc), pid as isize);
        }
    }

    /// Sends a harmless non-printing `VK_NONAME` keystroke while `Win` is held so Windows does
    /// not open the Start Menu when the user releases the `Win` key (same pattern as PowerToys).
    fn suppress_start_menu_on_win_release() {
        let inputs = [
            INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: VK_NONAME,
                        wScan: 0,
                        dwFlags: 0,
                        time: 0,
                        dwExtraInfo: TEXPLORER_SYNTH_EXTRA_INFO,
                    },
                },
            },
            INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: VK_NONAME,
                        wScan: 0,
                        dwFlags: KEYEVENTF_KEYUP,
                        time: 0,
                        dwExtraInfo: TEXPLORER_SYNTH_EXTRA_INFO,
                    },
                },
            },
        ];
        unsafe {
            let _ = SendInput(
                inputs.len() as u32,
                inputs.as_ptr(),
                std::mem::size_of::<INPUT>() as i32,
            );
        }
    }

    fn launch_or_focus_texplorer() {
        std::thread::spawn(|| {
            unsafe {
                let _ = AllowSetForegroundWindow(ASFW_ANY);
            }
            if try_forward_to_running_instance(&LaunchIntent::OpenDefault) {
                return;
            }
            if let Ok(exe) = std::env::current_exe() {
                if crate::elevation::is_elevated() {
                    let parent = exe.parent().unwrap_or_else(|| Path::new("."));
                    let exe_str = exe.to_string_lossy();
                    if crate::launch::spawn_as_desktop_user(&exe_str, "", parent).is_ok() {
                        return;
                    }
                }
                let _ = Command::new(exe)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn();
            }
        });
    }

    unsafe extern "system" fn low_level_keyboard_proc(
        code: i32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if code >= 0 && lparam != 0 {
            let kb = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
            if kb.dwExtraInfo != TEXPLORER_SYNTH_EXTRA_INFO {
                let msg = wparam as u32;
                let is_down = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
                let is_up = msg == WM_KEYUP || msg == WM_SYSKEYUP;

                if kb.vkCode == VK_E {
                    if is_down {
                        let win_down = unsafe {
                            GetAsyncKeyState(VK_LWIN as i32) < 0
                                || GetAsyncKeyState(VK_RWIN as i32) < 0
                        };
                        let other_mods = unsafe {
                            GetAsyncKeyState(VK_CONTROL as i32) < 0
                                || GetAsyncKeyState(VK_MENU as i32) < 0
                                || GetAsyncKeyState(VK_SHIFT as i32) < 0
                        };
                        if win_down && !other_mods {
                            if !SWALLOWED_E.swap(true, Ordering::AcqRel) {
                                let hwnd = DAEMON_HWND.load(Ordering::Acquire) as HWND;
                                if !hwnd.is_null() {
                                    unsafe {
                                        let _ = PostMessageW(hwnd, WM_APP_LAUNCH_TEXPLORER, 0, 0);
                                    }
                                }
                            }
                            return 1;
                        }
                    } else if is_up && SWALLOWED_E.swap(false, Ordering::AcqRel) {
                        return 1;
                    }
                } else if is_up
                    && (kb.vkCode == VK_LWIN as u32 || kb.vkCode == VK_RWIN as u32)
                {
                    SWALLOWED_E.store(false, Ordering::Release);
                }
            }
        }
        unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
    }

    unsafe extern "system" fn daemon_wnd_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_APP_LAUNCH_TEXPLORER => {
                suppress_start_menu_on_win_release();
                launch_or_focus_texplorer();
                0
            }
            WM_CLOSE => {
                unsafe {
                    let _ = DestroyWindow(hwnd);
                    PostQuitMessage(0);
                }
                0
            }
            WM_QUERYENDSESSION => 1,
            WM_ENDSESSION => {
                if wparam != 0 {
                    unsafe {
                        PostQuitMessage(0);
                    }
                }
                0
            }
            _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
        }
    }

    /// Runs the blocking message loop for `texplorer.exe --win-e-daemon`.
    pub fn run_win_e_daemon() -> Result<(), String> {
        let mutex_name = to_wide(DAEMON_MUTEX_NAME);
        let mutex = unsafe { CreateMutexW(std::ptr::null(), 1, mutex_name.as_ptr()) };
        if mutex.is_null() {
            return Err("Failed creating Win+E daemon single-instance mutex.".to_string());
        }
        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            unsafe {
                CloseHandle(mutex);
            }
            return Ok(());
        }

        let hinstance = unsafe { GetModuleHandleW(std::ptr::null()) };
        let class_name = to_wide(DAEMON_WINDOW_CLASS);
        let wc = WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(daemon_wnd_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinstance,
            hIcon: std::ptr::null_mut(),
            hCursor: std::ptr::null_mut(),
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: class_name.as_ptr(),
        };
        unsafe {
            RegisterClassW(&wc);
        }

        let hwnd = unsafe {
            CreateWindowExW(
                0,
                class_name.as_ptr(),
                class_name.as_ptr(),
                0,
                0,
                0,
                0,
                0,
                HWND_MESSAGE,
                std::ptr::null_mut(),
                hinstance,
                std::ptr::null(),
            )
        };
        if hwnd.is_null() {
            unsafe {
                CloseHandle(mutex);
            }
            return Err("Failed creating Win+E daemon message window.".to_string());
        }
        DAEMON_HWND.store(hwnd as isize, Ordering::Release);

        let hook: HHOOK = unsafe {
            SetWindowsHookExW(WH_KEYBOARD_LL, Some(low_level_keyboard_proc), hinstance, 0)
        };
        if hook.is_null() {
            DAEMON_HWND.store(0, Ordering::Release);
            unsafe {
                let _ = DestroyWindow(hwnd);
                CloseHandle(mutex);
            }
            return Err("Failed installing Windows Win+E keyboard hook.".to_string());
        }

        let mut msg: MSG = unsafe { std::mem::zeroed() };
        while unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) } > 0 {
            unsafe {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }

        DAEMON_HWND.store(0, Ordering::Release);
        SWALLOWED_E.store(false, Ordering::Release);
        unsafe {
            let _ = UnhookWindowsHookEx(hook);
            let _ = DestroyWindow(hwnd);
            CloseHandle(mutex);
        }
        Ok(())
    }
}

#[cfg(not(target_os = "windows"))]
mod imp {
    use std::path::Path;

    pub fn is_win_e_daemon_running() -> bool {
        false
    }

    pub fn stop_win_e_daemon() {}

    pub fn start_win_e_daemon(_exe_path: &Path) -> Result<(), String> {
        Ok(())
    }

    pub fn bring_current_process_window_to_front() {}

    pub fn run_win_e_daemon() -> Result<(), String> {
        Ok(())
    }
}

pub use imp::{
    bring_current_process_window_to_front, is_win_e_daemon_running, run_win_e_daemon,
    start_win_e_daemon, stop_win_e_daemon,
};

/// Ensures the `Win+E` background daemon is running for `exe_path` if not already active.
pub fn ensure_win_e_daemon_running(exe_path: &Path) -> Result<(), String> {
    if !is_win_e_daemon_running() {
        start_win_e_daemon(exe_path)?;
    }
    Ok(())
}
