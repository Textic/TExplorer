use std::cell::RefCell;
use std::collections::HashSet;
use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use gpui_kit::{
    BorderStyle, Bounds, ContentMask, Context, Entity, FontWeight, MouseButton, MouseDownEvent,
    MouseMoveEvent, PaintQuad, Pixels, Point, ShapedLine, SharedString, Size as GpuiSize,
    Subscription, TextAlign, TextRun, UniformListScrollHandle, Window,
    assets::IconName as LucideIcon,
    canvas,
    component::{
        Disableable as _, Icon, IconName, Sizable as _, Size, StyledExt as _,
        button::{Button, ButtonVariants as _},
        h_flex,
        input::{Input, InputEvent, InputState},
        resizable::{resizable_panel, v_resizable},
        v_flex,
    },
    div, fill, outline, point, prelude::*, px, relative, uniform_list,
};
use texplorer_core::{PaneSide, SizeUnit};
use texplorer_disk::{
    DiskScanProgress, DiskScanTarget, DiskSortColumn, DiskSortSpec, DiskTree, ExtensionStat,
    NodeId, TREEMAP_LEGEND_ITEMS, TreemapLayout, VisibleRow, color_for_extension,
};

use crate::theme::{
    Palette, RADIUS_CONTROL, RADIUS_ROW, crisp_resize_handle_appearance, format_size,
    format_timestamp, full_hit_input, soft_tint, solid,
};
use crate::ui::workspace::Workspace;

const ROW_HEIGHT: f32 = 28.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DiskAnalyzerTab {
    #[default]
    Tree,
    TopFiles,
    Extensions,
}

pub struct ShapedFolderHeader {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub line: ShapedLine,
}

#[derive(Default)]
pub struct TreemapCanvasCache {
    pub tree_version: u64,
    pub root_id: NodeId,
    pub show_hidden: bool,
    pub size_unit: SizeUnit,
    pub palette_key: (bool, u32, u32),
    pub bounds: Bounds<Pixels>,
    pub origin: Point<Pixels>,
    pub width: f32,
    pub height: f32,
    pub layout: TreemapLayout,
    pub shaped_headers: Vec<ShapedFolderHeader>,
}

#[derive(Debug, Clone)]
pub struct DiskTrashConfirm {
    pub node_id: NodeId,
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    pub size_bytes: u64,
}

/// Complete live state for the built-in Disk Space Analyzer workspace view.
pub struct DiskAnalyzerState {
    pub target: Option<DiskScanTarget>,
    pub scanning: bool,
    pub scan_generation: u64,
    pub cancel: Arc<AtomicBool>,
    pub progress: Option<DiskScanProgress>,
    pub elapsed_ms: u64,
    pub error: Option<String>,
    pub tree: Option<Arc<DiskTree>>,
    pub tree_version: u64,
    pub active_tab: DiskAnalyzerTab,
    pub sort: DiskSortSpec,
    pub expanded: HashSet<NodeId>,
    pub visible_rows: Vec<VisibleRow>,
    pub top_files: Vec<NodeId>,
    pub ext_stats: Vec<ExtensionStat>,
    pub filter_input: Entity<InputState>,
    pub filter_query: String,
    pub ext_filter: Option<String>,
    pub denied_only: bool,
    pub selected_node: Option<NodeId>,
    pub treemap_root: Option<NodeId>,
    pub show_treemap: bool,
    pub treemap_hovered: Option<NodeId>,
    pub list_hovered: Option<NodeId>,
    pub treemap_cache: Rc<RefCell<TreemapCanvasCache>>,
    pub tree_scroll: UniformListScrollHandle,
    pub files_scroll: UniformListScrollHandle,
    pub ext_scroll: UniformListScrollHandle,
    pub confirm_trash: Option<DiskTrashConfirm>,
    pub trashing: bool,
    _filter_sub: Subscription,
}

impl DiskAnalyzerState {
    pub fn new(window: &mut Window, cx: &mut Context<Workspace>) -> Self {
        let filter_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Filter analyzed items..."));
        let filter_sub = cx.subscribe_in(
            &filter_input,
            window,
            |this, state, ev: &InputEvent, _window, cx| {
                if matches!(ev, InputEvent::Change) {
                    let val = state.read(cx).value().to_string();
                    if this.disk_analyzer.filter_query != val {
                        this.disk_analyzer.filter_query = val;
                        let show_hidden = this.settings.show_hidden_files;
                        this.disk_analyzer.rebuild_views(show_hidden);
                        cx.notify();
                    }
                }
            },
        );

        Self {
            target: None,
            scanning: false,
            scan_generation: 0,
            cancel: Arc::new(AtomicBool::new(false)),
            progress: None,
            elapsed_ms: 0,
            error: None,
            tree: None,
            tree_version: 0,
            active_tab: DiskAnalyzerTab::Tree,
            sort: DiskSortSpec::default(),
            expanded: HashSet::new(),
            visible_rows: Vec::new(),
            top_files: Vec::new(),
            ext_stats: Vec::new(),
            filter_input,
            filter_query: String::new(),
            ext_filter: None,
            denied_only: false,
            selected_node: None,
            treemap_root: None,
            show_treemap: true,
            treemap_hovered: None,
            list_hovered: None,
            treemap_cache: Rc::new(RefCell::new(TreemapCanvasCache::default())),
            tree_scroll: UniformListScrollHandle::new(),
            files_scroll: UniformListScrollHandle::new(),
            ext_scroll: UniformListScrollHandle::new(),
            confirm_trash: None,
            trashing: false,
            _filter_sub: filter_sub,
        }
    }

    pub fn rebuild_views(&mut self, show_hidden: bool) {
        let Some(tree) = &self.tree else {
            self.visible_rows.clear();
            self.top_files.clear();
            self.ext_stats.clear();
            return;
        };

        self.visible_rows = tree.flatten_visible(
            &self.expanded,
            self.sort,
            show_hidden,
            &self.filter_query,
            self.denied_only,
        );
        self.top_files = tree.top_files(
            2_500,
            show_hidden,
            &self.filter_query,
            self.ext_filter.as_deref(),
            self.denied_only,
        );
        self.ext_stats = tree.extension_stats(show_hidden);
    }

    /// Expands all ancestors of `node_id` so it becomes visible in the Tree-Table.
    pub fn expand_ancestors_of(&mut self, node_id: NodeId) {
        let Some(tree) = &self.tree else {
            return;
        };
        let mut cursor = tree.get(node_id).and_then(|n| n.parent);
        let mut guard = 0u32;
        while let Some(pid) = cursor {
            guard += 1;
            if guard > 256 {
                break;
            }
            self.expanded.insert(pid);
            if pid == tree.root {
                break;
            }
            cursor = tree.get(pid).and_then(|n| n.parent);
        }
    }
}

/// Renders the complete built-in Disk Space Analyzer tool inside an explorer pane.
pub fn render_disk_analyzer(
    ws: &Workspace,
    _side: PaneSide,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let p = ws.palette();
    let da = &ws.disk_analyzer;
    let unit = ws.settings.size_unit;

    v_flex()
        .id("disk-analyzer-root")
        .size_full()
        .bg(solid(p.bg_base))
        .overflow_hidden()
        .child(render_target_toolbar(ws, p, cx))
        .when(da.scanning, |el| el.child(render_scan_progress_bar(ws, p, unit)))
        .when(da.tree.is_some(), |el| el.child(render_subtab_action_bar(ws, p, cx)))
        .when_some(da.confirm_trash.clone(), |el, confirm| {
            el.child(render_trash_confirm_bar(confirm, da.trashing, p, unit, cx))
        })
        .child(
            div()
                .flex_1()
                .min_h_0()
                .w_full()
                .overflow_hidden()
                .child(render_main_body(ws, p, cx)),
        )
        .when(da.tree.is_some(), |el| el.child(render_summary_footer(ws, p, unit, cx)))
}

