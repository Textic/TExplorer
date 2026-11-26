use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use futures_channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use futures_util::StreamExt as _;
use gpui_kit::{
    ClipboardItem, Context, Entity, FocusHandle, Focusable, KeyDownEvent, MouseDownEvent, Pixels,
    Point, RenderImage, ScrollStrategy, SharedString, Subscription, UniformListScrollHandle,
    Window,
    component::{
        h_flex,
        input::{InputEvent, InputState},
        resizable::{h_resizable, resizable_panel},
        v_flex,
    },
    div, prelude::*, px,
};
use texplorer_core::{
    AccentPreset, AppSettings, FileEntry, KnownFolder, KnownFolderKind, Location, PaneSide,
    SizeUnit, SortColumn, TabModel, ViewMode, VolumeInfo,
};
use texplorer_ops::{
    ConflictDecision, ConflictPrompt, ConflictResolution, TransferControl, TransferEvent,
    TransferKind, TransferSnapshot, TransferStatus, UndoAction, UndoHistory, spawn_transfer_job,
};
use texplorer_platform::{
    DefaultExplorerStatus, DirectoryWatcher, LaunchIntent, ShellWorker, cursor_screen_pos,
    delete_permanently, enable_default_explorer, find_textree_exe, get_clipboard_files,
    list_known_folders, list_volumes, list_wsl_distros, move_to_trash, open_in_editor, open_item,
    open_terminal_here, query_default_explorer_status, restore_windows_explorer,
    reveal_in_windows_explorer, set_clipboard_files, show_properties_dialog,
};
use uuid::Uuid;

use crate::preview::{
    PreviewPayload, ThumbnailCache, ThumbnailKey, load_preview_blocking, raw_bgra_to_render_image,
};
use crate::theme::{
    BG_BASE, BG_ELEVATED, BORDER, RADIUS_ROW, STATUS_ERROR, STATUS_INFO, STATUS_SUCCESS,
    TEXT_PRIMARY, apply_obsidian_theme, crisp_resize_handle_appearance, solid,
};
use crate::ui::context_menu::{ContextMenuState, ContextMenuTarget, render_context_menu};
use crate::ui::header::render_header;
use crate::ui::inspector::render_inspector;
use crate::ui::modals::{ActiveModal, TextPromptKind, render_active_modal};
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
        path: PathBuf,
        payload: PreviewPayload,
    },
    Transfer(TransferEvent),
    NativeShellMenuDone(Result<(), String>),
}

#[derive(Clone)]
struct ToastBanner {
    message: SharedString,
    accent_hex: u32,
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

/// Root GPUI view coordinating tabs, dual-pane splits, transfers, preview/QuickLook, and OS shell.
pub struct Workspace {
    focus_handle: FocusHandle,
    pub settings: AppSettings,
    pub volumes: Vec<VolumeInfo>,
    pub known_folders: Vec<KnownFolder>,
    pub wsl_distros: Vec<(String, PathBuf)>,
    pub default_explorer_status: DefaultExplorerStatus,
    pub update_state: UpdateState,
    pub tabs: Vec<TabUiState>,
    pub active_tab_index: usize,
    pub context_menu: Option<ContextMenuState>,
    pub active_modal: Option<ActiveModal>,
    modal_subs: Vec<Subscription>,
    pub quicklook_open: bool,
    pub preview_path: Option<PathBuf>,
    pub preview_payload: PreviewPayload,
    preview_generation: u64,
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
    bg_tx: UnboundedSender<BgEvent>,
}

impl Workspace {
    pub fn new(
        settings: AppSettings,
        initial_intent: LaunchIntent,
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
        let shell_worker = ShellWorker::spawn();

        let (initial_location, pending_select) = match initial_intent {
            LaunchIntent::OpenLocation(path) => (Location::local(path), None),
            LaunchIntent::SelectFile(path) => {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned());
                let parent = path
                    .parent()
                    .map(Location::local)
                    .unwrap_or(Location::Home);
                (parent, name)
            }
            _ => (Location::Home, None),
        };

