#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod preview;
mod theme;
mod ui;
mod updater;

use std::borrow::Cow;

use futures_channel::mpsc::unbounded;
use gpui_kit::{
    assets::{Assets, icon_assets},
    *,
};
use texplorer_core::AppSettings;
use texplorer_platform::{
    LaunchIntent, delegate_to_windows_explorer, parse_cli_args, restore_windows_explorer,
    start_single_instance_server, system_prefers_dark_mode, try_forward_to_running_instance,
};

use crate::ui::workspace::{
    CancelOrDismiss, CloseTab, CopySelected, CreateNewFolder, CutSelected, DeletePermanent,
    DeleteToTrash, FocusAddressBar, FocusFilter, GoBack, GoForward, GoUp, NewTab, NextTab,
    OpenGlobalSearch, OpenSettings, PasteItems, PrevTab, Quit, RedoActionCmd, RefreshPane,
    RenameSelected, SelectAllItems, SwitchActivePane, ToggleHiddenFiles, ToggleInspector,
    ToggleSidebar, ToggleSplitPane, UndoActionCmd, Workspace,
};

icon_assets!(
    ExtraIcons,
    [
        ArrowLeft,
        ArrowRight,
        ArrowUp,
        Check,
        ChevronDown,
        ChevronRight,
        ChevronUp,
        Clipboard,
        Code,
        Columns2,
        Copy,
        Disc,
        Download,
        ExternalLink,
        Eye,
        EyeOff,
        File,
        FileArchive,
        FileCode,
        FileImage,
        FilePlus,
        FileText,
        Film,
        Folder,
        FolderOpen,
        FolderPlus,
        HardDrive,
        House,
        Image,
        LayoutGrid,
        LayoutList,
        Link,
        List,
        Monitor,
        Music,
        Network,
        Package,
        Pause,
        Pencil,
        Pin,
        PinOff,
        Play,
        Plus,
        Redo2,
        RefreshCw,
        Scissors,
        Search,
        Settings,
        ShieldCheck,
        Terminal,
        TriangleAlert,
        Undo2,
        Usb,
        X,
    ]
);

struct TExplorerAssets;

impl AssetSource for TExplorerAssets {
    fn load(&self, path: &str) -> gpui_kit::Result<Option<Cow<'static, [u8]>>> {
        if path == "icons/texplorer.png" {
            return Ok(Some(Cow::Borrowed(include_bytes!(
                "../../../assets/texplorer.png"
            ))));
        }
        if path == "icons/texplorer-64.png" {
            return Ok(Some(Cow::Borrowed(include_bytes!(
                "../../../assets/texplorer-64.png"
            ))));
        }
        if let Some(asset) = ExtraIcons.load(path)? {
            return Ok(Some(asset));
        }
        Assets.load(path)
    }

    fn list(&self, path: &str) -> gpui_kit::Result<Vec<SharedString>> {
        let mut items = Assets.list(path)?;
        items.extend(ExtraIcons.list(path)?);
        Ok(items)
    }
}