fn render_target_toolbar(
    ws: &Workspace,
    p: Palette,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let da = &ws.disk_analyzer;
    let is_all_drives = matches!(da.target, Some(DiskScanTarget::AllFixedDrives));
    let is_custom_folder = matches!(da.target, Some(DiskScanTarget::Folder(_)));
    let custom_folder_label: Option<SharedString> = match &da.target {
        Some(DiskScanTarget::Folder(path)) => Some(path.to_string_lossy().into_owned().into()),
        _ => None,
    };
    let engine_label = da.tree.as_ref().map(|t| t.engine.label());
    let has_target = da.target.is_some();
    let scanning = da.scanning;

    h_flex()
        .w_full()
        .h(px(42.))
        .px_2p5()
        .gap_2()
        .flex_shrink_0()
        .items_center()
        .justify_between()
        .bg(solid(p.bg_surface))
        .border_b_1()
        .border_color(solid(p.border_subtle))
        .child(
            h_flex()
                .id("da-targets-strip")
                .flex_1()
                .min_w_0()
                .items_center()
                .gap_1p5()
                .overflow_x_scroll()
                .child(target_chip(
                    "da-target-all",
                    "All Drives",
                    LucideIcon::Monitor,
                    is_all_drives,
                    p,
                    cx.listener(|this, _, _, cx| {
                        this.start_disk_scan(DiskScanTarget::AllFixedDrives, cx);
                    }),
                ))
                .children(ws.volumes.iter().enumerate().map(|(idx, vol)| {
                    let selected = da
                        .target
                        .as_ref()
                        .is_some_and(|t| t.is_same_volume(&vol.root_path));
                    let vol_clone = vol.clone();
                    let label: SharedString = if vol.label.is_empty() {
                        vol.short_name.clone().into()
                    } else {
                        format!("{} ({})", vol.label, vol.short_name).into()
                    };
                    target_chip(
                        ("da-target-vol", idx),
                        label,
                        LucideIcon::HardDrive,
                        selected,
                        p,
                        cx.listener(move |this, _, _, cx| {
                            this.start_disk_scan(DiskScanTarget::Volume(vol_clone.clone()), cx);
                        }),
                    )
                }))
                .child(target_chip(
                    "da-target-folder",
                    "Folder...",
                    LucideIcon::FolderOpen,
                    is_custom_folder,
                    p,
                    cx.listener(|this, _, _, cx| {
                        this.pick_disk_analyzer_folder(cx);
                    }),
                ))
                .when_some(custom_folder_label, |el, folder_text| {
                    el.child(
                        div()
                            .max_w(px(260.))
                            .truncate()
                            .px_1p5()
                            .text_xs()
                            .text_color(solid(p.text_muted))
                            .child(folder_text),
                    )
                }),
        )
        .child(
            h_flex()
                .flex_shrink_0()
                .items_center()
                .gap_1p5()
                .when_some(engine_label, |el, label| {
                    el.child(
                        div()
                            .px_2()
                            .py_0p5()
                            .rounded(px(RADIUS_CONTROL))
                            .bg(solid(p.bg_elevated))
                            .border_1()
                            .border_color(solid(p.border_subtle))
                            .text_size(px(11.5))
                            .font_medium()
                            .text_color(solid(p.text_secondary))
                            .child(label),
                    )
                })
                .child(if scanning {
                    Button::new("da-cancel-scan")
                        .with_size(Size::XSmall)
                        .danger()
                        .icon(LucideIcon::X)
                        .label("Cancel")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.cancel_disk_scan(cx);
                        }))
                        .into_any_element()
                } else {
                    Button::new("da-rescan-btn")
                        .with_size(Size::XSmall)
                        .primary()
                        .disabled(!has_target)
                        .icon(LucideIcon::RefreshCw)
                        .label("Rescan")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.rescan_disk_analyzer(cx);
                        }))
                        .into_any_element()
                }),
        )
}

fn target_chip(
    id: impl Into<gpui_kit::ElementId>,
    label: impl Into<SharedString>,
    icon: LucideIcon,
    active: bool,
    p: Palette,
    on_click: impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut gpui_kit::App) + 'static,
) -> impl IntoElement {
    h_flex()
        .id(id)
        .h(px(26.))
        .px_2()
        .gap_1p5()
        .flex_shrink_0()
        .items_center()
        .rounded(px(RADIUS_CONTROL))
        .cursor_pointer()
        .bg(if active {
            soft_tint(p.accent_primary, 0.16)
        } else {
            solid(p.bg_base)
        })
        .border_1()
        .border_color(if active {
            solid(p.accent_primary)
        } else {
            solid(p.border_subtle)
        })
        .text_xs()
        .font_medium()
        .text_color(if active {
            solid(p.accent_primary)
        } else {
            solid(p.text_secondary)
        })
        .hover(move |s| {
            if active {
                s
            } else {
                s.bg(solid(p.bg_hover))
                    .border_color(solid(p.border))
                    .text_color(solid(p.text_primary))
            }
        })
        .on_click(on_click)
        .child(Icon::new(icon).xsmall())
        .child(label.into())
}

fn render_scan_progress_bar(
    ws: &Workspace,
    p: Palette,
    unit: SizeUnit,
) -> impl IntoElement {
    let prog = ws.disk_analyzer.progress.as_ref();
    let engine_label = prog.map(|pr| pr.engine.label()).unwrap_or("Scanning");
    let stats_line: SharedString = if let Some(pr) = prog {
        format!(
            "{} files  •  {} folders  •  {}  •  {:.1}s",
            format_count(pr.files_scanned),
            format_count(pr.dirs_scanned),
            format_size(pr.bytes_scanned, unit),
            pr.elapsed_ms as f64 / 1000.0
        )
        .into()
    } else {
        "Preparing scan...".into()
    };
    let current_path: SharedString = prog
        .map(|pr| pr.current_path.clone())
        .unwrap_or_default()
        .into();

    h_flex()
        .w_full()
        .h(px(30.))
        .px_3()
        .gap_2p5()
        .flex_shrink_0()
        .items_center()
        .justify_between()
        .bg(soft_tint(p.accent_primary, 0.10))
        .border_b_1()
        .border_color(soft_tint(p.accent_primary, 0.28))
        .child(
            h_flex()
                .flex_shrink_0()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .px_1p5()
                        .py_0p5()
                        .rounded(px(3.))
                        .bg(solid(p.accent_primary))
                        .text_size(px(11.))
                        .font_semibold()
                        .text_color(solid(p.accent_fg))
                        .child(engine_label),
                )
                .child(
                    div()
                        .text_xs()
                        .font_medium()
                        .text_color(solid(p.text_primary))
                        .child(stats_line),
                ),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_xs()
                .text_color(solid(p.text_secondary))
                .child(current_path),
        )
}

