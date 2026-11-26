use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use futures_channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use futures_util::StreamExt as _;
use gpui_kit::{
    Bounds, ClipboardItem, Context, Entity, FocusHandle, Focusable, KeyDownEvent, MouseButton,
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
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use texplorer_core::{
    AccentPreset, AppSession, AppSettings, FileEntry, FileKind, KnownFolder, KnownFolderKind,
    Location, NetworkConnection, NetworkProtocol, PaneSide, ParsedNetworkUri, SavedPaneState,
    SavedTabState, SearchScope, SidebarSection, SizeUnit, SortColumn, SortSpec, SshAuthMode,
    TabModel, ThemePreset, ViewMode, VolumeInfo, WindowGeometry, join_remote_path,
    normalize_remote_path, normalize_smb_share_path, parse_network_uri, sort_entries,
};
use texplorer_mft::{
    IndexServiceInstallState, IndexServiceStatus, install_service_elevated,
    query_live_service_status, query_service_install_state, rebuild_index_via_service,
    start_service_elevated, uninstall_service_elevated,
};
use texplorer_net::{
    ActiveP2pShare, IncomingP2pOffer, NearbyPeer, P2pDirection, P2pEvent, P2pService,
    RemoteClientPool, RemoteCredentials, decode_ticket, detect_default_ssh_key,
};
use texplorer_ops::{
    BulkRenameSpec, ConflictDecision, ConflictPrompt, ConflictResolution, RenameCasing,
    TransferControl, TransferEvent, TransferKind, TransferSnapshot, TransferStatus, UndoAction,
    UndoHistory, execute_bulk_rename, preview_bulk_rename, spawn_transfer_job,
    unique_keep_both_path,
};
use texplorer_platform::{
    DefaultExplorerStatus, DirectoryWatcher, FileLockStatus, LaunchIntent, NativeDragOutcome,
    NativeDropEvent, ShellWorker, VideoNotify, VideoPlayerHandle, VideoStartOptions,
    bring_current_process_window_to_front, connect_smb_share, cursor_screen_pos,
    delete_network_password, delete_permanently, disconnect_smb_share, enable_default_explorer,
    get_clipboard_files, inspect_file_lock, install_native_drop_target, is_elevated,
    list_known_folders, list_volumes, list_wsl_distros, load_network_password, move_to_trash,
    open_in_editor, open_item, open_terminal_here, pick_files_dialog, pick_folder_dialog,
    query_default_explorer_status, restore_windows_explorer, reveal_in_windows_explorer,
    save_network_password, set_clipboard_files, set_win_e_interception, spawn_as_desktop_user,
    spawn_video_player, start_native_file_drag, system_prefers_dark_mode,
};
use texplorer_vfs::archive::{
    compress_to_zip, extract_archive_all, extract_archive_entries, is_supported_archive,
};
use texplorer_vfs::folder_size::resolve_folder_sizes;
use texplorer_vfs::hash::{FileHashes, compute_file_hashes};
use texplorer_vfs::search::{SearchOutcome, execute_search};
use uuid::Uuid;

use crate::preview::{
    PDF_PREVIEW_MAX_DIM, PreviewPayload, ThumbnailCache, ThumbnailKey, decode_pdf_page_from_bytes,
    decode_pdf_page_from_path, load_archive_entry_preview_blocking, load_preview_blocking,
    preview_from_remote_bytes, raw_bgra_to_render_image,
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
use crate::ui::network::{
    ActiveP2pTransferUi, NetworkConnectModalState, P2pModalState, P2pModalTab, render_p2p_overlays,
};
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
    PdfPageReady {
        generation: u64,
        location: Location,
        page_index: u32,
        result: Result<PreviewPayload, String>,
    },
    VideoNotify {
        generation: u64,
        notify: VideoNotify,
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
    DismissToast(u64),
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
    NetworkTestFinished {
        result: Result<String, String>,
    },
    SmbConnectFinished {
        unc: String,
        new_tab: bool,
        in_split: bool,
        result: Result<String, String>,
    },
    RemoteOpFinished {
        result: Result<Option<String>, String>,
        refresh_side: Option<PaneSide>,
    },
    RemoteFileOpened {
        result: Result<PathBuf, String>,
    },
    P2pShareCreated {
        result: Result<ActiveP2pShare, String>,
    },
    P2pPickedSendPaths(Result<Vec<PathBuf>, String>),
    P2pPickedDestDir(Result<Option<PathBuf>, String>),
    NetworkPickedKeyPath(Result<Vec<PathBuf>, String>),
    P2p(P2pEvent),
    NativeDrop(NativeDropEvent),
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

/// Live video playback session active inside the QuickLook modal.
pub struct VideoPlaybackState {
    pub generation: u64,
    pub path: PathBuf,
    pub handle: VideoPlayerHandle,
    pub frame: Option<Arc<RenderImage>>,
    pub position_secs: f64,
    pub duration_secs: f64,
    pub playing: bool,
    pub muted: bool,
    pub volume: u8,
    pub native_width: u32,
    pub native_height: u32,
    pub ready: bool,
    pub error: Option<String>,
    pub scrubbing: bool,
    pub dragging_volume: bool,
    pub seek_bar_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    pub volume_bar_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
}

/// Active or recently finished background transfer job tracked by the UI drawer.
pub struct TransferJobState {
    pub snapshot: TransferSnapshot,
    pub control: Arc<TransferControl>,
    pub sources: Vec<PathBuf>,
    pub cleanup_stage_dir: Option<PathBuf>,
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

/// Active divider handle when drag-resizing columns in Details view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnDivider {
    NameModified,
    ModifiedType,
    TypeSize,
}

#[derive(Debug, Clone)]
pub struct ColumnResizeState {
    pub divider: ColumnDivider,
    pub start_x: f32,
    pub start_modified: f32,
    pub start_type: f32,
    pub start_size: f32,
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
    fresh_disk_size_dirs: HashMap<PathBuf, Instant>,
    pub native_drop_hover_side: Option<PaneSide>,
    pub quicklook_open: bool,
    pub preview_location: Option<Location>,
    pub preview_payload: PreviewPayload,
    pub preview_generation: u64,
    pub pdf_zoom: f32,
    pdf_page_cache: HashMap<u32, (Arc<RenderImage>, u32, u32)>,
    pub video_playback: Option<VideoPlaybackState>,
    video_generation: u64,
    stale_gpu_images: Vec<Arc<RenderImage>>,
    pub active_local_drag_paths: Rc<RefCell<Option<Vec<PathBuf>>>>,
    external_ole_drag_active: bool,
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
    pub column_resize: Option<ColumnResizeState>,
    entry_claimed_mouse_down: bool,
    remote_pool: RemoteClientPool,
    session_passwords: HashMap<Uuid, String>,
    p2p_service: P2pService,
    pub p2p_device_name: String,
    pub p2p_nearby_peers: Vec<NearbyPeer>,
    pub active_p2p_share: Option<ActiveP2pShare>,
    pub incoming_p2p_offers: Vec<IncomingP2pOffer>,
    pub active_p2p_transfers: Vec<ActiveP2pTransferUi>,
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
        let p2p_device_name = std::env::var("COMPUTERNAME")
            .or_else(|_| std::env::var("HOSTNAME"))
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "TExplorer-PC".to_string());
        let p2p_tx = bg_tx.clone();
        let p2p_service = {
            let _rt_guard = tokio_runtime().enter();
            P2pService::start(p2p_device_name.clone(), move |ev| {
                let _ = p2p_tx.unbounded_send(BgEvent::P2p(ev));
            })
        };

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
            fresh_disk_size_dirs: HashMap::new(),
            native_drop_hover_side: None,
            quicklook_open: false,
            preview_location: None,
            preview_payload: PreviewPayload::Loading,
            preview_generation: 0,
            pdf_zoom: 1.0,
            pdf_page_cache: HashMap::new(),
            video_playback: None,
            video_generation: 0,
            stale_gpu_images: Vec::new(),
            active_local_drag_paths: Rc::new(RefCell::new(None)),
            external_ole_drag_active: false,
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
            column_resize: None,
            entry_claimed_mouse_down: false,
            remote_pool: RemoteClientPool::new(),
            session_passwords: HashMap::new(),
            p2p_service,
            p2p_device_name,
            p2p_nearby_peers: Vec::new(),
            active_p2p_share: None,
            incoming_p2p_offers: Vec::new(),
            active_p2p_transfers: Vec::new(),
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
        if let Ok(wh) = HasWindowHandle::window_handle(window)
            && let RawWindowHandle::Win32(win32) = wh.as_raw()
        {
            let drop_tx = ws.bg_tx.clone();
            let _ = install_native_drop_target(win32.hwnd.get(), move |ev| {
                let _ = drop_tx.unbounded_send(BgEvent::NativeDrop(ev));
            });
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
                            let had_pending = pane.pending_select_name.is_some();
                            if pane.sort == sort
                                && !(patched_cached_sizes && pane.sort.column == SortColumn::Size)
                            {
                                pane.set_presorted_entries(entries);
                            } else {
                                pane.set_entries(entries);
                            }
                            if had_pending && let Some(idx) = pane.selection.cursor {
                                let scroll_idx = match pane.view_mode {
                                    ViewMode::Grid => {
                                        let cols = if tab.model.split_enabled { 4 } else { 6 };
                                        idx / cols
                                    }
                                    _ => idx,
                                };
                                tab.pane_ui(side)
                                    .scroll_handle
                                    .scroll_to_item(scroll_idx, ScrollStrategy::Center);
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
            BgEvent::DismissToast(id) => {
                if self.toast.as_ref().is_some_and(|t| t.id == id) {
                    self.toast = None;
                    cx.notify();
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
                    self.save_session(Some(window));
                    let dark_mode = self.palette().is_dark;
                    cx.notify();
                    if let Err(err) = spawn_detached_updater(&staged, dark_mode) {
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
                    if let PreviewPayload::Pdf {
                        ref image,
                        page_index,
                        page_width,
                        page_height,
                        ..
                    } = payload
                    {
                        self.pdf_page_cache
                            .insert(page_index, (image.clone(), page_width, page_height));
                    }
                    self.preview_location = Some(location);
                    self.preview_payload = payload;
                    cx.notify();
                }
            }
            BgEvent::PdfPageReady {
                generation,
                location,
                page_index,
                result,
            } => {
                if generation == self.preview_generation
                    && self.preview_location.as_ref() == Some(&location)
                {
                    match result {
                        Ok(PreviewPayload::Pdf {
                            image,
                            page_index: rendered_idx,
                            page_count,
                            page_width,
                            page_height,
                            ..
                        }) => {
                            if self.pdf_page_cache.len() >= 16 {
                                self.pdf_page_cache.clear();
                            }
                            self.pdf_page_cache
                                .insert(rendered_idx, (image.clone(), page_width, page_height));
                            self.preview_payload = PreviewPayload::Pdf {
                                image,
                                page_index: rendered_idx,
                                page_count,
                                page_width,
                                page_height,
                                loading_page: None,
                            };
                        }
                        Ok(_) => {}
                        Err(err) => {
                            if let PreviewPayload::Pdf {
                                ref mut loading_page,
                                ..
                            } = self.preview_payload
                                && *loading_page == Some(page_index)
                            {
                                *loading_page = None;
                            }
                            self.show_toast(err, ToastKind::Error, window, cx);
                        }
                    }
                    cx.notify();
                }
            }
            BgEvent::VideoNotify { generation, notify } => {
                let Some(video) = self.video_playback.as_mut() else {
                    return;
                };
                if video.generation != generation {
                    return;
                }
                match notify {
                    VideoNotify::Ready => {
                        video.ready = true;
                        cx.notify();
                    }
                    VideoNotify::FrameReady => {
                        if let Some(snap) = video.handle.take_latest_frame() {
                            if let Some(new_img) = raw_bgra_to_render_image(snap.image)
                                && let Some(old) = video.frame.replace(new_img)
                            {
                                let _ = window.drop_image(old);
                            }
                            if !video.scrubbing {
                                video.position_secs = snap.position_secs;
                            }
                            video.duration_secs = snap.duration_secs;
                            video.playing = snap.playing;
                            video.native_width = snap.native_width;
                            video.native_height = snap.native_height;
                            video.ready = true;
                            cx.notify();
                        }
                    }
                    VideoNotify::StatusChanged(st) => {
                        if !video.scrubbing {
                            video.position_secs = st.position_secs;
                        }
                        video.duration_secs = st.duration_secs;
                        video.playing = st.playing;
                        if st.native_width > 0 {
                            video.native_width = st.native_width;
                            video.native_height = st.native_height;
                        }
                        cx.notify();
                    }
                    VideoNotify::Error(err) => {
                        video.error = Some(err);
                        video.playing = false;
                        cx.notify();
                    }
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
                    let mut job_sources: Vec<PathBuf> = Vec::new();
                    let mut stage_to_clean: Option<PathBuf> = None;
                    if let Some(job) = self
                        .transfers
                        .iter_mut()
                        .find(|j| j.snapshot.job_id == job_id)
                    {
                        job.snapshot = snapshot;
                        job_sources = job.sources.clone();
                        stage_to_clean = job.cleanup_stage_dir.take();
                    }
                    if let Some(stage_dir) = stage_to_clean {
                        tokio_runtime().spawn(async move {
                            let _ = tokio::task::spawn_blocking(move || {
                                let _ = std::fs::remove_dir_all(&stage_dir);
                            })
                            .await;
                        });
                    }
                    if let Some(undo) = undo_action {
                        self.undo_history.push(undo);
                    }
                    if kind == TransferKind::Move && matches!(status, TransferStatus::Completed) {
                        self.cut_paths.clear();
                    }
                    let mut dirs_to_refresh: Vec<PathBuf> = vec![target_dir.clone()];
                    if let Some(parent) = target_dir.parent() {
                        dirs_to_refresh.push(parent.to_path_buf());
                    }
                    self.mark_fresh_disk_sizes(&target_dir);
                    if let Some(parent) = target_dir.parent() {
                        self.mark_fresh_disk_sizes(parent);
                    }
                    for src in &job_sources {
                        if let Some(name) = src.file_name() {
                            let dest_item = target_dir.join(name);
                            self.mark_fresh_disk_sizes(&dest_item);
                            self.invalidate_folder_sizes_under(&dest_item);
                        }
                        if let Some(src_parent) = src.parent() {
                            self.mark_fresh_disk_sizes(src_parent);
                            if !dirs_to_refresh.iter().any(|d| paths_same(d, src_parent)) {
                                dirs_to_refresh.push(src_parent.to_path_buf());
                            }
                        }
                    }
                    for dir in &dirs_to_refresh {
                        self.invalidate_folder_sizes_under(dir);
                        self.refresh_panes_showing(dir, cx);
                    }
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
                    if !self.is_path_under_active_transfer(path) {
                        self.folder_size_cache.insert(path.clone(), *bytes);
                    }
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
            BgEvent::NetworkTestFinished { result } => {
                if let Some(ActiveModal::NetworkConnect(ref mut form)) = self.active_modal {
                    form.busy = false;
                    match result {
                        Ok(msg) => {
                            form.status_message = Some(msg);
                            form.status_is_error = false;
                        }
                        Err(err) => {
                            form.status_message = Some(err);
                            form.status_is_error = true;
                        }
                    }
                    cx.notify();
                }
            }
            BgEvent::SmbConnectFinished {
                unc,
                new_tab,
                in_split,
                result,
            } => match result {
                Ok(connected_unc) => {
                    let target = Location::local(connected_unc);
                    if in_split {
                        self.open_in_opposite_pane(target, window, cx);
                    } else if new_tab && self.settings.show_tabs {
                        self.open_new_tab(target, window, cx);
                    } else {
                        self.navigate_active_pane(target, window, cx);
                    }
                }
                Err(err) => {
                    self.show_toast(format!("{unc}: {err}"), ToastKind::Error, window, cx);
                }
            },
            BgEvent::RemoteOpFinished {
                result,
                refresh_side,
            } => {
                match result {
                    Ok(Some(msg)) => self.show_toast(msg, ToastKind::Success, window, cx),
                    Ok(None) => {}
                    Err(err) => self.show_toast(err, ToastKind::Error, window, cx),
                }
                if let Some(side) = refresh_side {
                    self.refresh_pane(side, window, cx);
                }
                cx.notify();
            }
            BgEvent::RemoteFileOpened { result } => match result {
                Ok(local_cache_path) => {
                    let tx = self.bg_tx.clone();
                    tokio_runtime().spawn(async move {
                        let res = tokio::task::spawn_blocking(move || {
                            open_item(&local_cache_path).map(|()| None)
                        })
                        .await
                        .unwrap_or_else(|e| Err(format!("Failed opening downloaded file: {e}")));
                        let _ = tx.unbounded_send(BgEvent::ShellOpFinished {
                            result: res,
                            refresh_side: None,
                        });
                    });
                }
                Err(err) => self.show_toast(err, ToastKind::Error, window, cx),
            },
            BgEvent::P2pShareCreated { result } => {
                if let Some(ActiveModal::P2p(ref mut modal)) = self.active_modal {
                    modal.generating_share = false;
                    match result {
                        Ok(share) => {
                            modal.error = None;
                            self.active_p2p_share = Some(share);
                        }
                        Err(err) => {
                            modal.error = Some(err);
                        }
                    }
                    cx.notify();
                } else if let Ok(share) = result {
                    self.active_p2p_share = Some(share);
                    cx.notify();
                }
            }
            BgEvent::P2p(ev) => match ev {
                P2pEvent::EndpointReady { .. } => {
                    cx.notify();
                }
                P2pEvent::EndpointError(err) => {
                    self.show_toast(err, ToastKind::Error, window, cx);
                }
                P2pEvent::PeersUpdated(peers) => {
                    self.p2p_nearby_peers = peers;
                    cx.notify();
                }
                P2pEvent::IncomingOffer(offer) => {
                    self.incoming_p2p_offers.push(offer);
                    cx.notify();
                }
                P2pEvent::TransferProgress {
                    transfer_id,
                    direction,
                    peer_name,
                    summary,
                    current_file,
                    bytes_transferred,
                    total_bytes,
                } => {
                    if let Some(existing) = self
                        .active_p2p_transfers
                        .iter_mut()
                        .find(|t| t.transfer_id == transfer_id)
                    {
                        existing.current_file = current_file;
                        existing.bytes_transferred = bytes_transferred;
                        existing.total_bytes = total_bytes;
                    } else {
                        self.active_p2p_transfers.push(ActiveP2pTransferUi {
                            transfer_id,
                            direction,
                            peer_name,
                            summary,
                            current_file,
                            bytes_transferred,
                            total_bytes,
                        });
                    }
                    cx.notify();
                }
                P2pEvent::TransferFinished {
                    transfer_id,
                    direction,
                    peer_name,
                    summary,
                    dest_dir,
                    result,
                } => {
                    self.active_p2p_transfers
                        .retain(|t| t.transfer_id != transfer_id);
                    match result {
                        Ok(_) => {
                            let msg = match direction {
                                P2pDirection::Send => {
                                    format!("Sent {summary} to {peer_name} via P2P")
                                }
                                P2pDirection::Receive => {
                                    format!("Received {summary} from {peer_name}")
                                }
                            };
                            self.show_toast(msg, ToastKind::Success, window, cx);
                            if let Some(dest) = dest_dir {
                                self.mark_fresh_disk_sizes(&dest);
                                self.invalidate_folder_sizes_under(&dest);
                                self.refresh_panes_showing(&dest, cx);
                            }
                        }
                        Err(err) => {
                            self.show_toast(
                                format!("P2P transfer failed: {err}"),
                                ToastKind::Error,
                                window,
                                cx,
                            );
                        }
                    }
                    cx.notify();
                }
            },
            BgEvent::NativeDrop(drop_ev) => {
                self.handle_native_drop_event(drop_ev, window, cx);
            }
            BgEvent::P2pPickedSendPaths(res) => match res {
                Ok(paths) if !paths.is_empty() => {
                    if let Some(share) = self.active_p2p_share.take() {
                        let svc = self.p2p_service.clone();
                        tokio_runtime().spawn(async move {
                            svc.cancel_share_offer(share.transfer_id).await;
                        });
                    }
                    if let Some(ActiveModal::P2p(ref mut m)) = self.active_modal {
                        m.selected_paths = paths;
                        m.generating_share = false;
                        m.error = None;
                        cx.notify();
                    }
                }
                Ok(_) => {}
                Err(err) => {
                    if let Some(ActiveModal::P2p(ref mut m)) = self.active_modal {
                        m.error = Some(err);
                        cx.notify();
                    }
                }
            },
            BgEvent::P2pPickedDestDir(res) => match res {
                Ok(Some(dir)) => {
                    if let Some(ActiveModal::P2p(ref mut m)) = self.active_modal {
                        m.dest_dir = dir;
                        m.error = None;
                        cx.notify();
                    }
                }
                Ok(None) => {}
                Err(err) => {
                    if let Some(ActiveModal::P2p(ref mut m)) = self.active_modal {
                        m.error = Some(err);
                        cx.notify();
                    }
                }
            },
            BgEvent::NetworkPickedKeyPath(res) => match res {
                Ok(paths) => {
                    if let Some(first) = paths.first()
                        && let Some(ActiveModal::NetworkConnect(ref mut form)) = self.active_modal
                    {
                        let key_str = first.display().to_string();
                        let input = form.key_path_input.clone();
                        form.status_message = None;
                        input.update(cx, |s, cx| s.set_value(key_str, window, cx));
                        cx.notify();
                    }
                }
                Err(err) => {
                    if let Some(ActiveModal::NetworkConnect(ref mut form)) = self.active_modal {
                        form.status_message = Some(err);
                        form.status_is_error = true;
                        cx.notify();
                    }
                }
            },
        }
    }

    fn cleanup_stage_dir_async(stage_dir: Option<PathBuf>) {
        if let Some(dir) = stage_dir {
            tokio_runtime().spawn(async move {
                let _ = tokio::task::spawn_blocking(move || {
                    let _ = std::fs::remove_dir_all(&dir);
                })
                .await;
            });
        }
    }

    fn hit_test_native_drop_pane(
        &self,
        client_x: f32,
        client_y: f32,
        window: &Window,
    ) -> Option<(PaneSide, Option<PathBuf>)> {
        let tab = self.active_tab()?;
        let scale = window.scale_factor().max(1.0);
        let lx = client_x / scale;
        let ly = client_y / scale;

        let sides: &[PaneSide] = if tab.model.split_enabled {
            &[PaneSide::Left, PaneSide::Right]
        } else {
            &[PaneSide::Left]
        };

        let mut chosen_side: Option<PaneSide> = None;
        for &side in sides {
            let b = tab.pane_ui(side).scroll_handle.0.borrow().base_handle.bounds();
            let bx: f32 = b.origin.x.into();
            let by: f32 = b.origin.y.into();
            let bw: f32 = b.size.width.into();
            let bh: f32 = b.size.height.into();
            if bw > 1.0 && bh > 1.0 && lx >= bx && lx <= bx + bw && ly >= by && ly <= by + bh {
                chosen_side = Some(side);
                break;
            }
        }

        if chosen_side.is_none() && tab.model.split_enabled {
            let left_b = tab
                .pane_ui(PaneSide::Left)
                .scroll_handle
                .0
                .borrow()
                .base_handle
                .bounds();
            let right_b = tab
                .pane_ui(PaneSide::Right)
                .scroll_handle
                .0
                .borrow()
                .base_handle
                .bounds();
            let rx: f32 = right_b.origin.x.into();
            let rw: f32 = right_b.size.width.into();
            let lx_b: f32 = left_b.origin.x.into();
            let lw: f32 = left_b.size.width.into();
            if rw > 1.0 && lx >= rx {
                chosen_side = Some(PaneSide::Right);
            } else if lw > 1.0 && lx >= lx_b {
                chosen_side = Some(PaneSide::Left);
            } else {
                let win_w = self
                    .last_window_geometry
                    .as_ref()
                    .map(|g| g.width)
                    .unwrap_or(1280.0);
                let sidebar_w = if self.settings.show_sidebar { 256.0 } else { 0.0 };
                let inspector_w = if self.settings.show_inspector { 328.0 } else { 0.0 };
                let mid_x = sidebar_w + ((win_w - sidebar_w - inspector_w).max(200.0) * 0.5);
                chosen_side = Some(if lx >= mid_x {
                    PaneSide::Right
                } else {
                    PaneSide::Left
                });
            }
        }
        let side = chosen_side.unwrap_or(tab.model.active_pane);

        // If dropped directly over a local folder row in Details/Compact view, target that subfolder.
        let mut hovered_folder: Option<PathBuf> = None;
        let pane = tab.model.pane(side);
        if matches!(pane.view_mode, ViewMode::Details | ViewMode::Compact) {
            let state = tab.pane_ui(side).scroll_handle.0.borrow();
            let b = state.base_handle.bounds();
            let bx: f32 = b.origin.x.into();
            let by: f32 = b.origin.y.into();
            let bw: f32 = b.size.width.into();
            let bh: f32 = b.size.height.into();
            let scroll_y: f32 = state.base_handle.offset().y.into();
            if bw > 1.0 && bh > 1.0 && lx >= bx && lx <= bx + bw && ly >= by && ly <= by + bh {
                let row_h = if pane.view_mode == ViewMode::Details {
                    32.0
                } else {
                    28.0
                };
                let content_y = ly - by - scroll_y;
                if content_y >= 0.0 {
                    let row_idx = (content_y / row_h).floor() as usize;
                    if let Some(entry) = pane.visible_entry(row_idx)
                        && entry.is_dir()
                        && let Some(local) = entry.location.as_local_path()
                    {
                        hovered_folder = Some(local.to_path_buf());
                    }
                }
            }
        }

        Some((side, hovered_folder))
    }

    fn handle_native_drop_event(
        &mut self,
        event: NativeDropEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            NativeDropEvent::Entered { client_x, client_y }
            | NativeDropEvent::Over { client_x, client_y } => {
                let next = self
                    .hit_test_native_drop_pane(client_x, client_y, window)
                    .map(|(side, _)| side);
                if self.native_drop_hover_side != next {
                    self.native_drop_hover_side = next;
                    cx.notify();
                }
            }
            NativeDropEvent::Left => {
                if self.native_drop_hover_side.take().is_some() {
                    cx.notify();
                }
            }
            NativeDropEvent::Dropped {
                client_x,
                client_y,
                paths,
                stage_dir,
            } => {
                self.native_drop_hover_side = None;
                cx.notify();
                if paths.is_empty() {
                    Self::cleanup_stage_dir_async(stage_dir);
                    return;
                }
                let Some((side, hovered_folder)) =
                    self.hit_test_native_drop_pane(client_x, client_y, window)
                else {
                    Self::cleanup_stage_dir_async(stage_dir);
                    return;
                };
                let pane_loc = self
                    .active_tab()
                    .map(|t| t.model.pane(side).location.clone());
                match pane_loc {
                    Some(Location::Local(pane_dir)) => {
                        let dest_dir = hovered_folder.unwrap_or(pane_dir);
                        if !dest_dir.is_dir() {
                            Self::cleanup_stage_dir_async(stage_dir);
                            return;
                        }
                        if self.external_ole_drag_active && stage_dir.is_none() {
                            self.handle_dropped_paths(paths, dest_dir, false, window, cx);
                            return;
                        }
                        let effective = filter_effective_transfer_sources(
                            TransferKind::Copy,
                            paths,
                            &dest_dir,
                            true,
                        );
                        if effective.is_empty() {
                            Self::cleanup_stage_dir_async(stage_dir);
                            return;
                        }
                        self.enqueue_transfer_with_cleanup(
                            TransferKind::Copy,
                            effective,
                            dest_dir,
                            stage_dir,
                            cx,
                        );
                    }
                    Some(Location::Remote { .. }) => {
                        self.handle_dropped_paths_into_pane_with_cleanup(
                            side, paths, stage_dir, window, cx,
                        );
                    }
                    _ => {
                        Self::cleanup_stage_dir_async(stage_dir);
                        self.show_toast(
                            "Open a folder pane to drop items into it.",
                            ToastKind::Info,
                            window,
                            cx,
                        );
                    }
                }
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
                if tab
                    .model
                    .pane(side)
                    .location
                    .as_local_path()
                    .is_some_and(|p| paths_same(p, dir))
                {
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
        self.sync_video_playback_for_quicklook();

        if !self.settings.show_inspector && !self.quicklook_open {
            return;
        }
        let target = self
            .active_tab()
            .and_then(|t| t.model.active_pane_ref().primary_selected_entry())
            .filter(|e| {
                matches!(
                    e.location,
                    Location::Local(_) | Location::Archive { .. } | Location::Remote { .. }
                )
            })
            .map(|e| (e.location.clone(), e.kind));

        let Some((location, kind)) = target else {
            self.preview_location = None;
            self.preview_payload = PreviewPayload::Loading;
            self.pdf_page_cache.clear();
            self.pdf_zoom = 1.0;
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
        self.pdf_page_cache.clear();
        self.pdf_zoom = 1.0;

        let worker = self.shell_worker.clone();
        let palette = self.palette();
        let tx = self.bg_tx.clone();
        if let Location::Remote {
            connection_id,
            ref remote_path,
            ..
        } = location
        {
            if kind == FileKind::Folder {
                self.preview_payload = PreviewPayload::Unsupported {
                    reason: "Remote folder — open to browse its contents.".to_string(),
                };
                cx.notify();
                return;
            }
            let Some(conn) = self.settings.network_connection(connection_id).cloned() else {
                self.preview_payload = PreviewPayload::Unsupported {
                    reason: "Network connection not found.".to_string(),
                };
                cx.notify();
                return;
            };
            let creds = self.resolve_remote_credentials(connection_id);
            let pool = self.remote_pool.clone();
            let rpath = remote_path.clone();
            let is_pdf = rpath.to_ascii_lowercase().ends_with(".pdf");
            let max_bytes = if is_pdf {
                12 * 1024 * 1024
            } else if matches!(kind, FileKind::Image) {
                8 * 1024 * 1024
            } else {
                192 * 1024
            };
            tokio_runtime().spawn(async move {
                let payload = match pool.read_file_bytes(&conn, &creds, &rpath, max_bytes).await {
                    Ok((bytes, truncated)) => tokio::task::spawn_blocking(move || {
                        preview_from_remote_bytes(&rpath, bytes, truncated, palette)
                    })
                    .await
                    .unwrap_or_else(|_| PreviewPayload::Unsupported {
                        reason: "Preview decoder failed.".to_string(),
                    }),
                    Err(err) => PreviewPayload::Unsupported { reason: err },
                };
                let _ = tx.unbounded_send(BgEvent::PreviewReady {
                    generation,
                    location,
                    payload,
                });
            });
            cx.notify();
            return;
        }

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
        if let Location::Remote {
            connection_id,
            ref remote_path,
            ..
        } = location
        {
            let Some(conn) = self.settings.network_connection(connection_id).cloned() else {
                if let Some(tab) = self.tabs.iter_mut().find(|t| t.model.id == tab_id) {
                    let p = tab.model.pane_mut(side);
                    p.loading = false;
                    p.error = Some("Saved network connection no longer exists.".to_string());
                }
                cx.notify();
                return;
            };
            let creds = self.resolve_remote_credentials(connection_id);
            let pool = self.remote_pool.clone();
            let rpath = remote_path.clone();
            tokio_runtime().spawn(async move {
                let result = pool.list_dir(&conn, &creds, &rpath).await.map(|mut entries| {
                    sort_entries(&mut entries, sort);
                    entries
                });
                let _ = tx.unbounded_send(BgEvent::DirectoryLoaded {
                    tab_id,
                    side,
                    generation,
                    sort,
                    result,
                });
            });
            cx.notify();
            return;
        }

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

    fn mark_fresh_disk_sizes(&mut self, path: &Path) {
        let expiry = Instant::now() + Duration::from_secs(5);
        self.fresh_disk_size_dirs.insert(path.to_path_buf(), expiry);
    }

    fn is_path_under_active_transfer(&self, path: &Path) -> bool {
        self.transfers.iter().any(|job| {
            !job.snapshot.status.is_terminal()
                && (is_same_or_descendant_path(path, &job.snapshot.target_dir)
                    || is_same_or_descendant_path(&job.snapshot.target_dir, path))
        })
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
        let now = Instant::now();
        self.fresh_disk_size_dirs.retain(|_, exp| *exp > now);
        let force_disk = folders.iter().any(|folder| {
            self.is_path_under_active_transfer(folder)
                || self.fresh_disk_size_dirs.keys().any(|fresh| {
                    is_same_or_descendant_path(folder, fresh)
                        || is_same_or_descendant_path(fresh, folder)
                })
        });

        let cancel = Arc::new(AtomicBool::new(false));
        self.folder_size_cancels
            .insert((tab_id, side), Arc::clone(&cancel));
        let use_mft = self.settings.use_mft_service && !force_disk;
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
        self.folder_size_cache.retain(|k, _| {
            !is_same_or_descendant_path(k, dir) && !is_same_or_descendant_path(dir, k)
        });
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
        if let Some(parsed) = parse_network_uri(raw) {
            if parsed.protocol == NetworkProtocol::Smb {
                self.navigate_pane(side, Location::local(parsed.host), window, cx);
                return;
            }
            if let Some(existing) = self
                .settings
                .network_connections
                .iter()
                .find(|c| {
                    c.protocol == parsed.protocol
                        && c.host.eq_ignore_ascii_case(&parsed.host)
                        && c.port == parsed.port
                        && (parsed.username.is_empty()
                            || c.username.eq_ignore_ascii_case(&parsed.username))
                })
                .cloned()
            {
                let loc = Location::remote(
                    existing.id,
                    existing.display_name(),
                    existing.protocol,
                    parsed.remote_path,
                );
                self.navigate_pane(side, loc, window, cx);
                return;
            }
            self.open_network_connect_modal(None, Some(parsed), window, cx);
            return;
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

    /// Returns the clamped `(modified, type, size)` column widths for Details view.
    pub fn details_column_widths(&self) -> (f32, f32, f32) {
        (
            f32::from(self.settings.details_col_modified).clamp(96.0, 360.0),
            f32::from(self.settings.details_col_type).clamp(76.0, 360.0),
            f32::from(self.settings.details_col_size).clamp(68.0, 260.0),
        )
    }

    /// Starts drag-resizing a Details view column divider (double-click resets all widths).
    pub fn begin_column_resize(
        &mut self,
        divider: ColumnDivider,
        ev: &MouseDownEvent,
        cx: &mut Context<Self>,
    ) {
        if ev.click_count >= 2 {
            self.column_resize = None;
            self.settings.details_col_modified = 164;
            self.settings.details_col_type = 144;
            self.settings.details_col_size = 106;
            let _ = self.settings.save();
            cx.notify();
            return;
        }
        let (start_modified, start_type, start_size) = self.details_column_widths();
        self.column_resize = Some(ColumnResizeState {
            divider,
            start_x: ev.position.x.into(),
            start_modified,
            start_type,
            start_size,
        });
        cx.notify();
    }

    /// Tracks rubber-band drag motion, Details column resize drag, and outbound native OLE drag handoff.
    fn handle_mouse_move(
        &mut self,
        ev: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // When dragging local files out of the TExplorer window edge, hand off cleanly to
        // native Windows OLE Shell drag-and-drop so Chrome, Discord, Explorer, and other apps
        // accept the dropped files with native icons and cursors.
        if ev.dragging() && cx.has_active_drag() && !self.external_ole_drag_active {
            let vp = window.viewport_size();
            let x: f32 = ev.position.x.into();
            let y: f32 = ev.position.y.into();
            let vw: f32 = vp.width.into();
            let vh: f32 = vp.height.into();
            let left_window = x <= 1.0 || y <= 1.0 || x >= vw - 1.0 || y >= vh - 1.0;
            if left_window
                && let Some(paths) = self.active_local_drag_paths.borrow_mut().take()
                && !paths.is_empty()
            {
                cx.stop_active_drag(window);
                let hwnd_raw = HasWindowHandle::window_handle(window)
                    .ok()
                    .and_then(|wh| match wh.as_raw() {
                        RawWindowHandle::Win32(win32) => Some(win32.hwnd.get()),
                        _ => None,
                    })
                    .unwrap_or(0);
                self.external_ole_drag_active = true;
                cx.spawn(async move |weak, cx| {
                    let outcome = start_native_file_drag(hwnd_raw, &paths);
                    let _ = weak.update(cx, |this, cx| {
                        this.external_ole_drag_active = false;
                        if matches!(outcome, Ok(NativeDragOutcome::Moved)) {
                            this.refresh_visible_panes(cx);
                        }
                        cx.notify();
                    });
                })
                .detach();
                return;
            }
        } else if !ev.dragging() && !cx.has_active_drag() {
            *self.active_local_drag_paths.borrow_mut() = None;
        }
        if self
            .video_playback
            .as_ref()
            .is_some_and(|v| v.dragging_volume)
        {
            if !ev.dragging() {
                if let Some(video) = self.video_playback.as_mut() {
                    video.dragging_volume = false;
                }
                let _ = self.settings.save();
                cx.notify();
                return;
            }
            self.video_set_volume_from_position(ev.position.x, false, cx);
            return;
        }

        if self.video_playback.as_ref().is_some_and(|v| v.scrubbing) {
            if !ev.dragging() {
                if let Some(video) = self.video_playback.as_mut() {
                    video.scrubbing = false;
                }
                cx.notify();
                return;
            }
            self.video_seek_to_position(ev.position.x, false, cx);
            return;
        }

        if let Some(resize) = self.column_resize.clone() {
            if !ev.dragging() {
                self.finish_column_resize(cx);
                return;
            }
            let curr_x: f32 = ev.position.x.into();
            let dx = curr_x - resize.start_x;
            match resize.divider {
                ColumnDivider::NameModified => {
                    let next_mod = (resize.start_modified - dx).clamp(96.0, 360.0);
                    self.settings.details_col_modified = next_mod.round() as u16;
                }
                ColumnDivider::ModifiedType => {
                    let min_delta = -(resize.start_modified - 96.0);
                    let max_delta = (resize.start_type - 76.0).min(360.0 - resize.start_modified);
                    let clamped = dx.clamp(min_delta, max_delta.max(min_delta));
                    self.settings.details_col_modified =
                        (resize.start_modified + clamped).clamp(96.0, 360.0).round() as u16;
                    self.settings.details_col_type =
                        (resize.start_type - clamped).clamp(76.0, 360.0).round() as u16;
                }
                ColumnDivider::TypeSize => {
                    let min_delta = -(resize.start_type - 76.0);
                    let max_delta = (resize.start_size - 68.0).min(360.0 - resize.start_type);
                    let clamped = dx.clamp(min_delta, max_delta.max(min_delta));
                    self.settings.details_col_type =
                        (resize.start_type + clamped).clamp(76.0, 360.0).round() as u16;
                    self.settings.details_col_size =
                        (resize.start_size - clamped).clamp(68.0, 260.0).round() as u16;
                }
            }
            cx.notify();
            return;
        }
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

        let (bounds_x, bounds_y, bounds_w, bounds_h, mut scroll_y) = {
            let state = tab.pane_ui(side).scroll_handle.0.borrow();
            let b = state.base_handle.bounds();
            let bx: f32 = b.origin.x.into();
            let by: f32 = b.origin.y.into();
            let bw: f32 = b.size.width.into();
            let bh: f32 = b.size.height.into();
            let sy: f32 = state.base_handle.offset().y.into();
            (bx, by, bw, bh, sy)
        };

        // Note: `UniformListScrollState::last_item_size.item` is the viewport size, NOT a single
        // row's height. Use the exact fixed `px(...)` row heights rendered by `views.rs`.
        let row_h = match view_mode {
            ViewMode::Details => 32.0,
            ViewMode::Compact => 28.0,
            ViewMode::Grid => 136.0,
        };

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
        *self.active_local_drag_paths.borrow_mut() = None;
        let mut save_video_prefs = false;
        if let Some(video) = self.video_playback.as_mut() {
            if video.scrubbing {
                video.scrubbing = false;
                cx.notify();
            }
            if video.dragging_volume {
                video.dragging_volume = false;
                save_video_prefs = true;
                cx.notify();
            }
        }
        if save_video_prefs {
            let _ = self.settings.save();
        }
        self.finish_column_resize(cx);
        self.finish_marquee_selection(cx);
    }

    fn finish_column_resize(&mut self, cx: &mut Context<Self>) {
        if self.column_resize.take().is_some() {
            let _ = self.settings.save();
            cx.notify();
        }
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
        ev: &MouseDownEvent,
        cx: &mut Context<Self>,
    ) {
        let loc_opt = self
            .active_tab()
            .map(|t| t.model.pane(side).location.clone());
        match loc_opt {
            Some(Location::Local(directory)) => {
                self.set_active_pane(side, cx);
                if ev.modifiers.shift {
                    self.context_menu = None;
                    self.show_native_windows_bg_menu(directory, cx);
                    return;
                }
                self.open_context_menu(
                    ev.position,
                    ContextMenuTarget::PaneBackground { side, directory },
                    cx,
                );
            }
            Some(Location::Remote {
                connection_id,
                remote_path,
                ..
            }) => {
                self.set_active_pane(side, cx);
                self.open_context_menu(
                    ev.position,
                    ContextMenuTarget::RemotePaneBackground {
                        side,
                        connection_id,
                        remote_path,
                    },
                    cx,
                );
            }
            _ => {}
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
        } else if let Location::Remote {
            connection_id,
            remote_path,
            ..
        } = location
        {
            self.open_remote_file_cached(connection_id, remote_path, window, cx);
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

    pub fn handle_dropped_paths_into_pane(
        &mut self,
        side: PaneSide,
        sources: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.handle_dropped_paths_into_pane_with_cleanup(side, sources, None, window, cx);
    }

    fn handle_dropped_paths_into_pane_with_cleanup(
        &mut self,
        side: PaneSide,
        sources: Vec<PathBuf>,
        stage_dir: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if sources.is_empty() {
            Self::cleanup_stage_dir_async(stage_dir);
            return;
        }
        let Some(Location::Remote {
            connection_id,
            remote_path,
            ..
        }) = self.active_tab().map(|t| t.model.pane(side).location.clone())
        else {
            Self::cleanup_stage_dir_async(stage_dir);
            return;
        };
        let Some(conn) = self.settings.network_connection(connection_id).cloned() else {
            Self::cleanup_stage_dir_async(stage_dir);
            return;
        };
        let files: Vec<PathBuf> = sources.into_iter().filter(|p| p.is_file()).collect();
        if files.is_empty() {
            Self::cleanup_stage_dir_async(stage_dir);
            self.show_toast(
                "Drop files to upload to the remote server.",
                ToastKind::Info,
                window,
                cx,
            );
            return;
        }
        let count = files.len();
        self.show_toast(
            format!("Uploading {count} file(s) to {}...", conn.display_name()),
            ToastKind::Info,
            window,
            cx,
        );
        let creds = self.resolve_remote_credentials(connection_id);
        let pool = self.remote_pool.clone();
        let tx = self.bg_tx.clone();
        tokio_runtime().spawn(async move {
            let mut uploaded = 0usize;
            let mut first_err: Option<String> = None;
            for src in files {
                let Some(name) = src.file_name().map(|n| n.to_string_lossy().into_owned()) else {
                    continue;
                };
                let target_remote = join_remote_path(&remote_path, &name);
                match pool.upload_file(&conn, &creds, &src, &target_remote).await {
                    Ok(_) => uploaded += 1,
                    Err(err) if first_err.is_none() => first_err = Some(err),
                    Err(_) => {}
                }
            }
            if let Some(dir) = stage_dir {
                let _ = tokio::task::spawn_blocking(move || {
                    let _ = std::fs::remove_dir_all(&dir);
                })
                .await;
            }
            let result = if let Some(err) = first_err {
                Err(err)
            } else {
                Ok(Some(format!("Uploaded {uploaded} file(s)")))
            };
            let _ = tx.unbounded_send(BgEvent::RemoteOpFinished {
                result,
                refresh_side: Some(side),
            });
        });
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
        self.enqueue_transfer_with_cleanup(kind, sources, dest_dir, None, cx);
    }

    fn enqueue_transfer_with_cleanup(
        &mut self,
        kind: TransferKind,
        sources: Vec<PathBuf>,
        dest_dir: PathBuf,
        cleanup_stage_dir: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        let sources = filter_effective_transfer_sources(kind, sources, &dest_dir, false);
        if sources.is_empty() {
            Self::cleanup_stage_dir_async(cleanup_stage_dir);
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
            sources,
            cleanup_stage_dir,
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

    pub fn show_native_windows_bg_menu(&mut self, directory: PathBuf, _cx: &mut Context<Self>) {
        let (x, y) = cursor_screen_pos();
        let tx = self.bg_tx.clone();
        self.shell_worker
            .show_native_background_context_menu(directory, x, y, move |res| {
                let _ = tx.unbounded_send(BgEvent::NativeShellMenuDone(res));
            });
    }

    fn stop_video_playback(&mut self) {
        self.video_generation = self.video_generation.wrapping_add(1);
        if let Some(mut video) = self.video_playback.take() {
            video.handle.stop();
            if let Some(frame) = video.frame.take() {
                self.stale_gpu_images.push(frame);
            }
        }
    }

    fn sync_video_playback_for_quicklook(&mut self) {
        if !self.quicklook_open {
            self.stop_video_playback();
            return;
        }
        let target_video = self
            .active_tab()
            .and_then(|t| t.model.active_pane_ref().primary_selected_entry())
            .filter(|e| e.kind == FileKind::Video)
            .and_then(|e| e.location.as_local_path())
            .map(|p| p.to_path_buf());

        let Some(path) = target_video else {
            self.stop_video_playback();
            return;
        };

        if self
            .video_playback
            .as_ref()
            .is_some_and(|v| v.path == path)
        {
            return;
        }

        self.stop_video_playback();
        let generation = self.video_generation;
        let initial_vol = self.settings.quicklook_video_volume.min(100);
        let initial_muted = self.settings.quicklook_video_muted || initial_vol == 0;
        let options = VideoStartOptions {
            initial_muted,
            initial_volume: initial_vol as f64 / 100.0,
            loop_playback: true,
        };
        let tx = self.bg_tx.clone();
        match spawn_video_player(path.clone(), options, move |notify| {
            let _ = tx.unbounded_send(BgEvent::VideoNotify { generation, notify });
        }) {
            Ok(handle) => {
                self.video_playback = Some(VideoPlaybackState {
                    generation,
                    path,
                    handle,
                    frame: None,
                    position_secs: 0.0,
                    duration_secs: 0.0,
                    playing: false,
                    muted: initial_muted,
                    volume: initial_vol,
                    native_width: 0,
                    native_height: 0,
                    ready: false,
                    error: None,
                    scrubbing: false,
                    dragging_volume: false,
                    seek_bar_bounds: Rc::new(Cell::new(None)),
                    volume_bar_bounds: Rc::new(Cell::new(None)),
                });
            }
            Err(err) => {
                let _ = err;
            }
        }
    }

    pub fn video_toggle_play(&mut self, cx: &mut Context<Self>) {
        if let Some(video) = self.video_playback.as_mut() {
            video.handle.toggle_play();
            cx.notify();
        }
    }

    pub fn video_toggle_mute(&mut self, cx: &mut Context<Self>) {
        let Some(video) = self.video_playback.as_mut() else {
            return;
        };
        if video.muted {
            let restored = if video.volume > 0 {
                video.volume.min(100)
            } else if self.settings.quicklook_video_volume > 0 {
                self.settings.quicklook_video_volume.min(100)
            } else {
                70
            };
            video.volume = restored;
            video.muted = false;
            video.handle.set_volume(restored as f64 / 100.0);
            video.handle.set_muted(false);
            self.settings.quicklook_video_volume = restored;
            self.settings.quicklook_video_muted = false;
        } else {
            video.muted = true;
            video.handle.set_muted(true);
            self.settings.quicklook_video_muted = true;
        }
        let _ = self.settings.save();
        cx.notify();
    }

    pub fn video_set_volume_percent(
        &mut self,
        percent: u8,
        persist: bool,
        cx: &mut Context<Self>,
    ) {
        let vol = percent.min(100);
        let muted = vol == 0;
        if let Some(video) = self.video_playback.as_mut() {
            video.volume = vol;
            video.muted = muted;
            video.handle.set_volume(vol as f64 / 100.0);
            video.handle.set_muted(muted);
        }
        self.settings.quicklook_video_muted = muted;
        if vol > 0 {
            self.settings.quicklook_video_volume = vol;
        }
        if persist {
            let _ = self.settings.save();
        }
        cx.notify();
    }

    pub fn video_set_volume_from_position(
        &mut self,
        mouse_x: Pixels,
        start_drag: bool,
        cx: &mut Context<Self>,
    ) {
        let bounds = {
            let Some(video) = self.video_playback.as_mut() else {
                return;
            };
            if start_drag {
                video.dragging_volume = true;
            }
            video.volume_bar_bounds.get()
        };
        let Some(bounds) = bounds else {
            return;
        };
        let width: f32 = bounds.size.width.into();
        if width <= 1.0 {
            return;
        }
        let left: f32 = bounds.origin.x.into();
        let curr: f32 = mouse_x.into();
        let frac = ((curr - left) / width).clamp(0.0, 1.0);
        let percent = (frac * 100.0).round() as u8;
        self.video_set_volume_percent(percent, false, cx);
    }

    pub fn video_adjust_volume_step(&mut self, delta: i16, cx: &mut Context<Self>) {
        let Some(video) = self.video_playback.as_ref() else {
            return;
        };
        let effective = if video.muted { 0 } else { video.volume.min(100) };
        let next = (effective as i16 + delta).clamp(0, 100) as u8;
        self.video_set_volume_percent(next, true, cx);
    }

    pub fn video_seek_relative(&mut self, delta_secs: f64, cx: &mut Context<Self>) {
        if let Some(video) = self.video_playback.as_mut() {
            let max_t = video.duration_secs.max(0.0);
            let next = if max_t > 0.0 {
                (video.position_secs + delta_secs).clamp(0.0, max_t)
            } else {
                (video.position_secs + delta_secs).max(0.0)
            };
            video.position_secs = next;
            video.handle.seek(next);
            cx.notify();
        }
    }

    pub fn video_seek_to_position(
        &mut self,
        mouse_x: Pixels,
        start_scrub: bool,
        cx: &mut Context<Self>,
    ) {
        if let Some(video) = self.video_playback.as_mut() {
            if start_scrub {
                video.scrubbing = true;
            }
            let Some(bounds) = video.seek_bar_bounds.get() else {
                return;
            };
            let width: f32 = bounds.size.width.into();
            if width <= 1.0 || video.duration_secs <= 0.0 {
                return;
            }
            let left: f32 = bounds.origin.x.into();
            let curr: f32 = mouse_x.into();
            let frac = ((curr - left) / width).clamp(0.0, 1.0) as f64;
            let target = frac * video.duration_secs;
            video.position_secs = target;
            video.handle.seek(target);
            cx.notify();
        }
    }

    pub fn pdf_step_page(&mut self, delta: i32, cx: &mut Context<Self>) {
        let Some((current, page_count)) = (match &self.preview_payload {
            PreviewPayload::Pdf {
                page_index,
                page_count,
                loading_page,
                ..
            } => Some((loading_page.unwrap_or(*page_index), *page_count)),
            _ => None,
        }) else {
            return;
        };
        if page_count <= 1 {
            return;
        }
        let target = if delta >= 0 {
            current.saturating_add(delta as u32).min(page_count - 1)
        } else {
            current.saturating_sub(delta.unsigned_abs())
        };
        self.pdf_goto_page(target, cx);
    }

    pub fn pdf_goto_page(&mut self, target_page: u32, cx: &mut Context<Self>) {
        let Some(location) = self.preview_location.clone() else {
            return;
        };
        let (page_count, already_showing) = match &self.preview_payload {
            PreviewPayload::Pdf {
                page_index,
                page_count,
                loading_page,
                ..
            } => (
                *page_count,
                *page_index == target_page && loading_page.is_none(),
            ),
            _ => return,
        };
        if page_count == 0 || target_page >= page_count || already_showing {
            return;
        }

        if let Some((cached_img, w, h)) = self.pdf_page_cache.get(&target_page).cloned() {
            self.preview_payload = PreviewPayload::Pdf {
                image: cached_img,
                page_index: target_page,
                page_count,
                page_width: w,
                page_height: h,
                loading_page: None,
            };
            cx.notify();
            return;
        }

        if let PreviewPayload::Pdf {
            ref mut loading_page,
            ..
        } = self.preview_payload
        {
            if *loading_page == Some(target_page) {
                return;
            }
            *loading_page = Some(target_page);
        }
        cx.notify();

        let generation = self.preview_generation;
        let tx = self.bg_tx.clone();
        match location.clone() {
            Location::Local(path) => {
                tokio_runtime().spawn(async move {
                    let result = tokio::task::spawn_blocking(move || {
                        decode_pdf_page_from_path(&path, target_page, PDF_PREVIEW_MAX_DIM)
                    })
                    .await
                    .unwrap_or_else(|e| Err(format!("PDF render task failed: {e}")));
                    let _ = tx.unbounded_send(BgEvent::PdfPageReady {
                        generation,
                        location,
                        page_index: target_page,
                        result,
                    });
                });
            }
            Location::Archive {
                archive_path,
                inner_path,
            } => {
                tokio_runtime().spawn(async move {
                    let result = tokio::task::spawn_blocking(move || {
                        let (bytes, truncated) = texplorer_vfs::archive::read_archive_entry_bytes(
                            &archive_path,
                            &inner_path,
                            16 * 1024 * 1024,
                        )?;
                        if truncated {
                            return Err("Archived PDF exceeds 16 MiB inline limit.".to_string());
                        }
                        decode_pdf_page_from_bytes(&bytes, target_page, PDF_PREVIEW_MAX_DIM)
                    })
                    .await
                    .unwrap_or_else(|e| Err(format!("PDF render task failed: {e}")));
                    let _ = tx.unbounded_send(BgEvent::PdfPageReady {
                        generation,
                        location,
                        page_index: target_page,
                        result,
                    });
                });
            }
            Location::Remote {
                connection_id,
                remote_path,
                ..
            } => {
                let Some(conn) = self.settings.network_connection(connection_id).cloned() else {
                    return;
                };
                let creds = self.resolve_remote_credentials(connection_id);
                let pool = self.remote_pool.clone();
                tokio_runtime().spawn(async move {
                    let result = match pool
                        .read_file_bytes(&conn, &creds, &remote_path, 12 * 1024 * 1024)
                        .await
                    {
                        Ok((bytes, false)) => tokio::task::spawn_blocking(move || {
                            decode_pdf_page_from_bytes(&bytes, target_page, PDF_PREVIEW_MAX_DIM)
                        })
                        .await
                        .unwrap_or_else(|e| Err(format!("PDF render task failed: {e}"))),
                        Ok((_, true)) => Err("Remote PDF exceeds 12 MiB inline limit.".to_string()),
                        Err(err) => Err(err),
                    };
                    let _ = tx.unbounded_send(BgEvent::PdfPageReady {
                        generation,
                        location,
                        page_index: target_page,
                        result,
                    });
                });
            }
            _ => {}
        }
    }

    pub fn pdf_set_zoom(&mut self, zoom: f32, cx: &mut Context<Self>) {
        let clamped = zoom.clamp(1.0, 2.0);
        if (self.pdf_zoom - clamped).abs() > 0.001 {
            self.pdf_zoom = clamped;
            cx.notify();
        }
    }

    pub fn pdf_adjust_zoom(&mut self, delta: f32, cx: &mut Context<Self>) {
        self.pdf_set_zoom(self.pdf_zoom + delta, cx);
    }

    pub fn toggle_quicklook(&mut self, cx: &mut Context<Self>) {
        if self.quicklook_open {
            self.quicklook_open = false;
            self.stop_video_playback();
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
            self.stop_video_playback();
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
        self.stop_video_playback();
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
        self.stop_video_playback();
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
        self.stop_video_playback();
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
        self.stop_video_playback();
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
        match kind {
            TextPromptKind::RemoteNewFolder {
                side,
                connection_id,
                parent_remote_path,
            } => {
                let trimmed = value.trim().trim_matches(['/', '\\']);
                if trimmed.is_empty() {
                    self.show_toast("Folder name cannot be empty.", ToastKind::Error, window, cx);
                    return;
                }
                let Some(conn) = self.settings.network_connection(connection_id).cloned() else {
                    self.close_modal(cx);
                    return;
                };
                let target = join_remote_path(&parent_remote_path, trimmed);
                let creds = self.resolve_remote_credentials(connection_id);
                let pool = self.remote_pool.clone();
                let tx = self.bg_tx.clone();
                let name_owned = trimmed.to_string();
                self.close_modal(cx);
                tokio_runtime().spawn(async move {
                    let res = pool
                        .create_dir(&conn, &creds, &target)
                        .await
                        .map(|()| Some(format!("Created remote folder \"{name_owned}\"")));
                    let _ = tx.unbounded_send(BgEvent::RemoteOpFinished {
                        result: res,
                        refresh_side: Some(side),
                    });
                });
                return;
            }
            TextPromptKind::RemoteRename {
                side,
                connection_id,
                remote_path,
            } => {
                let trimmed = value.trim().trim_matches(['/', '\\']);
                if trimmed.is_empty() {
                    self.show_toast("Name cannot be empty.", ToastKind::Error, window, cx);
                    return;
                }
                let Some(conn) = self.settings.network_connection(connection_id).cloned() else {
                    self.close_modal(cx);
                    return;
                };
                let parent = match remote_path.rsplit_once('/') {
                    Some(("", _)) | None => "/",
                    Some((head, _)) => head,
                };
                let new_remote = join_remote_path(parent, trimmed);
                let creds = self.resolve_remote_credentials(connection_id);
                let pool = self.remote_pool.clone();
                let tx = self.bg_tx.clone();
                let name_owned = trimmed.to_string();
                self.close_modal(cx);
                tokio_runtime().spawn(async move {
                    let res = pool
                        .rename(&conn, &creds, &remote_path, &new_remote)
                        .await
                        .map(|()| Some(format!("Renamed to \"{name_owned}\"")));
                    let _ = tx.unbounded_send(BgEvent::RemoteOpFinished {
                        result: res,
                        refresh_side: Some(side),
                    });
                });
                return;
            }
            _ => {}
        }

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
            TextPromptKind::RemoteNewFolder { .. } | TextPromptKind::RemoteRename { .. } => {
                unreachable!()
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

    pub fn set_win_e_new_window(&mut self, new_window: bool, cx: &mut Context<Self>) {
        if self.settings.win_e_new_window != new_window {
            self.settings.win_e_new_window = new_window;
            let _ = self.settings.save();
            cx.notify();
        }
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

    pub fn reorder_quick_access(
        &mut self,
        from: KnownFolderKind,
        to: KnownFolderKind,
        cx: &mut Context<Self>,
    ) {
        if self
            .settings
            .reorder_quick_access(from, to, &self.known_folders)
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

    pub fn toggle_sidebar_p2p_row(&mut self, cx: &mut Context<Self>) {
        self.settings.show_sidebar_p2p_row = !self.settings.show_sidebar_p2p_row;
        let _ = self.settings.save();
        cx.notify();
    }

    pub fn reorder_pinned_folder(
        &mut self,
        from: PathBuf,
        to: PathBuf,
        cx: &mut Context<Self>,
    ) {
        if self.settings.reorder_pinned_folder(&from, &to) {
            let _ = self.settings.save();
            cx.notify();
        }
    }

    pub fn reorder_sidebar_section(
        &mut self,
        from: SidebarSection,
        to: SidebarSection,
        cx: &mut Context<Self>,
    ) {
        if self.settings.reorder_sidebar_section(from, to) {
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

    // --- Network Connections (SMB, SSH/SFTP, FTP/FTPS) & Encrypted P2P Transfers ---

    pub fn is_network_connected(&self, id: Uuid) -> bool {
        self.remote_pool.is_connected(id)
    }

    fn resolve_remote_credentials(&self, id: Uuid) -> RemoteCredentials {
        let password = self
            .session_passwords
            .get(&id)
            .cloned()
            .or_else(|| load_network_password(&id.to_string()));
        RemoteCredentials { password }
    }

    pub fn open_network_connect_modal(
        &mut self,
        editing: Option<NetworkConnection>,
        prefill: Option<ParsedNetworkUri>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.context_menu = None;
        self.quicklook_open = false;
        self.stop_video_playback();
        self.modal_subs.clear();

        let (editing_id, protocol, auth_mode, init_name, init_host, init_port, init_rpath, init_user, init_key, init_pass) =
            if let Some(conn) = editing {
                let stored_pass = self
                    .session_passwords
                    .get(&conn.id)
                    .cloned()
                    .or_else(|| load_network_password(&conn.id.to_string()))
                    .unwrap_or_default();
                let key_str = conn
                    .private_key_path
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .or_else(|| {
                        detect_default_ssh_key(Some(&conn.host)).map(|p| p.display().to_string())
                    })
                    .unwrap_or_default();
                (
                    Some(conn.id),
                    conn.protocol,
                    conn.auth_mode,
                    conn.name,
                    conn.host,
                    conn.port.to_string(),
                    conn.remote_path,
                    conn.username,
                    key_str,
                    stored_pass,
                )
            } else if let Some(uri) = prefill {
                let name_guess = uri.host.clone();
                let key_str = detect_default_ssh_key(Some(&uri.host))
                    .map(|p| p.display().to_string())
                    .unwrap_or_default();
                (
                    None,
                    uri.protocol,
                    SshAuthMode::Password,
                    name_guess,
                    uri.host,
                    uri.port.to_string(),
                    uri.remote_path,
                    uri.username,
                    key_str,
                    String::new(),
                )
            } else {
                let key_str = detect_default_ssh_key(None)
                    .map(|p| p.display().to_string())
                    .unwrap_or_default();
                (
                    None,
                    NetworkProtocol::Smb,
                    SshAuthMode::Password,
                    String::new(),
                    String::new(),
                    NetworkProtocol::Smb.default_port().to_string(),
                    "/".to_string(),
                    String::new(),
                    key_str,
                    String::new(),
                )
            };

        let name_input = cx.new(|cx| InputState::new(window, cx).placeholder("e.g. Home NAS or Prod VPS"));
        let host_placeholder = if protocol == NetworkProtocol::Smb {
            r"\\192.168.1.50\Shared"
        } else {
            "192.168.1.50 or server.example.com"
        };
        let host_input = cx.new(|cx| InputState::new(window, cx).placeholder(host_placeholder));
        let port_input = cx.new(|cx| InputState::new(window, cx).placeholder("22"));
        let remote_path_input = cx.new(|cx| InputState::new(window, cx).placeholder("/"));
        let username_input = cx.new(|cx| InputState::new(window, cx).placeholder("root / admin"));
        let password_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Password")
                .masked(true)
        });
        let key_path_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Auto-detected from system ~/.ssh")
        });

        name_input.update(cx, |s, cx| s.set_value(init_name, window, cx));
        host_input.update(cx, |s, cx| s.set_value(init_host, window, cx));
        port_input.update(cx, |s, cx| s.set_value(init_port, window, cx));
        remote_path_input.update(cx, |s, cx| s.set_value(init_rpath, window, cx));
        username_input.update(cx, |s, cx| s.set_value(init_user, window, cx));
        password_input.update(cx, |s, cx| s.set_value(init_pass, window, cx));
        key_path_input.update(cx, |s, cx| s.set_value(init_key, window, cx));
        host_input.update(cx, |s, cx| s.focus(window, cx));

        self.active_modal = Some(ActiveModal::NetworkConnect(NetworkConnectModalState {
            editing_id,
            protocol,
            auth_mode,
            name_input,
            host_input,
            port_input,
            remote_path_input,
            username_input,
            password_input,
            key_path_input,
            busy: false,
            status_message: None,
            status_is_error: false,
        }));
        cx.notify();
    }

    pub fn set_network_modal_protocol(
        &mut self,
        protocol: NetworkProtocol,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ActiveModal::NetworkConnect(ref mut form)) = self.active_modal else {
            return;
        };
        let prev_default = form.protocol.default_port().to_string();
        form.protocol = protocol;
        let port_input = form.port_input.clone();
        let current_port = port_input.read(cx).value().trim().to_string();
        if current_port.is_empty() || current_port == prev_default {
            let next_port = protocol.default_port().to_string();
            port_input.update(cx, |s, cx| s.set_value(next_port, window, cx));
        }
        form.status_message = None;
        cx.notify();
    }

    pub fn set_network_modal_auth_mode(
        &mut self,
        mode: SshAuthMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ActiveModal::NetworkConnect(ref mut form)) = self.active_modal else {
            return;
        };
        form.auth_mode = mode;
        form.status_message = None;
        if mode == SshAuthMode::PrivateKey {
            let host = form.host_input.read(cx).value().trim().to_string();
            let key_input = form.key_path_input.clone();
            let current_key = key_input.read(cx).value().trim().to_string();
            let generic_default = detect_default_ssh_key(None)
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            if (current_key.is_empty() || current_key == generic_default)
                && let Some(detected) =
                    detect_default_ssh_key((!host.is_empty()).then_some(host.as_str()))
            {
                let detected_str = detected.display().to_string();
                key_input.update(cx, |s, cx| s.set_value(detected_str, window, cx));
            }
        }
        cx.notify();
    }

    pub fn network_pick_ssh_key_file(&mut self, _cx: &mut Context<Self>) {
        let ssh_dir = dirs::home_dir()
            .map(|h| h.join(".ssh"))
            .filter(|p| p.is_dir())
            .or_else(dirs::home_dir);
        let tx = self.bg_tx.clone();
        tokio_runtime().spawn(async move {
            let res = tokio::task::spawn_blocking(move || {
                pick_files_dialog("Select SSH Private Key File", ssh_dir.as_deref())
            })
            .await
            .unwrap_or_else(|e| Err(format!("SSH key picker failed: {e}")));
            let _ = tx.unbounded_send(BgEvent::NetworkPickedKeyPath(res));
        });
    }

    fn build_network_conn_from_modal(
        &self,
        cx: &Context<Self>,
    ) -> Result<(NetworkConnection, String), String> {
        let Some(ActiveModal::NetworkConnect(ref form)) = self.active_modal else {
            return Err("No active network form.".to_string());
        };
        let raw_host = form.host_input.read(cx).value().trim().to_string();
        if raw_host.is_empty() {
            return Err(if form.protocol == NetworkProtocol::Smb {
                "Enter a shared folder path (e.g. \\\\server\\share).".to_string()
            } else {
                "Enter a host name or IP address.".to_string()
            });
        }

        let host = if form.protocol == NetworkProtocol::Smb {
            normalize_smb_share_path(&raw_host)
                .ok_or_else(|| "Invalid SMB path (expected \\\\server\\share).".to_string())?
        } else {
            raw_host
        };

        let port = form
            .port_input
            .read(cx)
            .value()
            .trim()
            .parse::<u16>()
            .unwrap_or_else(|_| form.protocol.default_port());
        let name = form.name_input.read(cx).value().trim().to_string();
        let username = form.username_input.read(cx).value().trim().to_string();
        let password = form.password_input.read(cx).value().to_string();
        let remote_path = normalize_remote_path(form.remote_path_input.read(cx).value().trim());
        let key_str = form.key_path_input.read(cx).value().trim().to_string();
        let private_key_path = if form.protocol == NetworkProtocol::Sftp
            && form.auth_mode == SshAuthMode::PrivateKey
            && !key_str.is_empty()
        {
            Some(PathBuf::from(key_str))
        } else {
            None
        };

        let conn = NetworkConnection {
            id: form.editing_id.unwrap_or_else(Uuid::new_v4),
            name,
            protocol: form.protocol,
            host,
            port,
            username,
            remote_path,
            auth_mode: form.auth_mode,
            private_key_path,
            save_password: true,
        };
        Ok((conn, password))
    }

    pub fn test_network_modal_connection(&mut self, cx: &mut Context<Self>) {
        let (conn, password) = match self.build_network_conn_from_modal(cx) {
            Ok(v) => v,
            Err(err) => {
                if let Some(ActiveModal::NetworkConnect(ref mut form)) = self.active_modal {
                    form.status_message = Some(err);
                    form.status_is_error = true;
                }
                cx.notify();
                return;
            }
        };
        if let Some(ActiveModal::NetworkConnect(ref mut form)) = self.active_modal {
            form.busy = true;
            form.status_message = Some("Testing connection...".to_string());
            form.status_is_error = false;
        }
        cx.notify();

        let tx = self.bg_tx.clone();
        let pool = self.remote_pool.clone();
        tokio_runtime().spawn(async move {
            let result = match conn.protocol {
                NetworkProtocol::Smb => {
                    let unc = conn.host.clone();
                    let user = conn.username.clone();
                    let pass = password.clone();
                    tokio::task::spawn_blocking(move || {
                        let connected = connect_smb_share(
                            &unc,
                            (!user.is_empty()).then_some(user.as_str()),
                            (!pass.is_empty()).then_some(pass.as_str()),
                        )?;
                        let count = std::fs::read_dir(&connected)
                            .map(|rd| rd.count())
                            .map_err(|e| format!("Share connected, but folder read failed: {e}"))?;
                        Ok(format!("Connected to {connected} ({count} items)"))
                    })
                    .await
                    .unwrap_or_else(|e| Err(format!("SMB test failed: {e}")))
                }
                _ => {
                    let creds = RemoteCredentials {
                        password: (!password.is_empty()).then_some(password),
                    };
                    pool.test_connection(&conn, &creds).await
                }
            };
            let _ = tx.unbounded_send(BgEvent::NetworkTestFinished { result });
        });
    }

    pub fn save_and_connect_network_modal(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (conn, password) = match self.build_network_conn_from_modal(cx) {
            Ok(v) => v,
            Err(err) => {
                if let Some(ActiveModal::NetworkConnect(ref mut form)) = self.active_modal {
                    form.status_message = Some(err);
                    form.status_is_error = true;
                }
                cx.notify();
                return;
            }
        };

        if !password.is_empty() {
            self.session_passwords.insert(conn.id, password.clone());
            if conn.save_password {
                if let Err(err) =
                    save_network_password(&conn.id.to_string(), &conn.username, &password)
                {
                    self.show_toast(err, ToastKind::Error, window, cx);
                }
            } else {
                let _ = delete_network_password(&conn.id.to_string());
            }
        } else if !conn.save_password {
            self.session_passwords.remove(&conn.id);
            let _ = delete_network_password(&conn.id.to_string());
        }

        self.settings.upsert_network_connection(conn.clone());
        let _ = self.settings.save();
        self.close_modal(cx);
        self.open_network_connection(conn, false, window, cx);
    }

    pub fn open_network_connection(
        &mut self,
        conn: NetworkConnection,
        new_tab: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_network_connection_target(conn, new_tab, false, window, cx);
    }

    pub fn open_network_connection_in_split(
        &mut self,
        conn: NetworkConnection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_network_connection_target(conn, false, true, window, cx);
    }

    fn open_network_connection_target(
        &mut self,
        conn: NetworkConnection,
        new_tab: bool,
        in_split: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match conn.protocol {
            NetworkProtocol::Smb => {
                let unc = conn.host.clone();
                let user = conn.username.clone();
                let pass = self
                    .resolve_remote_credentials(conn.id)
                    .password
                    .unwrap_or_default();
                let tx = self.bg_tx.clone();
                tokio_runtime().spawn(async move {
                    let unc_for_event = unc.clone();
                    let result = tokio::task::spawn_blocking(move || {
                        if user.is_empty() && pass.is_empty() && Path::new(&unc).is_dir() {
                            return Ok(unc);
                        }
                        connect_smb_share(
                            &unc,
                            (!user.is_empty()).then_some(user.as_str()),
                            (!pass.is_empty()).then_some(pass.as_str()),
                        )
                    })
                    .await
                    .unwrap_or_else(|e| Err(format!("SMB connection worker failed: {e}")));
                    let _ = tx.unbounded_send(BgEvent::SmbConnectFinished {
                        unc: unc_for_event,
                        new_tab,
                        in_split,
                        result,
                    });
                });
            }
            NetworkProtocol::Sftp | NetworkProtocol::Ftp | NetworkProtocol::Ftps => {
                let target = Location::remote(
                    conn.id,
                    conn.display_name(),
                    conn.protocol,
                    conn.normalized_remote_path(),
                );
                if in_split {
                    self.open_in_opposite_pane(target, window, cx);
                } else if new_tab && self.settings.show_tabs {
                    self.open_new_tab(target, window, cx);
                } else {
                    self.navigate_active_pane(target, window, cx);
                }
            }
        }
    }

    pub fn open_ssh_terminal(&mut self, conn: &NetworkConnection, cx: &mut Context<Self>) {
        let host = conn.host.trim();
        if host.is_empty() {
            return;
        }
        let target = if conn.username.trim().is_empty() {
            host.to_string()
        } else {
            format!("{}@{host}", conn.username.trim())
        };
        let port = if conn.port == 0 { 22 } else { conn.port };
        let term = self.settings.terminal_command.clone();
        let key_arg = conn
            .private_key_path
            .as_ref()
            .filter(|_| conn.auth_mode == SshAuthMode::PrivateKey)
            .map(|p| p.display().to_string());
        let tx = self.bg_tx.clone();
        tokio_runtime().spawn(async move {
            let result = tokio::task::spawn_blocking(move || {
                let mut ssh_args = vec!["ssh".to_string(), target, "-p".to_string(), port.to_string()];
                if let Some(k) = key_arg {
                    ssh_args.push("-i".to_string());
                    ssh_args.push(k);
                }
                let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
                if is_elevated() {
                    let arg_line = ssh_args
                        .iter()
                        .map(|a| {
                            if a.contains(' ') {
                                format!("\"{}\"", a.replace('"', "\\\""))
                            } else {
                                a.clone()
                            }
                        })
                        .collect::<Vec<_>>()
                        .join(" ");
                    let (exe, full_args) = if term.to_ascii_lowercase().contains("wt") {
                        (term.as_str(), arg_line)
                    } else {
                        ("cmd.exe", format!("/K {arg_line}"))
                    };
                    if spawn_as_desktop_user(exe, &full_args, &home).is_ok() {
                        return Ok(None);
                    }
                }
                let spawn_res = if term.to_ascii_lowercase().contains("wt") {
                    std::process::Command::new(&term).args(&ssh_args).spawn()
                } else {
                    std::process::Command::new("cmd.exe")
                        .arg("/K")
                        .args(&ssh_args)
                        .spawn()
                };
                spawn_res
                    .map(|_| None)
                    .map_err(|e| format!("Failed launching SSH terminal: {e}"))
            })
            .await
            .unwrap_or_else(|e| Err(format!("SSH terminal launch failed: {e}")));
            let _ = tx.unbounded_send(BgEvent::ShellOpFinished {
                result,
                refresh_side: None,
            });
        });
        self.set_toast_simple("Opening SSH terminal...", ToastKind::Info, cx);
    }

    pub fn disconnect_network_connection(
        &mut self,
        conn: NetworkConnection,
        cx: &mut Context<Self>,
    ) {
        let id = conn.id;
        let name = conn.display_name();
        let pool = self.remote_pool.clone();
        let tx = self.bg_tx.clone();
        if conn.protocol == NetworkProtocol::Smb {
            let unc = conn.host.clone();
            tokio_runtime().spawn(async move {
                let res = tokio::task::spawn_blocking(move || {
                    disconnect_smb_share(&unc).map(|()| Some(format!("Disconnected {name}")))
                })
                .await
                .unwrap_or_else(|e| Err(format!("Disconnect failed: {e}")));
                let _ = tx.unbounded_send(BgEvent::RemoteOpFinished {
                    result: res,
                    refresh_side: None,
                });
            });
        } else {
            tokio_runtime().spawn(async move {
                pool.disconnect(id).await;
                let _ = tx.unbounded_send(BgEvent::RemoteOpFinished {
                    result: Ok(Some(format!("Disconnected {name}"))),
                    refresh_side: None,
                });
            });
        }
        cx.notify();
    }

    pub fn remove_network_connection(&mut self, id: Uuid, cx: &mut Context<Self>) {
        if self.settings.remove_network_connection(id) {
            let _ = self.settings.save();
            self.session_passwords.remove(&id);
            let _ = delete_network_password(&id.to_string());
            let pool = self.remote_pool.clone();
            tokio_runtime().spawn(async move {
                pool.disconnect(id).await;
            });
            self.set_toast_simple("Removed network connection", ToastKind::Info, cx);
        }
    }

    fn open_remote_file_cached(
        &mut self,
        connection_id: Uuid,
        remote_path: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(conn) = self.settings.network_connection(connection_id).cloned() else {
            return;
        };
        let filename = remote_path
            .rsplit('/')
            .next()
            .filter(|s| !s.is_empty())
            .unwrap_or("remote_file")
            .to_string();
        let cache_dir = dirs::cache_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("TExplorer")
            .join("remote-cache")
            .join(connection_id.to_string());
        let local_dest = cache_dir.join(&filename);
        self.show_toast(
            format!("Downloading {filename}..."),
            ToastKind::Info,
            window,
            cx,
        );
        let creds = self.resolve_remote_credentials(connection_id);
        let pool = self.remote_pool.clone();
        let tx = self.bg_tx.clone();
        tokio_runtime().spawn(async move {
            let result = pool
                .download_file(&conn, &creds, &remote_path, &local_dest)
                .await
                .map(|_| local_dest);
            let _ = tx.unbounded_send(BgEvent::RemoteFileOpened { result });
        });
    }

    pub fn download_remote_file_to_local(
        &mut self,
        side: PaneSide,
        connection_id: Uuid,
        remote_path: String,
        filename: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(conn) = self.settings.network_connection(connection_id).cloned() else {
            return;
        };
        let dest_dir = self
            .active_tab()
            .filter(|t| t.model.split_enabled)
            .and_then(|t| {
                t.model
                    .pane(side.opposite())
                    .location
                    .as_local_path()
                    .map(|p| p.to_path_buf())
            })
            .or_else(dirs::download_dir)
            .unwrap_or_else(std::env::temp_dir);
        let dest_file = unique_keep_both_path(&dest_dir.join(&filename));
        let refresh_opposite = self
            .active_tab()
            .filter(|t| t.model.split_enabled)
            .and_then(|t| {
                t.model
                    .pane(side.opposite())
                    .location
                    .as_local_path()
                    .map(|_| side.opposite())
            });
        self.show_toast(
            format!("Downloading {filename} to {}...", dest_dir.display()),
            ToastKind::Info,
            window,
            cx,
        );
        let creds = self.resolve_remote_credentials(connection_id);
        let pool = self.remote_pool.clone();
        let tx = self.bg_tx.clone();
        tokio_runtime().spawn(async move {
            let result = pool
                .download_file(&conn, &creds, &remote_path, &dest_file)
                .await
                .map(|_| Some(format!("Downloaded {filename}")));
            let _ = tx.unbounded_send(BgEvent::RemoteOpFinished {
                result,
                refresh_side: refresh_opposite,
            });
        });
    }

    pub fn open_remote_new_folder_prompt(
        &mut self,
        side: PaneSide,
        connection_id: Uuid,
        parent_remote_path: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_text_prompt(
            "New Remote Folder",
            "Create",
            "new-folder",
            TextPromptKind::RemoteNewFolder {
                side,
                connection_id,
                parent_remote_path,
            },
            window,
            cx,
        );
    }

    pub fn open_remote_rename_prompt(
        &mut self,
        side: PaneSide,
        connection_id: Uuid,
        remote_path: String,
        current_name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_text_prompt(
            "Rename Remote Item",
            "Rename",
            &current_name,
            TextPromptKind::RemoteRename {
                side,
                connection_id,
                remote_path,
            },
            window,
            cx,
        );
    }

    pub fn delete_remote_entry(
        &mut self,
        side: PaneSide,
        connection_id: Uuid,
        remote_path: String,
        is_dir: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(conn) = self.settings.network_connection(connection_id).cloned() else {
            return;
        };
        let creds = self.resolve_remote_credentials(connection_id);
        let pool = self.remote_pool.clone();
        let tx = self.bg_tx.clone();
        let name = remote_path
            .rsplit('/')
            .next()
            .unwrap_or(&remote_path)
            .to_string();
        tokio_runtime().spawn(async move {
            let result = pool
                .delete_entry(&conn, &creds, &remote_path, is_dir)
                .await
                .map(|()| Some(format!("Deleted remote item \"{name}\"")));
            let _ = tx.unbounded_send(BgEvent::RemoteOpFinished {
                result,
                refresh_side: Some(side),
            });
        });
        cx.notify();
    }

    fn default_p2p_dest_dir(&self) -> PathBuf {
        self.active_tab()
            .and_then(|t| t.model.active_pane_ref().location.as_local_path())
            .filter(|p| p.is_dir())
            .map(|p| p.to_path_buf())
            .or_else(dirs::download_dir)
            .unwrap_or_else(std::env::temp_dir)
    }

    pub fn open_p2p_modal_from_sidebar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let side = self
            .active_tab()
            .map(|t| t.model.active_pane)
            .unwrap_or(PaneSide::Left);
        let selected = self.selected_local_paths(side);
        let tab = if selected.is_empty() && self.active_p2p_share.is_none() {
            P2pModalTab::Receive
        } else {
            P2pModalTab::Send
        };
        self.open_p2p_modal(tab, selected, window, cx);
    }

    pub fn open_p2p_send_modal(
        &mut self,
        paths: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_p2p_modal(P2pModalTab::Send, paths, window, cx);
    }

    pub fn open_p2p_send_to_peer(
        &mut self,
        peer: NearbyPeer,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let side = self
            .active_tab()
            .map(|t| t.model.active_pane)
            .unwrap_or(PaneSide::Left);
        let selected = self.selected_local_paths(side);
        if selected.is_empty() {
            self.open_p2p_modal(P2pModalTab::Send, Vec::new(), window, cx);
            self.show_toast(
                format!(
                    "Select files in a folder first to send directly to {}.",
                    peer.device_name
                ),
                ToastKind::Info,
                window,
                cx,
            );
            return;
        }
        let svc = self.p2p_service.clone();
        let peer_name = peer.device_name.clone();
        self.show_toast(
            format!("Sending P2P request to {peer_name}..."),
            ToastKind::Info,
            window,
            cx,
        );
        let tx = self.bg_tx.clone();
        tokio_runtime().spawn(async move {
            if let Err(err) = svc.send_to_peer(peer, selected).await {
                let _ = tx.unbounded_send(BgEvent::P2p(P2pEvent::EndpointError(err)));
            }
        });
    }

    fn open_p2p_modal(
        &mut self,
        tab: P2pModalTab,
        selected_paths: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.context_menu = None;
        self.quicklook_open = false;
        self.stop_video_playback();
        self.modal_subs.clear();

        let dest_dir = self.default_p2p_dest_dir();
        let ticket_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Paste texp1-... share code here"));
        let sub = cx.subscribe_in(
            &ticket_input,
            window,
            |this, state, ev: &InputEvent, _window, cx| {
                if matches!(ev, InputEvent::Change)
                    && let Some(ActiveModal::P2p(ref mut m)) = this.active_modal
                {
                    m.ticket_text = state.read(cx).value().to_string();
                    m.error = None;
                    cx.notify();
                }
            },
        );
        self.modal_subs.push(sub);
        if tab == P2pModalTab::Receive {
            ticket_input.update(cx, |s, cx| s.focus(window, cx));
        }

        self.active_modal = Some(ActiveModal::P2p(P2pModalState {
            tab,
            selected_paths,
            ticket_input,
            ticket_text: String::new(),
            dest_dir,
            generating_share: false,
            error: None,
        }));
        cx.notify();
    }

    pub fn set_p2p_modal_tab(
        &mut self,
        tab: P2pModalTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(ActiveModal::P2p(ref mut m)) = self.active_modal {
            m.tab = tab;
            m.error = None;
            if tab == P2pModalTab::Receive {
                let input = m.ticket_input.clone();
                input.update(cx, |s, cx| s.focus(window, cx));
            }
            cx.notify();
        }
    }

    pub fn p2p_pick_send_files(&mut self, _cx: &mut Context<Self>) {
        let initial = self.default_p2p_dest_dir();
        let tx = self.bg_tx.clone();
        tokio_runtime().spawn(async move {
            let res = tokio::task::spawn_blocking(move || {
                pick_files_dialog("Select files to send via P2P", Some(&initial))
            })
            .await
            .unwrap_or_else(|e| Err(format!("File picker task failed: {e}")));
            let _ = tx.unbounded_send(BgEvent::P2pPickedSendPaths(res));
        });
    }

    pub fn p2p_pick_send_folder(&mut self, _cx: &mut Context<Self>) {
        let initial = self.default_p2p_dest_dir();
        let tx = self.bg_tx.clone();
        tokio_runtime().spawn(async move {
            let res = tokio::task::spawn_blocking(move || {
                pick_folder_dialog("Select folder to send via P2P", Some(&initial))
                    .map(|opt| opt.into_iter().collect())
            })
            .await
            .unwrap_or_else(|e| Err(format!("Folder picker task failed: {e}")));
            let _ = tx.unbounded_send(BgEvent::P2pPickedSendPaths(res));
        });
    }

    pub fn p2p_pick_dest_folder(&mut self, _cx: &mut Context<Self>) {
        let initial = match &self.active_modal {
            Some(ActiveModal::P2p(m)) if m.dest_dir.is_dir() => m.dest_dir.clone(),
            _ => self.default_p2p_dest_dir(),
        };
        let tx = self.bg_tx.clone();
        tokio_runtime().spawn(async move {
            let res = tokio::task::spawn_blocking(move || {
                pick_folder_dialog("Choose P2P receive destination folder", Some(&initial))
            })
            .await
            .unwrap_or_else(|e| Err(format!("Folder picker task failed: {e}")));
            let _ = tx.unbounded_send(BgEvent::P2pPickedDestDir(res));
        });
    }

    pub fn generate_p2p_share_code(&mut self, cx: &mut Context<Self>) {
        let paths = match &mut self.active_modal {
            Some(ActiveModal::P2p(m)) if !m.selected_paths.is_empty() => {
                m.generating_share = true;
                m.error = None;
                m.selected_paths.clone()
            }
            _ => return,
        };
        let svc = self.p2p_service.clone();
        let tx = self.bg_tx.clone();
        tokio_runtime().spawn(async move {
            let result = svc.create_share_offer(paths).await;
            let _ = tx.unbounded_send(BgEvent::P2pShareCreated { result });
        });
        cx.notify();
    }

    pub fn stop_active_p2p_share(&mut self, cx: &mut Context<Self>) {
        if let Some(share) = self.active_p2p_share.take() {
            let svc = self.p2p_service.clone();
            tokio_runtime().spawn(async move {
                svc.cancel_share_offer(share.transfer_id).await;
            });
            self.set_toast_simple("Stopped P2P share", ToastKind::Info, cx);
        }
    }

    pub fn send_p2p_to_peer(&mut self, peer: NearbyPeer, cx: &mut Context<Self>) {
        let paths = match &self.active_modal {
            Some(ActiveModal::P2p(m)) if !m.selected_paths.is_empty() => m.selected_paths.clone(),
            _ => return,
        };
        let peer_name = peer.device_name.clone();
        let svc = self.p2p_service.clone();
        let tx = self.bg_tx.clone();
        self.set_toast_simple(
            format!("Waiting for {peer_name} to accept P2P transfer..."),
            ToastKind::Info,
            cx,
        );
        tokio_runtime().spawn(async move {
            if let Err(err) = svc.send_to_peer(peer, paths).await {
                let _ = tx.unbounded_send(BgEvent::P2p(P2pEvent::EndpointError(err)));
            }
        });
    }

    pub fn start_p2p_receive_from_modal(&mut self, cx: &mut Context<Self>) {
        let (ticket_code, dest_dir) = match &self.active_modal {
            Some(ActiveModal::P2p(m)) => (m.ticket_text.trim().to_string(), m.dest_dir.clone()),
            _ => return,
        };
        if let Err(err) = decode_ticket(&ticket_code) {
            if let Some(ActiveModal::P2p(ref mut m)) = self.active_modal {
                m.error = Some(err);
            }
            cx.notify();
            return;
        }
        let svc = self.p2p_service.clone();
        let tx = self.bg_tx.clone();
        self.set_toast_simple("Connecting to P2P sender...", ToastKind::Info, cx);
        tokio_runtime().spawn(async move {
            if let Err(err) = svc.receive_from_ticket(&ticket_code, dest_dir).await {
                let _ = tx.unbounded_send(BgEvent::P2p(P2pEvent::EndpointError(err)));
            }
        });
    }

    pub fn respond_incoming_p2p(
        &mut self,
        transfer_id: Uuid,
        accept: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.incoming_p2p_offers
            .retain(|o| o.transfer_id != transfer_id);
        let dest = if accept {
            let chosen = match &self.active_modal {
                Some(ActiveModal::P2p(m)) => m.dest_dir.clone(),
                _ => self.default_p2p_dest_dir(),
            };
            if !matches!(self.active_modal, Some(ActiveModal::P2p(_))) {
                self.open_p2p_modal(P2pModalTab::Receive, Vec::new(), window, cx);
            }
            Some(chosen)
        } else {
            None
        };
        let svc = self.p2p_service.clone();
        tokio_runtime().spawn(async move {
            svc.respond_to_incoming(transfer_id, dest).await;
        });
        cx.notify();
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
        let bg_tx = self.bg_tx.clone();
        tokio_runtime().spawn(async move {
            tokio::time::sleep(Duration::from_millis(3200)).await;
            let _ = bg_tx.unbounded_send(BgEvent::DismissToast(id));
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

        let Some(side) = self.active_tab().map(|t| t.model.active_pane) else {
            return;
        };

        // Ctrl+Shift+Up / Ctrl+Shift+Down: select all items from current selection to top/bottom.
        if mods.control
            && mods.shift
            && !mods.alt
            && !mods.platform
            && !self.quicklook_open
            && matches!(key, "up" | "down")
        {
            let to_end = key == "down";
            let moved = if let Some(tab) = self.active_tab_mut() {
                let pane = tab.model.pane_mut(side);
                let total = pane.filtered_indices.len();
                if let Some(idx) = pane.selection.select_to_boundary(to_end, total, true) {
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
            return;
        }

        if mods.control || mods.alt || mods.platform {
            return;
        }

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
            "home" | "end" if !self.quicklook_open => {
                let to_end = key == "end";
                let extend = mods.shift;
                let moved = if let Some(tab) = self.active_tab_mut() {
                    let pane = tab.model.pane_mut(side);
                    let total = pane.filtered_indices.len();
                    if let Some(idx) = pane.selection.select_to_boundary(to_end, total, extend) {
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
            "pageup" | "pagedown" => {
                let delta = if key == "pageup" { -1 } else { 1 };
                if self.quicklook_open {
                    if matches!(
                        self.preview_payload,
                        PreviewPayload::Pdf { page_count, .. } if page_count > 1
                    ) {
                        self.pdf_step_page(delta, cx);
                    }
                } else {
                    let extend = mods.shift;
                    let moved = if let Some(tab) = self.active_tab_mut() {
                        let pane = tab.model.pane_mut(side);
                        let total = pane.filtered_indices.len();
                        if let Some(idx) =
                            pane.selection
                                .move_cursor((delta as isize) * 15, total, extend)
                        {
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
                    self.stop_video_playback();
                    self.open_location_or_file(loc, is_dir, window, cx);
                }
            }
            "k" | "p" if self.quicklook_open && self.video_playback.is_some() => {
                self.video_toggle_play(cx);
            }
            "m" if self.quicklook_open && self.video_playback.is_some() => {
                self.video_toggle_mute(cx);
            }
            "left" if self.quicklook_open => {
                if self.video_playback.is_some() {
                    self.video_seek_relative(-5.0, cx);
                } else if matches!(
                    self.preview_payload,
                    PreviewPayload::Pdf { page_count, .. } if page_count > 1
                ) {
                    self.pdf_step_page(-1, cx);
                }
            }
            "right" if self.quicklook_open => {
                if self.video_playback.is_some() {
                    self.video_seek_relative(5.0, cx);
                } else if matches!(
                    self.preview_payload,
                    PreviewPayload::Pdf { page_count, .. } if page_count > 1
                ) {
                    self.pdf_step_page(1, cx);
                }
            }
            "left" if !self.quicklook_open => {
                let can_back = self
                    .active_tab()
                    .is_some_and(|t| t.model.pane(side).can_go_back());
                if can_back {
                    self.go_back(side, window, cx);
                } else {
                    self.go_up(side, window, cx);
                }
            }
            "right" if !self.quicklook_open => {
                let target = self.active_tab().and_then(|t| {
                    let pane = t.model.pane(side);
                    pane.primary_selected_entry().map(|e| {
                        let is_navigable = e.is_dir()
                            || e.location
                                .as_local_path()
                                .is_some_and(is_supported_archive);
                        (e.location.clone(), e.is_dir(), is_navigable)
                    })
                });
                match target {
                    Some((loc, is_dir, true)) => {
                        self.open_location_or_file(loc, is_dir, window, cx);
                    }
                    None => {
                        let can_fwd = self
                            .active_tab()
                            .is_some_and(|t| t.model.pane(side).can_go_forward());
                        if can_fwd {
                            self.go_forward(side, window, cx);
                        }
                    }
                    _ => {}
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
        for stale in self.stale_gpu_images.drain(..) {
            let _ = window.drop_image(stale);
        }
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
                let loc_info = this.active_tab().map(|t| {
                    let side = t.model.active_pane;
                    (side, t.model.pane(side).location.clone())
                });
                match loc_info {
                    Some((side, Location::Local(dir))) => {
                        this.open_new_folder_prompt(side, dir, window, cx);
                    }
                    Some((
                        side,
                        Location::Remote {
                            connection_id,
                            remote_path,
                            ..
                        },
                    )) => {
                        this.open_remote_new_folder_prompt(
                            side,
                            connection_id,
                            remote_path,
                            window,
                            cx,
                        );
                    }
                    _ => {}
                }
            }))
            .on_action(cx.listener(|this, _: &RenameSelected, window, cx| {
                if this.is_modal_blocking() {
                    return;
                }
                let info = this.active_tab().and_then(|t| {
                    let side = t.model.active_pane;
                    let entry = t.model.pane(side).primary_selected_entry()?;
                    Some((side, entry.location.clone(), entry.name.clone()))
                });
                match info {
                    Some((side, Location::Local(path), name)) => {
                        this.open_rename_prompt(side, path, name, window, cx);
                    }
                    Some((
                        side,
                        Location::Remote {
                            connection_id,
                            remote_path,
                            ..
                        },
                        name,
                    )) => {
                        this.open_remote_rename_prompt(
                            side,
                            connection_id,
                            remote_path,
                            name,
                            window,
                            cx,
                        );
                    }
                    _ => {}
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
                let remote_target = this.active_tab().and_then(|t| {
                    let entry = t.model.pane(side).primary_selected_entry()?;
                    let (conn_id, _, rpath) = entry.location.as_remote()?;
                    Some((conn_id, rpath.to_string(), entry.is_dir()))
                });
                if let Some((conn_id, rpath, is_dir)) = remote_target {
                    this.delete_remote_entry(side, conn_id, rpath, is_dir, cx);
                } else {
                    this.trash_selected(side, window, cx);
                }
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
            // Floating P2P incoming confirmations and live P2P transfer progress
            .when(
                !self.incoming_p2p_offers.is_empty()
                    && !matches!(self.active_modal, Some(ActiveModal::P2p(_))),
                |el| el.child(render_p2p_overlays(self, cx)),
            )
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
                        .cursor_pointer()
                        .bg(solid(p.bg_elevated))
                        .border_1()
                        .border_color(solid(p.border))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.toast = None;
                            cx.notify();
                        }))
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

        // Dragging strictly in empty space below all rows must select nothing.
        let empty_hits = compute_marquee_hits(
            ViewMode::Details,
            5,
            4,
            32.0,
            600.0,
            100.0,
            200.0,
            300.0,
            360.0,
        );
        assert!(empty_hits.is_empty());

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
