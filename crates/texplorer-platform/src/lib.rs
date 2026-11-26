//! Platform abstraction and native Windows integration for T Explorer.

pub mod clipboard;
pub mod com_shell;
pub mod credentials;
pub mod default_explorer;
pub mod dialogs;
pub mod elevation;
pub mod launch;
pub mod locks;
pub mod network_share;
pub mod ole_drag;
pub mod ole_drop;
pub mod pdf_render;
pub mod single_instance;
pub mod theme_detect;
pub mod video_player;
pub mod volumes;
pub mod watcher;
pub mod win_e_hook;

pub use clipboard::{ClipboardFiles, get_clipboard_files, set_clipboard_files};
pub use com_shell::{RawBgraImage, ShellWorker, cursor_screen_pos};
pub use credentials::{delete_network_password, load_network_password, save_network_password};
pub use default_explorer::{
    DefaultExplorerStatus, delegate_to_windows_explorer, enable_default_explorer,
    query_default_explorer_status, restore_windows_explorer, set_win_e_interception,
    should_delegate_to_windows_explorer,
};
pub use dialogs::{pick_files_dialog, pick_folder_dialog};
pub use elevation::is_elevated;
pub use launch::{
    delete_permanently, move_to_trash, open_in_editor, open_item, open_terminal_here,
    reveal_in_windows_explorer, show_properties_dialog, spawn_as_desktop_user,
};
pub use locks::{FileLockStatus, LockingProcess, inspect_file_lock};
pub use network_share::{connect_smb_share, disconnect_smb_share};
pub use ole_drag::{NativeDragOutcome, start_native_file_drag};
pub use ole_drop::{NativeDropEvent, install_native_drop_target};
pub use pdf_render::{
    RenderedPdfPage, fit_pdf_dimensions, render_pdf_page_from_bytes, render_pdf_page_from_path,
};
pub use single_instance::{
    LaunchIntent, SingleInstanceGuard, is_app_instance_running, parse_cli_args,
    start_single_instance_server, try_forward_to_running_instance,
};
pub use theme_detect::system_prefers_dark_mode;
pub use video_player::{
    VideoFrameSnapshot, VideoNotify, VideoPlaybackStatus, VideoPlayerHandle, VideoStartOptions,
    fit_video_dimensions, spawn_video_player,
};
pub use volumes::{list_known_folders, list_volumes, list_wsl_distros};
pub use watcher::{DirectoryWatcher, WatchEvent};
pub use win_e_hook::{
    bring_current_process_window_to_front, ensure_win_e_daemon_running, is_win_e_daemon_running,
    run_win_e_daemon, start_win_e_daemon, stop_win_e_daemon,
};