fn render_subtab_action_bar(
    ws: &Workspace,
    p: Palette,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let da = &ws.disk_analyzer;
    let active_tab = da.active_tab;
    let filter_non_empty = !da.filter_query.trim().is_empty();
    let ext_filter = da.ext_filter.clone();
    let denied_only = da.denied_only;
    let show_treemap = da.show_treemap;
    let denied_count = da.tree.as_ref().map(|t| t.total_denied).unwrap_or(0);
    let selected_id = da.selected_node;
    let selected_has_path = selected_id
        .and_then(|id| da.tree.as_ref().and_then(|t| t.full_path(id)))
        .is_some();
    let selected_is_deletable = selected_id.is_some_and(|id| {
        da.tree
            .as_ref()
            .is_some_and(|t| id != t.root && t.full_path(id).is_some())
    });

    h_flex()
        .w_full()
        .h(px(36.))
        .px_2p5()
        .gap_2()
        .flex_shrink_0()
        .items_center()
        .justify_between()
        .bg(solid(p.bg_base))
        .border_b_1()
        .border_color(solid(p.border_subtle))
        // Left: Subtabs + Filter + Active Chips
        .child(
            h_flex()
                .flex_1()
                .min_w_0()
                .items_center()
                .gap_1p5()
                .child(subtab_btn(
                    "da-tab-tree",
                    "Tree",
                    active_tab == DiskAnalyzerTab::Tree,
                    p,
                    cx.listener(|this, _, _, cx| {
                        this.set_disk_analyzer_tab(DiskAnalyzerTab::Tree, cx);
                    }),
                ))
                .child(subtab_btn(
                    "da-tab-files",
                    "Top Files",
                    active_tab == DiskAnalyzerTab::TopFiles,
                    p,
                    cx.listener(|this, _, _, cx| {
                        this.set_disk_analyzer_tab(DiskAnalyzerTab::TopFiles, cx);
                    }),
                ))
                .child(subtab_btn(
                    "da-tab-exts",
                    "Extensions",
                    active_tab == DiskAnalyzerTab::Extensions,
                    p,
                    cx.listener(|this, _, _, cx| {
                        this.set_disk_analyzer_tab(DiskAnalyzerTab::Extensions, cx);
                    }),
                ))
                .child(div().w(px(1.)).h(px(16.)).mx_0p5().bg(solid(p.border_subtle)))
                // Single-border inline filter input (zero double borders)
                .child(
                    h_flex()
                        .w(px(196.))
                        .h(px(26.))
                        .px_2()
                        .gap_1()
                        .flex_shrink_0()
                        .items_center()
                        .rounded(px(RADIUS_CONTROL))
                        .bg(solid(p.bg_surface))
                        .border_1()
                        .border_color(if filter_non_empty {
                            solid(p.accent_primary)
                        } else {
                            solid(p.border_subtle)
                        })
                        .child(
                            Icon::new(LucideIcon::Search)
                                .xsmall()
                                .text_color(solid(p.text_muted)),
                        )
                        .child(
                            full_hit_input(
                                "da-filter-input",
                                &da.filter_input,
                                Input::new(&da.filter_input)
                                    .with_size(Size::XSmall)
                                    .appearance(false),
                            )
                            .flex_1()
                            .min_w_0(),
                        )
                        .when(filter_non_empty, |inner| {
                            inner.child(
                                div()
                                    .id("da-filter-clear")
                                    .w(px(16.))
                                    .h(px(16.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(3.))
                                    .cursor_pointer()
                                    .text_color(solid(p.text_muted))
                                    .hover(move |s| {
                                        s.bg(solid(p.bg_hover)).text_color(solid(p.text_primary))
                                    })
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, window, cx| {
                                            this.clear_disk_filter(window, cx);
                                        }),
                                    )
                                    .child(Icon::new(LucideIcon::X).xsmall()),
                            )
                        }),
                )
                .when_some(ext_filter, |el, ext| {
                    let chip_label: SharedString = format!("ext: .{ext}").into();
                    el.child(
                        h_flex()
                            .id("da-ext-filter-chip")
                            .h(px(24.))
                            .px_2()
                            .gap_1()
                            .flex_shrink_0()
                            .items_center()
                            .rounded(px(RADIUS_CONTROL))
                            .cursor_pointer()
                            .bg(soft_tint(p.accent_primary, 0.16))
                            .border_1()
                            .border_color(soft_tint(p.accent_primary, 0.35))
                            .text_xs()
                            .font_medium()
                            .text_color(solid(p.accent_primary))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.set_disk_ext_filter(None, false, cx);
                            }))
                            .child(chip_label)
                            .child(Icon::new(LucideIcon::X).xsmall()),
                    )
                })
                .when(denied_count > 0 || denied_only, |el| {
                    el.child(
                        h_flex()
                            .id("da-denied-toggle")
                            .h(px(24.))
                            .px_2()
                            .gap_1()
                            .flex_shrink_0()
                            .items_center()
                            .rounded(px(RADIUS_CONTROL))
                            .cursor_pointer()
                            .bg(if denied_only {
                                soft_tint(p.status_error, 0.16)
                            } else {
                                solid(p.bg_surface)
                            })
                            .border_1()
                            .border_color(if denied_only {
                                solid(p.status_error)
                            } else {
                                solid(p.border_subtle)
                            })
                            .text_xs()
                            .text_color(if denied_only {
                                solid(p.status_error)
                            } else {
                                solid(p.text_secondary)
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.toggle_disk_denied_only(cx);
                            }))
                            .child(Icon::new(LucideIcon::Lock).xsmall())
                            .child(format!("Denied ({denied_count})")),
                    )
                }),
        )
        // Right: Selected item actions + Treemap toggle
        .child(
            h_flex()
                .flex_shrink_0()
                .items_center()
                .gap_1()
                .when(selected_has_path, |el| {
                    let nid = selected_id.unwrap_or(0);
                    el.child(
                        Button::new("da-act-reveal")
                            .with_size(Size::XSmall)
                            .ghost()
                            .icon(LucideIcon::FolderOpen)
                            .label("Open in TExplorer")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.reveal_disk_node_in_explorer(nid, window, cx);
                            })),
                    )
                    .child(
                        Button::new("da-act-open")
                            .with_size(Size::XSmall)
                            .ghost()
                            .icon(LucideIcon::ExternalLink)
                            .tooltip("Open File / Folder")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_disk_node_item(nid, window, cx);
                            })),
                    )
                    .child(
                        Button::new("da-act-copy")
                            .with_size(Size::XSmall)
                            .ghost()
                            .icon(LucideIcon::Copy)
                            .tooltip("Copy Full Path")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.copy_disk_node_path(nid, cx);
                            })),
                    )
                })
                .when(selected_is_deletable, |el| {
                    let nid = selected_id.unwrap_or(0);
                    el.child(
                        Button::new("da-act-trash")
                            .with_size(Size::XSmall)
                            .ghost()
                            .icon(IconName::Delete)
                            .tooltip("Move to Recycle Bin")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.prompt_trash_disk_node(nid, cx);
                            })),
                    )
                })
                .when(active_tab == DiskAnalyzerTab::Tree, |el| {
                    el.child(div().w(px(1.)).h(px(16.)).mx_0p5().bg(solid(p.border_subtle)))
                        .child(
                            h_flex()
                                .id("da-toggle-treemap")
                                .h(px(26.))
                                .px_2()
                                .gap_1()
                                .items_center()
                                .rounded(px(RADIUS_CONTROL))
                                .cursor_pointer()
                                .bg(if show_treemap {
                                    soft_tint(p.accent_primary, 0.16)
                                } else {
                                    gpui_kit::transparent_black()
                                })
                                .text_xs()
                                .font_medium()
                                .text_color(if show_treemap {
                                    solid(p.accent_primary)
                                } else {
                                    solid(p.text_muted)
                                })
                                .hover(move |s| {
                                    if show_treemap {
                                        s
                                    } else {
                                        s.bg(solid(p.bg_hover)).text_color(solid(p.text_primary))
                                    }
                                })
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.toggle_disk_treemap(cx);
                                }))
                                .child(Icon::new(LucideIcon::LayoutGrid).xsmall())
                                .child("Treemap"),
                        )
                }),
        )
}

fn subtab_btn(
    id: &'static str,
    label: &'static str,
    active: bool,
    p: Palette,
    on_click: impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut gpui_kit::App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .px_2p5()
        .py_1()
        .rounded(px(RADIUS_CONTROL))
        .cursor_pointer()
        .text_xs()
        .font_medium()
        .bg(if active {
            solid(p.bg_hover)
        } else {
            gpui_kit::transparent_black()
        })
        .text_color(if active {
            solid(p.text_primary)
        } else {
            solid(p.text_muted)
        })
        .hover(move |s| {
            if active {
                s
            } else {
                s.bg(solid(p.bg_elevated)).text_color(solid(p.text_primary))
            }
        })
        .on_click(on_click)
        .child(label)
}

fn render_trash_confirm_bar(
    confirm: DiskTrashConfirm,
    trashing: bool,
    p: Palette,
    unit: SizeUnit,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let kind_str = if confirm.is_dir { "folder" } else { "file" };
    let headline: SharedString = format!(
        "Move {kind_str} \"{}\" ({}) to Recycle Bin?",
        confirm.name,
        format_size(confirm.size_bytes, unit)
    )
    .into();
    let path_str: SharedString = confirm.path.to_string_lossy().into_owned().into();

    h_flex()
        .w_full()
        .px_3()
        .py_2()
        .gap_3()
        .flex_shrink_0()
        .items_center()
        .justify_between()
        .bg(soft_tint(p.status_error, 0.12))
        .border_b_1()
        .border_color(soft_tint(p.status_error, 0.35))
        .child(
            h_flex()
                .flex_1()
                .min_w_0()
                .gap_2()
                .items_center()
                .child(
                    Icon::new(LucideIcon::TriangleAlert)
                        .small()
                        .text_color(solid(p.status_error)),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .truncate()
                                .text_xs()
                                .font_semibold()
                                .text_color(solid(p.text_primary))
                                .child(headline),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_size(px(11.5))
                                .text_color(solid(p.text_secondary))
                                .child(path_str),
                        ),
                ),
        )
        .child(
            h_flex()
                .flex_shrink_0()
                .gap_1p5()
                .items_center()
                .child(
                    Button::new("da-trash-cancel")
                        .with_size(Size::XSmall)
                        .ghost()
                        .disabled(trashing)
                        .label("Cancel")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.cancel_trash_disk_node(cx);
                        })),
                )
                .child(
                    Button::new("da-trash-confirm")
                        .with_size(Size::XSmall)
                        .danger()
                        .disabled(trashing)
                        .icon(IconName::Delete)
                        .label(if trashing {
                            "Moving..."
                        } else {
                            "Move to Recycle Bin"
                        })
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.confirm_trash_disk_node(window, cx);
                        })),
                ),
        )
}

fn render_main_body(
    ws: &Workspace,
    p: Palette,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let da = &ws.disk_analyzer;

    if let Some(err) = &da.error
        && da.tree.is_none()
        && !da.scanning
    {
        let err_msg: SharedString = err.clone().into();
        return v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_2()
            .px_6()
            .child(
                Icon::new(LucideIcon::TriangleAlert)
                    .large()
                    .text_color(solid(p.status_error)),
            )
            .child(
                div()
                    .text_sm()
                    .font_semibold()
                    .text_color(solid(p.text_primary))
                    .child("Scan failed"),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(solid(p.text_muted))
                    .child(err_msg),
            )
            .child(
                Button::new("da-retry-err")
                    .with_size(Size::Small)
                    .primary()
                    .label("Retry Scan")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.rescan_disk_analyzer(cx);
                    })),
            )
            .into_any_element();
    }

    if da.tree.is_none() {
        return render_empty_state(ws, p, cx).into_any_element();
    }

    match da.active_tab {
        DiskAnalyzerTab::Tree => {
            if da.show_treemap {
                v_resizable("da-tree-treemap-split")
                    .with_handle_appearance(crisp_resize_handle_appearance(p))
                    .child(
                        resizable_panel()
                            .size(px(360.))
                            .size_range(px(160.)..px(1400.))
                            .child(render_tree_table(ws, p, cx)),
                    )
                    .child(
                        resizable_panel()
                            .size_range(px(160.)..px(1200.))
                            .child(render_treemap_pane(ws, p, cx)),
                    )
                    .into_any_element()
            } else {
                render_tree_table(ws, p, cx).into_any_element()
            }
        }
        DiskAnalyzerTab::TopFiles => render_top_files_view(ws, p, cx).into_any_element(),
        DiskAnalyzerTab::Extensions => render_extensions_view(ws, p, cx).into_any_element(),
    }
}

