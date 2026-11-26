use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use futures_channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use futures_util::StreamExt as _;
use gpui_kit::{
    ClipboardItem, Context, Entity, FocusHandle, Focusable, KeyDownEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, NavigationDirection, Pixels, Point, RenderImage,
    ScrollStrategy, SharedString, Subscription, UniformListScrollHandle, Window, WindowBounds,
    component::{
        h_flex,
        input::{InputEvent, InputState},
        resizable::{h_resizable, resizable_panel},
        v_flex,
    },
    div, prelude::*, px,
};
use texplorer_core::{
    AccentPreset, AppSession, AppSettings, FileEntry, KnownFolder, KnownFolderKind, Location,
    PaneSide, SavedPaneState, SavedTabState, SearchScope, SidebarSection, SizeUnit, SortColumn,
    SortSpec, TabModel, ThemePreset, ViewMode, VolumeInfo, WindowGeometry, sort_entries,
};
use texplorer_mft::{
    IndexServiceInstallState, IndexServiceStatus, install_service_elevated,
    query_live_service_status, query_service_install_state, rebuild_index_via_service,
    start_service_elevated, uninstall_service_elevated,
};
use texplorer_ops::{
    BulkRenameSpec, ConflictDecision, ConflictPrompt, ConflictResolution, RenameCasing,
    TransferControl, TransferEvent, TransferKind, TransferSnapshot, TransferStatus, UndoAction,
    UndoHistory, execute_bulk_rename, preview_bulk_rename, spawn_transfer_job,
    unique_keep_both_path,
};
use texplorer_platform::{
    DefaultExplorerStatus, DirectoryWatcher, FileLockStatus, LaunchIntent, ShellWorker,
    bring_current_process_window_to_front, cursor_screen_pos, delete_permanently,
    enable_default_explorer, get_clipboard_files, inspect_file_lock, list_known_folders,
    list_volumes, list_wsl_distros, move_to_trash, open_in_editor, open_item, open_terminal_here,
    query_default_explorer_status, restore_windows_explorer, reveal_in_windows_explorer,
    set_clipboard_files, set_win_e_interception, system_prefers_dark_mode,
};
use texplorer_vfs::archive::{
    compress_to_zip, extract_archive_all, extract_archive_entries, is_supported_archive,
};
use texplorer_vfs::folder_size::resolve_folder_sizes;
use texplorer_vfs::hash::{FileHashes, compute_file_hashes};
use texplorer_vfs::search::{SearchOutcome, execute_search};
use uuid::Uuid;

use crate::preview::{
    PreviewPayload, ThumbnailCache, ThumbnailKey, load_archive_entry_preview_blocking,
    load_preview_blocking, raw_bgra_to_render_image,
};
use crate::theme::{
    Palette, RADIUS_ROW, apply_theme, crisp_resize_handle_appearance, resolve_palette, solid,
};
use crate::ui::bulk_rename::{BulkRenameState, render_bulk_rename_modal};
use crate::ui::command_palette::{
    CommandPaletteState, execute_palette_command, filtered_palette_items,
    render_command_palette_modal,
};
use crate::ui::context_menu::{ContextMenuState, ContextMenuTarget, render_context_menu};
use crate::ui::global_search::{GlobalSearchState, render_global_search_modal};
use crate::ui::header::render_header;
use crate::ui::inspector::{HashInspectState, render_inspector};
use crate::ui::modals::{ActiveModal, SettingsCategory, TextPromptKind, render_active_modal};
use crate::ui::pane::render_pane;
use crate::ui::quicklook::render_quicklook_modal;
use crate::ui::sidebar::render_sidebar;
use crate::ui::transfers::{render_conflict_modal, render_transfer_drawer};
use crate::updater::{
    self, REPO_URL, StagedUpdate, UpdateInfo, UpdateState, open_url_in_browser,
    spawn_detached_updater,
};

gpui_kit::actions!(
    texplorer,
    [
        NewTab,
        CloseTab,
        NextTab,
        PrevTab,
        ToggleSplitPane,
        SwitchActivePane,
        FocusAddressBar,
        GoBack,
        GoForward,
        GoUp,
        RefreshPane,
        FocusFilter,
        OpenGlobalSearch,
        OpenCommandPalette,
        OpenBulkRename,
        ToggleSidebar,
        ToggleInspector,
        ToggleHiddenFiles,
        SelectAllItems,
        CopySelected,
        CutSelected,
        PasteItems,
        UndoActionCmd,
        RedoActionCmd,
        CreateNewFolder,
        RenameSelected,
        DeleteToTrash,
        DeletePermanent,
        OpenSettings,
        CancelOrDismiss,
        Quit,
    ]
);

static TOKIO_RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

fn tokio_runtime() -> &'static tokio::runtime::Runtime {
    TOKIO_RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()
            .expect("failed to initialize Tokio runtime")
    })
}

enum BgEvent {
    DirectoryLoaded {
        tab_id: Uuid,
        side: PaneSide,
        generation: u64,
        sort: SortSpec,
        result: Result<Vec<FileEntry>, String>,
    },
    DirectoryChanged {
        tab_id: Uuid,
        side: PaneSide,
        directory: PathBuf,
    },
    ExternalIntent(LaunchIntent),
    UpdateCheckFinished {
        manual: bool,
        result: Result<Option<UpdateInfo>, String>,
    },
    UpdateDownloadProgress {
        downloaded_bytes: u64,
        total_bytes: Option<u64>,
    },
    UpdateDownloadFinished {
        info: UpdateInfo,
        result: Result<StagedUpdate, String>,
    },
    ThumbnailReady {
        key: ThumbnailKey,
        image: Option<Arc<RenderImage>>,
    },
    PreviewReady {
        generation: u64,
        location: Location,
        payload: PreviewPayload,
    },
    HashesReady {
        generation: u64,
        path: PathBuf,
        result: Result<FileHashes, String>,
    },
    LockChecked {
        generation: u64,
        path: PathBuf,
        status: FileLockStatus,
    },
    ArchiveOpFinished {
        result: Result<(String, PathBuf), String>,
    },
    Transfer(TransferEvent),
    NativeShellMenuDone(Result<(), String>),
    ShellOpFinished {
        result: Result<Option<String>, String>,
        refresh_side: Option<PaneSide>,
    },
    FolderSizesReady {
        tab_id: Uuid,
        side: PaneSide,
        generation: u64,
        sizes: Vec<(PathBuf, u64)>,
    },
    GlobalSearchReady {
        generation: u64,
        outcome: Result<SearchOutcome, String>,
    },
    ServiceStatusReady {
        install_state: IndexServiceInstallState,
        live_status: Option<IndexServiceStatus>,
    },
    ServiceActionFinished {
        result: Result<String, String>,
        install_state: IndexServiceInstallState,
        live_status: Option<IndexServiceStatus>,
    },
}

#[derive(Clone, Copy)]
enum ToastKind {
    Info,
    Success,
    Error,
}

#[derive(Clone)]
struct ToastBanner {
    message: SharedString,
    kind: ToastKind,
    id: u64,
}

/// Active or recently finished background transfer job tracked by the UI drawer.
pub struct TransferJobState {
    pub snapshot: TransferSnapshot,
    pub control: Arc<TransferControl>,
}

/// UI-bound state for a single explorer pane.
pub struct PaneUiState {
    pub scroll_handle: UniformListScrollHandle,
    pub address_input: Entity<InputState>,
    pub filter_input: Entity<InputState>,
    pub editing_address: bool,
    watcher: Option<DirectoryWatcher>,
    _subs: Vec<Subscription>,
}

/// Combined domain model and GPUI controls for one top-level tab.
pub struct TabUiState {
    pub model: TabModel,
    pub left_ui: PaneUiState,
    pub right_ui: Option<PaneUiState>,
}

impl TabUiState {
    pub fn pane_ui(&self, side: PaneSide) -> &PaneUiState {
        match side {
            PaneSide::Left => &self.left_ui,
            PaneSide::Right => self.right_ui.as_ref().unwrap_or(&self.left_ui),
        }
    }

    pub fn pane_ui_mut(&mut self, side: PaneSide) -> &mut PaneUiState {
        match side {
            PaneSide::Left => &mut self.left_ui,
            PaneSide::Right => self.right_ui.as_mut().unwrap_or(&mut self.left_ui),
        }
    }
}

/// Live rubber-band / marquee drag selection state (click empty pane space + drag).
#[derive(Debug, Clone)]
pub struct MarqueeSelection {
    pub tab_id: Uuid,
    pub side: PaneSide,
    pub origin_x: f32,
    pub origin_y: f32,
    pub origin_content_y: f32,
    pub current_x: f32,
    pub current_y: f32,
    pub active: bool,
    pub additive: bool,
    pub initial_selection: Vec<usize>,
}

/// Root GPUI view coordinating tabs, dual-pane splits, transfers, preview/QuickLook, and OS shell.
pub struct Workspace {
    focus_handle: FocusHandle,
    pub settings: AppSettings,
    pub system_is_dark: bool,
    pub volumes: Vec<VolumeInfo>,
    pub known_folders: Vec<KnownFolder>,
    pub wsl_distros: Vec<(String, PathBuf)>,
    pub default_explorer_status: DefaultExplorerStatus,
    pub service_install_state: IndexServiceInstallState,
    pub service_live_status: Option<IndexServiceStatus>,
    pub service_busy: bool,
    pub update_state: UpdateState,
    pub tabs: Vec<TabUiState>,
    pub active_tab_index: usize,
    pub context_menu: Option<ContextMenuState>,
    pub active_modal: Option<ActiveModal>,
    pub settings_category: SettingsCategory,
    modal_subs: Vec<Subscription>,
    pub global_search: Option<GlobalSearchState>,
    global_search_subs: Vec<Subscription>,
    pub bulk_rename: Option<BulkRenameState>,
    bulk_rename_subs: Vec<Subscription>,
    pub command_palette: Option<CommandPaletteState>,
    command_palette_subs: Vec<Subscription>,
    folder_size_cache: HashMap<PathBuf, u64>,
    folder_size_cancels: HashMap<(Uuid, PaneSide), Arc<AtomicBool>>,
    pub quicklook_open: bool,
    pub preview_location: Option<Location>,
    pub preview_payload: PreviewPayload,
    preview_generation: u64,
    pub hash_target: Option<PathBuf>,
    pub hash_state: HashInspectState,
    hash_generation: u64,
    hash_cancel: Arc<AtomicBool>,
    pub hash_verify_input: Entity<InputState>,
    pub hash_verify_text: String,
    _hash_verify_sub: Subscription,
    pub lock_target: Option<PathBuf>,
    pub lock_status: FileLockStatus,
    lock_generation: u64,
    pub transfers: Vec<TransferJobState>,
    pub conflict_queue: VecDeque<ConflictPrompt>,
    pub conflict_apply_to_all: bool,
    undo_history: UndoHistory,
    cut_paths: HashSet<PathBuf>,
    shell_worker: ShellWorker,
    thumbnails: ThumbnailCache,
    toast: Option<ToastBanner>,
    toast_seq: u64,
    type_ahead_buf: String,
    type_ahead_at: Instant,
    last_window_geometry: Option<WindowGeometry>,
    pub marquee: Option<MarqueeSelection>,
    entry_claimed_mouse_down: bool,
    bg_tx: UnboundedSender<BgEvent>,
}

impl Workspace {
    pub fn new(
        settings: AppSettings,
        system_is_dark: bool,
        initial_intent: LaunchIntent,
        initial_session: Option<AppSession>,
        mut ipc_rx: UnboundedReceiver<LaunchIntent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (bg_tx, mut bg_rx) = unbounded::<BgEvent>();
        let ipc_forward_tx = bg_tx.clone();

        tokio_runtime().spawn(async move {
            while let Some(intent) = ipc_rx.next().await {
                let _ = ipc_forward_tx.unbounded_send(BgEvent::ExternalIntent(intent));
            }
        });

        let volumes = list_volumes();
        let known_folders = list_known_folders();
        let wsl_distros = list_wsl_distros();
        let default_explorer_status = query_default_explorer_status();
        let service_install_state = query_service_install_state();
        let shell_worker = ShellWorker::spawn();

        let explicit_launch = match initial_intent {
            LaunchIntent::OpenLocation(path) => Some((Location::local(path), None)),
            LaunchIntent::SelectFile(path) => {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned());
                let parent = path
                    .parent()
                    .map(Location::local)
                    .unwrap_or(Location::Home);
                Some((parent, name))
            }
            _ => None,
        };
        let initial_window_geom = initial_session.as_ref().and_then(|s| s.window);