        let mut ws = Self {
            focus_handle: cx.focus_handle(),
            settings,
            volumes,
            known_folders,
            wsl_distros,
            default_explorer_status,
            update_state: UpdateState::Idle,
            tabs: Vec::new(),
            active_tab_index: 0,
            context_menu: None,
            active_modal: None,
            modal_subs: Vec::new(),
            quicklook_open: false,
            preview_path: None,
            preview_payload: PreviewPayload::Loading,
            preview_generation: 0,
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

        ws.open_tab_with_selection(initial_location, pending_select, window, cx);
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
                result,
            } => {
                if let Some(tab) = self.tabs.iter_mut().find(|t| t.model.id == tab_id) {
                    let pane = tab.model.pane_mut(side);
                    if pane.load_generation != generation {
                        return;
                    }
                    match result {
                        Ok(entries) => pane.set_entries(entries),
                        Err(err) => {
                            pane.loading = false;
                            pane.entries.clear();
                            pane.filtered_indices.clear();
                            pane.selection.clear();
                            pane.error = Some(err);
                        }
                    }
                    self.sync_preview_for_selection(cx);
                    cx.notify();
                }
            }
            BgEvent::DirectoryChanged {
                tab_id,
                side,
                directory,
            } => {
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
                    LaunchIntent::RestoreDefaultExplorer => {
                        self.restore_default_file_manager(cx);
                    }
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
                path,
                payload,
            } => {
                if generation == self.preview_generation {
                    self.preview_path = Some(path);
                    self.preview_payload = payload;
                    cx.notify();
                }
            }
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
                    let target_dir = snapshot.target_dir.clone();
                    let status = snapshot.status.clone();
                    let kind = snapshot.kind;
                    if let Some(job) = self
                        .transfers
                        .iter_mut()
                        .find(|j| j.snapshot.job_id == snapshot.job_id)
                    {
                        job.snapshot = snapshot;
                    }
                    if let Some(undo) = undo_action {
                        self.undo_history.push(undo);
                    }
                    if kind == TransferKind::Move && matches!(status, TransferStatus::Completed) {
                        self.cut_paths.clear();
                    }
                    self.refresh_panes_showing(&target_dir, cx);
                    if let TransferStatus::Failed(err) = status {
                        self.show_toast(err, STATUS_ERROR, window, cx);
                    }
                    cx.notify();
                }
            },
            BgEvent::NativeShellMenuDone(res) => {
                if let Err(err) = res {
                    self.show_toast(err, STATUS_ERROR, window, cx);
                } else {
                    self.refresh_visible_panes(cx);
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

    /// Triggers an asynchronous preview load whenever Inspector or QuickLook is visible.
    fn sync_preview_for_selection(&mut self, cx: &mut Context<Self>) {
        if !self.settings.show_inspector && !self.quicklook_open {
            return;
        }
        let target = self
            .active_tab()
            .and_then(|t| t.model.active_pane_ref().primary_selected_entry())
            .and_then(|e| Some((e.location.as_local_path()?.to_path_buf(), e.kind)));

        let Some((path, kind)) = target else {
            self.preview_path = None;
            self.preview_payload = PreviewPayload::Loading;
            return;
        };

        if self.preview_path.as_ref() == Some(&path)
            && !matches!(self.preview_payload, PreviewPayload::Loading)
        {
            return;
        }

        self.preview_generation = self.preview_generation.wrapping_add(1);
        let generation = self.preview_generation;
        self.preview_path = Some(path.clone());
        self.preview_payload = PreviewPayload::Loading;

        let worker = self.shell_worker.clone();
        let tx = self.bg_tx.clone();
        tokio_runtime().spawn(async move {
            let p_clone = path.clone();
            let payload = tokio::task::spawn_blocking(move || {
                load_preview_blocking(p_clone, kind, worker)
            })
            .await
            .unwrap_or_else(|_| PreviewPayload::Unsupported {
                reason: "Preview worker failed.".to_string(),
            });
            let _ = tx.unbounded_send(BgEvent::PreviewReady {
                generation,
                path,
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
        cx.notify();
    }

    pub fn select_tab(&mut self, index: usize, _window: &mut Window, cx: &mut Context<Self>) {
        if index < self.tabs.len() {
            self.active_tab_index = index;
            self.context_menu = None;
            self.sync_preview_for_selection(cx);
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
        let Some(tab) = self.tabs.iter_mut().find(|t| t.model.id == tab_id) else {
            return;
        };
        let pane = tab.model.pane_mut(side);
        pane.load_generation = pane.load_generation.wrapping_add(1);
        let generation = pane.load_generation;
        let location = pane.location.clone();

        if location == Location::Home {
            pane.entries.clear();
            pane.filtered_indices.clear();
            pane.selection.clear();
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
            let result =
                tokio::task::spawn_blocking(move || texplorer_vfs::list_location(&loc_clone))
                    .await
                    .unwrap_or_else(|e| Err(format!("Directory task panicked: {e}")));
            let _ = tx.unbounded_send(BgEvent::DirectoryLoaded {
                tab_id,
                side,
                generation,
                result,
            });
        });
        cx.notify();
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
        if let Some(target) = Location::parse_user_input(raw) {
            if let Some(local) = target.as_local_path()
                && !local.exists()
            {
                self.show_toast(
                    format!("Path not found: {}", local.display()),
                    STATUS_ERROR,
                    window,
                    cx,
                );
                cx.notify();
                return;
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
            cx.notify();
        }
    }

    pub fn toggle_hidden_for_pane(&mut self, side: PaneSide, cx: &mut Context<Self>) {
        if let Some(tab) = self.active_tab_mut() {
            let pane = tab.model.pane_mut(side);
            pane.show_hidden = !pane.show_hidden;
            pane.rebuild_filter();
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
        self.context_menu = None;
        let open_target = {
            let Some(tab) = self.active_tab_mut() else {
                return;
            };
            tab.model.active_pane = side;
            let pane = tab.model.pane_mut(side);
            if ev.click_count >= 2 {
                pane.selection.select_only(vis_idx);
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
        } else if let Some(path) = location.as_local_path()
            && let Err(err) = open_item(path)
        {
            self.show_toast(err, STATUS_ERROR, window, cx);
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
                self.show_toast(msg, STATUS_SUCCESS, window, cx);
            }
            Err(err) => self.show_toast(err, STATUS_ERROR, window, cx),
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
                self.enqueue_transfer(kind, clip.paths, dest_dir, cx);
            }
            Ok(None) => {
                self.show_toast("Clipboard does not contain files.", STATUS_INFO, window, cx);
            }
            Err(err) => self.show_toast(err, STATUS_ERROR, window, cx),
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
        self.enqueue_transfer(kind, sources, dest_dir, cx);
        let _ = window;
    }

    fn enqueue_transfer(
        &mut self,
        kind: TransferKind,
        sources: Vec<PathBuf>,
        dest_dir: PathBuf,
        cx: &mut Context<Self>,
    ) {
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
                self.show_toast(msg, STATUS_INFO, window, cx);
            }
            Err(err) => self.show_toast(err, STATUS_ERROR, window, cx),
        }
    }

    pub fn redo_last_action(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.undo_history.redo() {
            Ok(msg) => {
                self.refresh_visible_panes(cx);
                self.show_toast(msg, STATUS_INFO, window, cx);
            }
            Err(err) => self.show_toast(err, STATUS_ERROR, window, cx),
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
        self.set_toast_simple(notice, STATUS_SUCCESS, cx);
    }

    pub fn open_terminal_at(&mut self, path: &Path, cx: &mut Context<Self>) {
        if let Err(err) = open_terminal_here(path, &self.settings.terminal_command) {
            self.set_toast_simple(err, STATUS_ERROR, cx);
        }
    }

    pub fn open_editor_at(&mut self, path: &Path, cx: &mut Context<Self>) {
        if let Err(err) = open_in_editor(path, &self.settings.editor_command) {
            self.set_toast_simple(err, STATUS_ERROR, cx);
        }
    }

    pub fn open_in_textree(&mut self, path: &Path, cx: &mut Context<Self>) {
        let Some(exe) = find_textree_exe(self.settings.textree_path.as_deref()) else {
            self.set_toast_simple("TexTree executable was not found.", STATUS_ERROR, cx);
            return;
        };
        if let Err(err) = std::process::Command::new(exe).arg(path).spawn() {
            self.set_toast_simple(format!("Failed to launch TexTree: {err}"), STATUS_ERROR, cx);
        }
    }

    pub fn reveal_in_windows_explorer(&mut self, path: &Path, cx: &mut Context<Self>) {
        if let Err(err) = reveal_in_windows_explorer(path) {
            self.set_toast_simple(err, STATUS_ERROR, cx);
        }
    }

    pub fn show_properties_for(&mut self, path: &Path, cx: &mut Context<Self>) {
        if let Err(err) = show_properties_dialog(path) {
            self.set_toast_simple(err, STATUS_ERROR, cx);
        }
    }

    pub fn pin_folder(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if self.settings.pin_folder(path) {
            let _ = self.settings.save();
            self.set_toast_simple("Pinned folder to sidebar", STATUS_SUCCESS, cx);
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

    pub fn open_settings_modal(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.default_explorer_status = query_default_explorer_status();
        self.context_menu = None;
        self.quicklook_open = false;
        self.active_modal = Some(ActiveModal::Settings);
        cx.notify();
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
                self.show_toast(err, STATUS_ERROR, window, cx);
            }
        }
    }

    pub fn trash_selected(&mut self, side: PaneSide, window: &mut Window, cx: &mut Context<Self>) {
        let paths = self.selected_local_paths(side);
        if paths.is_empty() {
            return;
        }
        let count = paths.len();
        match move_to_trash(&paths) {
            Ok(()) => {
                self.refresh_pane(side, window, cx);
                let msg = if count == 1 {
                    "Moved item to Recycle Bin".to_string()
                } else {
                    format!("Moved {count} items to Recycle Bin")
                };
                self.show_toast(msg, STATUS_SUCCESS, window, cx);
            }
            Err(err) => self.show_toast(err, STATUS_ERROR, window, cx),
        }
    }

    pub fn prompt_permanent_delete(&mut self, side: PaneSide, cx: &mut Context<Self>) {
        let paths = self.selected_local_paths(side);
        if paths.is_empty() {
            return;
        }
        self.active_modal = Some(ActiveModal::ConfirmPermanentDelete { side, paths });
        cx.notify();
    }

    pub fn confirm_permanent_delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ActiveModal::ConfirmPermanentDelete { side, paths }) = self.active_modal.take()
        else {
            return;
        };
        let mut failures = Vec::new();
        for path in &paths {
            if let Err(err) = delete_permanently(path) {
                failures.push(err);
            }
        }
        self.refresh_pane(side, window, cx);
        if failures.is_empty() {
            self.show_toast(
                format!("Permanently deleted {} item(s)", paths.len()),
                STATUS_SUCCESS,
                window,
                cx,
            );
        } else {
            self.show_toast(failures[0].clone(), STATUS_ERROR, window, cx);
        }
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

    pub fn set_accent(&mut self, preset: AccentPreset, cx: &mut Context<Self>) {
        self.settings.accent = preset;
        let _ = self.settings.save();
        apply_obsidian_theme(preset, cx);
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
                    STATUS_SUCCESS,
                    cx,
                );
            }
            Err(err) => self.set_toast_simple(err, STATUS_ERROR, cx),
        }
    }

    pub fn restore_default_file_manager(&mut self, cx: &mut Context<Self>) {
        match restore_windows_explorer() {
            Ok(()) => {
                self.settings.replace_default_explorer = false;
                let _ = self.settings.save();
                self.default_explorer_status = query_default_explorer_status();
                self.set_toast_simple("Restored original Windows Explorer", STATUS_INFO, cx);
            }
            Err(err) => self.set_toast_simple(err, STATUS_ERROR, cx),
        }
    }

    pub fn toggle_intercept_win_e(&mut self, cx: &mut Context<Self>) {
        self.settings.intercept_win_e = !self.settings.intercept_win_e;
        let _ = self.settings.save();
        if matches!(self.default_explorer_status, DefaultExplorerStatus::Enabled) {
            let _ = enable_default_explorer(self.settings.intercept_win_e);
            self.default_explorer_status = query_default_explorer_status();
        }
        cx.notify();
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
                STATUS_INFO,
                cx,
            );
        }
    }

    pub fn toggle_quick_access(&mut self, kind: KnownFolderKind, cx: &mut Context<Self>) {
        self.settings.toggle_quick_access(kind);
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
            self.set_toast_simple(err, STATUS_ERROR, cx);
        }
    }

    fn set_toast_simple(
        &mut self,
        message: impl Into<SharedString>,
        accent_hex: u32,
        cx: &mut Context<Self>,
    ) {
        self.toast_seq = self.toast_seq.wrapping_add(1);
        let id = self.toast_seq;
        self.toast = Some(ToastBanner {
            message: message.into(),
            accent_hex,
            id,
        });
        cx.notify();
    }

    fn show_toast(
        &mut self,
        message: impl Into<SharedString>,
        accent_hex: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toast_seq = self.toast_seq.wrapping_add(1);
        let id = self.toast_seq;
        self.toast = Some(ToastBanner {
            message: message.into(),
            accent_hex,
            id,
        });
        cx.spawn_in(window, async move |weak, cx| {
            smol_sleep(Duration::from_millis(3200)).await;
            let _ = weak.update(cx, |this, cx| {
                if this.toast.as_ref().is_some_and(|t| t.id == id) {
                    this.toast = None;
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    fn is_modal_blocking(&self) -> bool {
        self.active_modal.is_some() || !self.conflict_queue.is_empty()
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

async fn smol_sleep(duration: Duration) {
    let (tx, rx) = futures_channel::oneshot::channel::<()>();
    tokio_runtime().spawn(async move {
        tokio::time::sleep(duration).await;
        let _ = tx.send(());
    });
    let _ = rx.await;
}

impl Focusable for Workspace {
    fn focus_handle(&self, _cx: &gpui_kit::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let accent = self.settings.accent;
        let is_split = self.active_tab().is_some_and(|t| t.model.split_enabled);

        let main_panes = if is_split {
            h_resizable("dual-pane-split")
                .with_handle_appearance(crisp_resize_handle_appearance(accent))
                .child(resizable_panel().child(render_pane(self, PaneSide::Left, true, cx)))
                .child(resizable_panel().child(render_pane(self, PaneSide::Right, true, cx)))
                .into_any_element()
        } else {
            render_pane(self, PaneSide::Left, false, cx).into_any_element()
        };

        let center_and_inspector = if self.settings.show_inspector {
            h_resizable("center-inspector-split")
                .with_handle_appearance(crisp_resize_handle_appearance(accent))
                .child(resizable_panel().child(main_panes))
                .child(
                    resizable_panel()
                        .size(px(304.))
                        .size_range(px(240.)..px(480.))
                        .fixed()
                        .child(render_inspector(self, cx)),
                )
                .into_any_element()
        } else {
            main_panes
        };

        let body = if self.settings.show_sidebar {
            h_resizable("sidebar-main-split")
                .with_handle_appearance(crisp_resize_handle_appearance(accent))
                .child(
                    resizable_panel()
                        .size(px(238.))
                        .size_range(px(185.)..px(360.))
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
            .bg(solid(BG_BASE))
            .text_color(solid(TEXT_PRIMARY))
            .overflow_hidden()
            .on_key_down(cx.listener(Self::handle_key_down))
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
            .on_action(cx.listener(|_, _: &Quit, _, cx| {
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
                        .bg(solid(BG_ELEVATED))
                        .border_1()
                        .border_color(solid(BORDER))
                        .child(
                            div()
                                .w(px(6.))
                                .h(px(6.))
                                .rounded(px(999.))
                                .bg(solid(toast.accent_hex)),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(solid(TEXT_PRIMARY))
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