fn render_empty_state(
    ws: &Workspace,
    p: Palette,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let scanning = ws.disk_analyzer.scanning;
    v_flex()
        .size_full()
        .items_center()
        .justify_center()
        .gap_2p5()
        .px_6()
        .child(
            div()
                .w(px(44.))
                .h(px(44.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(RADIUS_ROW))
                .bg(soft_tint(p.accent_primary, 0.12))
                .child(
                    Icon::new(LucideIcon::HardDrive)
                        .large()
                        .text_color(solid(p.accent_primary)),
                ),
        )
        .child(
            div()
                .text_sm()
                .font_semibold()
                .text_color(solid(p.text_primary))
                .child(if scanning {
                    "Analyzing disk usage..."
                } else {
                    "Analyze disk space across drives or folders"
                }),
        )
        .child(
            div()
                .max_w(px(440.))
                .text_center()
                .text_xs()
                .text_color(solid(p.text_muted))
                .child(if scanning {
                    "Reading folder hierarchy and computing real disk sizes."
                } else {
                    "Select a drive above, scan all fixed drives, or pick any folder to visualize storage with an interactive Treemap."
                }),
        )
        .when(!scanning, |el| {
            el.child(
                h_flex()
                    .pt_1()
                    .gap_2()
                    .items_center()
                    .child(
                        Button::new("da-empty-all")
                            .with_size(Size::Small)
                            .primary()
                            .icon(LucideIcon::Monitor)
                            .label("Scan All Drives")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.start_disk_scan(DiskScanTarget::AllFixedDrives, cx);
                            })),
                    )
                    .child(
                        Button::new("da-empty-folder")
                            .with_size(Size::Small)
                            .outline()
                            .icon(LucideIcon::FolderOpen)
                            .label("Pick Folder...")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.pick_disk_analyzer_folder(cx);
                            })),
                    ),
            )
        })
}

// ================================================================================================
// Virtualized Tree-Table View
// ================================================================================================

fn render_tree_table(
    ws: &Workspace,
    p: Palette,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let da = &ws.disk_analyzer;
    let row_count = da.visible_rows.len();

    v_flex()
        .id("da-tree-table-root")
        .size_full()
        .bg(solid(p.bg_base))
        .overflow_hidden()
        .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
            if !*hovered {
                this.set_disk_list_hovered(None, cx);
            }
        }))
        .child(render_tree_header(da.sort, p, cx))
        .child(if row_count == 0 {
            v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .gap_1()
                .child(
                    div()
                        .text_sm()
                        .font_semibold()
                        .text_color(solid(p.text_primary))
                        .child("No matching items"),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(solid(p.text_muted))
                        .child("Clear the filter or scan another drive."),
                )
                .into_any_element()
        } else {
            uniform_list(
                "da-tree-virtual-list",
                row_count,
                cx.processor(move |this, range: Range<usize>, _window, cx| {
                    render_tree_rows(this, range, cx)
                }),
            )
            .track_scroll(&da.tree_scroll)
            .flex_1()
            .w_full()
            .into_any_element()
        })
}

fn render_tree_header(
    sort: DiskSortSpec,
    p: Palette,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    h_flex()
        .w_full()
        .h(px(30.))
        .px_2()
        .flex_shrink_0()
        .items_center()
        .bg(solid(p.bg_elevated))
        .border_b_1()
        .border_color(solid(p.border_subtle))
        .child(tree_header_cell(
            "da-hdr-name",
            "Name",
            DiskSortColumn::Name,
            sort,
            None,
            false,
            p,
            cx,
        ))
        .child(tree_header_cell(
            "da-hdr-pct",
            "% of Parent",
            DiskSortColumn::PercentOfParent,
            sort,
            Some(120.),
            false,
            p,
            cx,
        ))
        .child(tree_header_cell(
            "da-hdr-size",
            "Total Size",
            DiskSortColumn::LogicalSize,
            sort,
            Some(96.),
            true,
            p,
            cx,
        ))
        .child(tree_header_cell(
            "da-hdr-alloc",
            "Allocated",
            DiskSortColumn::AllocatedSize,
            sort,
            Some(96.),
            true,
            p,
            cx,
        ))
        .child(tree_header_cell(
            "da-hdr-items",
            "Items",
            DiskSortColumn::ItemCount,
            sort,
            Some(88.),
            true,
            p,
            cx,
        ))
        .child(tree_header_cell(
            "da-hdr-mod",
            "Modified",
            DiskSortColumn::Modified,
            sort,
            Some(128.),
            true,
            p,
            cx,
        ))
}

#[allow(clippy::too_many_arguments)]
fn tree_header_cell(
    id: &'static str,
    label: &'static str,
    col: DiskSortColumn,
    sort: DiskSortSpec,
    fixed_width: Option<f32>,
    align_right: bool,
    p: Palette,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let is_active = sort.column == col;
    let base = div()
        .id(id)
        .h_full()
        .px_1p5()
        .flex()
        .items_center()
        .gap_1()
        .cursor_pointer()
        .text_xs()
        .font_semibold()
        .text_color(if is_active {
            solid(p.text_primary)
        } else {
            solid(p.text_secondary)
        })
        .hover(move |s| s.text_color(solid(p.text_primary)))
        .on_click(cx.listener(move |this, _, _, cx| {
            this.set_disk_sort_column(col, cx);
        }));

    let sized = match fixed_width {
        Some(w) => base.w(px(w)).flex_shrink_0(),
        None => base.flex_1().min_w(px(180.)),
    };
    let aligned = if align_right {
        sized.justify_end()
    } else {
        sized.justify_start()
    };

    aligned.child(label).when(is_active, |el| {
        el.child(
            Icon::new(if sort.descending {
                IconName::ArrowDown
            } else {
                IconName::ArrowUp
            })
            .xsmall(),
        )
    })
}