        let hash_verify_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Paste expected SHA-256 or MD5 to verify...")
        });
        let hash_verify_sub = cx.subscribe_in(
            &hash_verify_input,
            window,
            |this, state, ev: &InputEvent, _window, cx| {
                if matches!(ev, InputEvent::Change) {
                    this.hash_verify_text = state.read(cx).value().to_string();
                    cx.notify();
                }
            },
        );

        let mut ws = Self {
            focus_handle: cx.focus_handle(),
            settings,
            system_is_dark,
            volumes,
            known_folders,
            wsl_distros,
            default_explorer_status,
            service_install_state,
            service_live_status: None,
            service_busy: false,
            update_state: UpdateState::Idle,
            tabs: Vec::new(),
            active_tab_index: 0,
            context_menu: None,
            active_modal: None,
            settings_category: SettingsCategory::Appearance,
            modal_subs: Vec::new(),
            global_search: None,
            global_search_subs: Vec::new(),
            bulk_rename: None,
            bulk_rename_subs: Vec::new(),
            command_palette: None,
            command_palette_subs: Vec::new(),
            folder_size_cache: HashMap::new(),
            folder_size_cancels: HashMap::new(),
            quicklook_open: false,
            preview_location: None,
            preview_payload: PreviewPayload::Loading,
            preview_generation: 0,
            hash_target: None,
            hash_state: HashInspectState::Idle,
            hash_generation: 0,
            hash_cancel: Arc::new(AtomicBool::new(false)),
            hash_verify_input,
            hash_verify_text: String::new(),
            _hash_verify_sub: hash_verify_sub,
            lock_target: None,
            lock_status: FileLockStatus::NotChecked,
            lock_generation: 0,
            transfers: Vec::new(),
            conflict_queue: VecDeque::new(),
            conflict_apply_to_all: false,
            undo_history: UndoHistory::default(),
            cut_paths: HashSet::new(),
            shell_worker,
            thumbnails: ThumbnailCache::default(),
            toast: None,
            toast_seq: 0,
            type_ahead_buf: String::new(),
            type_ahead_at: Instant::now() - Duration::from_secs(5),
            last_window_geometry: initial_window_geom,
            marquee: None,
            entry_claimed_mouse_down: false,
            bg_tx,
        };

        cx.spawn_in(window, async move |weak, cx| {
            while let Some(event) = bg_rx.next().await {
                let alive = weak.update_in(cx, |this, window, cx| {
                    this.handle_bg_event(event, window, cx);
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();

        let restored = if ws.settings.restore_tabs_on_startup
            && let Some(session) = initial_session
            && !session.tabs.is_empty()
        {
            ws.restore_tabs_from_session(session, window, cx);
            true
        } else {
            false
        };

        if let Some((loc, pending_select)) = explicit_launch {
            ws.open_tab_with_selection(loc, pending_select, window, cx);
        } else if !restored {
            ws.open_tab_with_selection(Location::Home, None, window, cx);
        }
        ws.refresh_service_status(cx);
        if ws.settings.auto_check_updates {
            ws.check_for_updates(false, cx);
        }
        ws.focus_handle.focus(window, cx);
        ws
    }

    pub fn active_tab(&self) -> Option<&TabUiState> {
        self.tabs.get(self.active_tab_index)
    }

    pub fn active_tab_mut(&mut self) -> Option<&mut TabUiState> {
        self.tabs.get_mut(self.active_tab_index)
    }

    /// Resolves the active dynamic surface, text, accent, and syntax color palette.
    pub fn palette(&self) -> Palette {
        resolve_palette(
            self.settings.theme,
            self.settings.accent,
            self.system_is_dark,
        )
    }

    pub fn is_path_cut(&self, path: &Path) -> bool {
        self.cut_paths.contains(path)
    }

    /// Returns a cached native Shell thumbnail or queues a deduplicated COM STA lookup.
    pub fn get_or_request_thumbnail(
        &mut self,
        path: &Path,
        modified_secs: i64,
        size_px: i32,
    ) -> Option<Arc<RenderImage>> {
        let key = ThumbnailKey {
            path: path.to_path_buf(),
            modified_secs,
            size_px,
        };
        if let Some(cached) = self.thumbnails.get(&key) {
            return cached.clone();
        }
        if self.thumbnails.try_mark_in_flight(key.clone()) {
            let tx = self.bg_tx.clone();
            let path_owned = path.to_path_buf();
            self.shell_worker.request_thumbnail(path_owned, size_px, move |raw| {
                let image = raw.and_then(raw_bgra_to_render_image);
                let _ = tx.unbounded_send(BgEvent::ThumbnailReady { key, image });
            });
        }
        None
    }

    fn create_pane_ui(
        &self,
        tab_id: Uuid,
        side: PaneSide,
        initial_location: &Location,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PaneUiState {
        let initial_path = initial_location.display_path();
        let address_input = cx.new(|cx| {
            let mut state = InputState::new(window, cx).placeholder("Enter path or Home...");
            state.set_value(initial_path, window, cx);
            state
        });
        let filter_input = cx.new(|cx| InputState::new(window, cx).placeholder("Filter..."));

        let sub_addr = cx.subscribe_in(
            &address_input,
            window,
            move |this, state, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } => {
                    let raw = state.read(cx).value().to_string();
                    this.commit_address_input(tab_id, side, &raw, window, cx);
                }
                InputEvent::Blur => {
                    this.cancel_address_edit(tab_id, side, window, cx);
                }
                _ => {}
            },
        );

        let sub_filter = cx.subscribe_in(
            &filter_input,
            window,
            move |this, state, event: &InputEvent, _window, cx| {
                if matches!(event, InputEvent::Change) {
                    let val = state.read(cx).value().to_string();
                    this.update_pane_filter(tab_id, side, val, cx);
                }
            },
        );

        PaneUiState {
            scroll_handle: UniformListScrollHandle::new(),
            address_input,
            filter_input,
            editing_address: false,
            watcher: None,
            _subs: vec![sub_addr, sub_filter],
        }
    }

    fn handle_bg_event(
        &mut self,
        event: BgEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            BgEvent::DirectoryLoaded {
                tab_id,
                side,
                generation,
                sort,
                result,
            } => {
                let mut uncached_folders: Vec<PathBuf> = Vec::new();
                let calc_folders = self.settings.calculate_folder_sizes;
                if let Some(tab) = self.tabs.iter_mut().find(|t| t.model.id == tab_id) {
                    let pane = tab.model.pane_mut(side);
                    if pane.load_generation != generation {
                        return;
                    }
                    let is_local_dir = matches!(pane.location, Location::Local(_));
                    match result {
                        Ok(mut entries) => {
                            let mut patched_cached_sizes = false;
                            if calc_folders && is_local_dir {
                                for entry in &mut entries {
                                    if entry.is_dir()
                                        && entry.size.is_none()
                                        && let Some(local) = entry.location.as_local_path()
                                    {
                                        if let Some(&cached) = self.folder_size_cache.get(local) {
                                            entry.size = Some(cached);
                                            patched_cached_sizes = true;
                                        } else {
                                            uncached_folders.push(local.to_path_buf());
                                        }
                                    }
                                }
                            }
                            if pane.sort == sort
                                && !(patched_cached_sizes && pane.sort.column == SortColumn::Size)
                            {
                                pane.set_presorted_entries(entries);
                            } else {
                                pane.set_entries(entries);
                            }
                        }
                        Err(err) => {
                            pane.loading = false;
                            pane.entries.clear();
                            pane.filtered_indices.clear();
                            pane.selection.clear();
                            pane.selected_bytes = 0;
                            pane.error = Some(err);
                        }
                    }
                    self.sync_preview_for_selection(cx);
                    if !uncached_folders.is_empty() {
                        self.spawn_folder_size_calculation(
                            tab_id,
                            side,
                            generation,
                            uncached_folders,
                        );
                    }
                    cx.notify();
                }
            }
            BgEvent::DirectoryChanged {
                tab_id,
                side,
                directory,
            } => {
                self.invalidate_folder_sizes_under(&directory);
                let should_reload = self
                    .tabs
                    .iter()
                    .find(|t| t.model.id == tab_id)
                    .is_some_and(|tab| {
                        tab.model.pane(side).location.as_local_path() == Some(directory.as_path())
                    });
                if should_reload {
                    self.reload_pane_by_id(tab_id, side, cx);
                }
            }
            BgEvent::ExternalIntent(intent) => {
                window.activate_window();
                bring_current_process_window_to_front();
                let open_in_tab = self.settings.show_tabs;
                match intent {
                    LaunchIntent::OpenLocation(path) => {
                        let target = Location::local(path);
                        if open_in_tab {
                            self.open_new_tab(target, window, cx);
                        } else {
                            self.navigate_active_pane(target, window, cx);
                        }
                    }
                    LaunchIntent::SelectFile(path) => {
                        let name = path
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned());
                        let parent = path
                            .parent()
                            .map(Location::local)
                            .unwrap_or(Location::Home);
                        if open_in_tab {
                            self.open_tab_with_selection(parent, name, window, cx);
                        } else {
                            if let Some(tab) = self.active_tab_mut() {
                                tab.model.active_pane_mut().pending_select_name = name;
                            }
                            self.navigate_active_pane(parent, window, cx);
                        }
                    }
                    LaunchIntent::OpenDefault => {
                        if open_in_tab {
                            self.open_new_tab(Location::Home, window, cx);
                        } else {
                            self.navigate_active_pane(Location::Home, window, cx);
                        }
                    }
                    LaunchIntent::DelegateToWindowsExplorer(raw) => {
                        let _ = texplorer_platform::delegate_to_windows_explorer(&raw);
                    }
                    LaunchIntent::EnableDefaultExplorer { intercept_win_e } => {
                        self.settings.intercept_win_e = intercept_win_e;
                        self.enable_default_file_manager(cx);
                    }
                    LaunchIntent::RestoreDefaultExplorer => {
                        self.restore_default_file_manager(cx);
                    }
                    LaunchIntent::RunWinEDaemon | LaunchIntent::StopWinEDaemon => {}
                }
            }
            BgEvent::UpdateCheckFinished { manual, result } => {
                if matches!(
                    self.update_state,
                    UpdateState::Downloading { .. } | UpdateState::Installing(_)
                ) {
                    return;
                }
                self.update_state = match result {
                    Ok(Some(info)) => UpdateState::Available(info),
                    Ok(None) => {
                        if manual {
                            UpdateState::UpToDate
                        } else {
                            UpdateState::Idle
                        }
                    }
                    Err(err) => {
                        if manual {
                            UpdateState::Failed(err)
                        } else {
                            UpdateState::Idle
                        }
                    }
                };
                cx.notify();
            }
            BgEvent::UpdateDownloadProgress {
                downloaded_bytes,
                total_bytes,
            } => {
                if let UpdateState::Downloading {
                    downloaded_bytes: cur,
                    total_bytes: total,
                    ..
                } = &mut self.update_state
                {
                    *cur = downloaded_bytes;
                    *total = total_bytes;
                    cx.notify();
                }
            }
            BgEvent::UpdateDownloadFinished { info, result } => match result {
                Ok(staged) => {
                    self.update_state = UpdateState::Installing(info);
                    cx.notify();
                    if let Err(err) = spawn_detached_updater(&staged) {
                        self.update_state = UpdateState::Failed(err);
                        cx.notify();
                    } else {
                        cx.quit();
                    }
                }
                Err(err) => {
                    self.update_state = UpdateState::Failed(err);
                    cx.notify();
                }
            },
            BgEvent::ThumbnailReady { key, image } => {
                self.thumbnails.insert(key, image);
                cx.notify();
            }
            BgEvent::PreviewReady {
                generation,
                location,
                payload,
            } => {
                if generation == self.preview_generation {
                    self.preview_location = Some(location);
                    self.preview_payload = payload;
                    cx.notify();
                }
            }
            BgEvent::HashesReady {
                generation,
                path,
                result,
            } => {
                if generation == self.hash_generation && self.hash_target.as_ref() == Some(&path) {
                    self.hash_state = match result {
                        Ok(hashes) => HashInspectState::Ready(hashes),
                        Err(err) => HashInspectState::Error(err),
                    };
                    cx.notify();
                }
            }
            BgEvent::LockChecked {
                generation,
                path,
                status,
            } => {
                if generation == self.lock_generation && self.lock_target.as_ref() == Some(&path) {
                    self.lock_status = status;
                    cx.notify();
                }
            }
            BgEvent::ArchiveOpFinished { result } => match result {
                Ok((msg, target_dir)) => {
                    self.invalidate_folder_sizes_under(&target_dir);
                    self.refresh_panes_showing(&target_dir, cx);
                    if let Some(parent) = target_dir.parent() {
                        self.invalidate_folder_sizes_under(parent);
                        self.refresh_panes_showing(parent, cx);
                    }
                    self.show_toast(msg, ToastKind::Success, window, cx);
                }
                Err(err) => {
                    self.show_toast(err, ToastKind::Error, window, cx);
                }
            },
            BgEvent::Transfer(xfer_ev) => match xfer_ev {
                TransferEvent::Progress(snap) => {
                    if let Some(job) = self
                        .transfers
                        .iter_mut()
                        .find(|j| j.snapshot.job_id == snap.job_id)
                    {
                        job.snapshot = snap;
                        cx.notify();
                    }
                }
                TransferEvent::Conflict(prompt) => {
                    self.conflict_apply_to_all = false;
                    self.conflict_queue.push_back(prompt);
                    cx.notify();
                }
                TransferEvent::Finished {
                    snapshot,
                    undo_action,
                } => {
                    let job_id = snapshot.job_id;
                    let target_dir = snapshot.target_dir.clone();
                    let status = snapshot.status.clone();
                    let kind = snapshot.kind;
                    let auto_dismiss = matches!(
                        status,
                        TransferStatus::Completed | TransferStatus::Cancelled
                    );
                    if let Some(job) = self
                        .transfers
                        .iter_mut()
                        .find(|j| j.snapshot.job_id == job_id)
                    {
                        job.snapshot = snapshot;
                    }
                    if let Some(undo) = undo_action {
                        self.undo_history.push(undo);
                    }
                    if kind == TransferKind::Move && matches!(status, TransferStatus::Completed) {
                        self.cut_paths.clear();
                    }
                    self.invalidate_folder_sizes_under(&target_dir);
                    self.refresh_panes_showing(&target_dir, cx);
                    if let TransferStatus::Failed(err) = status {
                        self.show_toast(err, ToastKind::Error, window, cx);
                    }
                    if auto_dismiss {
                        cx.spawn_in(window, async move |weak, cx| {
                            smol_sleep(Duration::from_secs(3)).await;
                            let _ = weak.update_in(cx, |this, _window, cx| {
                                let before = this.transfers.len();
                                this.transfers.retain(|j| j.snapshot.job_id != job_id);
                                if this.transfers.len() != before {
                                    cx.notify();
                                }
                            });
                        })
                        .detach();
                    }
                    cx.notify();
                }
            },
            BgEvent::NativeShellMenuDone(res) => {
                if let Err(err) = res {
                    self.show_toast(err, ToastKind::Error, window, cx);
                } else {
                    self.refresh_visible_panes(cx);
                }
            }
            BgEvent::ShellOpFinished {
                result,
                refresh_side,
            } => {
                if let Some(side) = refresh_side {
                    self.refresh_pane(side, window, cx);
                }
                match result {
                    Ok(Some(msg)) => self.show_toast(msg, ToastKind::Success, window, cx),
                    Ok(None) => {}
                    Err(err) => self.show_toast(err, ToastKind::Error, window, cx),
                }
            }
            BgEvent::FolderSizesReady {
                tab_id,
                side,
                generation,
                sizes,
            } => {
                if self.folder_size_cache.len() > 20_000 {
                    self.folder_size_cache.clear();
                }
                for (path, bytes) in &sizes {
                    self.folder_size_cache.insert(path.clone(), *bytes);
                }
                if let Some(tab) = self.tabs.iter_mut().find(|t| t.model.id == tab_id) {
                    let pane = tab.model.pane_mut(side);
                    if pane.load_generation == generation {
                        let size_map: HashMap<&Path, u64> =
                            sizes.iter().map(|(p, b)| (p.as_path(), *b)).collect();
                        let mut changed = false;
                        for entry in &mut pane.entries {
                            if entry.is_dir()
                                && let Some(local) = entry.location.as_local_path()
                                && let Some(&bytes) = size_map.get(local)
                            {
                                entry.size = Some(bytes);
                                changed = true;
                            }
                        }
                        if changed {
                            if pane.sort.column == SortColumn::Size {
                                pane.resort();
                            } else {
                                pane.recompute_selection_bytes();
                            }
                            cx.notify();
                        }
                    }
                }
            }
            BgEvent::GlobalSearchReady {
                generation,
                outcome,
            } => {
                if let Some(gs) = self.global_search.as_mut()
                    && gs.generation == generation
                {
                    gs.searching = false;
                    match outcome {
                        Ok(res) => {
                            gs.results = res.entries;
                            gs.total_matches = res.total_matches;
                            gs.elapsed_ms = res.elapsed_ms;
                            gs.from_mft = res.from_mft;
                            gs.truncated = res.truncated;
                            gs.selected_idx = 0;
                        }
                        Err(err) => {
                            gs.results.clear();
                            gs.total_matches = 0;
                            self.show_toast(err, ToastKind::Error, window, cx);
                        }
                    }
                    cx.notify();
                }
            }
            BgEvent::ServiceStatusReady {
                install_state,
                live_status,
            } => {
                self.service_install_state = install_state;
                self.service_live_status = live_status;
                cx.notify();
            }
            BgEvent::ServiceActionFinished {
                result,
                install_state,
                live_status,
            } => {
                self.service_busy = false;
                self.service_install_state = install_state;
                self.service_live_status = live_status;
                match result {
                    Ok(msg) => self.show_toast(msg, ToastKind::Success, window, cx),
                    Err(err) => self.show_toast(err, ToastKind::Error, window, cx),
                }
                cx.notify();
            }
        }
    }

    fn refresh_panes_showing(&mut self, dir: &Path, cx: &mut Context<Self>) {
        let mut to_reload = Vec::new();
        for tab in &self.tabs {
            for side in [PaneSide::Left, PaneSide::Right] {
                if side == PaneSide::Right && !tab.model.split_enabled {
                    continue;
                }
                if tab.model.pane(side).location.as_local_path() == Some(dir) {
                    to_reload.push((tab.model.id, side));
                }
            }
        }
        for (tab_id, side) in to_reload {
            self.reload_pane_by_id(tab_id, side, cx);
        }
    }

    fn refresh_visible_panes(&mut self, cx: &mut Context<Self>) {
        let Some(tab) = self.active_tab() else {
            return;
        };
        let tab_id = tab.model.id;
        let is_split = tab.model.split_enabled;
        self.reload_pane_by_id(tab_id, PaneSide::Left, cx);
        if is_split {
            self.reload_pane_by_id(tab_id, PaneSide::Right, cx);
        }
    }

    /// Triggers an asynchronous preview load whenever Inspector or QuickLook is visible
    /// (supporting both local filesystem items and virtual archive entries).
    fn sync_preview_for_selection(&mut self, cx: &mut Context<Self>) {
        if !self.settings.show_inspector && !self.quicklook_open {
            return;
        }
        let target = self
            .active_tab()
            .and_then(|t| t.model.active_pane_ref().primary_selected_entry())
            .filter(|e| matches!(e.location, Location::Local(_) | Location::Archive { .. }))
            .map(|e| (e.location.clone(), e.kind));

        let Some((location, kind)) = target else {
            self.preview_location = None;
            self.preview_payload = PreviewPayload::Loading;
            return;
        };

        if self.preview_location.as_ref() == Some(&location)
            && !matches!(self.preview_payload, PreviewPayload::Loading)
        {
            return;
        }

        self.preview_generation = self.preview_generation.wrapping_add(1);
        let generation = self.preview_generation;
        self.preview_location = Some(location.clone());
        self.preview_payload = PreviewPayload::Loading;

        let worker = self.shell_worker.clone();
        let palette = self.palette();
        let tx = self.bg_tx.clone();
        tokio_runtime().spawn(async move {
            let loc_clone = location.clone();
            let payload = tokio::task::spawn_blocking(move || match loc_clone {
                Location::Local(path) => load_preview_blocking(path, kind, worker, palette),
                Location::Archive {
                    archive_path,
                    inner_path,
                } => load_archive_entry_preview_blocking(archive_path, inner_path, kind, palette),
                _ => PreviewPayload::Unsupported {
                    reason: "Preview is not available for this location.".to_string(),
                },
            })
            .await
            .unwrap_or_else(|_| PreviewPayload::Unsupported {
                reason: "Preview worker failed.".to_string(),
            });
            let _ = tx.unbounded_send(BgEvent::PreviewReady {
                generation,
                location,
                payload,
            });
        });
        cx.notify();
    }

    pub fn open_new_tab(
        &mut self,
        location: Location,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_tab_with_selection(location, None, window, cx);
    }

    fn open_tab_with_selection(
        &mut self,
        location: Location,
        pending_select: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut model = TabModel::new(
            location.clone(),
            self.settings.default_view_mode,
            self.settings.show_hidden_files,
        );
        model.left.pending_select_name = pending_select;
        let tab_id = model.id;
        let left_ui = self.create_pane_ui(tab_id, PaneSide::Left, &location, window, cx);
        self.tabs.push(TabUiState {
            model,
            left_ui,
            right_ui: None,
        });
        self.active_tab_index = self.tabs.len() - 1;
        self.reload_pane_by_id(tab_id, PaneSide::Left, cx);
        self.rebind_watcher(tab_id, PaneSide::Left);
        self.save_session(Some(window));
        cx.notify();
    }

    fn restore_tabs_from_session(
        &mut self,
        session: AppSession,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut panes_to_load: Vec<(Uuid, PaneSide)> = Vec::new();
        for saved in session.tabs {
            let mut model = TabModel::new(
                saved.left.location.clone(),
                saved.left.view_mode,
                saved.left.show_hidden,
            );
            model.left.sort = saved.left.sort;
            let tab_id = model.id;
            let left_ui =
                self.create_pane_ui(tab_id, PaneSide::Left, &saved.left.location, window, cx);
            panes_to_load.push((tab_id, PaneSide::Left));

            let right_ui = if let Some(saved_right) = saved.right {
                let mut r_model = texplorer_core::PaneModel::new(
                    saved_right.location.clone(),
                    saved_right.view_mode,
                    saved_right.show_hidden,
                );
                r_model.sort = saved_right.sort;
                model.right = Some(r_model);
                model.split_enabled = saved.split_enabled;
                model.active_pane = if saved.split_enabled {
                    saved.active_pane
                } else {
                    PaneSide::Left
                };
                let ui = self.create_pane_ui(
                    tab_id,
                    PaneSide::Right,
                    &saved_right.location,
                    window,
                    cx,
                );
                if saved.split_enabled {
                    panes_to_load.push((tab_id, PaneSide::Right));
                }
                Some(ui)
            } else {
                None
            };

            self.tabs.push(TabUiState {
                model,
                left_ui,
                right_ui,
            });
        }

        if !self.tabs.is_empty() {
            self.active_tab_index = session.active_tab_index.min(self.tabs.len() - 1);
            for (tab_id, side) in panes_to_load {
                self.reload_pane_by_id(tab_id, side, cx);
                self.rebind_watcher(tab_id, side);
            }
        }
    }

    fn capture_window_geometry(&mut self, window: &Window) {
        let (rect, maximized) = match window.window_bounds() {
            WindowBounds::Windowed(r) => (r, false),
            WindowBounds::Maximized(r) | WindowBounds::Fullscreen(r) => (r, true),
        };
        let geom = WindowGeometry {
            x: f32::from(rect.origin.x),
            y: f32::from(rect.origin.y),
            width: f32::from(rect.size.width),
            height: f32::from(rect.size.height),
            maximized,
        };
        if geom.is_valid() {
            self.last_window_geometry = Some(geom);
        }
    }

    fn save_session(&mut self, window: Option<&Window>) {
        if let Some(w) = window {
            self.capture_window_geometry(w);
        }
        self.persist_session_snapshot();
    }

    fn persist_session_snapshot(&self) {
        if !self.settings.restore_tabs_on_startup || self.tabs.is_empty() {
            return;
        }
        let tabs: Vec<SavedTabState> = self
            .tabs
            .iter()
            .map(|t| SavedTabState {
                split_enabled: t.model.split_enabled,
                active_pane: t.model.active_pane,
                left: SavedPaneState {
                    location: t.model.left.location.clone(),
                    view_mode: t.model.left.view_mode,
                    sort: t.model.left.sort,
                    show_hidden: t.model.left.show_hidden,
                },
                right: t.model.right.as_ref().map(|r| SavedPaneState {
                    location: r.location.clone(),
                    view_mode: r.view_mode,
                    sort: r.sort,
                    show_hidden: r.show_hidden,
                }),
            })
            .collect();
        let session = AppSession {
            version: 1,
            active_tab_index: self.active_tab_index.min(tabs.len().saturating_sub(1)),
            tabs,
            window: self.last_window_geometry,
        };
        let _ = session.save();
    }

    pub fn toggle_restore_tabs_on_startup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.settings.restore_tabs_on_startup = !self.settings.restore_tabs_on_startup;
        let _ = self.settings.save();
        if self.settings.restore_tabs_on_startup {
            self.save_session(Some(window));
        } else {
            AppSession::clear();
        }
        cx.notify();
    }

    pub fn select_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index < self.tabs.len() {
            self.active_tab_index = index;
            self.context_menu = None;
            self.sync_preview_for_selection(cx);
            self.save_session(Some(window));
            cx.notify();
        }
    }

    pub fn close_tab_at(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.tabs.len() <= 1 {
            self.navigate_active_pane(Location::Home, window, cx);
            return;
        }
        if index < self.tabs.len() {
            self.tabs.remove(index);
            if self.active_tab_index >= self.tabs.len() {
                self.active_tab_index = self.tabs.len() - 1;
            }
            self.context_menu = None;
            self.sync_preview_for_selection(cx);
            self.save_session(Some(window));
            cx.notify();
        }
    }

    pub fn toggle_split_active_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get_mut(self.active_tab_index) else {
            return;
        };
        let tab_id = tab.model.id;
        let enabled = tab.model.toggle_split();
        if enabled && tab.right_ui.is_none() {
            let loc = tab.model.pane(PaneSide::Right).location.clone();
            let ui = self.create_pane_ui(tab_id, PaneSide::Right, &loc, window, cx);
            if let Some(tab_mut) = self.tabs.get_mut(self.active_tab_index) {
                tab_mut.right_ui = Some(ui);
            }
            self.reload_pane_by_id(tab_id, PaneSide::Right, cx);
            self.rebind_watcher(tab_id, PaneSide::Right);
        }
        self.save_session(Some(window));
        cx.notify();
    }

    pub fn open_in_opposite_pane(
        &mut self,
        location: Location,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.get(self.active_tab_index) else {
            return;
        };
        let was_split = tab.model.split_enabled;
        let target_side = if was_split {
            tab.model.active_pane.opposite()
        } else {
            PaneSide::Right
        };
        if !was_split {
            self.toggle_split_active_tab(window, cx);
        }
        self.navigate_pane(target_side, location, window, cx);
        self.set_active_pane(target_side, cx);
    }

    pub fn set_active_pane(&mut self, side: PaneSide, cx: &mut Context<Self>) {
        if let Some(tab) = self.active_tab_mut() {
            let effective = if tab.model.split_enabled {
                side
            } else {
                PaneSide::Left
            };
            if tab.model.active_pane != effective {
                tab.model.active_pane = effective;
                self.sync_preview_for_selection(cx);
                cx.notify();
            }
        }
    }

    pub fn navigate_active_pane(
        &mut self,
        target: Location,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let side = self
            .active_tab()
            .map(|t| t.model.active_pane)
            .unwrap_or(PaneSide::Left);
        self.navigate_pane(side, target, window, cx);
    }

    pub fn navigate_pane(
        &mut self,
        side: PaneSide,
        target: Location,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.active_tab_mut() else {
            return;
        };
        let tab_id = tab.model.id;
        tab.model.active_pane = side;
        let changed = tab.model.pane_mut(side).navigate_to(target.clone());
        let ui = tab.pane_ui_mut(side);
        ui.editing_address = false;
        let display = target.display_path();
        ui.address_input
            .update(cx, |s, cx| s.set_value(display, window, cx));
        ui.filter_input
            .update(cx, |s, cx| s.set_value("", window, cx));

        if changed {
            self.reload_pane_by_id(tab_id, side, cx);
            self.rebind_watcher(tab_id, side);
            self.save_session(Some(window));
        }
        self.focus_handle.focus(window, cx);
        cx.notify();
    }

    pub fn go_back(&mut self, side: PaneSide, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.active_tab_mut() else {
            return;
        };
        let tab_id = tab.model.id;
        if let Some(loc) = tab.model.pane_mut(side).go_back() {
            let display = loc.display_path();
            let ui = tab.pane_ui_mut(side);
            ui.editing_address = false;
            ui.address_input
                .update(cx, |s, cx| s.set_value(display, window, cx));
            ui.filter_input
                .update(cx, |s, cx| s.set_value("", window, cx));
            self.reload_pane_by_id(tab_id, side, cx);
            self.rebind_watcher(tab_id, side);
            self.save_session(Some(window));
            cx.notify();
        }
    }

    pub fn go_forward(&mut self, side: PaneSide, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.active_tab_mut() else {
            return;
        };
        let tab_id = tab.model.id;
        if let Some(loc) = tab.model.pane_mut(side).go_forward() {
            let display = loc.display_path();
            let ui = tab.pane_ui_mut(side);
            ui.editing_address = false;
            ui.address_input
                .update(cx, |s, cx| s.set_value(display, window, cx));
            ui.filter_input
                .update(cx, |s, cx| s.set_value("", window, cx));
            self.reload_pane_by_id(tab_id, side, cx);
            self.rebind_watcher(tab_id, side);
            self.save_session(Some(window));
            cx.notify();
        }
    }

    pub fn go_up(&mut self, side: PaneSide, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.active_tab_mut() else {
            return;
        };
        let tab_id = tab.model.id;
        if let Some(loc) = tab.model.pane_mut(side).go_up() {
            let display = loc.display_path();
            let ui = tab.pane_ui_mut(side);
            ui.editing_address = false;
            ui.address_input
                .update(cx, |s, cx| s.set_value(display, window, cx));
            ui.filter_input
                .update(cx, |s, cx| s.set_value("", window, cx));
            self.reload_pane_by_id(tab_id, side, cx);
            self.rebind_watcher(tab_id, side);
            self.save_session(Some(window));
            cx.notify();
        }
    }

    pub fn refresh_pane(&mut self, side: PaneSide, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab_id) = self.active_tab().map(|t| t.model.id) else {
            return;
        };
        if self
            .active_tab()
            .is_some_and(|t| t.model.pane(side).location == Location::Home)
        {
            self.refresh_volumes(cx);
        } else {
            self.reload_pane_by_id(tab_id, side, cx);
        }
    }

    fn reload_pane_by_id(&mut self, tab_id: Uuid, side: PaneSide, cx: &mut Context<Self>) {
        if let Some(prev_cancel) = self.folder_size_cancels.remove(&(tab_id, side)) {
            prev_cancel.store(true, Ordering::Relaxed);
        }
        let use_mft = self.settings.use_mft_service;
        let Some(tab) = self.tabs.iter_mut().find(|t| t.model.id == tab_id) else {
            return;
        };
        let pane = tab.model.pane_mut(side);
        pane.load_generation = pane.load_generation.wrapping_add(1);
        let generation = pane.load_generation;
        let location = pane.location.clone();
        let sort = pane.sort;

        if location == Location::Home {
            pane.entries.clear();
            pane.filtered_indices.clear();
            pane.selection.clear();
            pane.selected_bytes = 0;
            pane.loading = false;
            pane.error = None;
            cx.notify();
            return;
        }

        pane.loading = true;
        pane.error = None;
        let tx = self.bg_tx.clone();
        tokio_runtime().spawn(async move {
            let loc_clone = location.clone();
            let result = tokio::task::spawn_blocking(move || {
                let mut entries = match &loc_clone {
                    Location::Search { query, scope } => {
                        let cancel = AtomicBool::new(false);
                        execute_search(query, scope, use_mft, 1_000, &cancel).map(|r| r.entries)?
                    }
                    _ => texplorer_vfs::list_location(&loc_clone)?,
                };
                sort_entries(&mut entries, sort);
                Ok(entries)
            })
            .await
            .unwrap_or_else(|e| Err(format!("Directory task panicked: {e}")));
            let _ = tx.unbounded_send(BgEvent::DirectoryLoaded {
                tab_id,
                side,
                generation,
                sort,
                result,
            });
        });
        cx.notify();
    }

    fn spawn_folder_size_calculation(
        &mut self,
        tab_id: Uuid,
        side: PaneSide,
        generation: u64,
        folders: Vec<PathBuf>,
    ) {
        if let Some(prev) = self.folder_size_cancels.remove(&(tab_id, side)) {
            prev.store(true, Ordering::Relaxed);
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.folder_size_cancels
            .insert((tab_id, side), Arc::clone(&cancel));
        let use_mft = self.settings.use_mft_service;
        let tx = self.bg_tx.clone();

        tokio_runtime().spawn(async move {
            let sizes = tokio::task::spawn_blocking(move || {
                resolve_folder_sizes(&folders, use_mft, &cancel)
            })
            .await
            .unwrap_or_default();
            if !sizes.is_empty() {
                let _ = tx.unbounded_send(BgEvent::FolderSizesReady {
                    tab_id,
                    side,
                    generation,
                    sizes,
                });
            }
        });
    }

    fn invalidate_folder_sizes_under(&mut self, dir: &Path) {
        self.folder_size_cache
            .retain(|k, _| !k.starts_with(dir) && !dir.starts_with(k));
    }

    fn rebind_watcher(&mut self, tab_id: Uuid, side: PaneSide) {
        let tx = self.bg_tx.clone();
        let Some(tab) = self.tabs.iter_mut().find(|t| t.model.id == tab_id) else {
            return;
        };
        let dir_opt = tab
            .model
            .pane(side)
            .location
            .as_local_path()
            .map(|p| p.to_path_buf());
        let ui = tab.pane_ui_mut(side);
        ui.watcher = None;
        if let Some(dir) = dir_opt
            && dir.is_dir()
            && let Ok(watcher) = DirectoryWatcher::start(dir, move |ev| {
                let _ = tx.unbounded_send(BgEvent::DirectoryChanged {
                    tab_id,
                    side,
                    directory: ev.directory,
                });
            })
        {
            ui.watcher = Some(watcher);
        }
    }

    pub fn begin_edit_address(
        &mut self,
        side: PaneSide,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.active_tab_mut() else {
            return;
        };
        tab.model.active_pane = side;
        let display = tab.model.pane(side).location.display_path();
        let ui = tab.pane_ui_mut(side);
        ui.editing_address = true;
        let input = ui.address_input.clone();
        input.update(cx, |s, cx| {
            s.set_value(display, window, cx);
            s.focus(window, cx);
            s.select_all(window, cx);
        });
        cx.notify();
    }

    fn commit_address_input(
        &mut self,
        tab_id: Uuid,
        side: PaneSide,
        raw: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(tab) = self.tabs.iter_mut().find(|t| t.model.id == tab_id) {
            tab.pane_ui_mut(side).editing_address = false;
        }
        if let Some(mut target) = Location::parse_user_input(raw) {
            if let Some(local) = target.as_local_path() {
                if !local.exists() {
                    self.show_toast(
                        format!("Path not found: {}", local.display()),
                        ToastKind::Error,
                        window,
                        cx,
                    );
                    cx.notify();
                    return;
                }
                if local.is_file() && is_supported_archive(local) {
                    target = Location::archive(local, "");
                }
            }
            self.navigate_pane(side, target, window, cx);
        } else {
            cx.notify();
        }
    }

    fn cancel_address_edit(
        &mut self,
        tab_id: Uuid,
        side: PaneSide,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(tab) = self.tabs.iter_mut().find(|t| t.model.id == tab_id) {
            let display = tab.model.pane(side).location.display_path();
            let ui = tab.pane_ui_mut(side);
            if ui.editing_address {
                ui.editing_address = false;
                ui.address_input
                    .update(cx, |s, cx| s.set_value(display, window, cx));
                cx.notify();
            }
        }
    }

    fn update_pane_filter(
        &mut self,
        tab_id: Uuid,
        side: PaneSide,
        query: String,
        cx: &mut Context<Self>,
    ) {
        if let Some(tab) = self.tabs.iter_mut().find(|t| t.model.id == tab_id) {
            let pane = tab.model.pane_mut(side);
            if pane.quick_filter != query {
                pane.quick_filter = query;
                pane.rebuild_filter();
                cx.notify();
            }
        }
    }

    pub fn clear_pane_filter(
        &mut self,
        side: PaneSide,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.active_tab_mut() else {
            return;
        };
        tab.model.pane_mut(side).quick_filter.clear();
        tab.model.pane_mut(side).rebuild_filter();
        tab.pane_ui_mut(side)
            .filter_input
            .update(cx, |s, cx| s.set_value("", window, cx));
        cx.notify();
    }

    pub fn set_pane_view_mode(&mut self, side: PaneSide, mode: ViewMode, cx: &mut Context<Self>) {
        if let Some(tab) = self.active_tab_mut() {
            tab.model.pane_mut(side).view_mode = mode;
            self.save_session(None);
            cx.notify();
        }
    }

    pub fn toggle_hidden_for_pane(&mut self, side: PaneSide, cx: &mut Context<Self>) {
        if let Some(tab) = self.active_tab_mut() {
            let pane = tab.model.pane_mut(side);
            pane.show_hidden = !pane.show_hidden;
            pane.rebuild_filter();
            self.save_session(None);
            cx.notify();
        }
    }

    pub fn toggle_sort_column(
        &mut self,
        side: PaneSide,
        column: SortColumn,
        cx: &mut Context<Self>,
    ) {
        if let Some(tab) = self.active_tab_mut() {
            let pane = tab.model.pane_mut(side);
            pane.sort.toggle_column(column);
            pane.resort();
            self.save_session(None);
            cx.notify();
        }
    }

    pub fn handle_entry_left_click(
        &mut self,
        side: PaneSide,
        vis_idx: usize,
        ev: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.entry_claimed_mouse_down = true;
        self.marquee = None;
        self.context_menu = None;
        let open_target = {
            let Some(tab) = self.active_tab_mut() else {
                return;
            };
            tab.model.active_pane = side;
            let pane = tab.model.pane_mut(side);
            if ev.click_count >= 2 {
                pane.selection.select_only(vis_idx);
                pane.recompute_selection_bytes();
                pane.visible_entry(vis_idx)
                    .map(|e| (e.location.clone(), e.is_dir()))
            } else {
                if ev.modifiers.shift {
                    pane.selection.select_range(vis_idx, ev.modifiers.control);
                } else if ev.modifiers.control {
                    pane.selection.toggle(vis_idx);
                } else {
                    pane.selection.select_only(vis_idx);
                }
                pane.recompute_selection_bytes();
                None
            }
        };

        self.focus_handle.focus(window, cx);
        if let Some((loc, is_dir)) = open_target {
            self.open_location_or_file(loc, is_dir, window, cx);
        } else {
            self.sync_preview_for_selection(cx);
            cx.notify();
        }
    }

    /// Estimates the available inner width (in logical px) of a single explorer pane for
    /// responsive Home and Grid column counts.
    pub fn estimated_pane_width(&self, split_enabled: bool) -> f32 {
        let mut width = self
            .last_window_geometry
            .as_ref()
            .map(|g| g.width)
            .unwrap_or(1280.0)
            .max(640.0);
        if self.settings.show_sidebar {
            width -= 256.0;
        }
        if self.settings.show_inspector {
            width -= 320.0;
        }
        if split_enabled {
            width *= 0.5;
        }
        width.max(260.0)
    }

    /// Computes responsive column count for Grid view based on window & split layout.
    pub fn grid_view_columns(&self, split_enabled: bool) -> usize {
        let inner = (self.estimated_pane_width(split_enabled) - 24.0).max(160.0);
        ((inner / 150.0).floor() as usize).clamp(2, 10)
    }

    /// Starts rubber-band drag selection when left-clicking empty background inside a file list.
    pub fn begin_marquee_selection(
        &mut self,
        side: PaneSide,
        ev: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if std::mem::take(&mut self.entry_claimed_mouse_down) {
            return;
        }
        self.context_menu = None;
        self.focus_handle.focus(window, cx);

        let Some(tab) = self.active_tab_mut() else {
            return;
        };
        tab.model.active_pane = side;
        let tab_id = tab.model.id;
        let additive = ev.modifiers.control;

        let (bounds_y, scroll_y) = {
            let state = tab.pane_ui(side).scroll_handle.0.borrow();
            let by: f32 = state.base_handle.bounds().origin.y.into();
            let sy: f32 = state.base_handle.offset().y.into();
            (by, sy)
        };

        let pane = tab.model.pane_mut(side);
        let initial_selection: Vec<usize> = if additive {
            pane.selection.selected.iter().copied().collect()
        } else {
            pane.selection.clear();
            pane.recompute_selection_bytes();
            Vec::new()
        };

        let origin_x: f32 = ev.position.x.into();
        let origin_y: f32 = ev.position.y.into();

        self.marquee = Some(MarqueeSelection {
            tab_id,
            side,
            origin_x,
            origin_y,
            origin_content_y: origin_y - bounds_y - scroll_y,
            current_x: origin_x,
            current_y: origin_y,
            active: false,
            additive,
            initial_selection,
        });

        self.sync_preview_for_selection(cx);
        cx.notify();
    }

    /// Tracks rubber-band drag motion and live-updates selected items.
    fn handle_mouse_move(
        &mut self,
        ev: &MouseMoveEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.marquee.is_none() {
            return;
        }
        if !ev.dragging() {
            self.finish_marquee_selection(cx);
            return;
        }

        let curr_x: f32 = ev.position.x.into();
        let curr_y: f32 = ev.position.y.into();

        let (tab_id, side, origin_x, origin_content_y, additive, initial_selection) = {
            let Some(m) = self.marquee.as_mut() else {
                return;
            };
            m.current_x = curr_x;
            m.current_y = curr_y;
            if !m.active {
                if (curr_x - m.origin_x).abs() < 4.0 && (curr_y - m.origin_y).abs() < 4.0 {
                    return;
                }
                m.active = true;
            }
            (
                m.tab_id,
                m.side,
                m.origin_x,
                m.origin_content_y,
                m.additive,
                m.initial_selection.clone(),
            )
        };

        let split_enabled = self.active_tab().is_some_and(|t| t.model.split_enabled);
        let grid_cols = self.grid_view_columns(split_enabled);

        let Some(tab) = self.active_tab_mut() else {
            self.marquee = None;
            return;
        };
        if tab.model.id != tab_id {
            self.marquee = None;
            return;
        }

        let view_mode = tab.model.pane(side).view_mode;
        let total = tab.model.pane(side).filtered_indices.len();

        let (bounds_x, bounds_y, bounds_w, bounds_h, mut scroll_y, measured_row_h) = {
            let state = tab.pane_ui(side).scroll_handle.0.borrow();
            let b = state.base_handle.bounds();
            let bx: f32 = b.origin.x.into();
            let by: f32 = b.origin.y.into();
            let bw: f32 = b.size.width.into();
            let bh: f32 = b.size.height.into();
            let sy: f32 = state.base_handle.offset().y.into();
            let rh: Option<f32> = state.last_item_size.map(|s| s.item.height.into());
            (bx, by, bw, bh, sy, rh)
        };

        let row_h = measured_row_h.filter(|&h| h > 4.0).unwrap_or(match view_mode {
            ViewMode::Details => 32.0,
            ViewMode::Compact => 28.0,
            ViewMode::Grid => 136.0,
        });

        // Smooth edge auto-scroll when dragging above or below the list viewport.
        let total_rows = match view_mode {
            ViewMode::Grid => total.div_ceil(grid_cols.max(1)),
            _ => total,
        };
        let max_scroll = ((total_rows as f32) * row_h - bounds_h).max(0.0);
        if curr_y < bounds_y && scroll_y < 0.0 {
            scroll_y = (scroll_y + 18.0).min(0.0);
            let off = tab.pane_ui(side).scroll_handle.0.borrow().base_handle.offset();
            tab.pane_ui(side)
                .scroll_handle
                .0
                .borrow()
                .base_handle
                .set_offset(Point {
                    x: off.x,
                    y: px(scroll_y),
                });
        } else if curr_y > bounds_y + bounds_h && -scroll_y < max_scroll {
            scroll_y = (scroll_y - 18.0).max(-max_scroll);
            let off = tab.pane_ui(side).scroll_handle.0.borrow().base_handle.offset();
            tab.pane_ui(side)
                .scroll_handle
                .0
                .borrow()
                .base_handle
                .set_offset(Point {
                    x: off.x,
                    y: px(scroll_y),
                });
        }

        let start_local_x = origin_x - bounds_x;
        let curr_local_x = curr_x - bounds_x;
        let curr_content_y = curr_y - bounds_y - scroll_y;

        let rect_left = start_local_x.min(curr_local_x);
        let rect_right = start_local_x.max(curr_local_x);
        let rect_top = origin_content_y.min(curr_content_y);
        let rect_bottom = origin_content_y.max(curr_content_y);

        let hits = compute_marquee_hits(
            view_mode,
            total,
            grid_cols,
            row_h,
            bounds_w,
            rect_left,
            rect_top,
            rect_right,
            rect_bottom,
        );

        let pane = tab.model.pane_mut(side);
        pane.selection.selected.clear();
        if additive {
            for idx in initial_selection {
                if idx < total {
                    pane.selection.selected.insert(idx);
                }
            }
        }
        for idx in &hits {
            pane.selection.selected.insert(*idx);
        }
        pane.selection.anchor = pane.selection.selected.iter().next().copied();
        pane.selection.cursor = hits.last().copied().or(pane.selection.anchor);
        pane.recompute_selection_bytes();
        cx.notify();
    }

    fn handle_mouse_up(
        &mut self,
        _ev: &MouseUpEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.entry_claimed_mouse_down = false;
        self.finish_marquee_selection(cx);
    }

    fn finish_marquee_selection(&mut self, cx: &mut Context<Self>) {
        if self.marquee.take().is_some() {
            self.sync_preview_for_selection(cx);
            cx.notify();
        }
    }

    /// Returns `(left, top, width, height)` in viewport-local pixels when a rubber-band selection
    /// box is actively being dragged inside `side`.
    pub fn active_marquee_rect(&self, side: PaneSide) -> Option<(f32, f32, f32, f32)> {
        let m = self.marquee.as_ref()?;
        if !m.active || m.side != side {
            return None;
        }
        let tab = self.active_tab()?;
        if tab.model.id != m.tab_id {
            return None;
        }
        let state = tab.pane_ui(side).scroll_handle.0.borrow();
        let b = state.base_handle.bounds();
        let bx: f32 = b.origin.x.into();
        let by: f32 = b.origin.y.into();
        let bw: f32 = b.size.width.into();
        let bh: f32 = b.size.height.into();
        if bw <= 0.0 || bh <= 0.0 {
            return None;
        }
        let scroll_y: f32 = state.base_handle.offset().y.into();

        let x1 = m.origin_x - bx;
        let x2 = m.current_x - bx;
        let y1 = m.origin_content_y + scroll_y;
        let y2 = m.current_y - by;

        let left = x1.min(x2).clamp(0.0, bw);
        let right = x1.max(x2).clamp(0.0, bw);
        let top = y1.min(y2).clamp(0.0, bh);
        let bottom = y1.max(y2).clamp(0.0, bh);
        if right <= left || bottom <= top {
            return None;
        }
        Some((left, top, right - left, bottom - top))
    }

    pub fn handle_entry_right_click(
        &mut self,
        side: PaneSide,
        vis_idx: usize,
        ev: &MouseDownEvent,
        cx: &mut Context<Self>,
    ) {
        let entry_opt = {
            let Some(tab) = self.active_tab_mut() else {
                return;
            };
            tab.model.active_pane = side;
            let pane = tab.model.pane_mut(side);
            if !pane.selection.contains(vis_idx) {
                pane.selection.select_only(vis_idx);
                pane.recompute_selection_bytes();
            }
            pane.visible_entry(vis_idx).cloned()
        };

        self.sync_preview_for_selection(cx);

        // Shift + Right Click invokes the native Windows Shell menu directly.
        if ev.modifiers.shift {
            let paths = self.selected_local_paths(side);
            if !paths.is_empty() {
                self.show_native_windows_menu(paths, cx);
                return;
            }
        }

        if let Some(entry) = entry_opt {
            self.open_context_menu(ev.position, ContextMenuTarget::Entry { side, entry }, cx);
        }
    }

    pub fn open_pane_bg_context_menu(
        &mut self,
        side: PaneSide,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let dir_opt = self.active_tab().and_then(|t| {
            t.model
                .pane(side)
                .location
                .as_local_path()
                .map(|p| p.to_path_buf())
        });
        if let Some(directory) = dir_opt {
            self.set_active_pane(side, cx);
            self.open_context_menu(
                position,
                ContextMenuTarget::PaneBackground { side, directory },
                cx,
            );
        }
    }

    pub fn open_context_menu(
        &mut self,
        position: Point<Pixels>,
        target: ContextMenuTarget,
        cx: &mut Context<Self>,
    ) {
        self.context_menu = Some(ContextMenuState { position, target });
        cx.notify();
    }

    pub fn dismiss_context_menu(&mut self, cx: &mut Context<Self>) {
        if self.context_menu.take().is_some() {
            cx.notify();
        }
    }

    pub fn open_location_or_file(
        &mut self,
        location: Location,
        is_dir: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if is_dir || matches!(location, Location::Home) {
            self.navigate_active_pane(location, window, cx);
        } else if let Some(path) = location.as_local_path() {
            if is_supported_archive(path) {
                self.navigate_active_pane(Location::archive(path, ""), window, cx);
            } else {
                // Run ShellExecuteW off the GPUI UI thread so Win32 COM never pumps nested
                // messages while GPUI holds a mutable App borrow.
                let path_owned = path.to_path_buf();
                let tx = self.bg_tx.clone();
                tokio_runtime().spawn(async move {
                    let result = tokio::task::spawn_blocking(move || {
                        open_item(&path_owned).map(|()| None)
                    })
                    .await
                    .unwrap_or_else(|e| Err(format!("Failed opening item: {e}")));
                    let _ = tx.unbounded_send(BgEvent::ShellOpFinished {
                        result,
                        refresh_side: None,
                    });
                });
            }
        } else if matches!(location, Location::Archive { .. }) {
            self.toggle_quicklook(cx);
        }
    }

    // --- Phase 2: Clipboard (CF_HDROP), Drag & Drop, Transfer Queue, Conflicts, Undo/Redo, QuickLook ---

    pub fn copy_or_cut_selected(
        &mut self,
        side: PaneSide,
        is_cut: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let paths = self.selected_local_paths(side);
        if paths.is_empty() {
            return;
        }
        match set_clipboard_files(&paths, is_cut) {
            Ok(()) => {
                self.cut_paths.clear();
                if is_cut {
                    self.cut_paths.extend(paths.iter().cloned());
                }
                let verb = if is_cut { "Cut" } else { "Copied" };
                let msg = if paths.len() == 1 {
                    format!("{verb} 1 item to clipboard")
                } else {
                    format!("{verb} {} items to clipboard", paths.len())
                };
                self.show_toast(msg, ToastKind::Success, window, cx);
            }
            Err(err) => self.show_toast(err, ToastKind::Error, window, cx),
        }
    }

    pub fn paste_clipboard_into(
        &mut self,
        dest_dir: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match get_clipboard_files() {
            Ok(Some(clip)) => {
                let kind = if clip.is_cut {
                    TransferKind::Move
                } else {
                    TransferKind::Copy
                };
                let effective = filter_effective_transfer_sources(kind, clip.paths, &dest_dir, false);
                if effective.is_empty() {
                    if clip.is_cut && !self.cut_paths.is_empty() {
                        self.cut_paths.clear();
                        cx.notify();
                    }
                    return;
                }
                self.enqueue_transfer(kind, effective, dest_dir, cx);
            }
            Ok(None) => {
                self.show_toast("Clipboard does not contain files.", ToastKind::Info, window, cx);
            }
            Err(err) => self.show_toast(err, ToastKind::Error, window, cx),
        }
    }

    pub fn handle_dropped_paths(
        &mut self,
        sources: Vec<PathBuf>,
        dest_dir: PathBuf,
        force_copy: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if sources.is_empty() || !dest_dir.is_dir() {
            return;
        }
        let kind = if force_copy || !same_volume(&sources[0], &dest_dir) {
            TransferKind::Copy
        } else {
            TransferKind::Move
        };
        let effective = filter_effective_transfer_sources(kind, sources, &dest_dir, true);
        if effective.is_empty() {
            return;
        }
        self.enqueue_transfer(kind, effective, dest_dir, cx);
        let _ = window;
    }

    fn enqueue_transfer(
        &mut self,
        kind: TransferKind,
        sources: Vec<PathBuf>,
        dest_dir: PathBuf,
        cx: &mut Context<Self>,
    ) {
        let sources = filter_effective_transfer_sources(kind, sources, &dest_dir, false);
        if sources.is_empty() {
            return;
        }
        let (xfer_tx, mut xfer_rx) = unbounded::<TransferEvent>();
        let (job_id, control) = spawn_transfer_job(kind, sources.clone(), dest_dir.clone(), xfer_tx);

        let title = if sources.len() == 1 {
            let name = sources[0]
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            format!("{} \"{name}\"", kind.verb_present())
        } else {
            format!("{} {} items", kind.verb_present(), sources.len())
        };

        self.transfers.push(TransferJobState {
            snapshot: TransferSnapshot {
                job_id,
                kind,
                title,
                target_dir: dest_dir,
                status: TransferStatus::Queued,
                files_done: 0,
                files_total: sources.len() as u64,
                bytes_done: 0,
                bytes_total: 0,
                speed_bytes_per_sec: 0.0,
                current_item: String::new(),
            },
            control,
        });

        let bg_tx = self.bg_tx.clone();
        tokio_runtime().spawn(async move {
            while let Some(ev) = xfer_rx.next().await {
                if bg_tx.unbounded_send(BgEvent::Transfer(ev)).is_err() {
                    break;
                }
            }
        });
        cx.notify();
    }

    pub fn toggle_pause_transfer(&mut self, job_id: Uuid, cx: &mut Context<Self>) {
        if let Some(job) = self.transfers.iter().find(|j| j.snapshot.job_id == job_id) {
            job.control.toggle_pause();
            cx.notify();
        }
    }

    pub fn cancel_transfer(&mut self, job_id: Uuid, cx: &mut Context<Self>) {
        if let Some(job) = self.transfers.iter().find(|j| j.snapshot.job_id == job_id) {
            job.control.cancel();
            cx.notify();
        }
    }

    pub fn dismiss_transfer(&mut self, job_id: Uuid, cx: &mut Context<Self>) {
        self.transfers.retain(|j| j.snapshot.job_id != job_id);
        cx.notify();
    }

    pub fn clear_finished_transfers(&mut self, cx: &mut Context<Self>) {
        self.transfers.retain(|j| !j.snapshot.status.is_terminal());
        cx.notify();
    }

    pub fn toggle_conflict_apply_all(&mut self, cx: &mut Context<Self>) {
        self.conflict_apply_to_all = !self.conflict_apply_to_all;
        cx.notify();
    }

    pub fn resolve_active_conflict(
        &mut self,
        resolution: ConflictResolution,
        cx: &mut Context<Self>,
    ) {
        if let Some(mut prompt) = self.conflict_queue.pop_front() {
            prompt.respond(ConflictDecision {
                resolution,
                apply_to_all: self.conflict_apply_to_all,
            });
            self.conflict_apply_to_all = false;
            cx.notify();
        }
    }

    pub fn undo_last_action(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.undo_history.undo() {
            Ok(msg) => {
                self.refresh_visible_panes(cx);
                self.show_toast(msg, ToastKind::Info, window, cx);
            }
            Err(err) => self.show_toast(err, ToastKind::Error, window, cx),
        }
    }

    pub fn redo_last_action(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.undo_history.redo() {
            Ok(msg) => {
                self.refresh_visible_panes(cx);
                self.show_toast(msg, ToastKind::Info, window, cx);
            }
            Err(err) => self.show_toast(err, ToastKind::Error, window, cx),
        }
    }

    pub fn show_native_windows_menu(&mut self, paths: Vec<PathBuf>, _cx: &mut Context<Self>) {
        if paths.is_empty() {
            return;
        }
        let (x, y) = cursor_screen_pos();
        let tx = self.bg_tx.clone();
        self.shell_worker
            .show_native_context_menu(paths, x, y, move |res| {
                let _ = tx.unbounded_send(BgEvent::NativeShellMenuDone(res));
            });
    }

    pub fn toggle_quicklook(&mut self, cx: &mut Context<Self>) {
        if self.quicklook_open {
            self.quicklook_open = false;
            cx.notify();
            return;
        }
        let has_selection = self
            .active_tab()
            .and_then(|t| t.model.active_pane_ref().primary_selected_entry())
            .is_some();
        if has_selection {
            self.quicklook_open = true;
            self.context_menu = None;
            self.sync_preview_for_selection(cx);
            cx.notify();
        }
    }

    pub fn close_quicklook(&mut self, cx: &mut Context<Self>) {
        if self.quicklook_open {
            self.quicklook_open = false;
            cx.notify();
        }
    }

    pub fn copy_text_to_clipboard(&mut self, text: &str, notice: &str, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()));
        self.set_toast_simple(notice, ToastKind::Success, cx);
    }

    pub fn open_terminal_at(&mut self, path: &Path, _cx: &mut Context<Self>) {
        let path_owned = path.to_path_buf();
        let cmd = self.settings.terminal_command.clone();
        let tx = self.bg_tx.clone();
        tokio_runtime().spawn(async move {
            let result = tokio::task::spawn_blocking(move || {
                open_terminal_here(&path_owned, &cmd).map(|()| None)
            })
            .await
            .unwrap_or_else(|e| Err(format!("Failed launching terminal: {e}")));
            let _ = tx.unbounded_send(BgEvent::ShellOpFinished {
                result,
                refresh_side: None,
            });
        });
    }

    pub fn open_editor_at(&mut self, path: &Path, _cx: &mut Context<Self>) {
        let path_owned = path.to_path_buf();
        let cmd = self.settings.editor_command.clone();
        let tx = self.bg_tx.clone();
        tokio_runtime().spawn(async move {
            let result = tokio::task::spawn_blocking(move || {
                open_in_editor(&path_owned, &cmd).map(|()| None)
            })
            .await
            .unwrap_or_else(|e| Err(format!("Failed launching editor: {e}")));
            let _ = tx.unbounded_send(BgEvent::ShellOpFinished {
                result,
                refresh_side: None,
            });
        });
    }

    pub fn reveal_in_windows_explorer(&mut self, path: &Path, _cx: &mut Context<Self>) {
        let path_owned = path.to_path_buf();
        let tx = self.bg_tx.clone();
        tokio_runtime().spawn(async move {
            let result = tokio::task::spawn_blocking(move || {
                reveal_in_windows_explorer(&path_owned).map(|()| None)
            })
            .await
            .unwrap_or_else(|e| Err(format!("Failed launching Windows Explorer: {e}")));
            let _ = tx.unbounded_send(BgEvent::ShellOpFinished {
                result,
                refresh_side: None,
            });
        });
    }

    pub fn show_properties_for(&mut self, path: &Path, _cx: &mut Context<Self>) {
        let tx = self.bg_tx.clone();
        self.shell_worker.show_properties(path.to_path_buf(), move |res| {
            let _ = tx.unbounded_send(BgEvent::ShellOpFinished {
                result: res.map(|()| None),
                refresh_side: None,
            });
        });
    }

    pub fn pin_folder(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if self.settings.pin_folder(path) {
            let _ = self.settings.save();
            self.set_toast_simple("Pinned folder to sidebar", ToastKind::Success, cx);
        }
    }

    pub fn unpin_folder(&mut self, path: &Path, cx: &mut Context<Self>) {
        if self.settings.unpin_folder(path) {
            let _ = self.settings.save();
            cx.notify();
        }
    }

    pub fn refresh_volumes(&mut self, cx: &mut Context<Self>) {
        self.volumes = list_volumes();
        self.known_folders = list_known_folders();
        self.wsl_distros = list_wsl_distros();
        cx.notify();
    }

    pub fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        self.settings.show_sidebar = !self.settings.show_sidebar;
        let _ = self.settings.save();
        cx.notify();
    }

    pub fn toggle_inspector(&mut self, cx: &mut Context<Self>) {
        self.settings.show_inspector = !self.settings.show_inspector;
        let _ = self.settings.save();
        if self.settings.show_inspector {
            self.sync_preview_for_selection(cx);
        }
        cx.notify();
    }

    pub fn set_settings_category(&mut self, category: SettingsCategory, cx: &mut Context<Self>) {
        if self.settings_category != category {
            self.settings_category = category;
            cx.notify();
        }
    }

    pub fn open_settings_category(
        &mut self,
        category: SettingsCategory,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.settings_category = category;
        self.open_settings_modal(window, cx);
    }

    pub fn open_settings_modal(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.system_is_dark = system_prefers_dark_mode();
        if self.settings.theme == ThemePreset::FilesSystem {
            apply_theme(self.palette(), cx);
        }
        self.default_explorer_status = query_default_explorer_status();
        self.refresh_service_status(cx);
        self.context_menu = None;
        self.quicklook_open = false;
        self.global_search = None;
        self.global_search_subs.clear();
        self.bulk_rename = None;
        self.bulk_rename_subs.clear();
        self.command_palette = None;
        self.command_palette_subs.clear();
        self.active_modal = Some(ActiveModal::Settings);
        cx.notify();
    }

    // --- Phase 3: Global Search (Ctrl+K), Background Folder Sizes & MFT Service Controls ---

    pub fn open_global_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.context_menu = None;
        self.quicklook_open = false;
        self.active_modal = None;
        self.modal_subs.clear();
        self.bulk_rename = None;
        self.bulk_rename_subs.clear();
        self.command_palette = None;
        self.command_palette_subs.clear();
        self.refresh_service_status(cx);

        let active_folder = self
            .active_tab()
            .and_then(|t| t.model.active_pane_ref().location.as_local_path())
            .map(|p| p.to_path_buf());
        let initial_scope = if self.service_install_state == IndexServiceInstallState::Running
            && self.settings.use_mft_service
        {
            SearchScope::Global
        } else if let Some(ref dir) = active_folder {
            SearchScope::Folder(dir.clone())
        } else {
            SearchScope::Global
        };

        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Search files... (ext:rs;toml, size:>10mb, dir:, !target, /regex/)")
        });
        input.update(cx, |s, cx| s.focus(window, cx));

        let sub = cx.subscribe_in(&input, window, |this, state, ev: &InputEvent, window, cx| {
            match ev {
                InputEvent::Change => {
                    let query = state.read(cx).value().to_string();
                    this.schedule_global_search(query, window, cx);
                }
                InputEvent::PressEnter { secondary, shift } => {
                    if *shift || *secondary {
                        this.open_global_search_in_tab(window, cx);
                    } else {
                        this.open_global_search_selected(window, cx);
                    }
                }
                _ => {}
            }
        });

        self.global_search_subs = vec![sub];
        self.global_search = Some(GlobalSearchState {
            input,
            scroll_handle: UniformListScrollHandle::new(),
            scope: initial_scope,
            active_folder,
            query_text: String::new(),
            results: Vec::new(),
            total_matches: 0,
            elapsed_ms: 0,
            from_mft: false,
            truncated: false,
            searching: false,
            selected_idx: 0,
            generation: 0,
            cancel_flag: Arc::new(AtomicBool::new(false)),
        });
        cx.notify();
    }

    pub fn close_global_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(gs) = self.global_search.take() {
            gs.cancel_flag.store(true, Ordering::Relaxed);
            self.global_search_subs.clear();
            self.focus_handle.focus(window, cx);
            cx.notify();
        }
    }

    pub fn set_global_search_scope(&mut self, scope: SearchScope, cx: &mut Context<Self>) {
        let query = {
            let Some(gs) = self.global_search.as_mut() else {
                return;
            };
            if gs.scope == scope {
                return;
            }
            gs.scope = scope;
            gs.query_text.clone()
        };
        self.dispatch_global_search_now(query, cx);
    }

    fn schedule_global_search(
        &mut self,
        query: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let generation = {
            let Some(gs) = self.global_search.as_mut() else {
                return;
            };
            gs.cancel_flag.store(true, Ordering::Relaxed);
            gs.generation = gs.generation.wrapping_add(1);
            gs.query_text = query.clone();
            if query.trim().is_empty() {
                gs.results.clear();
                gs.total_matches = 0;
                gs.searching = false;
                cx.notify();
                return;
            }
            gs.searching = true;
            gs.generation
        };
        cx.notify();

        let delay_ms = if self.service_install_state == IndexServiceInstallState::Running
            && self.settings.use_mft_service
        {
            35
        } else {
            140
        };
        cx.spawn_in(window, async move |weak, cx| {
            smol_sleep(Duration::from_millis(delay_ms)).await;
            let _ = weak.update_in(cx, |this, _window, cx| {
                let matches_gen = this
                    .global_search
                    .as_ref()
                    .is_some_and(|gs| gs.generation == generation);
                if matches_gen {
                    this.dispatch_global_search_now(query, cx);
                }
            });
        })
        .detach();
    }

    fn dispatch_global_search_now(&mut self, query: String, cx: &mut Context<Self>) {
        let (scope, generation, cancel) = {
            let Some(gs) = self.global_search.as_mut() else {
                return;
            };
            gs.cancel_flag.store(true, Ordering::Relaxed);
            if query.trim().is_empty() {
                gs.results.clear();
                gs.total_matches = 0;
                gs.searching = false;
                cx.notify();
                return;
            }
            gs.generation = gs.generation.wrapping_add(1);
            let cancel = Arc::new(AtomicBool::new(false));
            gs.cancel_flag = Arc::clone(&cancel);
            gs.searching = true;
            (gs.scope.clone(), gs.generation, cancel)
        };
        let use_mft = self.settings.use_mft_service;
        let tx = self.bg_tx.clone();
        tokio_runtime().spawn(async move {
            let outcome = tokio::task::spawn_blocking(move || {
                execute_search(&query, &scope, use_mft, 400, &cancel)
            })
            .await
            .unwrap_or_else(|e| Err(format!("Search worker failed: {e}")));
            let _ = tx.unbounded_send(BgEvent::GlobalSearchReady {
                generation,
                outcome,
            });
        });
        cx.notify();
    }

    pub fn move_global_search_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(gs) = self.global_search.as_mut() else {
            return;
        };
        let total = gs.results.len();
        if total == 0 {
            return;
        }
        let next = gs
            .selected_idx
            .saturating_add_signed(delta)
            .min(total.saturating_sub(1));
        if next != gs.selected_idx {
            gs.selected_idx = next;
            gs.scroll_handle
                .scroll_to_item(next, ScrollStrategy::Nearest);
            cx.notify();
        }
    }

    pub fn open_global_search_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let target = self.global_search.as_ref().and_then(|gs| {
            let entry = gs.results.get(gs.selected_idx)?;
            Some((entry.location.clone(), entry.is_dir()))
        });
        if let Some((loc, is_dir)) = target {
            self.close_global_search(window, cx);
            self.open_location_or_file(loc, is_dir, window, cx);
        }
    }

    pub fn open_global_search_in_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let req = self.global_search.as_ref().and_then(|gs| {
            let q = gs.query_text.trim();
            if q.is_empty() {
                None
            } else {
                Some(Location::Search {
                    query: q.to_string(),
                    scope: gs.scope.clone(),
                })
            }
        });
        if let Some(loc) = req {
            self.close_global_search(window, cx);
            if self.settings.show_tabs {
                self.open_new_tab(loc, window, cx);
            } else {
                self.navigate_active_pane(loc, window, cx);
            }
        }
    }

    pub fn reveal_search_hit_in_pane(
        &mut self,
        path: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let parent = path
            .parent()
            .map(Location::local)
            .unwrap_or(Location::Home);
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned());
        self.close_global_search(window, cx);
        if let Some(tab) = self.active_tab_mut() {
            tab.model.active_pane_mut().pending_select_name = name;
        }
        self.navigate_active_pane(parent, window, cx);
    }

    pub fn refresh_service_status(&mut self, _cx: &mut Context<Self>) {
        let tx = self.bg_tx.clone();
        tokio_runtime().spawn(async move {
            let (install_state, live_status) = tokio::task::spawn_blocking(|| {
                let install = query_service_install_state();
                let live = if install == IndexServiceInstallState::Running {
                    query_live_service_status()
                } else {
                    None
                };
                (install, live)
            })
            .await
            .unwrap_or((IndexServiceInstallState::Unavailable, None));
            let _ = tx.unbounded_send(BgEvent::ServiceStatusReady {
                install_state,
                live_status,
            });
        });
    }

    fn run_service_action<F>(&mut self, action: F, cx: &mut Context<Self>)
    where
        F: FnOnce() -> Result<String, String> + Send + 'static,
    {
        if self.service_busy {
            return;
        }
        self.service_busy = true;
        cx.notify();
        let tx = self.bg_tx.clone();
        tokio_runtime().spawn(async move {
            let (result, install_state, live_status) = tokio::task::spawn_blocking(move || {
                let res = action();
                let install = query_service_install_state();
                let live = if install == IndexServiceInstallState::Running {
                    query_live_service_status()
                } else {
                    None
                };
                (res, install, live)
            })
            .await
            .unwrap_or_else(|e| {
                (
                    Err(format!("Service worker failed: {e}")),
                    query_service_install_state(),
                    None,
                )
            });
            let _ = tx.unbounded_send(BgEvent::ServiceActionFinished {
                result,
                install_state,
                live_status,
            });
        });
    }

    pub fn install_mft_service(&mut self, cx: &mut Context<Self>) {
        self.run_service_action(
            || {
                install_service_elevated()?;
                Ok("Installed and started TExplorer NTFS MFT Index Service".to_string())
            },
            cx,
        );
    }

    pub fn start_mft_service(&mut self, cx: &mut Context<Self>) {
        self.run_service_action(
            || {
                start_service_elevated()?;
                Ok("Started TExplorer NTFS MFT Index Service".to_string())
            },
            cx,
        );
    }

    pub fn uninstall_mft_service(&mut self, cx: &mut Context<Self>) {
        self.run_service_action(
            || {
                uninstall_service_elevated()?;
                Ok("Uninstalled TExplorer NTFS MFT Index Service".to_string())
            },
            cx,
        );
    }

    pub fn rebuild_mft_index(&mut self, cx: &mut Context<Self>) {
        self.folder_size_cache.clear();
        self.run_service_action(
            || {
                rebuild_index_via_service()?;
                Ok("Rebuilding NTFS $MFT index in background...".to_string())
            },
            cx,
        );
    }

    pub fn toggle_use_mft_service(&mut self, cx: &mut Context<Self>) {
        self.settings.use_mft_service = !self.settings.use_mft_service;
        let _ = self.settings.save();
        cx.notify();
    }

    pub fn toggle_calculate_folder_sizes(&mut self, cx: &mut Context<Self>) {
        self.settings.calculate_folder_sizes = !self.settings.calculate_folder_sizes;
        let _ = self.settings.save();
        if self.settings.calculate_folder_sizes {
            self.refresh_visible_panes(cx);
        }
        cx.notify();
    }

    // --- Phase 4: Bulk Rename, Command Palette, Checksums, File Locks & Transparent Archives ---

    pub fn open_bulk_rename_for_pane(
        &mut self,
        side: PaneSide,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut sources = self.selected_local_paths(side);
        if sources.is_empty()
            && let Some(tab) = self.active_tab()
        {
            let pane = tab.model.pane(side);
            sources = pane
                .filtered_indices
                .iter()
                .filter_map(|&idx| {
                    pane.entries
                        .get(idx)?
                        .location
                        .as_local_path()
                        .map(|p| p.to_path_buf())
                })
                .collect();
        }
        if sources.is_empty() {
            self.show_toast(
                "Select local files or folders to bulk rename.",
                ToastKind::Info,
                window,
                cx,
            );
            return;
        }

        self.context_menu = None;
        self.quicklook_open = false;
        self.active_modal = None;
        self.modal_subs.clear();
        self.global_search = None;
        self.global_search_subs.clear();
        self.command_palette = None;
        self.command_palette_subs.clear();

        let find_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Text or regex pattern..."));
        let replace_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Replacement ($1, $2 for regex)...")
        });
        let template_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Optional: e.g. photo_{n:03}_{name}")
        });
        let ext_input = cx.new(|cx| InputState::new(window, cx).placeholder("Keep original"));
        find_input.update(cx, |s, cx| s.focus(window, cx));

        let mut subs = Vec::with_capacity(4);
        for input in [&find_input, &replace_input, &template_input, &ext_input] {
            let sub = cx.subscribe_in(input, window, |this, _, ev: &InputEvent, window, cx| {
                match ev {
                    InputEvent::Change => this.sync_bulk_rename_inputs(cx),
                    InputEvent::PressEnter { .. } => this.apply_bulk_rename(window, cx),
                    _ => {}
                }
            });
            subs.push(sub);
        }

        let spec = BulkRenameSpec::default();
        let preview_rows = preview_bulk_rename(&sources, &spec).unwrap_or_default();

        self.bulk_rename_subs = subs;
        self.bulk_rename = Some(BulkRenameState {
            side,
            sources,
            find_input,
            replace_input,
            template_input,
            ext_input,
            spec,
            preview_rows,
            regex_error: None,
            scroll_handle: UniformListScrollHandle::new(),
        });
        cx.notify();
    }

    pub fn close_bulk_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.bulk_rename.take().is_some() {
            self.bulk_rename_subs.clear();
            self.focus_handle.focus(window, cx);
            cx.notify();
        }
    }

    fn sync_bulk_rename_inputs(&mut self, cx: &mut Context<Self>) {
        let Some((find_e, repl_e, tmpl_e, ext_e)) = self.bulk_rename.as_ref().map(|br| {
            (
                br.find_input.clone(),
                br.replace_input.clone(),
                br.template_input.clone(),
                br.ext_input.clone(),
            )
        }) else {
            return;
        };
        let find = find_e.read(cx).value().to_string();
        let replace = repl_e.read(cx).value().to_string();
        let template = tmpl_e.read(cx).value().to_string();
        let ext = ext_e.read(cx).value().to_string();

        if let Some(br) = self.bulk_rename.as_mut() {
            br.spec.find = find;
            br.spec.replace = replace;
            br.spec.template = template;
            br.spec.extension_override = ext;
        }
        self.recompute_bulk_rename_preview(cx);
    }

    fn recompute_bulk_rename_preview(&mut self, cx: &mut Context<Self>) {
        if let Some(br) = self.bulk_rename.as_mut() {
            match preview_bulk_rename(&br.sources, &br.spec) {
                Ok(rows) => {
                    br.preview_rows = rows;
                    br.regex_error = None;
                }
                Err(err) => {
                    br.regex_error = Some(err);
                }
            }
            cx.notify();
        }
    }

    pub fn toggle_bulk_rename_regex(&mut self, cx: &mut Context<Self>) {
        if let Some(br) = self.bulk_rename.as_mut() {
            br.spec.use_regex = !br.spec.use_regex;
        }
        self.recompute_bulk_rename_preview(cx);
    }

    pub fn toggle_bulk_rename_case(&mut self, cx: &mut Context<Self>) {
        if let Some(br) = self.bulk_rename.as_mut() {
            br.spec.case_sensitive = !br.spec.case_sensitive;
        }
        self.recompute_bulk_rename_preview(cx);
    }

    pub fn set_bulk_rename_casing(&mut self, casing: RenameCasing, cx: &mut Context<Self>) {
        if let Some(br) = self.bulk_rename.as_mut() {
            br.spec.casing = casing;
        }
        self.recompute_bulk_rename_preview(cx);
    }

    pub fn apply_bulk_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((side, rows, has_regex_err)) = self
            .bulk_rename
            .as_ref()
            .map(|br| (br.side, br.preview_rows.clone(), br.regex_error.is_some()))
        else {
            return;
        };
        if has_regex_err {
            return;
        }
        match execute_bulk_rename(&rows) {
            Ok(undo_action) => {
                let renamed_count = match &undo_action {
                    UndoAction::BulkRename { pairs } => pairs.len(),
                    _ => 0,
                };
                self.undo_history.push(undo_action);
                self.close_bulk_rename(window, cx);
                self.refresh_pane(side, window, cx);
                self.show_toast(
                    format!("Renamed {renamed_count} item(s) (Ctrl+Z to undo)"),
                    ToastKind::Success,
                    window,
                    cx,
                );
            }
            Err(err) => {
                self.show_toast(err, ToastKind::Error, window, cx);
            }
        }
    }

    pub fn open_command_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.context_menu = None;
        self.quicklook_open = false;
        self.active_modal = None;
        self.modal_subs.clear();
        self.global_search = None;
        self.global_search_subs.clear();
        self.bulk_rename = None;
        self.bulk_rename_subs.clear();

        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Type a command... (search, rename, hash, lock, zip, theme, split)")
        });
        input.update(cx, |s, cx| s.focus(window, cx));

        let sub = cx.subscribe_in(&input, window, |this, state, ev: &InputEvent, window, cx| {
            match ev {
                InputEvent::Change => {
                    let q = state.read(cx).value().to_string();
                    if let Some(cp) = this.command_palette.as_mut() {
                        cp.query = q;
                        cp.selected_idx = 0;
                        cx.notify();
                    }
                }
                InputEvent::PressEnter { .. } => {
                    this.execute_selected_palette_command(window, cx);
                }
                _ => {}
            }
        });

        self.command_palette_subs = vec![sub];
        self.command_palette = Some(CommandPaletteState {
            input,
            query: String::new(),
            selected_idx: 0,
        });
        cx.notify();
    }

    pub fn close_command_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.command_palette.take().is_some() {
            self.command_palette_subs.clear();
            self.focus_handle.focus(window, cx);
            cx.notify();
        }
    }

    pub fn move_command_palette_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(cp) = self.command_palette.as_mut() else {
            return;
        };
        let total = filtered_palette_items(&cp.query).len();
        if total == 0 {
            return;
        }
        let next = cp
            .selected_idx
            .saturating_add_signed(delta)
            .min(total.saturating_sub(1));
        if next != cp.selected_idx {
            cp.selected_idx = next;
            cx.notify();
        }
    }

    fn execute_selected_palette_command(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let cmd_opt = self.command_palette.as_ref().and_then(|cp| {
            let items = filtered_palette_items(&cp.query);
            let idx = cp.selected_idx.min(items.len().saturating_sub(1));
            items.get(idx).map(|i| i.cmd)
        });
        self.close_command_palette(window, cx);
        if let Some(cmd) = cmd_opt {
            execute_palette_command(self, cmd, window, cx);
        }
    }

    pub fn compute_checksums_for_selection(&mut self, cx: &mut Context<Self>) {
        let file_path = self.active_tab().and_then(|t| {
            let entry = t.model.active_pane_ref().primary_selected_entry()?;
            if entry.is_dir() {
                return None;
            }
            entry.location.as_local_path().map(|p| p.to_path_buf())
        });
        let Some(path) = file_path else {
            self.set_toast_simple(
                "Select a local file to compute SHA-256 & MD5 checksums.",
                ToastKind::Info,
                cx,
            );
            return;
        };

        if !self.settings.show_inspector {
            self.settings.show_inspector = true;
            let _ = self.settings.save();
            self.sync_preview_for_selection(cx);
        }

        self.hash_cancel.store(true, Ordering::Relaxed);
        let cancel = Arc::new(AtomicBool::new(false));
        self.hash_cancel = Arc::clone(&cancel);
        self.hash_generation = self.hash_generation.wrapping_add(1);
        let generation = self.hash_generation;
        self.hash_target = Some(path.clone());
        self.hash_state = HashInspectState::Computing;

        let tx = self.bg_tx.clone();
        tokio_runtime().spawn(async move {
            let p_clone = path.clone();
            let result =
                tokio::task::spawn_blocking(move || compute_file_hashes(&p_clone, &cancel))
                    .await
                    .unwrap_or_else(|e| Err(format!("Checksum worker panicked: {e}")));
            let _ = tx.unbounded_send(BgEvent::HashesReady {
                generation,
                path,
                result,
            });
        });
        cx.notify();
    }

    pub fn check_locks_for_selection(&mut self, cx: &mut Context<Self>) {
        let file_path = self.active_tab().and_then(|t| {
            let entry = t.model.active_pane_ref().primary_selected_entry()?;
            if entry.is_dir() {
                return None;
            }
            entry.location.as_local_path().map(|p| p.to_path_buf())
        });
        let Some(path) = file_path else {
            self.set_toast_simple(
                "Select a local file to inspect Windows file locks.",
                ToastKind::Info,
                cx,
            );
            return;
        };

        if !self.settings.show_inspector {
            self.settings.show_inspector = true;
            let _ = self.settings.save();
            self.sync_preview_for_selection(cx);
        }

        self.lock_generation = self.lock_generation.wrapping_add(1);
        let generation = self.lock_generation;
        self.lock_target = Some(path.clone());
        self.lock_status = FileLockStatus::Checking;

        let tx = self.bg_tx.clone();
        tokio_runtime().spawn(async move {
            let p_clone = path.clone();
            let status = tokio::task::spawn_blocking(move || inspect_file_lock(&p_clone))
                .await
                .unwrap_or_else(|e| FileLockStatus::Error(format!("Lock probe failed: {e}")));
            let _ = tx.unbounded_send(BgEvent::LockChecked {
                generation,
                path,
                status,
            });
        });
        cx.notify();
    }

    pub fn selected_archive_inner_paths(&self, side: PaneSide, archive_path: &Path) -> Vec<String> {
        let Some(tab) = self.active_tab() else {
            return Vec::new();
        };
        tab.model
            .pane(side)
            .selected_entries()
            .into_iter()
            .filter_map(|e| match &e.location {
                Location::Archive {
                    archive_path: ap,
                    inner_path,
                } if ap == archive_path => Some(inner_path.clone()),
                _ => None,
            })
            .collect()
    }

    pub fn compress_selected_to_zip(
        &mut self,
        side: PaneSide,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let sources = self.selected_local_paths(side);
        if sources.is_empty() {
            self.show_toast(
                "Select local files or folders to compress.",
                ToastKind::Info,
                window,
                cx,
            );
            return;
        }
        let Some(dest_dir) = sources[0].parent().map(|p| p.to_path_buf()) else {
            return;
        };
        let base_name = if sources.len() == 1 {
            let stem = sources[0]
                .file_stem()
                .or_else(|| sources[0].file_name())
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Archive".to_string());
            format!("{stem}.zip")
        } else {
            "Archive.zip".to_string()
        };
        let output_zip = unique_keep_both_path(&dest_dir.join(base_name));
        let zip_label = output_zip
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Archive.zip".to_string());

        self.show_toast(
            format!("Compressing to \"{zip_label}\"..."),
            ToastKind::Info,
            window,
            cx,
        );

        let tx = self.bg_tx.clone();
        tokio_runtime().spawn(async move {
            let result = tokio::task::spawn_blocking(move || {
                let count = compress_to_zip(&sources, &output_zip)?;
                Ok((format!("Created \"{zip_label}\" ({count} file(s))"), dest_dir))
            })
            .await
            .unwrap_or_else(|e| Err(format!("Compression task failed: {e}")));
            let _ = tx.unbounded_send(BgEvent::ArchiveOpFinished { result });
        });
    }

    pub fn extract_selected_archive_here(
        &mut self,
        side: PaneSide,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let archive_opt = self.active_tab().and_then(|t| {
            let pane = t.model.pane(side);
            if let Some(entry) = pane.primary_selected_entry()
                && !entry.is_dir()
                && let Some(local) = entry.location.as_local_path()
                && is_supported_archive(local)
            {
                return Some(local.to_path_buf());
            }
            if let Location::Archive { archive_path, .. } = &pane.location {
                return Some(archive_path.clone());
            }
            None
        });
        if let Some(archive_path) = archive_opt {
            self.extract_archive_here(archive_path, window, cx);
        } else {
            self.show_toast(
                "Select a .zip, .7z, .tar, or .tar.gz archive to extract.",
                ToastKind::Info,
                window,
                cx,
            );
        }
    }

    pub fn extract_archive_here(
        &mut self,
        archive_path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(parent) = archive_path.parent() else {
            return;
        };
        let dest_dir = parent.join(archive_stem_name(&archive_path));
        self.extract_archive_entries_into(archive_path, Vec::new(), dest_dir, window, cx);
    }

    pub fn extract_archive_item_to_target(
        &mut self,
        archive_path: PathBuf,
        inner_paths: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // When Dual-Pane is open and the opposite pane points to a local folder, extract straight into it;
        // otherwise extract into a clean sibling folder next to the archive.
        let dest_dir = self
            .active_tab()
            .filter(|t| t.model.split_enabled)
            .and_then(|t| {
                let opp = t.model.active_pane.opposite();
                t.model
                    .pane(opp)
                    .location
                    .as_local_path()
                    .map(|p| p.to_path_buf())
            })
            .unwrap_or_else(|| {
                archive_path
                    .parent()
                    .unwrap_or_else(|| Path::new("."))
                    .join(archive_stem_name(&archive_path))
            });

        self.extract_archive_entries_into(archive_path, inner_paths, dest_dir, window, cx);
    }

    pub fn extract_archive_entries_into(
        &mut self,
        archive_path: PathBuf,
        inner_paths: Vec<String>,
        dest_dir: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let dest_label = dest_dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| dest_dir.display().to_string());
        self.show_toast(
            format!("Extracting to \"{dest_label}\"..."),
            ToastKind::Info,
            window,
            cx,
        );

        let tx = self.bg_tx.clone();
        tokio_runtime().spawn(async move {
            let result = tokio::task::spawn_blocking(move || {
                let count = if inner_paths.is_empty() {
                    extract_archive_all(&archive_path, &dest_dir)?
                } else {
                    extract_archive_entries(&archive_path, &inner_paths, &dest_dir)?
                };
                Ok((
                    format!("Extracted {count} file(s) into \"{dest_label}\""),
                    dest_dir,
                ))
            })
            .await
            .unwrap_or_else(|e| Err(format!("Extraction task failed: {e}")));
            let _ = tx.unbounded_send(BgEvent::ArchiveOpFinished { result });
        });
    }

    pub fn open_new_folder_prompt(
        &mut self,
        side: PaneSide,
        parent: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_text_prompt(
            "New Folder",
            "Create",
            "New folder",
            TextPromptKind::NewFolder { side, parent },
            window,
            cx,
        );
    }

    pub fn open_new_file_prompt(
        &mut self,
        side: PaneSide,
        parent: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_text_prompt(
            "New File",
            "Create",
            "untitled.txt",
            TextPromptKind::NewFile { side, parent },
            window,
            cx,
        );
    }

    pub fn open_rename_prompt(
        &mut self,
        side: PaneSide,
        target: PathBuf,
        current_name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_text_prompt(
            "Rename Item",
            "Rename",
            &current_name,
            TextPromptKind::Rename { side, target },
            window,
            cx,
        );
    }

    fn open_text_prompt(
        &mut self,
        title: &'static str,
        confirm_label: &'static str,
        initial: &str,
        kind: TextPromptKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let initial_owned = initial.to_string();
        let input = cx.new(|cx| {
            let mut state = InputState::new(window, cx);
            state.set_value(initial_owned, window, cx);
            state
        });
        input.update(cx, |s, cx| s.focus(window, cx));
        let sub = cx.subscribe_in(&input, window, |this, _, ev: &InputEvent, window, cx| {
            if matches!(ev, InputEvent::PressEnter { .. }) {
                this.submit_text_prompt(window, cx);
            }
        });
        self.modal_subs = vec![sub];
        self.active_modal = Some(ActiveModal::TextPrompt {
            title,
            confirm_label,
            kind,
            input,
        });
        cx.notify();
    }

    pub fn submit_text_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ActiveModal::TextPrompt { kind, input, .. }) = self.active_modal.clone() else {
            return;
        };
        let value = input.read(cx).value().to_string();
        let outcome = match kind {
            TextPromptKind::NewFolder { side, parent } => {
                texplorer_vfs::create_folder(&parent, &value)
                    .map(|p| (side, p.clone(), UndoAction::CreateDir { path: p }))
            }
            TextPromptKind::NewFile { side, parent } => {
                texplorer_vfs::create_file(&parent, &value)
                    .map(|p| (side, p.clone(), UndoAction::CreateFile { path: p }))
            }
            TextPromptKind::Rename { side, target } => {
                texplorer_vfs::rename_item(&target, &value).map(|new_p| {
                    (
                        side,
                        new_p.clone(),
                        UndoAction::Rename {
                            from: target,
                            to: new_p,
                        },
                    )
                })
            }
        };

        match outcome {
            Ok((side, created_path, undo_action)) => {
                self.undo_history.push(undo_action);
                let name = created_path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned());
                if let Some(tab) = self.active_tab_mut() {
                    tab.model.pane_mut(side).pending_select_name = name;
                }
                self.close_modal(cx);
                self.refresh_pane(side, window, cx);
                self.focus_handle.focus(window, cx);
            }
            Err(err) => {
                self.show_toast(err, ToastKind::Error, window, cx);
            }
        }
    }

    pub fn trash_selected(&mut self, side: PaneSide, _window: &mut Window, _cx: &mut Context<Self>) {
        let paths = self.selected_local_paths(side);
        if paths.is_empty() {
            return;
        }
        let count = paths.len();
        let tx = self.bg_tx.clone();
        // Run COM IFileOperation (Recycle Bin) off the GPUI UI thread to prevent Win32 message re-entrancy.
        tokio_runtime().spawn(async move {
            let result = tokio::task::spawn_blocking(move || {
                move_to_trash(&paths).map(|()| {
                    let msg = if count == 1 {
                        "Moved item to Recycle Bin".to_string()
                    } else {
                        format!("Moved {count} items to Recycle Bin")
                    };
                    Some(msg)
                })
            })
            .await
            .unwrap_or_else(|e| Err(format!("Recycle Bin worker failed: {e}")));
            let _ = tx.unbounded_send(BgEvent::ShellOpFinished {
                result,
                refresh_side: Some(side),
            });
        });
    }

    pub fn prompt_permanent_delete(&mut self, side: PaneSide, cx: &mut Context<Self>) {
        let paths = self.selected_local_paths(side);
        if paths.is_empty() {
            return;
        }
        self.active_modal = Some(ActiveModal::ConfirmPermanentDelete { side, paths });
        cx.notify();
    }

    pub fn confirm_permanent_delete(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(ActiveModal::ConfirmPermanentDelete { side, paths }) = self.active_modal.take()
        else {
            return;
        };
        cx.notify();
        let total = paths.len();
        let tx = self.bg_tx.clone();
        tokio_runtime().spawn(async move {
            let result = tokio::task::spawn_blocking(move || {
                let mut first_err: Option<String> = None;
                for path in &paths {
                    if let Err(err) = delete_permanently(path)
                        && first_err.is_none()
                    {
                        first_err = Some(err);
                    }
                }
                match first_err {
                    Some(err) => Err(err),
                    None => Ok(Some(format!("Permanently deleted {total} item(s)"))),
                }
            })
            .await
            .unwrap_or_else(|e| Err(format!("Delete worker failed: {e}")));
            let _ = tx.unbounded_send(BgEvent::ShellOpFinished {
                result,
                refresh_side: Some(side),
            });
        });
    }

    pub fn selected_local_paths(&self, side: PaneSide) -> Vec<PathBuf> {
        let Some(tab) = self.active_tab() else {
            return Vec::new();
        };
        tab.model
            .pane(side)
            .selected_entries()
            .into_iter()
            .filter_map(|e| e.location.as_local_path().map(|p| p.to_path_buf()))
            .collect()
    }

    pub fn close_modal(&mut self, cx: &mut Context<Self>) {
        self.active_modal = None;
        self.modal_subs.clear();
        cx.notify();
    }

    pub fn set_theme(&mut self, preset: ThemePreset, cx: &mut Context<Self>) {
        self.system_is_dark = system_prefers_dark_mode();
        self.settings.theme = preset;
        let _ = self.settings.save();
        apply_theme(self.palette(), cx);
        if self.settings.show_inspector || self.quicklook_open {
            self.preview_location = None;
            self.sync_preview_for_selection(cx);
        }
        cx.notify();
    }

    pub fn set_accent(&mut self, preset: AccentPreset, cx: &mut Context<Self>) {
        self.settings.accent = preset;
        let _ = self.settings.save();
        apply_theme(self.palette(), cx);
        cx.notify();
    }

    pub fn set_size_unit(&mut self, unit: SizeUnit, cx: &mut Context<Self>) {
        self.settings.size_unit = unit;
        let _ = self.settings.save();
        cx.notify();
    }

    pub fn set_default_view_mode(&mut self, mode: ViewMode, cx: &mut Context<Self>) {
        self.settings.default_view_mode = mode;
        let _ = self.settings.save();
        cx.notify();
    }

    pub fn enable_default_file_manager(&mut self, cx: &mut Context<Self>) {
        match enable_default_explorer(self.settings.intercept_win_e) {
            Ok(()) => {
                self.settings.replace_default_explorer = true;
                let _ = self.settings.save();
                self.default_explorer_status = query_default_explorer_status();
                self.set_toast_simple(
                    "TExplorer is now your default Windows file manager",
                    ToastKind::Success,
                    cx,
                );
            }
            Err(err) => self.set_toast_simple(err, ToastKind::Error, cx),
        }
    }

    pub fn restore_default_file_manager(&mut self, cx: &mut Context<Self>) {
        match restore_windows_explorer() {
            Ok(()) => {
                self.settings.replace_default_explorer = false;
                let _ = self.settings.save();
                self.default_explorer_status = query_default_explorer_status();
                self.set_toast_simple("Restored original Windows Explorer", ToastKind::Info, cx);
            }
            Err(err) => self.set_toast_simple(err, ToastKind::Error, cx),
        }
    }

    pub fn toggle_intercept_win_e(&mut self, cx: &mut Context<Self>) {
        let next = !self.settings.intercept_win_e;
        match set_win_e_interception(next) {
            Ok(()) => {
                self.settings.intercept_win_e = next;
                let _ = self.settings.save();
                self.default_explorer_status = query_default_explorer_status();
                self.set_toast_simple(
                    if next {
                        "Win+E shortcut now opens TExplorer"
                    } else {
                        "Restored Windows Win+E shortcut"
                    },
                    if next {
                        ToastKind::Success
                    } else {
                        ToastKind::Info
                    },
                    cx,
                );
            }
            Err(err) => self.set_toast_simple(err, ToastKind::Error, cx),
        }
    }

    pub fn toggle_show_tabs(&mut self, cx: &mut Context<Self>) {
        self.settings.show_tabs = !self.settings.show_tabs;
        let _ = self.settings.save();
        cx.notify();
    }

    pub fn toggle_show_brand_title(&mut self, cx: &mut Context<Self>) {
        self.settings.show_brand_title = !self.settings.show_brand_title;
        let _ = self.settings.save();
        cx.notify();
    }

    pub fn toggle_auto_check_updates(&mut self, cx: &mut Context<Self>) {
        self.settings.auto_check_updates = !self.settings.auto_check_updates;
        let _ = self.settings.save();
        cx.notify();
    }

    pub fn hide_quick_access(&mut self, kind: KnownFolderKind, cx: &mut Context<Self>) {
        if self.settings.hide_quick_access(kind) {
            let _ = self.settings.save();
            self.set_toast_simple(
                "Removed from Quick Access (restore anytime in Settings)",
                ToastKind::Info,
                cx,
            );
        }
    }

    pub fn toggle_quick_access(&mut self, kind: KnownFolderKind, cx: &mut Context<Self>) {
        self.settings.toggle_quick_access(kind);
        let _ = self.settings.save();
        cx.notify();
    }

    pub fn move_quick_access(
        &mut self,
        kind: KnownFolderKind,
        delta: isize,
        cx: &mut Context<Self>,
    ) {
        if self
            .settings
            .move_quick_access(kind, delta, &self.known_folders)
        {
            let _ = self.settings.save();
            cx.notify();
        }
    }

    pub fn reset_quick_access(&mut self, cx: &mut Context<Self>) {
        self.settings.reset_quick_access();
        let _ = self.settings.save();
        cx.notify();
    }

    pub fn toggle_sidebar_section_collapsed(
        &mut self,
        section: SidebarSection,
        cx: &mut Context<Self>,
    ) {
        self.settings.toggle_sidebar_section_collapsed(section);
        let _ = self.settings.save();
        cx.notify();
    }

    pub fn toggle_sidebar_section_visible(
        &mut self,
        section: SidebarSection,
        cx: &mut Context<Self>,
    ) {
        self.settings.toggle_sidebar_section_visible(section);
        let _ = self.settings.save();
        cx.notify();
    }

    pub fn move_sidebar_section(
        &mut self,
        section: SidebarSection,
        delta: isize,
        cx: &mut Context<Self>,
    ) {
        if self.settings.move_sidebar_section(section, delta) {
            let _ = self.settings.save();
            cx.notify();
        }
    }

    pub fn reset_sidebar_sections(&mut self, cx: &mut Context<Self>) {
        self.settings.reset_sidebar_sections();
        let _ = self.settings.save();
        cx.notify();
    }

    pub fn check_for_updates(&mut self, manual: bool, cx: &mut Context<Self>) {
        if self.update_state.is_busy() {
            return;
        }
        if manual {
            self.update_state = UpdateState::Checking;
            cx.notify();
        }
        let tx = self.bg_tx.clone();
        tokio_runtime().spawn(async move {
            let result = updater::check_for_updates().await;
            let _ = tx.unbounded_send(BgEvent::UpdateCheckFinished { manual, result });
        });
    }

    pub fn start_update_install(&mut self, cx: &mut Context<Self>) {
        if self.update_state.is_busy() {
            return;
        }
        let UpdateState::Available(info) = self.update_state.clone() else {
            return;
        };
        if !info.supports_in_app_install() {
            if let Err(err) = open_url_in_browser(&info.release_url) {
                self.update_state = UpdateState::Failed(err);
                cx.notify();
            }
            return;
        }

        self.update_state = UpdateState::Downloading {
            info: info.clone(),
            downloaded_bytes: 0,
            total_bytes: None,
        };
        cx.notify();

        let tx = self.bg_tx.clone();
        tokio_runtime().spawn(async move {
            let progress_tx = tx.clone();
            let result = updater::download_and_stage_update(&info, move |downloaded, total| {
                let _ = progress_tx.unbounded_send(BgEvent::UpdateDownloadProgress {
                    downloaded_bytes: downloaded,
                    total_bytes: total,
                });
            })
            .await;
            let _ = tx.unbounded_send(BgEvent::UpdateDownloadFinished { info, result });
        });
    }

    pub fn open_releases_page(&mut self, cx: &mut Context<Self>) {
        let url = format!("{REPO_URL}/releases");
        if let Err(err) = open_url_in_browser(&url) {
            self.set_toast_simple(err, ToastKind::Error, cx);
        }
    }

    fn set_toast_simple(
        &mut self,
        message: impl Into<SharedString>,
        kind: ToastKind,
        cx: &mut Context<Self>,
    ) {
        self.toast_seq = self.toast_seq.wrapping_add(1);
        let id = self.toast_seq;
        self.toast = Some(ToastBanner {
            message: message.into(),
            kind,
            id,
        });
        cx.notify();
    }

    fn show_toast(
        &mut self,
        message: impl Into<SharedString>,
        kind: ToastKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toast_seq = self.toast_seq.wrapping_add(1);
        let id = self.toast_seq;
        self.toast = Some(ToastBanner {
            message: message.into(),
            kind,
            id,
        });
        cx.spawn_in(window, async move |weak, cx| {
            smol_sleep(Duration::from_millis(3200)).await;
            let _ = weak.update_in(cx, |this, _window, cx| {
                if this.toast.as_ref().is_some_and(|t| t.id == id) {
                    this.toast = None;
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    pub fn is_modal_blocking(&self) -> bool {
        self.active_modal.is_some()
            || self.global_search.is_some()
            || self.bulk_rename.is_some()
            || self.command_palette.is_some()
            || !self.conflict_queue.is_empty()
    }

    fn handle_key_down(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_modal_blocking() || !self.focus_handle.is_focused(window) {
            return;
        }
        let key = ev.keystroke.key.as_str();
        let mods = ev.keystroke.modifiers;
        if mods.control || mods.alt || mods.platform {
            return;
        }

        let Some(side) = self.active_tab().map(|t| t.model.active_pane) else {
            return;
        };

        // Space toggles QuickLook when the file list has focus (never while editing inputs).
        if key == "space" {
            self.toggle_quicklook(cx);
            return;
        }

        match key {
            "up" | "down" => {
                let delta = if key == "up" { -1 } else { 1 };
                let extend = mods.shift && !self.quicklook_open;
                let moved = if let Some(tab) = self.active_tab_mut() {
                    let pane = tab.model.pane_mut(side);
                    let total = pane.filtered_indices.len();
                    if let Some(idx) = pane.selection.move_cursor(delta, total, extend) {
                        pane.recompute_selection_bytes();
                        let scroll_idx = match pane.view_mode {
                            ViewMode::Grid => {
                                let cols = if tab.model.split_enabled { 4 } else { 6 };
                                idx / cols
                            }
                            _ => idx,
                        };
                        tab.pane_ui(side)
                            .scroll_handle
                            .scroll_to_item(scroll_idx, ScrollStrategy::Nearest);
                        true
                    } else {
                        false
                    }
                } else {
                    false
                };
                if moved {
                    self.sync_preview_for_selection(cx);
                    cx.notify();
                }
            }
            "enter" => {
                let target = self.active_tab().and_then(|t| {
                    t.model
                        .pane(side)
                        .primary_selected_entry()
                        .map(|e| (e.location.clone(), e.is_dir()))
                });
                if let Some((loc, is_dir)) = target {
                    self.quicklook_open = false;
                    self.open_location_or_file(loc, is_dir, window, cx);
                }
            }
            "backspace" if !self.quicklook_open => {
                self.go_up(side, window, cx);
            }
            "tab" if !self.quicklook_open => {
                if let Some(tab) = self.active_tab_mut()
                    && tab.model.split_enabled
                {
                    tab.model.active_pane = tab.model.active_pane.opposite();
                    self.sync_preview_for_selection(cx);
                    cx.notify();
                }
            }
            _ if !self.quicklook_open => {
                if let Some(ch) = ev.keystroke.key_char.as_deref()
                    && ch.len() == 1
                    && !ch.chars().all(|c| c.is_control())
                {
                    if self.type_ahead_at.elapsed() > Duration::from_millis(750) {
                        self.type_ahead_buf.clear();
                    }
                    self.type_ahead_at = Instant::now();
                    self.type_ahead_buf.push_str(ch);
                    let prefix = self.type_ahead_buf.clone();

                    let jumped = if let Some(tab) = self.active_tab_mut() {
                        let pane = tab.model.pane_mut(side);
                        if let Some(vis_idx) = pane.find_type_ahead(&prefix) {
                            pane.selection.select_only(vis_idx);
                            pane.recompute_selection_bytes();
                            let scroll_idx = match pane.view_mode {
                                ViewMode::Grid => {
                                    let cols = if tab.model.split_enabled { 4 } else { 6 };
                                    vis_idx / cols
                                }
                                _ => vis_idx,
                            };
                            tab.pane_ui(side)
                                .scroll_handle
                                .scroll_to_item(scroll_idx, ScrollStrategy::Center);
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    };
                    if jumped {
                        self.sync_preview_for_selection(cx);
                        cx.notify();
                    }
                }
            }
            _ => {}
        }
    }
}

fn archive_stem_name(path: &Path) -> String {
    let raw = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Extracted".to_string());
    let lower = raw.to_ascii_lowercase();
    if lower.ends_with(".tar.gz") && raw.len() > 7 {
        return raw[..raw.len() - 7].to_string();
    }
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Extracted".to_string())
}

fn same_volume(a: &Path, b: &Path) -> bool {
    use std::path::Component;
    match (a.components().next(), b.components().next()) {
        (Some(Component::Prefix(pa)), Some(Component::Prefix(pb))) => pa
            .as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case(&pb.as_os_str().to_string_lossy()),
        _ => true,
    }
}

fn normalize_path_key(path: &Path) -> String {
    #[cfg(target_os = "windows")]
    {
        let s = path.to_string_lossy().replace('/', "\\");
        let trimmed = if s.len() > 3 {
            s.trim_end_matches('\\')
        } else {
            s.as_str()
        };
        trimmed.to_ascii_lowercase()
    }
    #[cfg(not(target_os = "windows"))]
    {
        let s = path.to_string_lossy();
        if s.len() > 1 {
            s.trim_end_matches('/').to_string()
        } else {
            s.into_owned()
        }
    }
}

fn paths_same(a: &Path, b: &Path) -> bool {
    normalize_path_key(a) == normalize_path_key(b)
}

fn is_same_or_descendant_path(candidate: &Path, ancestor: &Path) -> bool {
    let c = normalize_path_key(candidate);
    let a = normalize_path_key(ancestor);
    if c == a {
        return true;
    }
    #[cfg(target_os = "windows")]
    let sep = '\\';
    #[cfg(not(target_os = "windows"))]
    let sep = '/';

    if a.ends_with(sep) {
        c.starts_with(&a)
    } else {
        c.starts_with(&format!("{a}{sep}"))
    }
}

fn filter_effective_transfer_sources(
    kind: TransferKind,
    sources: Vec<PathBuf>,
    dest_dir: &Path,
    ignore_self_nesting: bool,
) -> Vec<PathBuf> {
    sources
        .into_iter()
        .filter(|src| {
            let Some(name) = src.file_name() else {
                return false;
            };
            if kind == TransferKind::Move && paths_same(src, &dest_dir.join(name)) {
                return false;
            }
            if ignore_self_nesting && is_same_or_descendant_path(dest_dir, src) {
                return false;
            }
            true
        })
        .collect()
}

/// Returns `true` when dropping `sources` into `dest_dir` via internal drag-and-drop
/// would actually move or copy at least one item.
pub fn is_valid_internal_drop_target(sources: &[PathBuf], dest_dir: &Path) -> bool {
    if sources.is_empty() {
        return false;
    }
    let kind = if !same_volume(&sources[0], dest_dir) {
        TransferKind::Copy
    } else {
        TransferKind::Move
    };
    !filter_effective_transfer_sources(kind, sources.to_vec(), dest_dir, true).is_empty()
}

async fn smol_sleep(duration: Duration) {
    let (tx, rx) = futures_channel::oneshot::channel::<()>();
    tokio_runtime().spawn(async move {
        tokio::time::sleep(duration).await;
        let _ = tx.send(());
    });
    let _ = rx.await;
}

impl Drop for Workspace {
    fn drop(&mut self) {
        self.persist_session_snapshot();
    }
}

impl Focusable for Workspace {
    fn focus_handle(&self, _cx: &gpui_kit::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.capture_window_geometry(window);
        let p = self.palette();
        let is_split = self.active_tab().is_some_and(|t| t.model.split_enabled);

        let main_panes = if is_split {
            h_resizable("dual-pane-split")
                .with_handle_appearance(crisp_resize_handle_appearance(p))
                .child(resizable_panel().child(render_pane(self, PaneSide::Left, true, cx)))
                .child(resizable_panel().child(render_pane(self, PaneSide::Right, true, cx)))
                .into_any_element()
        } else {
            render_pane(self, PaneSide::Left, false, cx).into_any_element()
        };

        let center_and_inspector = if self.settings.show_inspector {
            h_resizable("center-inspector-split")
                .with_handle_appearance(crisp_resize_handle_appearance(p))
                .child(resizable_panel().child(main_panes))
                .child(
                    resizable_panel()
                        .size(px(328.))
                        .size_range(px(260.)..px(500.))
                        .fixed()
                        .child(render_inspector(self, cx)),
                )
                .into_any_element()
        } else {
            main_panes
        };

        let body = if self.settings.show_sidebar {
            h_resizable("sidebar-main-split")
                .with_handle_appearance(crisp_resize_handle_appearance(p))
                .child(
                    resizable_panel()
                        .size(px(256.))
                        .size_range(px(200.)..px(380.))
                        .fixed()
                        .child(render_sidebar(self, cx)),
                )
                .child(resizable_panel().child(center_and_inspector))
                .into_any_element()
        } else {
            center_and_inspector
        };

        let quicklook_entry = if self.quicklook_open {
            self.active_tab()
                .and_then(|t| t.model.active_pane_ref().primary_selected_entry())
                .cloned()
        } else {
            None
        };
        let quicklook_payload = self.preview_payload.clone();
        let has_transfers = !self.transfers.is_empty();

        v_flex()
            .id("texplorer-workspace")
            .key_context("TExplorerWorkspace")
            .track_focus(&self.focus_handle)
            .size_full()
            .relative()
            .bg(solid(p.bg_base))
            .text_color(solid(p.text_primary))
            .overflow_hidden()
            .on_key_down(cx.listener(Self::handle_key_down))
            .on_mouse_move(cx.listener(Self::handle_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::handle_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::handle_mouse_up))
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Back),
                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                    if !window.default_prevented()
                        && !this.is_modal_blocking()
                        && let Some(side) = this.active_tab().map(|t| t.model.active_pane)
                    {
                        this.go_back(side, window, cx);
                    }
                }),
            )
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Forward),
                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                    if !window.default_prevented()
                        && !this.is_modal_blocking()
                        && let Some(side) = this.active_tab().map(|t| t.model.active_pane)
                    {
                        this.go_forward(side, window, cx);
                    }
                }),
            )
            .on_action(cx.listener(|this, _: &NewTab, window, cx| {
                if this.is_modal_blocking() {
                    return;
                }
                if !this.settings.show_tabs {
                    this.settings.show_tabs = true;
                    let _ = this.settings.save();
                }
                this.open_new_tab(Location::Home, window, cx);
            }))
            .on_action(cx.listener(|this, _: &CloseTab, window, cx| {
                if this.is_modal_blocking() {
                    return;
                }
                let idx = this.active_tab_index;
                this.close_tab_at(idx, window, cx);
            }))
            .on_action(cx.listener(|this, _: &NextTab, window, cx| {
                if this.is_modal_blocking() {
                    return;
                }
                if !this.tabs.is_empty() {
                    let next = (this.active_tab_index + 1) % this.tabs.len();
                    this.select_tab(next, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &PrevTab, window, cx| {
                if this.is_modal_blocking() {
                    return;
                }
                if !this.tabs.is_empty() {
                    let prev = if this.active_tab_index == 0 {
                        this.tabs.len() - 1
                    } else {
                        this.active_tab_index - 1
                    };
                    this.select_tab(prev, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ToggleSplitPane, window, cx| {
                if this.is_modal_blocking() {
                    return;
                }
                this.toggle_split_active_tab(window, cx);
            }))
            .on_action(cx.listener(|this, _: &SwitchActivePane, _, cx| {
                if this.is_modal_blocking() {
                    return;
                }
                if let Some(tab) = this.active_tab_mut()
                    && tab.model.split_enabled
                {
                    tab.model.active_pane = tab.model.active_pane.opposite();
                    this.sync_preview_for_selection(cx);
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|this, _: &FocusAddressBar, window, cx| {
                if this.is_modal_blocking() {
                    return;
                }
                let side = this
                    .active_tab()
                    .map(|t| t.model.active_pane)
                    .unwrap_or(PaneSide::Left);
                this.begin_edit_address(side, window, cx);
            }))
            .on_action(cx.listener(|this, _: &GoBack, window, cx| {
                if this.is_modal_blocking() {
                    return;
                }
                let side = this
                    .active_tab()
                    .map(|t| t.model.active_pane)
                    .unwrap_or(PaneSide::Left);
                this.go_back(side, window, cx);
            }))
            .on_action(cx.listener(|this, _: &GoForward, window, cx| {
                if this.is_modal_blocking() {
                    return;
                }
                let side = this
                    .active_tab()
                    .map(|t| t.model.active_pane)
                    .unwrap_or(PaneSide::Left);
                this.go_forward(side, window, cx);
            }))
            .on_action(cx.listener(|this, _: &GoUp, window, cx| {
                if this.is_modal_blocking() {
                    return;
                }
                let side = this
                    .active_tab()
                    .map(|t| t.model.active_pane)
                    .unwrap_or(PaneSide::Left);
                this.go_up(side, window, cx);
            }))
            .on_action(cx.listener(|this, _: &RefreshPane, window, cx| {
                if this.is_modal_blocking() {
                    return;
                }
                let side = this
                    .active_tab()
                    .map(|t| t.model.active_pane)
                    .unwrap_or(PaneSide::Left);
                this.refresh_pane(side, window, cx);
            }))
            .on_action(cx.listener(|this, _: &FocusFilter, window, cx| {
                if this.is_modal_blocking() {
                    return;
                }
                if let Some(tab) = this.active_tab() {
                    let side = tab.model.active_pane;
                    let input = tab.pane_ui(side).filter_input.clone();
                    input.update(cx, |s, cx| s.focus(window, cx));
                }
            }))
            .on_action(cx.listener(|this, _: &OpenGlobalSearch, window, cx| {
                if this.active_modal.is_some() || !this.conflict_queue.is_empty() {
                    return;
                }
                if this.global_search.is_some() {
                    this.close_global_search(window, cx);
                } else {
                    this.open_global_search(window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &OpenCommandPalette, window, cx| {
                if this.active_modal.is_some() || !this.conflict_queue.is_empty() {
                    return;
                }
                if this.command_palette.is_some() {
                    this.close_command_palette(window, cx);
                } else {
                    this.open_command_palette(window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &OpenBulkRename, window, cx| {
                if this.active_modal.is_some() || !this.conflict_queue.is_empty() {
                    return;
                }
                if this.bulk_rename.is_some() {
                    this.close_bulk_rename(window, cx);
                } else {
                    let side = this
                        .active_tab()
                        .map(|t| t.model.active_pane)
                        .unwrap_or(PaneSide::Left);
                    this.open_bulk_rename_for_pane(side, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ToggleSidebar, _, cx| {
                if this.is_modal_blocking() {
                    return;
                }
                this.toggle_sidebar(cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleInspector, _, cx| {
                if this.is_modal_blocking() {
                    return;
                }
                this.toggle_inspector(cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleHiddenFiles, _, cx| {
                if this.is_modal_blocking() {
                    return;
                }
                let side = this
                    .active_tab()
                    .map(|t| t.model.active_pane)
                    .unwrap_or(PaneSide::Left);
                this.toggle_hidden_for_pane(side, cx);
            }))
            .on_action(cx.listener(|this, _: &SelectAllItems, _, cx| {
                if this.is_modal_blocking() {
                    return;
                }
                if let Some(tab) = this.active_tab_mut() {
                    let pane = tab.model.active_pane_mut();
                    let total = pane.filtered_indices.len();
                    pane.selection.select_all(total);
                    pane.recompute_selection_bytes();
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|this, _: &CopySelected, window, cx| {
                if this.is_modal_blocking() {
                    return;
                }
                let side = this
                    .active_tab()
                    .map(|t| t.model.active_pane)
                    .unwrap_or(PaneSide::Left);
                this.copy_or_cut_selected(side, false, window, cx);
            }))
            .on_action(cx.listener(|this, _: &CutSelected, window, cx| {
                if this.is_modal_blocking() {
                    return;
                }
                let side = this
                    .active_tab()
                    .map(|t| t.model.active_pane)
                    .unwrap_or(PaneSide::Left);
                this.copy_or_cut_selected(side, true, window, cx);
            }))
            .on_action(cx.listener(|this, _: &PasteItems, window, cx| {
                if this.is_modal_blocking() {
                    return;
                }
                let dir = this.active_tab().and_then(|t| {
                    t.model
                        .active_pane_ref()
                        .location
                        .as_local_path()
                        .map(|p| p.to_path_buf())
                });
                if let Some(dest) = dir {
                    this.paste_clipboard_into(dest, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &UndoActionCmd, window, cx| {
                if this.is_modal_blocking() {
                    return;
                }
                this.undo_last_action(window, cx);
            }))
            .on_action(cx.listener(|this, _: &RedoActionCmd, window, cx| {
                if this.is_modal_blocking() {
                    return;
                }
                this.redo_last_action(window, cx);
            }))
            .on_action(cx.listener(|this, _: &CreateNewFolder, window, cx| {
                if this.is_modal_blocking() {
                    return;
                }
                let info = this.active_tab().and_then(|t| {
                    let side = t.model.active_pane;
                    t.model
                        .pane(side)
                        .location
                        .as_local_path()
                        .map(|p| (side, p.to_path_buf()))
                });
                if let Some((side, dir)) = info {
                    this.open_new_folder_prompt(side, dir, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &RenameSelected, window, cx| {
                if this.is_modal_blocking() {
                    return;
                }
                let info = this.active_tab().and_then(|t| {
                    let side = t.model.active_pane;
                    let entry = t.model.pane(side).primary_selected_entry()?;
                    let path = entry.location.as_local_path()?.to_path_buf();
                    Some((side, path, entry.name.clone()))
                });
                if let Some((side, path, name)) = info {
                    this.open_rename_prompt(side, path, name, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &DeleteToTrash, window, cx| {
                if this.is_modal_blocking() {
                    return;
                }
                let side = this
                    .active_tab()
                    .map(|t| t.model.active_pane)
                    .unwrap_or(PaneSide::Left);
                this.trash_selected(side, window, cx);
            }))
            .on_action(cx.listener(|this, _: &DeletePermanent, _, cx| {
                if this.is_modal_blocking() {
                    return;
                }
                let side = this
                    .active_tab()
                    .map(|t| t.model.active_pane)
                    .unwrap_or(PaneSide::Left);
                this.prompt_permanent_delete(side, cx);
            }))
            .on_action(cx.listener(|this, _: &OpenSettings, window, cx| {
                this.open_settings_modal(window, cx);
            }))
            .on_action(cx.listener(|this, _: &CancelOrDismiss, window, cx| {
                if this.context_menu.is_some() {
                    this.dismiss_context_menu(cx);
                } else if this.command_palette.is_some() {
                    this.close_command_palette(window, cx);
                } else if this.bulk_rename.is_some() {
                    this.close_bulk_rename(window, cx);
                } else if this.global_search.is_some() {
                    this.close_global_search(window, cx);
                } else if this.quicklook_open {
                    this.close_quicklook(cx);
                    this.focus_handle.focus(window, cx);
                } else if this.active_modal.is_some() {
                    this.close_modal(cx);
                    this.focus_handle.focus(window, cx);
                } else if let Some(tab) = this.active_tab() {
                    let tab_id = tab.model.id;
                    let side = tab.model.active_pane;
                    if tab.pane_ui(side).editing_address {
                        this.cancel_address_edit(tab_id, side, window, cx);
                        this.focus_handle.focus(window, cx);
                    } else if !tab.model.pane(side).quick_filter.is_empty() {
                        this.clear_pane_filter(side, window, cx);
                        this.focus_handle.focus(window, cx);
                    }
                }
            }))
            .on_action(cx.listener(|this, _: &Quit, window, cx| {
                this.save_session(Some(window));
                cx.quit();
            }))
            // Top 38px frameless header
            .child(render_header(self, window, cx))
            // Edge-to-edge split body
            .child(div().flex_1().min_h_0().w_full().overflow_hidden().child(body))
            // Bottom-right floating transfer queue drawer
            .when(has_transfers, |el| el.child(render_transfer_drawer(self, cx)))
            // Subtle bottom-right toast banner (lifted above transfer drawer when visible)
            .when_some(self.toast.clone(), |el, toast| {
                let dot_hex = match toast.kind {
                    ToastKind::Info => p.status_info,
                    ToastKind::Success => p.status_success,
                    ToastKind::Error => p.status_error,
                };
                el.child(
                    h_flex()
                        .id("toast-banner")
                        .absolute()
                        .right(px(16.))
                        .bottom(if has_transfers { px(16.) } else { px(34.) })
                        .px_3()
                        .py_2()
                        .gap_2()
                        .items_center()
                        .rounded(px(RADIUS_ROW))
                        .bg(solid(p.bg_elevated))
                        .border_1()
                        .border_color(solid(p.border))
                        .child(
                            div()
                                .w(px(6.))
                                .h(px(6.))
                                .rounded(px(999.))
                                .bg(solid(dot_hex)),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(solid(p.text_primary))
                                .child(toast.message),
                        ),
                )
            })
            // Floating context menu overlay
            .when_some(self.context_menu.clone(), |el, menu| {
                el.child(render_context_menu(self, &menu, window, cx))
            })
            // QuickLook Spacebar modal overlay
            .when_some(quicklook_entry, |el, entry| {
                el.child(render_quicklook_modal(self, &entry, &quicklook_payload, cx))
            })
            // Global Search overlay (Ctrl+K)
            .when_some(self.global_search.as_ref(), |el, gs| {
                el.child(render_global_search_modal(self, gs, cx))
            })
            // Bulk Rename overlay (Ctrl+Shift+R)
            .when_some(self.bulk_rename.as_ref(), |el, br| {
                el.child(render_bulk_rename_modal(self, br, cx))
            })
            // Command Palette overlay (Ctrl+Shift+P)
            .when_some(self.command_palette.as_ref(), |el, cp| {
                el.child(render_command_palette_modal(self, cp, cx))
            })
            // Active modal overlay (Settings / Prompts / Confirmations)
            .when_some(self.active_modal.clone(), |el, modal| {
                el.child(render_active_modal(self, &modal, cx))
            })
            // Highest-priority Conflict Resolution modal
            .when_some(self.conflict_queue.front(), |el, prompt| {
                el.child(render_conflict_modal(self, prompt, cx))
            })
    }
}

/// Computes which visible item indices intersect a marquee selection rectangle in list-content
/// coordinates (supports dragging in any direction across Details, Compact, and Grid views).
#[allow(clippy::too_many_arguments)]
fn compute_marquee_hits(
    view_mode: ViewMode,
    total: usize,
    grid_cols: usize,
    row_h: f32,
    bounds_w: f32,
    rect_left: f32,
    rect_top: f32,
    rect_right: f32,
    rect_bottom: f32,
) -> Vec<usize> {
    if total == 0 || row_h <= 0.0 || bounds_w <= 0.0 || rect_right <= 0.0 || rect_left >= bounds_w {
        return Vec::new();
    }
    match view_mode {
        ViewMode::Details | ViewMode::Compact => {
            let max_y = (total as f32) * row_h;
            if rect_bottom <= 0.0 || rect_top >= max_y {
                return Vec::new();
            }
            let first = (rect_top.max(0.0) / row_h).floor() as usize;
            let last = ((rect_bottom - 0.01).max(0.0) / row_h).floor() as usize;
            (first..=last.min(total - 1)).collect()
        }
        ViewMode::Grid => {
            let cols = grid_cols.max(1);
            let total_rows = total.div_ceil(cols);
            let max_y = (total_rows as f32) * row_h;
            if rect_bottom <= 0.0 || rect_top >= max_y {
                return Vec::new();
            }
            let pad_x = 12.0_f32.min(bounds_w * 0.1);
            let inner_w = (bounds_w - pad_x * 2.0).max(1.0);
            let cell_w = inner_w / (cols as f32);
            let first_row = (rect_top.max(0.0) / row_h).floor() as usize;
            let last_row = ((rect_bottom - 0.01).max(0.0) / row_h)
                .floor()
                .min((total_rows - 1) as f32) as usize;

            let mut hits = Vec::new();
            for row in first_row..=last_row {
                let card_top = (row as f32) * row_h + 4.0;
                let card_bottom = ((row + 1) as f32) * row_h - 4.0;
                if rect_bottom <= card_top || rect_top >= card_bottom {
                    continue;
                }
                for col in 0..cols {
                    let vis_idx = row * cols + col;
                    if vis_idx >= total {
                        break;
                    }
                    let card_left = pad_x + (col as f32) * cell_w + 4.0;
                    let card_right = pad_x + ((col + 1) as f32) * cell_w - 4.0;
                    if rect_right > card_left && rect_left < card_right {
                        hits.push(vis_idx);
                    }
                }
            }
            hits
        }
    }
}

#[cfg(test)]
mod transfer_drop_tests {
    use super::*;

    #[test]
    fn marquee_selects_items_dragging_upward_in_details_and_grid() {
        // Dragging upward from empty area below 5 Details rows (32px each, total 160px) up to row 2 (y=70).
        let hits = compute_marquee_hits(
            ViewMode::Details,
            5,
            4,
            32.0,
            600.0,
            100.0,
            70.0,
            300.0,
            280.0,
        );
        assert_eq!(hits, vec![2, 3, 4]);

        // Grid 2D intersection across 3 columns (6 items, 2 rows of 136px).
        let grid_hits = compute_marquee_hits(
            ViewMode::Grid,
            6,
            3,
            136.0,
            624.0,
            230.0,
            10.0,
            580.0,
            200.0,
        );
        assert_eq!(grid_hits, vec![1, 2, 4, 5]);
    }

    #[test]
    fn ignores_dropping_items_back_into_same_directory_or_self() {
        let desktop = PathBuf::from(r"C:\Users\danie\Desktop");
        let file = desktop.join("Roblox Studio.lnk");
        let folder = desktop.join("Projects");
        let subfolder = folder.join("Sub");
        let sibling_prefix = desktop.join("Projects2");

        // Same-folder drop is a no-op.
        assert!(!is_valid_internal_drop_target(std::slice::from_ref(&file), &desktop));
        // Dropping into a subfolder inside the same directory IS valid.
        assert!(is_valid_internal_drop_target(std::slice::from_ref(&file), &folder));
        // Dropping a folder onto itself or inside a descendant is ignored.
        assert!(!is_valid_internal_drop_target(std::slice::from_ref(&folder), &folder));
        assert!(!is_valid_internal_drop_target(std::slice::from_ref(&folder), &subfolder));
        // Dropping a folder onto a sibling with shared prefix is valid.
        assert!(is_valid_internal_drop_target(std::slice::from_ref(&folder), &sibling_prefix));
        // Copy (Ctrl+C -> Ctrl+V) inside the same folder still duplicates.
        assert_eq!(
            filter_effective_transfer_sources(
                TransferKind::Copy,
                vec![file.clone()],
                &desktop,
                false,
            ),
            vec![file]
        );
    }
}
