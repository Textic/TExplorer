use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};

/// Notification emitted when the contents of a watched directory change externally.
#[derive(Debug, Clone)]
pub struct WatchEvent {
    pub directory: PathBuf,
}

/// Live OS directory watcher with automatic debouncing and instant, deadlock-free cancellation.
pub struct DirectoryWatcher {
    stop_flag: Arc<AtomicBool>,
    #[cfg(target_os = "windows")]
    stop_event: usize,
    thread: Option<JoinHandle<()>>,
}

impl DirectoryWatcher {
    /// Starts watching `directory` for creations, deletions, renames, and metadata updates.
    #[cfg(target_os = "windows")]
    pub fn start(
        directory: PathBuf,
        on_change: impl Fn(WatchEvent) + Send + 'static,
    ) -> Result<Self, String> {
        use std::os::windows::ffi::OsStrExt;
        use std::time::{Duration, Instant};
        use windows_sys::Win32::Foundation::{
            CloseHandle, ERROR_IO_PENDING, GetLastError, INVALID_HANDLE_VALUE, WAIT_OBJECT_0,
        };
        use windows_sys::Win32::Storage::FileSystem::{
            CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OVERLAPPED, FILE_LIST_DIRECTORY,
            FILE_NOTIFY_CHANGE_ATTRIBUTES, FILE_NOTIFY_CHANGE_DIR_NAME,
            FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SIZE,
            FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
            ReadDirectoryChangesW,
        };
        use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
        use windows_sys::Win32::System::Threading::{
            CreateEventW, INFINITE, ResetEvent, WaitForMultipleObjects,
        };

        let wide_dir: Vec<u16> = directory
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();

        let dir_handle = unsafe {
            CreateFileW(
                wide_dir.as_ptr(),
                FILE_LIST_DIRECTORY,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED,
                std::ptr::null_mut(),
            )
        };
        if dir_handle == INVALID_HANDLE_VALUE {
            return Err(format!(
                "Failed to open directory watcher handle for \"{}\".",
                directory.display()
            ));
        }

        let stop_event = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
        if stop_event.is_null() {
            unsafe {
                CloseHandle(dir_handle);
            }
            return Err("Failed to create directory watcher stop event.".to_string());
        }

        let io_event = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
        if io_event.is_null() {
            unsafe {
                CloseHandle(stop_event);
                CloseHandle(dir_handle);
            }
            return Err("Failed to create directory watcher I/O event.".to_string());
        }

        let stop_flag = Arc::new(AtomicBool::new(false));
        let stop_flag_bg = stop_flag.clone();
        let dir_handle_usize = dir_handle as usize;
        let stop_event_usize = stop_event as usize;
        let io_event_usize = io_event as usize;

        let handle = thread::Builder::new()
            .name("texplorer-dir-watcher".to_string())
            .spawn(move || {
                let dir_h = dir_handle_usize as *mut core::ffi::c_void;
                let stop_h = stop_event_usize as *mut core::ffi::c_void;
                let io_h = io_event_usize as *mut core::ffi::c_void;
                let mut buffer = vec![0u8; 32 * 1024];
                let notify_filter = FILE_NOTIFY_CHANGE_FILE_NAME
                    | FILE_NOTIFY_CHANGE_DIR_NAME
                    | FILE_NOTIFY_CHANGE_SIZE
                    | FILE_NOTIFY_CHANGE_LAST_WRITE
                    | FILE_NOTIFY_CHANGE_ATTRIBUTES;

                let mut last_emit = Instant::now() - Duration::from_secs(1);

                while !stop_flag_bg.load(Ordering::Acquire) {
                    unsafe {
                        ResetEvent(io_h);
                    }
                    let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
                    overlapped.hEvent = io_h;
                    let mut bytes_returned: u32 = 0;

                    let ok = unsafe {
                        ReadDirectoryChangesW(
                            dir_h,
                            buffer.as_mut_ptr() as *mut core::ffi::c_void,
                            buffer.len() as u32,
                            0,
                            notify_filter,
                            &mut bytes_returned,
                            &mut overlapped,
                            None,
                        )
                    };

                    if ok == 0 {
                        let err = unsafe { GetLastError() };
                        if err != ERROR_IO_PENDING {
                            break;
                        }
                    }

                    let handles = [io_h, stop_h];
                    let wait = unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, INFINITE) };
                    if wait == WAIT_OBJECT_0 {
                        let mut transferred: u32 = 0;
                        let res_ok =
                            unsafe { GetOverlappedResult(dir_h, &overlapped, &mut transferred, 0) };
                        if res_ok == 0 {
                            break;
                        }
                        if !stop_flag_bg.load(Ordering::Acquire)
                            && last_emit.elapsed() >= Duration::from_millis(120)
                        {
                            last_emit = Instant::now();
                            on_change(WatchEvent {
                                directory: directory.clone(),
                            });
                        }
                    } else {
                        unsafe {
                            CancelIoEx(dir_h, &overlapped);
                        }
                        break;
                    }
                }

                unsafe {
                    CloseHandle(io_h);
                    CloseHandle(dir_h);
                }
            })
            .map_err(|e| format!("Failed spawning watcher thread: {e}"))?;

        Ok(Self {
            stop_flag,
            stop_event: stop_event_usize,
            thread: Some(handle),
        })
    }

    #[cfg(not(target_os = "windows"))]
    pub fn start(
        _directory: PathBuf,
        _on_change: impl Fn(WatchEvent) + Send + 'static,
    ) -> Result<Self, String> {
        Ok(Self {
            stop_flag: Arc::new(AtomicBool::new(false)),
            thread: None,
        })
    }
}

impl Drop for DirectoryWatcher {
    fn drop(&mut self) {
        self.stop_flag.store(true, Ordering::Release);
        #[cfg(target_os = "windows")]
        unsafe {
            use windows_sys::Win32::Foundation::CloseHandle;
            use windows_sys::Win32::System::Threading::SetEvent;
            let stop_h = self.stop_event as *mut core::ffi::c_void;
            SetEvent(stop_h);
            if let Some(handle) = self.thread.take() {
                let _ = handle.join();
            }
            CloseHandle(stop_h);
        }
        #[cfg(not(target_os = "windows"))]
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}