fn render_tree_rows(
    ws: &mut Workspace,
    range: Range<usize>,
    cx: &mut Context<Workspace>,
) -> Vec<impl IntoElement + use<>> {
    let p = ws.palette();
    let unit = ws.settings.size_unit;
    let da = &ws.disk_analyzer;
    let Some(tree) = &da.tree else {
        return Vec::new();
    };
    let selected = da.selected_node;
    let is_multi_root = tree.is_virtual_multi_root;
    let root_id = tree.root;

    range
        .filter_map(|ix| {
            let row = *da.visible_rows.get(ix)?;
            let node = tree.get(row.id)?;
            let node_id = row.id;
            let is_selected = selected == Some(node_id);
            let indent_px = f32::from(row.depth.min(18)) * 16.0;
            let is_denied = node.is_access_denied();
            let is_symlink = node.is_symlink();
            let is_dir = node.is_dir();

            let (icon_el, icon_color) = if is_multi_root && node_id == root_id {
                (Icon::new(LucideIcon::Monitor), p.accent_primary)
            } else if is_multi_root && node.parent == Some(root_id) {
                (Icon::new(LucideIcon::HardDrive), p.accent_primary)
            } else if is_denied {
                (Icon::new(LucideIcon::Lock), p.status_error)
            } else if is_symlink {
                (Icon::new(LucideIcon::Link), p.status_info)
            } else if is_dir {
                if row.is_expanded {
                    (Icon::new(LucideIcon::FolderOpen), p.accent_primary)
                } else {
                    (Icon::new(LucideIcon::Folder), p.accent_primary)
                }
            } else {
                (Icon::new(LucideIcon::File), p.text_secondary)
            };

            let name_ss = SharedString::new(Arc::clone(&node.name));
            let pct = row.percent_of_parent;
            let pct_label: SharedString = format!("{pct:.1}%").into();
            let size_label: SharedString = format_size(node.logical_size, unit).into();
            let alloc_label: SharedString = format_size(node.allocated_size, unit).into();
            let items_label: SharedString = if is_dir {
                format_count(node.total_items()).into()
            } else {
                "—".into()
            };
            let mod_label: SharedString = format_timestamp(node.modified_ts).into();
            let is_hidden = node.is_hidden_or_system();

            let row_bg = if is_selected {
                soft_tint(p.accent_primary, if p.is_dark { 0.18 } else { 0.14 })
            } else {
                gpui_kit::transparent_black()
            };

            Some(
                h_flex()
                    .id(("da-row", ix))
                    .w_full()
                    .h(px(ROW_HEIGHT))
                    .px_2()
                    .items_center()
                    .bg(row_bg)
                    .border_b_1()
                    .border_color(solid(p.border_subtle))
                    .cursor_pointer()
                    .hover(move |s| {
                        if is_selected {
                            s
                        } else {
                            s.bg(solid(p.bg_elevated))
                        }
                    })
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        if *hovered {
                            this.set_disk_list_hovered(Some(node_id), cx);
                        }
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                            if this.register_disk_node_click(node_id, event.click_count) {
                                if is_dir && row.has_children {
                                    this.toggle_disk_node_expand(node_id, cx);
                                    this.set_disk_treemap_root(node_id, cx);
                                } else {
                                    this.reveal_disk_node_in_explorer(node_id, window, cx);
                                }
                            } else {
                                this.select_disk_node(node_id, false, cx);
                            }
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.open_disk_node_context_menu(node_id, event.position, cx);
                        }),
                    )
                    // Name column with indentation & chevron
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w(px(180.))
                            .px_1p5()
                            .gap_1()
                            .items_center()
                            .overflow_hidden()
                            .child(div().w(px(indent_px)).flex_shrink_0())
                            .child(if row.has_children {
                                div()
                                    .id(("da-exp", ix))
                                    .w(px(16.))
                                    .h(px(16.))
                                    .flex()
                                    .flex_shrink_0()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(3.))
                                    .text_color(solid(p.text_muted))
                                    .hover(move |s| {
                                        s.bg(solid(p.bg_hover)).text_color(solid(p.text_primary))
                                    })
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.toggle_disk_node_expand(node_id, cx);
                                        }),
                                    )
                                    .child(
                                        Icon::new(if row.is_expanded {
                                            LucideIcon::ChevronDown
                                        } else {
                                            LucideIcon::ChevronRight
                                        })
                                        .xsmall(),
                                    )
                                    .into_any_element()
                            } else {
                                div().w(px(16.)).flex_shrink_0().into_any_element()
                            })
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .child(icon_el.xsmall().text_color(solid(icon_color))),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_xs()
                                    .when(is_dir, |d| d.font_medium())
                                    .text_color(if is_denied {
                                        solid(p.status_error)
                                    } else if is_hidden {
                                        solid(p.text_muted)
                                    } else {
                                        solid(p.text_primary)
                                    })
                                    .child(name_ss),
                            )
                            .when(is_denied, |el| {
                                el.child(
                                    div()
                                        .flex_shrink_0()
                                        .px_1()
                                        .rounded(px(3.))
                                        .bg(soft_tint(p.status_error, 0.14))
                                        .text_size(px(10.5))
                                        .text_color(solid(p.status_error))
                                        .child("Denied"),
                                )
                            }),
                    )
                    // % of Parent horizontal bar
                    .child(
                        h_flex()
                            .w(px(120.))
                            .flex_shrink_0()
                            .px_1p5()
                            .gap_1p5()
                            .items_center()
                            .child(
                                div()
                                    .flex_1()
                                    .h(px(6.))
                                    .rounded(px(2.))
                                    .bg(solid(p.bg_elevated))
                                    .overflow_hidden()
                                    .child(
                                        div()
                                            .h_full()
                                            .w(relative((pct / 100.0).clamp(0.0, 1.0)))
                                            .bg(solid(if pct >= 60.0 {
                                                p.status_warning
                                            } else {
                                                p.accent_primary
                                            })),
                                    ),
                            )
                            .child(
                                div()
                                    .w(px(38.))
                                    .flex_shrink_0()
                                    .text_right()
                                    .text_size(px(11.5))
                                    .text_color(solid(p.text_secondary))
                                    .child(pct_label),
                            ),
                    )
                    // Total Size
                    .child(
                        div()
                            .w(px(96.))
                            .flex_shrink_0()
                            .px_1p5()
                            .text_right()
                            .text_xs()
                            .font_medium()
                            .text_color(solid(p.text_primary))
                            .child(size_label),
                    )
                    // Allocated Size
                    .child(
                        div()
                            .w(px(96.))
                            .flex_shrink_0()
                            .px_1p5()
                            .text_right()
                            .text_xs()
                            .text_color(solid(p.text_secondary))
                            .child(alloc_label),
                    )
                    // Items
                    .child(
                        div()
                            .w(px(88.))
                            .flex_shrink_0()
                            .px_1p5()
                            .text_right()
                            .text_xs()
                            .text_color(solid(p.text_muted))
                            .child(items_label),
                    )
                    // Modified
                    .child(
                        div()
                            .w(px(128.))
                            .flex_shrink_0()
                            .px_1p5()
                            .text_right()
                            .text_xs()
                            .text_color(solid(p.text_muted))
                            .child(mod_label),
                    ),
            )
        })
        .collect()
}

// ================================================================================================
// GPU Squarified Treemap View (Dynamic Theme-Aware)
// ================================================================================================

