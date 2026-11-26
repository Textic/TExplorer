//! Platform abstraction and native Windows integration for T Explorer.

pub mod clipboard;
pub mod com_shell;
pub mod default_explorer;
pub mod elevation;
pub mod launch;
pub mod single_instance;
pub mod volumes;
pub mod watcher;

pub use clipboard::{ClipboardFiles, get_clipboard_files, set_clipboard_files};
pub use com_shell::{RawBgraImage, ShellWorker, cursor_screen_pos};
pub use default_explorer::{
    DefaultExplorerStatus, delegate_to_windows_explorer, enable_default_explorer,
    query_default_explorer_status, restore_windows_explorer, should_delegate_to_windows_explorer,
};
pub use elevation::is_elevated;
pub use launch::{
    delete_permanently, find_textree_exe, move_to_trash, open_in_editor, open_item,
    open_terminal_here, reveal_in_windows_explorer, show_properties_dialog,
};
pub use single_instance::{
    LaunchIntent, SingleInstanceGuard, parse_cli_args, start_single_instance_server,
    try_forward_to_running_instance,
};
pub use volumes::{list_known_folders, list_volumes, list_wsl_distros};
pub use watcher::{DirectoryWatcher, WatchEvent};