fn main() {
    let raw_args: Vec<String> = std::env::args().skip(1).collect();
    let (launch_intent, force_new_window) = parse_cli_args(&raw_args);

    match &launch_intent {
        LaunchIntent::RestoreDefaultExplorer => {
            if let Err(err) = restore_windows_explorer() {
                eprintln!("Failed to restore Windows Explorer: {err}");
                std::process::exit(1);
            }
            return;
        }
        LaunchIntent::DelegateToWindowsExplorer(target) => {
            let _ = delegate_to_windows_explorer(target);
            return;
        }
        _ => {}
    }

    if !force_new_window && try_forward_to_running_instance(&launch_intent) {
        return;
    }

    let (ipc_tx, ipc_rx) = unbounded::<LaunchIntent>();
    let _single_instance_guard = start_single_instance_server(move |intent| {
        let _ = ipc_tx.unbounded_send(intent);
    });

    let initial_settings = AppSettings::load();
    let system_is_dark = system_prefers_dark_mode();

    gpui_kit::application()
        .with_assets(TExplorerAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            let initial_palette = theme::resolve_palette(
                initial_settings.theme,
                initial_settings.accent,
                system_is_dark,
            );
            theme::apply_theme(initial_palette, cx);

            cx.bind_keys([
                KeyBinding::new("ctrl-t", NewTab, Some("TExplorerWorkspace")),
                KeyBinding::new("ctrl-w", CloseTab, Some("TExplorerWorkspace")),
                KeyBinding::new("ctrl-tab", NextTab, Some("TExplorerWorkspace")),
                KeyBinding::new("ctrl-shift-tab", PrevTab, Some("TExplorerWorkspace")),
                KeyBinding::new("ctrl-\\", ToggleSplitPane, Some("TExplorerWorkspace")),
                KeyBinding::new("f3", ToggleSplitPane, Some("TExplorerWorkspace")),
                KeyBinding::new("ctrl-shift-s", SwitchActivePane, Some("TExplorerWorkspace")),
                KeyBinding::new("ctrl-l", FocusAddressBar, Some("TExplorerWorkspace")),
                KeyBinding::new("alt-d", FocusAddressBar, Some("TExplorerWorkspace")),
                KeyBinding::new("alt-left", GoBack, Some("TExplorerWorkspace")),
                KeyBinding::new("alt-right", GoForward, Some("TExplorerWorkspace")),
                KeyBinding::new("alt-up", GoUp, Some("TExplorerWorkspace")),
                KeyBinding::new("f5", RefreshPane, Some("TExplorerWorkspace")),
                KeyBinding::new("ctrl-r", RefreshPane, Some("TExplorerWorkspace")),
                KeyBinding::new("ctrl-f", FocusFilter, Some("TExplorerWorkspace")),
                KeyBinding::new("ctrl-k", OpenGlobalSearch, Some("TExplorerWorkspace")),
                KeyBinding::new("ctrl-b", ToggleSidebar, Some("TExplorerWorkspace")),
                KeyBinding::new("alt-p", ToggleInspector, Some("TExplorerWorkspace")),
                KeyBinding::new("ctrl-h", ToggleHiddenFiles, Some("TExplorerWorkspace")),
                KeyBinding::new("ctrl-a", SelectAllItems, Some("TExplorerWorkspace")),
                KeyBinding::new("ctrl-c", CopySelected, Some("TExplorerWorkspace")),
                KeyBinding::new("ctrl-x", CutSelected, Some("TExplorerWorkspace")),
                KeyBinding::new("ctrl-v", PasteItems, Some("TExplorerWorkspace")),
                KeyBinding::new("ctrl-z", UndoActionCmd, Some("TExplorerWorkspace")),
                KeyBinding::new("ctrl-y", RedoActionCmd, Some("TExplorerWorkspace")),
                KeyBinding::new("ctrl-shift-z", RedoActionCmd, Some("TExplorerWorkspace")),
                KeyBinding::new("ctrl-shift-n", CreateNewFolder, Some("TExplorerWorkspace")),
                KeyBinding::new("f2", RenameSelected, Some("TExplorerWorkspace")),
                KeyBinding::new("delete", DeleteToTrash, Some("TExplorerWorkspace")),
                KeyBinding::new("shift-delete", DeletePermanent, Some("TExplorerWorkspace")),
                KeyBinding::new("ctrl-,", OpenSettings, Some("TExplorerWorkspace")),
                KeyBinding::new("escape", CancelOrDismiss, Some("TExplorerWorkspace")),
                KeyBinding::new("ctrl-q", Quit, Some("TExplorerWorkspace")),
            ]);

            let bounds = WindowBounds::centered(size(px(1440.), px(900.)), cx);
            let options = WindowOptions {
                window_bounds: Some(bounds),
                titlebar: Some(TitlebarOptions {
                    title: None,
                    appears_transparent: true,
                    traffic_light_position: Some(point(px(9.), px(9.))),
                }),
                app_owns_titlebar_drag: true,
                ..Default::default()
            };

            let settings_for_window = initial_settings.clone();
            let intent_for_window = launch_intent.clone();
            let mut ipc_rx_opt = Some(ipc_rx);
            gpui_kit::open_window(options, cx, move |window, cx| {
                window.set_rem_size(px(theme::UI_REM_SIZE));
                let rx = ipc_rx_opt.take().expect("single window initialization");
                cx.new(|cx| {
                    Workspace::new(
                        settings_for_window,
                        system_is_dark,
                        intent_for_window,
                        rx,
                        window,
                        cx,
                    )
                })
            })
            .expect("failed to open TExplorer window");
        });
}