fn render_treemap_pane(
    ws: &Workspace,
    p: Palette,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let da = &ws.disk_analyzer;
    let Some(tree) = da.tree.clone() else {
        return div().size_full().bg(solid(p.bg_base)).into_any_element();
    };

    let unit = ws.settings.size_unit;
    let show_hidden = ws.settings.show_hidden_files;
    let treemap_root = da
        .treemap_root
        .filter(|&id| tree.get(id).is_some_and(|n| !n.removed))
        .unwrap_or(tree.root);
    let can_go_up = treemap_root != tree.root
        && tree.get(treemap_root).and_then(|n| n.parent).is_some();

    let mut crumbs: Vec<(NodeId, SharedString)> = Vec::new();
    let mut cursor = Some(treemap_root);
    while let Some(cid) = cursor {
        if let Some(n) = tree.get(cid) {
            crumbs.push((cid, SharedString::new(Arc::clone(&n.name))));
            if cid == tree.root {
                break;
            }
            cursor = n.parent;
        } else {
            break;
        }
    }
    crumbs.reverse();

    let active_hover = da
        .treemap_hovered
        .or(da.list_hovered)
        .filter(|&hid| hid != treemap_root);
    let hover_from_list = da.treemap_hovered.is_none() && active_hover.is_some();

    let hover_info: Option<(Option<SharedString>, SharedString, SharedString)> =
        active_hover.and_then(|hid| {
            let node = tree.get(hid)?;
            let parent_prefix = tree.full_path(hid).and_then(|fp| {
                let parent = fp.parent()?;
                let s = parent.to_string_lossy();
                if s.is_empty() {
                    None
                } else {
                    Some(SharedString::from(format_parent_prefix(&s)))
                }
            });
            let item_name = SharedString::new(Arc::clone(&node.name));
            let root_sz = tree.get(treemap_root).map(|r| r.logical_size).unwrap_or(0);
            let pct = if root_sz == 0 {
                0.0
            } else {
                (node.logical_size as f64 / root_sz as f64 * 100.0).clamp(0.0, 100.0)
            };
            Some((
                parent_prefix,
                item_name,
                SharedString::from(format!(
                    "{} ({:.1}%)",
                    format_size(node.logical_size, unit),
                    pct
                )),
            ))
        });
    let has_hover_info = hover_info.is_some();

    let cache_for_canvas = da.treemap_cache.clone();
    let cache_for_paint = da.treemap_cache.clone();
    let tree_for_prepaint = tree.clone();
    let tree_for_paint = tree;
    let tree_version = da.tree_version;
    let selected_node = da.selected_node;
    let hovered_node = active_hover;
    let palette_key = (p.is_dark, p.text_primary, p.text_muted);

    v_flex()
        .size_full()
        .bg(solid(p.bg_base))
        .overflow_hidden()
        // Treemap Header Bar (Zoom Breadcrumbs + Live Hover Readout / Legend)
        .child(
            h_flex()
                .w_full()
                .h(px(28.))
                .px_2p5()
                .gap_2()
                .flex_shrink_0()
                .items_center()
                .justify_between()
                .bg(solid(p.bg_surface))
                .border_b_1()
                .border_color(solid(p.border_subtle))
                .child(
                    h_flex()
                        .min_w_0()
                        .when(!has_hover_info, |r| r.flex_1())
                        .when(has_hover_info, |r| r.flex_shrink_0().max_w(px(260.)))
                        .items_center()
                        .gap_1()
                        .overflow_hidden()
                        .when(can_go_up, |row| {
                            row.child(
                                Button::new("da-tm-up-btn")
                                    .with_size(Size::XSmall)
                                    .ghost()
                                    .icon(LucideIcon::ArrowUp)
                                    .tooltip("Zoom Out One Level")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.disk_treemap_zoom_up(cx);
                                    })),
                            )
                        })
                        .children(crumbs.into_iter().enumerate().map(|(ix, (cid, label))| {
                            let is_last = cid == treemap_root;
                            h_flex()
                                .items_center()
                                .gap_0p5()
                                .flex_shrink_0()
                                .when(ix > 0, |el| {
                                    el.child(
                                        Icon::new(LucideIcon::ChevronRight)
                                            .xsmall()
                                            .text_color(solid(p.text_muted)),
                                    )
                                })
                                .child(
                                    div()
                                        .id(("da-tm-crumb", ix))
                                        .px_1p5()
                                        .py_0p5()
                                        .rounded(px(RADIUS_CONTROL))
                                        .cursor_pointer()
                                        .text_xs()
                                        .when(is_last, |d| {
                                            d.font_semibold().text_color(solid(p.text_primary))
                                        })
                                        .when(!is_last, |d| {
                                            d.text_color(solid(p.text_secondary)).hover(move |s| {
                                                s.bg(solid(p.bg_elevated))
                                                    .text_color(solid(p.text_primary))
                                            })
                                        })
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.set_disk_treemap_root(cid, cx);
                                        }))
                                        .child(label),
                                )
                        })),
                )
                .child(
                    h_flex()
                        .when(has_hover_info, |r| r.flex_1().min_w_0().justify_end())
                        .when(!has_hover_info, |r| r.flex_shrink_0())
                        .items_center()
                        .gap_2()
                        .child(if let Some((parent_prefix, item_name, size_pct)) = hover_info {
                            h_flex()
                                .flex_1()
                                .min_w_0()
                                .justify_end()
                                .items_center()
                                .gap_2()
                                .child(
                                    h_flex()
                                        .min_w_0()
                                        .flex_1()
                                        .justify_end()
                                        .items_center()
                                        .overflow_hidden()
                                        .when_some(parent_prefix, |row, prefix| {
                                            row.child(
                                                div()
                                                    .min_w_0()
                                                    .flex_shrink(1.0)
                                                    .truncate()
                                                    .text_xs()
                                                    .text_color(solid(p.text_muted))
                                                    .child(prefix),
                                            )
                                        })
                                        .child(
                                            div()
                                                .flex_shrink_0()
                                                .max_w(px(320.))
                                                .truncate()
                                                .text_xs()
                                                .font_medium()
                                                .text_color(solid(p.text_primary))
                                                .child(item_name),
                                        ),
                                )
                                .child(
                                    div()
                                        .flex_shrink_0()
                                        .text_xs()
                                        .font_semibold()
                                        .text_color(solid(p.accent_primary))
                                        .child(size_pct),
                                )
                                .into_any_element()
                        } else {
                            h_flex()
                                .items_center()
                                .gap_2()
                                .children(TREEMAP_LEGEND_ITEMS.into_iter().map(|(label, color)| {
                                    h_flex()
                                        .items_center()
                                        .gap_1()
                                        .child(
                                            div()
                                                .w(px(7.))
                                                .h(px(7.))
                                                .rounded(px(2.))
                                                .bg(solid(color)),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(solid(p.text_muted))
                                                .child(label),
                                        )
                                }))
                                .into_any_element()
                        })
                        .child(
                            Button::new("da-tm-hide-btn")
                                .with_size(Size::XSmall)
                                .ghost()
                                .icon(IconName::Close)
                                .tooltip("Hide Treemap")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.toggle_disk_treemap(cx);
                                })),
                        ),
                ),
        )
        // GPU Canvas
        .child(
            div()
                .id("da-treemap-canvas-wrap")
                .flex_1()
                .min_h_0()
                .w_full()
                .bg(solid(p.bg_base))
                .cursor_pointer()
                .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                    if !*hovered {
                        this.clear_disk_treemap_hover(cx);
                    }
                }))
                .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                    this.handle_disk_treemap_hover(event.position, cx);
                }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, event: &MouseDownEvent, _, cx| {
                        this.handle_disk_treemap_click(event.position, event.click_count, cx);
                    }),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|this, event: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        this.handle_disk_treemap_right_click(event.position, cx);
                    }),
                )
                .child(
                    canvas(
                        move |bounds: Bounds<Pixels>, window, _cx| {
                            let w = f32::from(bounds.size.width).max(0.0);
                            let h = f32::from(bounds.size.height).max(0.0);
                            let mut cache = cache_for_canvas.borrow_mut();
                            cache.bounds = bounds;
                            cache.origin = bounds.origin;
                            let needs_recompute = cache.tree_version != tree_version
                                || cache.root_id != treemap_root
                                || cache.show_hidden != show_hidden
                                || cache.size_unit != unit
                                || cache.palette_key != palette_key
                                || (cache.width - w).abs() > 1.5
                                || (cache.height - h).abs() > 1.5;
                            if needs_recompute {
                                cache.layout = TreemapLayout::compute(
                                    &tree_for_prepaint,
                                    treemap_root,
                                    w,
                                    h,
                                    show_hidden,
                                );

                                let base_font = window.text_style().font();
                                let mut bold_font = base_font.clone();
                                bold_font.weight = FontWeight::SEMIBOLD;
                                let font_size = px(10.5);
                                let text_sys = window.text_system();

                                cache.shaped_headers = cache
                                    .layout
                                    .headers
                                    .iter()
                                    .filter_map(|hdr| {
                                        let node = tree_for_prepaint.get(hdr.node_id)?;
                                        let name_str = node.name.replace('\n', " ");
                                        let show_size = hdr.w >= 135.0;
                                        let (full_text, runs) = if show_size {
                                            let size_suffix = format!(
                                                "  {}",
                                                format_size(node.logical_size, unit)
                                            );
                                            let name_len = name_str.len();
                                            let suffix_len = size_suffix.len();
                                            let combined = format!("{name_str}{size_suffix}");
                                            (
                                                SharedString::from(combined),
                                                vec![
                                                    TextRun {
                                                        len: name_len,
                                                        font: bold_font.clone(),
                                                        color: solid(p.text_primary),
                                                        background_color: None,
                                                        underline: None,
                                                        strikethrough: None,
                                                    },
                                                    TextRun {
                                                        len: suffix_len,
                                                        font: base_font.clone(),
                                                        color: solid(p.text_muted),
                                                        background_color: None,
                                                        underline: None,
                                                        strikethrough: None,
                                                    },
                                                ],
                                            )
                                        } else {
                                            let name_len = name_str.len();
                                            (
                                                SharedString::from(name_str),
                                                vec![TextRun {
                                                    len: name_len,
                                                    font: bold_font.clone(),
                                                    color: solid(p.text_primary),
                                                    background_color: None,
                                                    underline: None,
                                                    strikethrough: None,
                                                }],
                                            )
                                        };
                                        let line = text_sys.shape_line(
                                            full_text,
                                            font_size,
                                            &runs,
                                            None,
                                        );
                                        Some(ShapedFolderHeader {
                                            x: hdr.x,
                                            y: hdr.y,
                                            w: hdr.w,
                                            h: hdr.h,
                                            line,
                                        })
                                    })
                                    .collect();

                                cache.tree_version = tree_version;
                                cache.root_id = treemap_root;
                                cache.show_hidden = show_hidden;
                                cache.size_unit = unit;
                                cache.palette_key = palette_key;
                                cache.width = w;
                                cache.height = h;
                            }
                        },
                        move |bounds: Bounds<Pixels>, (), window, cx| {
                            window.paint_quad(fill(bounds, solid(p.bg_base)));

                            let cache = cache_for_paint.borrow();
                            let ox = f32::from(bounds.origin.x);
                            let oy = f32::from(bounds.origin.y);

                            // 1. Folder container plates adapted to active theme palette
                            for frame in &cache.layout.frames {
                                let rect = Bounds {
                                    origin: point(px(ox + frame.x), px(oy + frame.y)),
                                    size: GpuiSize {
                                        width: px(frame.w),
                                        height: px(frame.h),
                                    },
                                };
                                let (fill_col, border_col) = if frame.depth == 1 {
                                    (solid(p.bg_surface), solid(p.border))
                                } else {
                                    (solid(p.bg_elevated), solid(p.border_subtle))
                                };
                                window.paint_quad(PaintQuad {
                                    bounds: rect,
                                    corner_radii: (0.).into(),
                                    background: fill_col.into(),
                                    border_widths: px(1.).into(),
                                    border_color: border_col,
                                    border_style: BorderStyle::Solid,
                                });
                            }

                            // 2. Inline depth-1 folder headers
                            for hdr in &cache.shaped_headers {
                                let hdr_rect = Bounds {
                                    origin: point(px(ox + hdr.x), px(oy + hdr.y)),
                                    size: GpuiSize {
                                        width: px(hdr.w),
                                        height: px(hdr.h),
                                    },
                                };
                                window.paint_quad(fill(hdr_rect, solid(p.bg_elevated)));
                                let div_rect = Bounds {
                                    origin: point(px(ox + hdr.x), px(oy + hdr.y + hdr.h - 1.0)),
                                    size: GpuiSize {
                                        width: px(hdr.w),
                                        height: px(1.0),
                                    },
                                };
                                window.paint_quad(fill(div_rect, solid(p.border_subtle)));

                                let clip_rect = Bounds {
                                    origin: point(px(ox + hdr.x + 4.0), px(oy + hdr.y)),
                                    size: GpuiSize {
                                        width: px((hdr.w - 8.0).max(0.0)),
                                        height: px(hdr.h),
                                    },
                                };
                                window.with_content_mask(
                                    Some(ContentMask { bounds: clip_rect }),
                                    |window| {
                                        let _ = hdr.line.paint(
                                            point(px(ox + hdr.x + 5.0), px(oy + hdr.y + 2.0)),
                                            px(14.0),
                                            TextAlign::Left,
                                            None,
                                            window,
                                            cx,
                                        );
                                    },
                                );
                            }

                            // 3. Leaf tiles inset 0.5px for razor-sharp 1px seams
                            for tile in &cache.layout.leaves {
                                let inset = if tile.w > 4.0 && tile.h > 4.0 { 0.5 } else { 0.0 };
                                let tw = (tile.w - inset * 2.0).max(1.0);
                                let th = (tile.h - inset * 2.0).max(1.0);
                                let rect = Bounds {
                                    origin: point(px(ox + tile.x + inset), px(oy + tile.y + inset)),
                                    size: GpuiSize {
                                        width: px(tw),
                                        height: px(th),
                                    },
                                };
                                window.paint_quad(fill(rect, solid(tile.color_hex)));
                            }

                            // 4. Crisp depth-1 outlines
                            for frame in &cache.layout.frames {
                                if frame.depth == 1 {
                                    let rect = Bounds {
                                        origin: point(px(ox + frame.x), px(oy + frame.y)),
                                        size: GpuiSize {
                                            width: px(frame.w),
                                            height: px(frame.h),
                                        },
                                    };
                                    window.paint_quad(outline(
                                        rect,
                                        solid(p.border),
                                        BorderStyle::Solid,
                                    ));
                                }
                            }

                            // 5. Selected node highlight
                            if let Some(sel_id) = selected_node
                                && sel_id != treemap_root
                                && let Some(sel) =
                                    cache.layout.highlight_rect_for_node(&tree_for_paint, sel_id)
                            {
                                let rect = Bounds {
                                    origin: point(px(ox + sel.x), px(oy + sel.y)),
                                    size: GpuiSize {
                                        width: px(sel.w.max(2.0)),
                                        height: px(sel.h.max(2.0)),
                                    },
                                };
                                window.paint_quad(PaintQuad {
                                    bounds: rect,
                                    corner_radii: (0.).into(),
                                    background: soft_tint(p.accent_primary, 0.14).into(),
                                    border_widths: px(2.).into(),
                                    border_color: solid(p.accent_primary),
                                    border_style: BorderStyle::Solid,
                                });
                            }

                            // 6. Hovered node highlight
                            if let Some(hov_id) = hovered_node
                                && let Some(hov) =
                                    cache.layout.highlight_rect_for_node(&tree_for_paint, hov_id)
                            {
                                let rect = Bounds {
                                    origin: point(px(ox + hov.x), px(oy + hov.y)),
                                    size: GpuiSize {
                                        width: px(hov.w.max(2.0)),
                                        height: px(hov.h.max(2.0)),
                                    },
                                };
                                window.paint_quad(PaintQuad {
                                    bounds: rect,
                                    corner_radii: (0.).into(),
                                    background: soft_tint(
                                        p.accent_primary,
                                        if hover_from_list { 0.24 } else { 0.14 },
                                    )
                                    .into(),
                                    border_widths: if hover_from_list {
                                        px(2.).into()
                                    } else {
                                        px(1.5).into()
                                    },
                                    border_color: solid(p.text_primary),
                                    border_style: BorderStyle::Solid,
                                });
                            }
                        },
                    )
                    .size_full(),
                ),
        )
        .into_any_element()
}

// ================================================================================================
// Top Files View
// ================================================================================================

fn render_top_files_view(
    ws: &Workspace,
    p: Palette,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let da = &ws.disk_analyzer;
    let count = da.top_files.len();

    v_flex()
        .id("da-top-files-root")
        .size_full()
        .bg(solid(p.bg_base))
        .overflow_hidden()
        .child(
            h_flex()
                .w_full()
                .h(px(30.))
                .px_2()
                .flex_shrink_0()
                .items_center()
                .bg(solid(p.bg_elevated))
                .border_b_1()
                .border_color(solid(p.border_subtle))
                .child(col_hdr_static("#", Some(46.), false, p))
                .child(col_hdr_static("File / Path", None, false, p))
                .child(col_hdr_static("Type", Some(76.), false, p))
                .child(col_hdr_static("Size", Some(96.), true, p))
                .child(col_hdr_static("Allocated", Some(96.), true, p))
                .child(col_hdr_static("Modified", Some(128.), true, p)),
        )
        .child(if count == 0 {
            v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .gap_1()
                .child(
                    div()
                        .text_sm()
                        .font_semibold()
                        .text_color(solid(p.text_primary))
                        .child("No matching files"),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(solid(p.text_muted))
                        .child("Clear active filters to inspect the largest files."),
                )
                .into_any_element()
        } else {
            uniform_list(
                "da-top-files-list",
                count,
                cx.processor(move |this, range: Range<usize>, _window, cx| {
                    render_top_file_rows(this, range, cx)
                }),
            )
            .track_scroll(&da.files_scroll)
            .flex_1()
            .w_full()
            .into_any_element()
        })
}

fn render_top_file_rows(
    ws: &mut Workspace,
    range: Range<usize>,
    cx: &mut Context<Workspace>,
) -> Vec<impl IntoElement + use<>> {
    let p = ws.palette();
    let unit = ws.settings.size_unit;
    let da = &ws.disk_analyzer;
    let Some(tree) = &da.tree else {
        return Vec::new();
    };
    let selected = da.selected_node;

    range
        .filter_map(|ix| {
            let node_id = *da.top_files.get(ix)?;
            let node = tree.get(node_id)?;
            let is_selected = selected == Some(node_id);
            let ext = tree.ext_str(node_id);
            let color = color_for_extension(ext);
            let ext_badge: SharedString = if ext.is_empty() {
                "—".into()
            } else {
                format!(".{ext}").into()
            };
            let parent_text: SharedString = tree
                .full_path(node_id)
                .and_then(|fp| fp.parent().map(|pr| pr.to_string_lossy().into_owned()))
                .unwrap_or_default()
                .into();
            let name_ss = SharedString::new(Arc::clone(&node.name));
            let size_ss: SharedString = format_size(node.logical_size, unit).into();
            let alloc_ss: SharedString = format_size(node.allocated_size, unit).into();
            let mod_ss: SharedString = format_timestamp(node.modified_ts).into();
            let rank_ss: SharedString = format!("{}", ix + 1).into();

            Some(
                h_flex()
                    .id(("da-file-row", ix))
                    .w_full()
                    .h(px(ROW_HEIGHT))
                    .px_2()
                    .items_center()
                    .bg(if is_selected {
                        soft_tint(p.accent_primary, if p.is_dark { 0.18 } else { 0.14 })
                    } else {
                        gpui_kit::transparent_black()
                    })
                    .border_b_1()
                    .border_color(solid(p.border_subtle))
                    .cursor_pointer()
                    .hover(move |s| {
                        if is_selected {
                            s
                        } else {
                            s.bg(solid(p.bg_elevated))
                        }
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                            if this.register_disk_node_click(node_id, event.click_count) {
                                this.reveal_disk_node_in_explorer(node_id, window, cx);
                            } else {
                                this.select_disk_node(node_id, false, cx);
                            }
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.open_disk_node_context_menu(node_id, event.position, cx);
                        }),
                    )
                    .child(
                        div()
                            .w(px(46.))
                            .flex_shrink_0()
                            .px_1p5()
                            .text_xs()
                            .text_color(solid(p.text_muted))
                            .child(rank_ss),
                    )
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w(px(180.))
                            .px_1p5()
                            .gap_2()
                            .items_center()
                            .overflow_hidden()
                            .child(
                                div()
                                    .w(px(7.))
                                    .h(px(7.))
                                    .flex_shrink_0()
                                    .rounded(px(2.))
                                    .bg(solid(color)),
                            )
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .max_w(px(300.))
                                    .truncate()
                                    .text_xs()
                                    .font_medium()
                                    .text_color(solid(p.text_primary))
                                    .child(name_ss),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_xs()
                                    .text_color(solid(p.text_muted))
                                    .child(parent_text),
                            ),
                    )
                    .child(
                        div()
                            .w(px(76.))
                            .flex_shrink_0()
                            .px_1p5()
                            .truncate()
                            .text_xs()
                            .text_color(solid(p.text_secondary))
                            .child(ext_badge),
                    )
                    .child(
                        div()
                            .w(px(96.))
                            .flex_shrink_0()
                            .px_1p5()
                            .text_right()
                            .text_xs()
                            .font_medium()
                            .text_color(solid(p.text_primary))
                            .child(size_ss),
                    )
                    .child(
                        div()
                            .w(px(96.))
                            .flex_shrink_0()
                            .px_1p5()
                            .text_right()
                            .text_xs()
                            .text_color(solid(p.text_secondary))
                            .child(alloc_ss),
                    )
                    .child(
                        div()
                            .w(px(128.))
                            .flex_shrink_0()
                            .px_1p5()
                            .text_right()
                            .text_xs()
                            .text_color(solid(p.text_muted))
                            .child(mod_ss),
                    ),
            )
        })
        .collect()
}

// ================================================================================================
// Extensions Breakdown View
// ================================================================================================

fn render_extensions_view(
    ws: &Workspace,
    p: Palette,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let da = &ws.disk_analyzer;
    let count = da.ext_stats.len();

    v_flex()
        .id("da-extensions-root")
        .size_full()
        .bg(solid(p.bg_base))
        .overflow_hidden()
        .child(
            h_flex()
                .w_full()
                .h(px(30.))
                .px_2()
                .flex_shrink_0()
                .items_center()
                .bg(solid(p.bg_elevated))
                .border_b_1()
                .border_color(solid(p.border_subtle))
                .child(col_hdr_static("Extension", Some(160.), false, p))
                .child(col_hdr_static("% of Total", None, false, p))
                .child(col_hdr_static("Total Size", Some(110.), true, p))
                .child(col_hdr_static("Allocated", Some(110.), true, p))
                .child(col_hdr_static("Files", Some(96.), true, p)),
        )
        .child(if count == 0 {
            v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .text_xs()
                        .text_color(solid(p.text_muted))
                        .child("No files found in scan."),
                )
                .into_any_element()
        } else {
            uniform_list(
                "da-extensions-list",
                count,
                cx.processor(move |this, range: Range<usize>, _window, cx| {
                    render_extension_rows(this, range, cx)
                }),
            )
            .track_scroll(&da.ext_scroll)
            .flex_1()
            .w_full()
            .into_any_element()
        })
}

fn render_extension_rows(
    ws: &mut Workspace,
    range: Range<usize>,
    cx: &mut Context<Workspace>,
) -> Vec<impl IntoElement + use<>> {
    let p = ws.palette();
    let unit = ws.settings.size_unit;
    let da = &ws.disk_analyzer;

    range
        .filter_map(|ix| {
            let stat = da.ext_stats.get(ix)?;
            let raw_ext = stat.ext.trim_start_matches('.').to_string();
            let color = if stat.ext == "(no extension)" {
                0x52525b
            } else {
                color_for_extension(&raw_ext)
            };
            let ext_ss: SharedString = stat.ext.clone().into();
            let pct = stat.percent_of_total;
            let pct_ss: SharedString = format!("{pct:.1}%").into();
            let size_ss: SharedString = format_size(stat.logical_size, unit).into();
            let alloc_ss: SharedString = format_size(stat.allocated_size, unit).into();
            let count_ss: SharedString = format_count(stat.file_count).into();
            let clickable_ext = if stat.ext == "(no extension)" {
                None
            } else {
                Some(raw_ext)
            };

            Some(
                h_flex()
                    .id(("da-ext-row", ix))
                    .w_full()
                    .h(px(ROW_HEIGHT))
                    .px_2()
                    .items_center()
                    .border_b_1()
                    .border_color(solid(p.border_subtle))
                    .cursor_pointer()
                    .hover(move |s| s.bg(solid(p.bg_elevated)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            if let Some(ref ext) = clickable_ext {
                                this.set_disk_ext_filter(Some(ext.clone()), true, cx);
                            }
                        }),
                    )
                    .child(
                        h_flex()
                            .w(px(160.))
                            .flex_shrink_0()
                            .px_1p5()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .w(px(8.))
                                    .h(px(8.))
                                    .flex_shrink_0()
                                    .rounded(px(2.))
                                    .bg(solid(color)),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_xs()
                                    .font_medium()
                                    .text_color(solid(p.text_primary))
                                    .child(ext_ss),
                            ),
                    )
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w(px(140.))
                            .px_1p5()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .flex_1()
                                    .h(px(7.))
                                    .rounded(px(2.))
                                    .bg(solid(p.bg_elevated))
                                    .overflow_hidden()
                                    .child(
                                        div()
                                            .h_full()
                                            .w(relative((pct / 100.0).clamp(0.0, 1.0)))
                                            .bg(solid(color)),
                                    ),
                            )
                            .child(
                                div()
                                    .w(px(44.))
                                    .flex_shrink_0()
                                    .text_right()
                                    .text_xs()
                                    .text_color(solid(p.text_secondary))
                                    .child(pct_ss),
                            ),
                    )
                    .child(
                        div()
                            .w(px(110.))
                            .flex_shrink_0()
                            .px_1p5()
                            .text_right()
                            .text_xs()
                            .font_medium()
                            .text_color(solid(p.text_primary))
                            .child(size_ss),
                    )
                    .child(
                        div()
                            .w(px(110.))
                            .flex_shrink_0()
                            .px_1p5()
                            .text_right()
                            .text_xs()
                            .text_color(solid(p.text_secondary))
                            .child(alloc_ss),
                    )
                    .child(
                        div()
                            .w(px(96.))
                            .flex_shrink_0()
                            .px_1p5()
                            .text_right()
                            .text_xs()
                            .text_color(solid(p.text_muted))
                            .child(count_ss),
                    ),
            )
        })
        .collect()
}

fn col_hdr_static(
    label: &'static str,
    fixed_width: Option<f32>,
    align_right: bool,
    p: Palette,
) -> impl IntoElement {
    let base = div()
        .h_full()
        .px_1p5()
        .flex()
        .items_center()
        .text_xs()
        .font_semibold()
        .text_color(solid(p.text_secondary));
    let sized = match fixed_width {
        Some(w) => base.w(px(w)).flex_shrink_0(),
        None => base.flex_1().min_w(px(140.)),
    };
    let aligned = if align_right {
        sized.justify_end()
    } else {
        sized.justify_start()
    };
    aligned.child(label)
}

// ================================================================================================
// Compact Bottom Summary Strip
// ================================================================================================

fn render_summary_footer(
    ws: &Workspace,
    p: Palette,
    unit: SizeUnit,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let da = &ws.disk_analyzer;
    let Some(tree) = &da.tree else {
        return div().into_any_element();
    };
    let root = tree.get(tree.root);
    let total_logical = root.map(|r| r.logical_size).unwrap_or(0);
    let total_alloc = root.map(|r| r.allocated_size).unwrap_or(0);
    let summary: SharedString = format!(
        "{} ({} allocated)  •  {} files, {} folders  •  {:.2}s ({})",
        format_size(total_logical, unit),
        format_size(total_alloc, unit),
        format_count(tree.total_files),
        format_count(tree.total_dirs),
        da.elapsed_ms as f64 / 1000.0,
        tree.engine.label(),
    )
    .into();
    let denied = tree.total_denied;

    h_flex()
        .w_full()
        .h(px(28.))
        .px_3()
        .gap_2()
        .flex_shrink_0()
        .items_center()
        .justify_between()
        .bg(solid(p.bg_surface))
        .border_t_1()
        .border_color(solid(p.border_subtle))
        .child(
            div()
                .truncate()
                .text_size(px(12.5))
                .text_color(solid(p.text_muted))
                .child(summary),
        )
        .when(denied > 0, |el| {
            el.child(
                h_flex()
                    .id("da-footer-denied")
                    .px_1p5()
                    .py_0p5()
                    .gap_1()
                    .flex_shrink_0()
                    .items_center()
                    .rounded(px(3.))
                    .cursor_pointer()
                    .bg(soft_tint(p.status_error, 0.12))
                    .text_size(px(11.5))
                    .text_color(solid(p.status_error))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.toggle_disk_denied_only(cx);
                    }))
                    .child(Icon::new(LucideIcon::Lock).xsmall())
                    .child(format!("{denied} access denied")),
            )
        })
        .into_any_element()
}

fn format_count(n: u64) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, &b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(b as char);
    }
    out
}

fn format_parent_prefix(parent: &str) -> String {
    let sep = if cfg!(target_os = "windows") { '\\' } else { '/' };
    let trimmed = parent.trim_end_matches(['\\', '/']);
    let with_sep = if trimmed.is_empty() {
        sep.to_string()
    } else {
        format!("{trimmed}{sep}")
    };

    if with_sep.chars().count() <= 54 {
        return with_sep;
    }

    let parts: Vec<&str> = trimmed.split(['\\', '/']).filter(|s| !s.is_empty()).collect();
    if parts.len() <= 4 {
        return with_sep;
    }

    let head = if parent.starts_with('/') {
        format!("/{}/{}/", parts[0], parts[1])
    } else {
        format!("{}{sep}{}{sep}", parts[0], parts[1])
    };
    let tail_a = parts[parts.len() - 2];
    let tail_b = parts[parts.len() - 1];
    format!("{head}…{sep}{tail_a}{sep}{tail_b}{sep}")
}
