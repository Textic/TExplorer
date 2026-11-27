use std::ops::Range;
use std::path::PathBuf;

use gpui_kit::{
    Context, ExternalPaths, MouseButton, MouseDownEvent, ObjectFit, Pixels, Point, Render,
    SharedString, Window,
    assets::IconName as LucideIcon,
    component::{
        Icon, Sizable as _, Size, StyledExt as _, h_flex, scroll::ScrollableElement as _, v_flex,
    },
    div, img, prelude::*, px, relative, uniform_list,
};
use texplorer_core::{EntryFlags, Location, PaneSide, SortColumn, SortSpec};

use crate::theme::{
    Palette, RADIUS_CONTROL, RADIUS_ROW, drive_kind_icon, file_kind_visual, format_size,
    format_timestamp, known_folder_icon, soft_tint, solid, usage_bar_color,
};
use crate::ui::context_menu::ContextMenuTarget;
use crate::ui::workspace::{ColumnDivider, Workspace, is_valid_internal_drop_target};

const LIST_MARQUEE_GUTTER_PX: f32 = 44.0;
const DETAILS_MIN_NAME_PX: f32 = 160.0;
const DETAILS_MIN_MODIFIED_PX: f32 = 118.0;
const DETAILS_MIN_TYPE_PX: f32 = 84.0;
const DETAILS_MIN_SIZE_PX: f32 = 72.0;
const DETAILS_ROW_CHROME_PX: f32 = 28.0;

/// Responsive column visibility and pixel widths for Details view so `Name` never collides with
/// `Date Modified`, `Type`, or `Size` in narrow split panes or when the Inspector/Sidebar is open.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DetailsColumnLayout {
    pub show_modified: bool,
    pub show_type: bool,
    pub show_size: bool,
    pub mod_w: f32,
    pub type_w: f32,
    pub size_w: f32,
    pub gutter_w: f32,
}

pub fn compute_details_column_layout(
    pane_width: f32,
    pref_mod: f32,
    pref_type: f32,
    pref_size: f32,
) -> DetailsColumnLayout {
    let avail = (pane_width - DETAILS_ROW_CHROME_PX).max(100.0);
    let full_need = DETAILS_MIN_NAME_PX + pref_mod + pref_type + pref_size + LIST_MARQUEE_GUTTER_PX;
    if avail >= full_need {
        return DetailsColumnLayout {
            show_modified: true,
            show_type: true,
            show_size: true,
            mod_w: pref_mod,
            type_w: pref_type,
            size_w: pref_size,
            gutter_w: LIST_MARQUEE_GUTTER_PX,
        };
    }

    // Step 1: Fit all 4 columns by reducing the right gutter and smoothly compressing right columns
    // toward their readable minimums while preserving at least `DETAILS_MIN_NAME_PX` for Name.
    let compact_gutter = 24.0;
    let min_all_four = DETAILS_MIN_NAME_PX
        + DETAILS_MIN_MODIFIED_PX
        + DETAILS_MIN_TYPE_PX
        + DETAILS_MIN_SIZE_PX
        + compact_gutter;
    if avail >= min_all_four {
        let budget = (avail - DETAILS_MIN_NAME_PX - compact_gutter).max(0.0);
        let min_sum = DETAILS_MIN_MODIFIED_PX + DETAILS_MIN_TYPE_PX + DETAILS_MIN_SIZE_PX;
        let pref_sum = pref_mod + pref_type + pref_size;
        let extra = (budget - min_sum).max(0.0);
        let span = (pref_sum - min_sum).max(1.0);
        let t = (extra / span).clamp(0.0, 1.0);
        let mod_w = (DETAILS_MIN_MODIFIED_PX + (pref_mod - DETAILS_MIN_MODIFIED_PX) * t).floor();
        let type_w = (DETAILS_MIN_TYPE_PX + (pref_type - DETAILS_MIN_TYPE_PX) * t).floor();
        let size_w = (DETAILS_MIN_SIZE_PX + (pref_size - DETAILS_MIN_SIZE_PX) * t).floor();
        return DetailsColumnLayout {
            show_modified: true,
            show_type: true,
            show_size: true,
            mod_w,
            type_w,
            size_w,
            gutter_w: compact_gutter,
        };
    }

    // Step 2: Hide `Type` first when a split pane + side panel leaves limited width.
    let mod_size_gutter = 20.0;
    let min_mod_size =
        DETAILS_MIN_NAME_PX + DETAILS_MIN_MODIFIED_PX + DETAILS_MIN_SIZE_PX + mod_size_gutter;
    if avail >= min_mod_size {
        let budget = (avail - DETAILS_MIN_NAME_PX - mod_size_gutter).max(0.0);
        let min_sum = DETAILS_MIN_MODIFIED_PX + DETAILS_MIN_SIZE_PX;
        let pref_sum = pref_mod + pref_size;
        let extra = (budget - min_sum).max(0.0);
        let span = (pref_sum - min_sum).max(1.0);
        let t = (extra / span).clamp(0.0, 1.0);
        let mod_w = (DETAILS_MIN_MODIFIED_PX + (pref_mod - DETAILS_MIN_MODIFIED_PX) * t).floor();
        let size_w = (DETAILS_MIN_SIZE_PX + (pref_size - DETAILS_MIN_SIZE_PX) * t).floor();
        return DetailsColumnLayout {
            show_modified: true,
            show_type: false,
            show_size: true,
            mod_w,
            type_w: 0.0,
            size_w,
            gutter_w: mod_size_gutter,
        };
    }

    // Step 3: Hide `Date Modified`, keeping `Name` + `Size`.
    let size_gutter = 16.0;
    let min_size_only = DETAILS_MIN_NAME_PX + DETAILS_MIN_SIZE_PX + size_gutter;
    if avail >= min_size_only {
        let size_w = pref_size.min((avail - DETAILS_MIN_NAME_PX - size_gutter).max(DETAILS_MIN_SIZE_PX));
        return DetailsColumnLayout {
            show_modified: false,
            show_type: false,
            show_size: true,
            mod_w: 0.0,
            type_w: 0.0,
            size_w: size_w.floor(),
            gutter_w: size_gutter,
        };
    }

    // Step 4: Ultra-narrow fallback — `Name` only.
    DetailsColumnLayout {
        show_modified: false,
        show_type: false,
        show_size: false,
        mod_w: 0.0,
        type_w: 0.0,
        size_w: 0.0,
        gutter_w: 12.0,
    }
}

/// Drag payload for internal file/folder drag & drop (into folders or across Dual-Pane).
#[derive(Clone)]
pub struct DraggedEntries {
    pub paths: Vec<PathBuf>,
    pub label: SharedString,
}

/// Drag payload for extracting entries directly out of a browsed archive (`Location::Archive`)
/// into a local folder or opposite Dual-Pane via Drag & Drop.
#[derive(Clone)]
pub struct DraggedArchiveEntries {
    pub archive_path: PathBuf,
    pub inner_paths: Vec<String>,
    pub label: SharedString,
}

struct DragBadgeView {
    label: SharedString,
    palette: Palette,
    cursor_offset: Point<Pixels>,
}

impl Render for DragBadgeView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.palette;
        // GPUI roots the drag view at `(mouse_position - cursor_offset)`, so shifting by
        // `cursor_offset + (14, 12)` anchors the pill right next to the mouse cursor regardless
        // of which column (Name, Date Modified, Type, or Size) initiated the drag.
        h_flex()
            .relative()
            .left(self.cursor_offset.x + px(14.))
            .top(self.cursor_offset.y + px(12.))
            .px_2p5()
            .py_1()
            .gap_1p5()
            .items_center()
            .rounded(px(RADIUS_CONTROL))
            .bg(solid(p.bg_elevated))
            .border_1()
            .border_color(solid(p.accent_primary))
            .child(
                Icon::new(LucideIcon::FolderOpen)
                    .xsmall()
                    .text_color(solid(p.accent_primary)),
            )
            .child(
                div()
                    .text_xs()
                    .font_medium()
                    .text_color(solid(p.text_primary))
                    .child(self.label.clone()),
            )
    }
}

/// Renders the "Home / This PC" overview dashboard with Quick Access cards and Drive cards.
pub fn render_home_view(
    ws: &Workspace,
    side: PaneSide,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let p = ws.palette();
    let unit = ws.settings.size_unit;
    let split_enabled = ws.active_tab().is_some_and(|t| t.model.split_enabled);
    let inner_w = (ws.estimated_pane_width(split_enabled) - 48.0).max(220.0);
    let visible_quick_access: Vec<_> = ws
        .settings
        .ordered_quick_access(&ws.known_folders)
        .into_iter()
        .filter(|f| ws.settings.is_quick_access_visible(f.kind))
        .collect();
    let qa_cols = (((inner_w + 10.0) / 230.0).floor().clamp(1.0, 5.0) as u16)
        .min(visible_quick_access.len().max(1) as u16)
        .max(1);
    let drv_cols = (((inner_w + 14.0) / 280.0).floor().clamp(1.0, 4.0) as u16)
        .min(ws.volumes.len().max(1) as u16)
        .max(1);

    v_flex()
        .id(match side {
            PaneSide::Left => "home-view-left",
            PaneSide::Right => "home-view-right",
        })
        .size_full()
        .bg(solid(p.bg_base))
        .overflow_y_scrollbar()
        .p_5()
        .gap_6()
        .when(!visible_quick_access.is_empty(), |el| {
            el.child(
                v_flex()
                    .gap_2p5()
                    .child(home_section_header("Quick Access", p))
                    .child(
                        div()
                            .w_full()
                            .grid()
                            .grid_cols(qa_cols)
                            .gap_2()
                            .children(visible_quick_access.into_iter().enumerate().map(
                                |(i, folder)| {
                                    let target = Location::local(&folder.path);
                                    let mid_target = target.clone();
                                    let folder_ctx = folder.clone();
                                    let icon = known_folder_icon(folder.kind);
                                    let name: SharedString = folder.name.clone().into();
                                    let sub: SharedString =
                                        folder.path.to_string_lossy().into_owned().into();

                                    h_flex()
                                        .id((
                                            match side {
                                                PaneSide::Left => "home-qa-l",
                                                PaneSide::Right => "home-qa-r",
                                            },
                                            i,
                                        ))
                                        .w_full()
                                        .min_w_0()
                                        .p_2p5()
                                        .gap_2p5()
                                        .items_center()
                                        .rounded(px(RADIUS_ROW))
                                        .cursor_pointer()
                                        .bg(solid(p.bg_surface))
                                        .border_1()
                                        .border_color(solid(p.border_subtle))
                                        .hover(move |s| {
                                            s.bg(solid(p.bg_elevated))
                                                .border_color(solid(p.border))
                                        })
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(
                                                move |this, _: &MouseDownEvent, window, cx| {
                                                    this.navigate_pane(
                                                        side,
                                                        target.clone(),
                                                        window,
                                                        cx,
                                                    );
                                                },
                                            ),
                                        )
                                        .on_mouse_down(
                                            MouseButton::Middle,
                                            cx.listener(
                                                move |this, _: &MouseDownEvent, window, cx| {
                                                    if this.settings.show_tabs {
                                                        this.open_new_tab(
                                                            mid_target.clone(),
                                                            window,
                                                            cx,
                                                        );
                                                    } else {
                                                        this.navigate_pane(
                                                            side,
                                                            mid_target.clone(),
                                                            window,
                                                            cx,
                                                        );
                                                    }
                                                },
                                            ),
                                        )
                                        .on_mouse_down(
                                            MouseButton::Right,
                                            cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                                                this.open_context_menu(
                                                    ev.position,
                                                    ContextMenuTarget::KnownFolder(
                                                        folder_ctx.clone(),
                                                    ),
                                                    cx,
                                                );
                                            }),
                                        )
                                        .child(
                                            div()
                                                .w(px(32.))
                                                .h(px(32.))
                                                .flex()
                                                .flex_shrink_0()
                                                .items_center()
                                                .justify_center()
                                                .rounded(px(RADIUS_CONTROL))
                                                .bg(soft_tint(p.accent_primary, 0.12))
                                                .child(
                                                    Icon::new(icon)
                                                        .small()
                                                        .text_color(solid(p.accent_primary)),
                                                ),
                                        )
                                        .child(
                                            v_flex()
                                                .flex_1()
                                                .min_w_0()
                                                .gap_0p5()
                                                .child(
                                                    div()
                                                        .truncate()
                                                        .text_xs()
                                                        .font_semibold()
                                                        .text_color(solid(p.text_primary))
                                                        .child(name),
                                                )
                                                .child(
                                                    div()
                                                        .truncate()
                                                        .text_size(px(13.))
                                                        .text_color(solid(p.text_muted))
                                                        .child(sub),
                                                ),
                                        )
                                },
                            )),
                    ),
            )
        })
        .child(
            v_flex()
                .gap_2p5()
                .child(home_section_header("Devices and Drives", p))
                .child(
                    div()
                        .w_full()
                        .grid()
                        .grid_cols(drv_cols)
                        .gap_3()
                        .children(ws.volumes.iter().enumerate().map(|(i, vol)| {
                            let target = Location::local(&vol.root_path);
                            let mid_target = target.clone();
                            let vol_ctx = vol.clone();
                            let frac = vol.used_fraction();
                            let bar_hex = usage_bar_color(frac, p);
                            let title: SharedString = vol.display_name().into();
                            let fs_label: SharedString = vol.file_system.clone().into();
                            let pct_label: SharedString = format!("{:.0}%", frac * 100.0).into();
                            let space_label: SharedString = format!(
                                "{} free of {}",
                                format_size(vol.free_bytes, unit),
                                format_size(vol.total_bytes, unit)
                            )
                            .into();

                            v_flex()
                                .id((
                                    match side {
                                        PaneSide::Left => "home-drv-l",
                                        PaneSide::Right => "home-drv-r",
                                    },
                                    i,
                                ))
                                .w_full()
                                .min_w_0()
                                .p_3()
                                .gap_2()
                                .rounded(px(RADIUS_ROW))
                                .cursor_pointer()
                                .bg(solid(p.bg_surface))
                                .border_1()
                                .border_color(solid(p.border_subtle))
                                .hover(move |s| {
                                    s.bg(solid(p.bg_elevated)).border_color(solid(p.border))
                                })
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                                        this.navigate_pane(side, target.clone(), window, cx);
                                    }),
                                )
                                .on_mouse_down(
                                    MouseButton::Middle,
                                    cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                                        if this.settings.show_tabs {
                                            this.open_new_tab(mid_target.clone(), window, cx);
                                        } else {
                                            this.navigate_pane(
                                                side,
                                                mid_target.clone(),
                                                window,
                                                cx,
                                            );
                                        }
                                    }),
                                )
                                .on_mouse_down(
                                    MouseButton::Right,
                                    cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                                        this.open_context_menu(
                                            ev.position,
                                            ContextMenuTarget::Volume(vol_ctx.clone()),
                                            cx,
                                        );
                                    }),
                                )
                                .child(
                                    h_flex()
                                        .items_center()
                                        .justify_between()
                                        .gap_2()
                                        .child(
                                            h_flex()
                                                .flex_1()
                                                .min_w_0()
                                                .items_center()
                                                .gap_2()
                                                .child(
                                                    Icon::new(drive_kind_icon(vol.kind))
                                                        .small()
                                                        .text_color(solid(p.accent_primary)),
                                                )
                                                .child(
                                                    div()
                                                        .flex_1()
                                                        .min_w_0()
                                                        .truncate()
                                                        .text_xs()
                                                        .font_semibold()
                                                        .text_color(solid(p.text_primary))
                                                        .child(title),
                                                ),
                                        )
                                        .when(!vol.file_system.is_empty(), |el| {
                                            el.child(
                                                div()
                                                    .px_1p5()
                                                    .py_0p5()
                                                    .rounded(px(3.))
                                                    .bg(solid(p.bg_base))
                                                    .border_1()
                                                    .border_color(solid(p.border_subtle))
                                                    .text_size(px(12.))
                                                    .text_color(solid(p.text_secondary))
                                                    .child(fs_label),
                                            )
                                        }),
                                )
                                .child(
                                    div()
                                        .w_full()
                                        .h(px(6.))
                                        .rounded(px(3.))
                                        .bg(solid(p.border_subtle))
                                        .overflow_hidden()
                                        .child(div().h_full().w(relative(frac)).bg(solid(bar_hex))),
                                )
                                .child(
                                    h_flex()
                                        .items_center()
                                        .justify_between()
                                        .child(
                                            div()
                                                .text_size(px(13.))
                                                .text_color(solid(p.text_muted))
                                                .child(space_label),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(13.))
                                                .font_medium()
                                                .text_color(solid(p.text_secondary))
                                                .child(pct_label),
                                        ),
                                )
                        })),
                ),
        )
        .child(
            v_flex()
                .gap_2p5()
                .child(home_section_header("Tools", p))
                .child(
                    div()
                        .w_full()
                        .grid()
                        .grid_cols(qa_cols.clamp(1, 3))
                        .gap_2()
                        .child({
                            let active_count = ws.active_p2p_transfers.len();
                            let peer_count = ws.p2p_nearby_peers.len();
                            let subtitle: SharedString = if active_count > 0 {
                                format!("{active_count} active encrypted transfer(s)").into()
                            } else if ws.active_p2p_share.is_some() {
                                "Sharing active • click to view code".into()
                            } else if peer_count > 0 {
                                format!("{peer_count} nearby PC(s) on LAN • Internet & LAN").into()
                            } else {
                                "Send & receive encrypted files over LAN or Internet".into()
                            };
                            h_flex()
                                .id(match side {
                                    PaneSide::Left => "home-tool-p2p-l",
                                    PaneSide::Right => "home-tool-p2p-r",
                                })
                                .w_full()
                                .min_w_0()
                                .p_2p5()
                                .gap_2p5()
                                .items_center()
                                .rounded(px(RADIUS_ROW))
                                .cursor_pointer()
                                .bg(solid(p.bg_surface))
                                .border_1()
                                .border_color(solid(p.border_subtle))
                                .hover(move |s| {
                                    s.bg(solid(p.bg_elevated)).border_color(solid(p.border))
                                })
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _: &MouseDownEvent, window, cx| {
                                        this.open_p2p_modal_from_sidebar(window, cx);
                                    }),
                                )
                                .child(
                                    div()
                                        .w(px(32.))
                                        .h(px(32.))
                                        .flex()
                                        .flex_shrink_0()
                                        .items_center()
                                        .justify_center()
                                        .rounded(px(RADIUS_CONTROL))
                                        .bg(soft_tint(p.accent_primary, 0.12))
                                        .child(
                                            Icon::new(LucideIcon::Radio)
                                                .small()
                                                .text_color(solid(p.accent_primary)),
                                        ),
                                )
                                .child(
                                    v_flex()
                                        .flex_1()
                                        .min_w_0()
                                        .gap_0p5()
                                        .child(
                                            div()
                                                .truncate()
                                                .text_xs()
                                                .font_semibold()
                                                .text_color(solid(p.text_primary))
                                                .child("P2P File Transfer"),
                                        )
                                        .child(
                                            div()
                                                .truncate()
                                                .text_size(px(13.))
                                                .text_color(solid(p.text_muted))
                                                .child(subtitle),
                                        ),
                                )
                        })
                        .child({
                            let da = &ws.disk_analyzer;
                            let subtitle: SharedString = if da.scanning {
                                "Scanning disk usage...".into()
                            } else if let Some(ref tree) = da.tree
                                && let Some(ref target) = da.target
                            {
                                let sz = tree
                                    .get(tree.root)
                                    .map(|r| r.logical_size)
                                    .unwrap_or(0);
                                format!(
                                    "{} • {}",
                                    target.label(),
                                    format_size(sz, unit)
                                )
                                .into()
                            } else {
                                "Visualize folder sizes, Treemap, largest files & extensions".into()
                            };
                            h_flex()
                                .id(match side {
                                    PaneSide::Left => "home-tool-disk-l",
                                    PaneSide::Right => "home-tool-disk-r",
                                })
                                .w_full()
                                .min_w_0()
                                .p_2p5()
                                .gap_2p5()
                                .items_center()
                                .rounded(px(RADIUS_ROW))
                                .cursor_pointer()
                                .bg(solid(p.bg_surface))
                                .border_1()
                                .border_color(solid(p.border_subtle))
                                .hover(move |s| {
                                    s.bg(solid(p.bg_elevated)).border_color(solid(p.border))
                                })
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _: &MouseDownEvent, window, cx| {
                                        this.open_disk_analyzer(None, false, window, cx);
                                    }),
                                )
                                .child(
                                    div()
                                        .w(px(32.))
                                        .h(px(32.))
                                        .flex()
                                        .flex_shrink_0()
                                        .items_center()
                                        .justify_center()
                                        .rounded(px(RADIUS_CONTROL))
                                        .bg(soft_tint(p.accent_primary, 0.12))
                                        .child(
                                            Icon::new(LucideIcon::HardDrive)
                                                .small()
                                                .text_color(solid(p.accent_primary)),
                                        ),
                                )
                                .child(
                                    v_flex()
                                        .flex_1()
                                        .min_w_0()
                                        .gap_0p5()
                                        .child(
                                            div()
                                                .truncate()
                                                .text_xs()
                                                .font_semibold()
                                                .text_color(solid(p.text_primary))
                                                .child("Disk Space Analyzer"),
                                        )
                                        .child(
                                            div()
                                                .truncate()
                                                .text_size(px(13.))
                                                .text_color(solid(p.text_muted))
                                                .child(subtitle),
                                        ),
                                )
                        }),
                ),
        )
}

/// Renders the virtualized tabular Details view (`28px` uniform rows + Drag & Drop).
pub fn render_details_view(
    ws: &Workspace,
    side: PaneSide,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let p = ws.palette();
    let Some(tab) = ws.active_tab() else {
        return div().size_full().into_any_element();
    };
    let model = tab.model.pane(side);
    let pane_ui = tab.pane_ui(side);
    let row_count = model.filtered_indices.len();
    let sort = model.sort;
    let is_loading = model.loading;
    let error = model.error.clone();
    let (col_mod_w, col_type_w, col_size_w) = ws.details_column_widths();
    let measured_w: f32 = pane_ui
        .scroll_handle
        .0
        .borrow()
        .base_handle
        .bounds()
        .size
        .width
        .into();
    let pane_w = if measured_w > 60.0 {
        measured_w
    } else {
        ws.estimated_pane_width(tab.model.split_enabled)
    };
    let layout = compute_details_column_layout(pane_w, col_mod_w, col_type_w, col_size_w);
    let active_divider = ws.column_resize.as_ref().map(|r| r.divider);
    let pane_dir = model.location.as_local_path().map(|path| path.to_path_buf());
    let list_id = match side {
        PaneSide::Left => "details-list-left",
        PaneSide::Right => "details-list-right",
    };
    let native_drop_active = ws.native_drop_hover_side == Some(side)
        && matches!(model.location, Location::Local(_) | Location::Remote { .. });
    let pane_dir_drag = pane_dir.clone();
    let pane_dir_has_local = pane_dir.is_some();
    let pane_dir_int = pane_dir.clone();
    let pane_dir_ext = pane_dir.clone();
    let pane_dir_arch = pane_dir;

    v_flex()
        .size_full()
        .bg(solid(p.bg_base))
        .overflow_hidden()
        .when(native_drop_active, |el| {
            el.border_1().border_color(solid(p.accent_primary))
        })
        .drag_over::<DraggedEntries>(move |style, drag, _, _| {
            if pane_dir_drag
                .as_ref()
                .is_some_and(|dest| is_valid_internal_drop_target(&drag.paths, dest))
            {
                style.border_1().border_color(solid(p.accent_primary))
            } else {
                style
            }
        })
        .drag_over::<DraggedArchiveEntries>(move |style, _, _, _| {
            if pane_dir_has_local {
                style.border_1().border_color(solid(p.accent_primary))
            } else {
                style
            }
        })
        .drag_over::<ExternalPaths>(move |style, _, _, _| {
            if pane_dir_has_local {
                style.border_1().border_color(solid(p.accent_primary))
            } else {
                style
            }
        })
        .on_drop(cx.listener(move |this, drag: &DraggedEntries, window, cx| {
            if let Some(ref dest) = pane_dir_int {
                this.handle_dropped_paths(drag.paths.clone(), dest.clone(), false, window, cx);
            } else {
                this.handle_dropped_paths_into_pane(side, drag.paths.clone(), window, cx);
            }
        }))
        .on_drop(cx.listener(move |this, drag: &DraggedArchiveEntries, window, cx| {
            if let Some(ref dest) = pane_dir_arch {
                this.extract_archive_entries_into(
                    drag.archive_path.clone(),
                    drag.inner_paths.clone(),
                    dest.clone(),
                    window,
                    cx,
                );
            }
        }))
        .on_drop(cx.listener(move |this, ext: &ExternalPaths, window, cx| {
            if let Some(ref dest) = pane_dir_ext {
                this.handle_dropped_paths(ext.paths().to_vec(), dest.clone(), true, window, cx);
            } else {
                this.handle_dropped_paths_into_pane(side, ext.paths().to_vec(), window, cx);
            }
        }))
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                if !window.default_prevented() {
                    this.open_pane_bg_context_menu(side, ev, cx);
                }
            }),
        )
        .child(
            div()
                .w_full()
                .h(px(34.))
                .pl(px(6.))
                .pr(px(layout.gutter_w))
                .flex_shrink_0()
                .bg(solid(p.bg_elevated))
                .border_b_1()
                .border_color(solid(p.border_subtle))
                .child(
                    h_flex()
                        .w_full()
                        .h_full()
                        .px_2p5()
                        .items_center()
                        .overflow_hidden()
                        .child(sort_header_cell(
                            side,
                            "Name",
                            SortColumn::Name,
                            sort,
                            None,
                            false,
                            p,
                            cx,
                        ))
                        .when(layout.show_modified, |el| {
                            el.child(resizable_header_column(
                                side,
                                ColumnDivider::NameModified,
                                active_divider == Some(ColumnDivider::NameModified),
                                layout.mod_w,
                                sort_header_cell(
                                    side,
                                    "Date Modified",
                                    SortColumn::Modified,
                                    sort,
                                    None,
                                    false,
                                    p,
                                    cx,
                                ),
                                p,
                                cx,
                            ))
                        })
                        .when(layout.show_type, |el| {
                            el.child(resizable_header_column(
                                side,
                                ColumnDivider::ModifiedType,
                                active_divider == Some(ColumnDivider::ModifiedType),
                                layout.type_w,
                                sort_header_cell(
                                    side,
                                    "Type",
                                    SortColumn::Type,
                                    sort,
                                    None,
                                    false,
                                    p,
                                    cx,
                                ),
                                p,
                                cx,
                            ))
                        })
                        .when(layout.show_size, |el| {
                            el.child(resizable_header_column(
                                side,
                                ColumnDivider::TypeSize,
                                active_divider == Some(ColumnDivider::TypeSize),
                                layout.size_w,
                                sort_header_cell(
                                    side,
                                    "Size",
                                    SortColumn::Size,
                                    sort,
                                    None,
                                    true,
                                    p,
                                    cx,
                                ),
                                p,
                                cx,
                            ))
                        }),
                ),
        )
        .child(if row_count == 0 {
            render_empty_state(
                is_loading,
                error,
                !model.quick_filter.trim().is_empty(),
                p,
            )
            .into_any_element()
        } else {
            div()
                .relative()
                .flex_1()
                .min_h_0()
                .w_full()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                        this.begin_marquee_selection(side, ev, window, cx);
                    }),
                )
                .child(
                    uniform_list(
                        list_id,
                        row_count,
                        cx.processor(move |this, range: Range<usize>, _window, cx| {
                            render_details_rows(this, side, range, cx)
                        }),
                    )
                    .track_scroll(&pane_ui.scroll_handle)
                    .size_full(),
                )
                .when_some(ws.active_marquee_rect(side), |el, (l, t, w, h)| {
                    el.child(render_marquee_overlay(l, t, w, h, p))
                })
                .into_any_element()
        })
        .into_any_element()
}

#[allow(clippy::too_many_arguments)]
fn render_row_entry_icon(
    sys_icon: Option<std::sync::Arc<gpui_kit::RenderImage>>,
    fallback_icon: LucideIcon,
    icon_hex: u32,
    px_size: f32,
    small: bool,
    is_hidden: bool,
    is_cut: bool,
    p: Palette,
) -> gpui_kit::AnyElement {
    if let Some(render_img) = sys_icon {
        div()
            .w(px(px_size))
            .h(px(px_size))
            .flex()
            .flex_shrink_0()
            .items_center()
            .justify_center()
            .opacity(if is_cut {
                0.45
            } else if is_hidden {
                0.65
            } else {
                1.0
            })
            .child(
                img(render_img)
                    .w(px(px_size))
                    .h(px(px_size))
                    .object_fit(ObjectFit::Contain),
            )
            .into_any_element()
    } else {
        let icon = Icon::new(fallback_icon);
        let sized = if small { icon.small() } else { icon.xsmall() };
        sized
            .text_color(if is_hidden || is_cut {
                solid(p.text_muted)
            } else {
                solid(icon_hex)
            })
            .into_any_element()
    }
}

fn render_details_rows(
    ws: &mut Workspace,
    side: PaneSide,
    range: Range<usize>,
    cx: &mut Context<Workspace>,
) -> Vec<gpui_kit::AnyElement> {
    let p = ws.palette();
    let unit = ws.settings.size_unit;
    let (col_mod_w, col_type_w, col_size_w) = ws.details_column_widths();

    let Some(tab) = ws.active_tab() else {
        return Vec::new();
    };
    let measured_w: f32 = tab
        .pane_ui(side)
        .scroll_handle
        .0
        .borrow()
        .base_handle
        .bounds()
        .size
        .width
        .into();
    let pane_w = if measured_w > 60.0 {
        measured_w
    } else {
        ws.estimated_pane_width(tab.model.split_enabled)
    };
    let layout = compute_details_column_layout(pane_w, col_mod_w, col_type_w, col_size_w);

    let mut visible_items = Vec::with_capacity(range.len());
    {
        let model = tab.model.pane(side);
        for vis_idx in range {
            if let Some(entry) = model.visible_entry(vis_idx) {
                visible_items.push((
                    vis_idx,
                    entry.clone(),
                    model.selection.contains(vis_idx),
                    model.selection.cursor == Some(vis_idx),
                ));
            }
        }
    }

    let mut sys_icons = Vec::with_capacity(visible_items.len());
    for (_, entry, _, _) in &visible_items {
        sys_icons.push(ws.get_or_request_entry_icon(entry, 32));
    }

    visible_items
        .into_iter()
        .zip(sys_icons)
        .map(|((vis_idx, entry, selected, is_cursor), sys_icon)| {
            let (icon, icon_hex) = file_kind_visual(entry.kind, p);
            let is_hidden = entry.is_hidden();
            let local_path = entry.location.as_local_path().map(|path| path.to_path_buf());
            let is_cut = local_path.as_ref().is_some_and(|path| ws.is_path_cut(path));
            let is_dir = entry.is_dir();
            let name: SharedString = entry.name.clone().into();
            let modified: SharedString = format_timestamp(entry.modified_secs).into();
            let type_label: SharedString = entry.type_label.clone().into();
            let size_label: SharedString = match entry.size {
                Some(bytes) => format_size(bytes, unit).into(),
                None => "—".into(),
            };
            let is_symlink = entry.flags.contains(EntryFlags::IS_SYMLINK);
            let row_bg = if selected {
                soft_tint(p.accent_primary, if p.is_dark { 0.16 } else { 0.12 })
            } else if vis_idx % 2 == 1 {
                solid(p.bg_surface)
            } else {
                solid(p.bg_base)
            };

            let drag_payload = build_drag_payload(ws, side, local_path.as_ref(), &entry.name);
            let archive_drag_payload = build_archive_drag_payload(ws, side, &entry);
            let drop_dir_int = if is_dir { local_path.clone() } else { None };
            let drop_dir_ext = drop_dir_int.clone();
            let drop_dir_arch = drop_dir_int.clone();

            let row = h_flex()
                .id((
                    match side {
                        PaneSide::Left => "det-row-l",
                        PaneSide::Right => "det-row-r",
                    },
                    vis_idx,
                ))
                .w_full()
                .h_full()
                .px_2p5()
                .items_center()
                .rounded(px(RADIUS_CONTROL))
                .cursor_pointer()
                .bg(row_bg)
                .border_b_1()
                .border_color(if is_cursor && selected {
                    solid(p.accent_primary)
                } else {
                    gpui_kit::transparent_black()
                })
                .hover(move |s| {
                    if selected {
                        s
                    } else {
                        s.bg(solid(p.bg_hover))
                    }
                })
                .when_some(drag_payload, |el, payload| {
                    el.on_drag(payload, move |drag, cursor_offset, _, cx| {
                        cx.new(|_| DragBadgeView {
                            label: drag.label.clone(),
                            palette: p,
                            cursor_offset,
                        })
                    })
                })
                .when_some(archive_drag_payload, |el, payload| {
                    el.on_drag(payload, move |drag, cursor_offset, _, cx| {
                        cx.new(|_| DragBadgeView {
                            label: drag.label.clone(),
                            palette: p,
                            cursor_offset,
                        })
                    })
                })
                .when_some(drop_dir_int, |el, dest| {
                    let dest_hover = dest.clone();
                    el.drag_over::<DraggedEntries>(move |style, drag, _, _| {
                        if is_valid_internal_drop_target(&drag.paths, &dest_hover) {
                            style.bg(soft_tint(p.accent_primary, 0.22))
                        } else {
                            style
                        }
                    })
                    .on_drop(cx.listener(
                        move |this, drag: &DraggedEntries, window, cx| {
                            cx.stop_propagation();
                            this.handle_dropped_paths(
                                drag.paths.clone(),
                                dest.clone(),
                                false,
                                window,
                                cx,
                            );
                        },
                    ))
                })
                .when_some(drop_dir_arch, |el, dest| {
                    el.drag_over::<DraggedArchiveEntries>(move |style, _, _, _| {
                        style.bg(soft_tint(p.accent_primary, 0.22))
                    })
                    .on_drop(cx.listener(
                        move |this, drag: &DraggedArchiveEntries, window, cx| {
                            cx.stop_propagation();
                            this.extract_archive_entries_into(
                                drag.archive_path.clone(),
                                drag.inner_paths.clone(),
                                dest.clone(),
                                window,
                                cx,
                            );
                        },
                    ))
                })
                .when_some(drop_dir_ext, |el, dest| {
                    el.drag_over::<ExternalPaths>(move |style, _, _, _| {
                        style.bg(soft_tint(p.accent_primary, 0.22))
                    })
                    .on_drop(cx.listener(
                        move |this, ext: &ExternalPaths, window, cx| {
                            cx.stop_propagation();
                            this.handle_dropped_paths(
                                ext.paths().to_vec(),
                                dest.clone(),
                                true,
                                window,
                                cx,
                            );
                        },
                    ))
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                        this.handle_entry_left_click(side, vis_idx, ev, window, cx);
                    }),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                        window.prevent_default();
                        this.handle_entry_right_click(side, vis_idx, ev, cx);
                    }),
                )
                .child(
                    h_flex()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .pr_2()
                        .gap_2()
                        .items_center()
                        .child(render_row_entry_icon(
                            sys_icon,
                            icon,
                            icon_hex,
                            16.0,
                            true,
                            is_hidden,
                            is_cut,
                            p,
                        ))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(if is_cut {
                                    solid(p.text_muted)
                                } else if is_hidden {
                                    solid(p.text_secondary)
                                } else {
                                    solid(p.text_primary)
                                })
                                .child(name),
                        )
                        .when(is_symlink, |el| {
                            el.child(
                                Icon::new(LucideIcon::Link)
                                    .xsmall()
                                    .text_color(solid(p.text_muted)),
                            )
                        }),
                )
                .when(layout.show_modified, |el| {
                    el.child(
                        div()
                            .w(px(layout.mod_w))
                            .flex_shrink_0()
                            .overflow_hidden()
                            .pl_2()
                            .truncate()
                            .text_xs()
                            .text_color(solid(p.text_secondary))
                            .child(modified),
                    )
                })
                .when(layout.show_type, |el| {
                    el.child(
                        div()
                            .w(px(layout.type_w))
                            .flex_shrink_0()
                            .overflow_hidden()
                            .pl_2()
                            .truncate()
                            .text_xs()
                            .text_color(solid(p.text_muted))
                            .child(type_label),
                    )
                })
                .when(layout.show_size, |el| {
                    el.child(
                        div()
                            .w(px(layout.size_w))
                            .flex_shrink_0()
                            .overflow_hidden()
                            .pl_2()
                            .text_right()
                            .truncate()
                            .text_xs()
                            .text_color(solid(p.text_secondary))
                            .child(size_label),
                    )
                });

            div()
                .w_full()
                .h(px(32.))
                .pl(px(6.))
                .pr(px(layout.gutter_w))
                .bg(solid(p.bg_base))
                .child(row)
                .into_any_element()
        })
        .collect()
}

/// Renders a virtualized Grid of flat tiles powered by native Windows Shell thumbnails.
pub fn render_grid_view(
    ws: &Workspace,
    side: PaneSide,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let p = ws.palette();
    let Some(tab) = ws.active_tab() else {
        return div().size_full().into_any_element();
    };
    let model = tab.model.pane(side);
    let pane_ui = tab.pane_ui(side);
    let total = model.filtered_indices.len();
    let is_loading = model.loading;
    let error = model.error.clone();
    let pane_dir = model.location.as_local_path().map(|path| path.to_path_buf());
    let cols = ws.grid_view_columns(tab.model.split_enabled);
    let row_count = total.div_ceil(cols);
    let list_id = match side {
        PaneSide::Left => "grid-list-left",
        PaneSide::Right => "grid-list-right",
    };
    let native_drop_active = ws.native_drop_hover_side == Some(side)
        && matches!(model.location, Location::Local(_) | Location::Remote { .. });
    let pane_dir_drag = pane_dir.clone();
    let pane_dir_has_local = pane_dir.is_some();
    let pane_dir_int = pane_dir.clone();
    let pane_dir_ext = pane_dir.clone();
    let pane_dir_arch = pane_dir;

    v_flex()
        .size_full()
        .bg(solid(p.bg_base))
        .overflow_hidden()
        .when(native_drop_active, |el| {
            el.border_1().border_color(solid(p.accent_primary))
        })
        .drag_over::<DraggedEntries>(move |style, drag, _, _| {
            if pane_dir_drag
                .as_ref()
                .is_some_and(|dest| is_valid_internal_drop_target(&drag.paths, dest))
            {
                style.border_1().border_color(solid(p.accent_primary))
            } else {
                style
            }
        })
        .drag_over::<DraggedArchiveEntries>(move |style, _, _, _| {
            if pane_dir_has_local {
                style.border_1().border_color(solid(p.accent_primary))
            } else {
                style
            }
        })
        .drag_over::<ExternalPaths>(move |style, _, _, _| {
            if pane_dir_has_local {
                style.border_1().border_color(solid(p.accent_primary))
            } else {
                style
            }
        })
        .on_drop(cx.listener(move |this, drag: &DraggedEntries, window, cx| {
            if let Some(ref dest) = pane_dir_int {
                this.handle_dropped_paths(drag.paths.clone(), dest.clone(), false, window, cx);
            } else {
                this.handle_dropped_paths_into_pane(side, drag.paths.clone(), window, cx);
            }
        }))
        .on_drop(cx.listener(move |this, drag: &DraggedArchiveEntries, window, cx| {
            if let Some(ref dest) = pane_dir_arch {
                this.extract_archive_entries_into(
                    drag.archive_path.clone(),
                    drag.inner_paths.clone(),
                    dest.clone(),
                    window,
                    cx,
                );
            }
        }))
        .on_drop(cx.listener(move |this, ext: &ExternalPaths, window, cx| {
            if let Some(ref dest) = pane_dir_ext {
                this.handle_dropped_paths(ext.paths().to_vec(), dest.clone(), true, window, cx);
            } else {
                this.handle_dropped_paths_into_pane(side, ext.paths().to_vec(), window, cx);
            }
        }))
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                if !window.default_prevented() {
                    this.open_pane_bg_context_menu(side, ev, cx);
                }
            }),
        )
        .child(if total == 0 {
            render_empty_state(
                is_loading,
                error,
                !model.quick_filter.trim().is_empty(),
                p,
            )
            .into_any_element()
        } else {
            div()
                .relative()
                .flex_1()
                .min_h_0()
                .w_full()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                        this.begin_marquee_selection(side, ev, window, cx);
                    }),
                )
                .child(
                    uniform_list(
                        list_id,
                        row_count,
                        cx.processor(move |this, range: Range<usize>, _window, cx| {
                            render_grid_rows(this, side, cols, range, cx)
                        }),
                    )
                    .track_scroll(&pane_ui.scroll_handle)
                    .size_full(),
                )
                .when_some(ws.active_marquee_rect(side), |el, (l, t, w, h)| {
                    el.child(render_marquee_overlay(l, t, w, h, p))
                })
                .into_any_element()
        })
        .into_any_element()
}

fn render_grid_rows(
    ws: &mut Workspace,
    side: PaneSide,
    cols: usize,
    range: Range<usize>,
    cx: &mut Context<Workspace>,
) -> Vec<gpui_kit::AnyElement> {
    let p = ws.palette();
    let unit = ws.settings.size_unit;

    // Collect visible entries for this range first so we can query/enqueue thumbnails cleanly.
    let total = ws
        .active_tab()
        .map(|t| t.model.pane(side).filtered_indices.len())
        .unwrap_or(0);
    if total == 0 {
        return Vec::new();
    }

    let first_idx = range.start * cols;
    let last_idx = (range.end * cols).min(total);
    let mut visible_items = Vec::with_capacity(last_idx.saturating_sub(first_idx));
    if let Some(tab) = ws.active_tab() {
        let model = tab.model.pane(side);
        for vis_idx in first_idx..last_idx {
            if let Some(entry) = model.visible_entry(vis_idx) {
                visible_items.push((
                    vis_idx,
                    entry.clone(),
                    model.selection.contains(vis_idx),
                ));
            }
        }
    }

    // Resolve or enqueue native Windows Shell thumbnails/icons strictly for the visible viewport range.
    let icon_style = ws.settings.icon_style;
    let mut thumbs: Vec<Option<std::sync::Arc<gpui_kit::RenderImage>>> =
        Vec::with_capacity(visible_items.len());
    for (_, entry, _) in &visible_items {
        let thumb = if entry.is_dir() {
            if icon_style.uses_system_folder_icons() {
                ws.get_or_request_entry_icon(entry, 48)
            } else {
                None
            }
        } else if let Some(local) = entry.location.as_local_path() {
            ws.get_or_request_thumbnail(
                local,
                entry.modified_secs,
                96,
                icon_style.uses_system_file_icons(),
            )
        } else if icon_style.uses_system_file_icons() {
            ws.get_or_request_entry_icon(entry, 48)
        } else {
            None
        };
        thumbs.push(thumb);
    }

    range
        .map(|row_idx| {
            let start = row_idx * cols;
            let end = (start + cols).min(total);

            h_flex()
                .w_full()
                .h(px(136.))
                .px_3()
                .py_1p5()
                .gap_2()
                .items_center()
                .children((start..end).filter_map(|vis_idx| {
                    let offset = vis_idx.checked_sub(first_idx)?;
                    let (_, entry, selected) = visible_items.get(offset)?;
                    let thumb = thumbs.get(offset).cloned().flatten();
                    let (icon, icon_hex) = file_kind_visual(entry.kind, p);
                    let local_path = entry.location.as_local_path().map(|path| path.to_path_buf());
                    let is_cut = local_path.as_ref().is_some_and(|path| ws.is_path_cut(path));
                    let is_dir = entry.is_dir();
                    let name: SharedString = entry.name.clone().into();
                    let sub: SharedString = if is_dir {
                        "Folder".into()
                    } else if let Some(bytes) = entry.size {
                        format_size(bytes, unit).into()
                    } else {
                        entry.type_label.clone().into()
                    };

                    let drag_payload =
                        build_drag_payload(ws, side, local_path.as_ref(), &entry.name);
                    let archive_drag_payload = build_archive_drag_payload(ws, side, entry);
                    let drop_dir_int = if is_dir { local_path.clone() } else { None };
                    let drop_dir_ext = drop_dir_int.clone();
                    let drop_dir_arch = drop_dir_int.clone();

                    let visual = if let Some(render_img) = thumb {
                        div()
                            .w(px(64.))
                            .h(px(52.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .overflow_hidden()
                            .rounded(px(RADIUS_CONTROL))
                            .child(
                                img(render_img)
                                    .max_w_full()
                                    .max_h_full()
                                    .object_fit(ObjectFit::Contain),
                            )
                            .into_any_element()
                    } else {
                        div()
                            .w(px(40.))
                            .h(px(40.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(RADIUS_CONTROL))
                            .bg(soft_tint(icon_hex, 0.14))
                            .child(
                                Icon::new(icon)
                                    .with_size(Size::Medium)
                                    .text_color(if is_cut {
                                        solid(p.text_muted)
                                    } else {
                                        solid(icon_hex)
                                    }),
                            )
                            .into_any_element()
                    };

                    let selected_val = *selected;
                    Some(
                        v_flex()
                            .id((
                                match side {
                                    PaneSide::Left => "grid-tile-l",
                                    PaneSide::Right => "grid-tile-r",
                                },
                                vis_idx,
                            ))
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .p_2()
                            .gap_1p5()
                            .items_center()
                            .justify_center()
                            .rounded(px(RADIUS_ROW))
                            .cursor_pointer()
                            .bg(if selected_val {
                                soft_tint(p.accent_primary, if p.is_dark { 0.16 } else { 0.12 })
                            } else {
                                solid(p.bg_surface)
                            })
                            .border_1()
                            .border_color(if selected_val {
                                solid(p.accent_primary)
                            } else {
                                solid(p.border_subtle)
                            })
                            .hover(move |s| {
                                if selected_val {
                                    s
                                } else {
                                    s.bg(solid(p.bg_elevated)).border_color(solid(p.border))
                                }
                            })
                            .when_some(drag_payload, |el, payload| {
                                el.on_drag(payload, move |drag, cursor_offset, _, cx| {
                                    cx.new(|_| DragBadgeView {
                                        label: drag.label.clone(),
                                        palette: p,
                                        cursor_offset,
                                    })
                                })
                            })
                            .when_some(archive_drag_payload, |el, payload| {
                                el.on_drag(payload, move |drag, cursor_offset, _, cx| {
                                    cx.new(|_| DragBadgeView {
                                        label: drag.label.clone(),
                                        palette: p,
                                        cursor_offset,
                                    })
                                })
                            })
                            .when_some(drop_dir_int, |el, dest| {
                                let dest_hover = dest.clone();
                                el.drag_over::<DraggedEntries>(move |style, drag, _, _| {
                                    if is_valid_internal_drop_target(&drag.paths, &dest_hover) {
                                        style.border_color(solid(p.accent_primary))
                                    } else {
                                        style
                                    }
                                })
                                .on_drop(cx.listener(
                                    move |this, drag: &DraggedEntries, window, cx| {
                                        cx.stop_propagation();
                                        this.handle_dropped_paths(
                                            drag.paths.clone(),
                                            dest.clone(),
                                            false,
                                            window,
                                            cx,
                                        );
                                    },
                                ))
                            })
                            .when_some(drop_dir_arch, |el, dest| {
                                el.drag_over::<DraggedArchiveEntries>(move |style, _, _, _| {
                                    style.border_color(solid(p.accent_primary))
                                })
                                .on_drop(cx.listener(
                                    move |this, drag: &DraggedArchiveEntries, window, cx| {
                                        cx.stop_propagation();
                                        this.extract_archive_entries_into(
                                            drag.archive_path.clone(),
                                            drag.inner_paths.clone(),
                                            dest.clone(),
                                            window,
                                            cx,
                                        );
                                    },
                                ))
                            })
                            .when_some(drop_dir_ext, |el, dest| {
                                el.drag_over::<ExternalPaths>(move |style, _, _, _| {
                                    style.border_color(solid(p.accent_primary))
                                })
                                .on_drop(cx.listener(
                                    move |this, ext: &ExternalPaths, window, cx| {
                                        cx.stop_propagation();
                                        this.handle_dropped_paths(
                                            ext.paths().to_vec(),
                                            dest.clone(),
                                            true,
                                            window,
                                            cx,
                                        );
                                    },
                                ))
                            })
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                                    this.handle_entry_left_click(side, vis_idx, ev, window, cx);
                                }),
                            )
                            .on_mouse_down(
                                MouseButton::Right,
                                cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                                    window.prevent_default();
                                    this.handle_entry_right_click(side, vis_idx, ev, cx);
                                }),
                            )
                            .child(visual)
                            .child(
                                div()
                                    .w_full()
                                    .truncate()
                                    .text_center()
                                    .text_xs()
                                    .font_medium()
                                    .text_color(if is_cut {
                                        solid(p.text_muted)
                                    } else {
                                        solid(p.text_primary)
                                    })
                                    .child(name),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(solid(p.text_muted))
                                    .child(sub),
                            ),
                    )
                }))
                .children((end..start + cols).map(|_| div().flex_1().min_w_0()))
                .into_any_element()
        })
        .collect()
}

/// Renders a high-density Compact virtualized list (`24px` rows + Drag & Drop).
pub fn render_compact_view(
    ws: &Workspace,
    side: PaneSide,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let p = ws.palette();
    let Some(tab) = ws.active_tab() else {
        return div().size_full().into_any_element();
    };
    let model = tab.model.pane(side);
    let pane_ui = tab.pane_ui(side);
    let row_count = model.filtered_indices.len();
    let is_loading = model.loading;
    let error = model.error.clone();
    let pane_dir = model.location.as_local_path().map(|path| path.to_path_buf());
    let list_id = match side {
        PaneSide::Left => "compact-list-left",
        PaneSide::Right => "compact-list-right",
    };
    let native_drop_active = ws.native_drop_hover_side == Some(side)
        && matches!(model.location, Location::Local(_) | Location::Remote { .. });
    let pane_dir_drag = pane_dir.clone();
    let pane_dir_has_local = pane_dir.is_some();
    let pane_dir_int = pane_dir.clone();
    let pane_dir_ext = pane_dir.clone();
    let pane_dir_arch = pane_dir;

    v_flex()
        .size_full()
        .bg(solid(p.bg_base))
        .overflow_hidden()
        .when(native_drop_active, |el| {
            el.border_1().border_color(solid(p.accent_primary))
        })
        .drag_over::<DraggedEntries>(move |style, drag, _, _| {
            if pane_dir_drag
                .as_ref()
                .is_some_and(|dest| is_valid_internal_drop_target(&drag.paths, dest))
            {
                style.border_1().border_color(solid(p.accent_primary))
            } else {
                style
            }
        })
        .drag_over::<DraggedArchiveEntries>(move |style, _, _, _| {
            if pane_dir_has_local {
                style.border_1().border_color(solid(p.accent_primary))
            } else {
                style
            }
        })
        .drag_over::<ExternalPaths>(move |style, _, _, _| {
            if pane_dir_has_local {
                style.border_1().border_color(solid(p.accent_primary))
            } else {
                style
            }
        })
        .on_drop(cx.listener(move |this, drag: &DraggedEntries, window, cx| {
            if let Some(ref dest) = pane_dir_int {
                this.handle_dropped_paths(drag.paths.clone(), dest.clone(), false, window, cx);
            } else {
                this.handle_dropped_paths_into_pane(side, drag.paths.clone(), window, cx);
            }
        }))
        .on_drop(cx.listener(move |this, drag: &DraggedArchiveEntries, window, cx| {
            if let Some(ref dest) = pane_dir_arch {
                this.extract_archive_entries_into(
                    drag.archive_path.clone(),
                    drag.inner_paths.clone(),
                    dest.clone(),
                    window,
                    cx,
                );
            }
        }))
        .on_drop(cx.listener(move |this, ext: &ExternalPaths, window, cx| {
            if let Some(ref dest) = pane_dir_ext {
                this.handle_dropped_paths(ext.paths().to_vec(), dest.clone(), true, window, cx);
            } else {
                this.handle_dropped_paths_into_pane(side, ext.paths().to_vec(), window, cx);
            }
        }))
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                if !window.default_prevented() {
                    this.open_pane_bg_context_menu(side, ev, cx);
                }
            }),
        )
        .child(if row_count == 0 {
            render_empty_state(
                is_loading,
                error,
                !model.quick_filter.trim().is_empty(),
                p,
            )
            .into_any_element()
        } else {
            div()
                .relative()
                .flex_1()
                .min_h_0()
                .w_full()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                        this.begin_marquee_selection(side, ev, window, cx);
                    }),
                )
                .child(
                    uniform_list(
                        list_id,
                        row_count,
                        cx.processor(move |this, range: Range<usize>, _window, cx| {
                            render_compact_rows(this, side, range, cx)
                        }),
                    )
                    .track_scroll(&pane_ui.scroll_handle)
                    .size_full(),
                )
                .when_some(ws.active_marquee_rect(side), |el, (l, t, w, h)| {
                    el.child(render_marquee_overlay(l, t, w, h, p))
                })
                .into_any_element()
        })
        .into_any_element()
}

fn render_marquee_overlay(
    left: f32,
    top: f32,
    width: f32,
    height: f32,
    p: Palette,
) -> impl IntoElement {
    div()
        .absolute()
        .left(px(left))
        .top(px(top))
        .w(px(width))
        .h(px(height))
        .bg(soft_tint(p.accent_primary, 0.16))
        .border_1()
        .border_color(solid(p.accent_primary))
        .rounded(px(2.))
}

fn render_compact_rows(
    ws: &mut Workspace,
    side: PaneSide,
    range: Range<usize>,
    cx: &mut Context<Workspace>,
) -> Vec<gpui_kit::AnyElement> {
    let p = ws.palette();
    let unit = ws.settings.size_unit;

    let mut visible_items = Vec::with_capacity(range.len());
    if let Some(tab) = ws.active_tab() {
        let model = tab.model.pane(side);
        for vis_idx in range {
            if let Some(entry) = model.visible_entry(vis_idx) {
                visible_items.push((
                    vis_idx,
                    entry.clone(),
                    model.selection.contains(vis_idx),
                ));
            }
        }
    }

    let mut sys_icons = Vec::with_capacity(visible_items.len());
    for (_, entry, _) in &visible_items {
        sys_icons.push(ws.get_or_request_entry_icon(entry, 32));
    }

    visible_items
        .into_iter()
        .zip(sys_icons)
        .map(|((vis_idx, entry, selected), sys_icon)| {
            let (icon, icon_hex) = file_kind_visual(entry.kind, p);
            let is_hidden = entry.is_hidden();
            let local_path = entry.location.as_local_path().map(|path| path.to_path_buf());
            let is_cut = local_path.as_ref().is_some_and(|path| ws.is_path_cut(path));
            let name: SharedString = entry.name.clone().into();
            let size: SharedString = entry
                .size
                .map(|b| format_size(b, unit))
                .unwrap_or_default()
                .into();
            let drag_payload = build_drag_payload(ws, side, local_path.as_ref(), &entry.name);
            let archive_drag_payload = build_archive_drag_payload(ws, side, &entry);

            let row = h_flex()
                .id((
                    match side {
                        PaneSide::Left => "cmp-row-l",
                        PaneSide::Right => "cmp-row-r",
                    },
                    vis_idx,
                ))
                .w_full()
                .h_full()
                .px_2p5()
                .gap_2()
                .items_center()
                .rounded(px(RADIUS_CONTROL))
                .cursor_pointer()
                .bg(if selected {
                    soft_tint(p.accent_primary, if p.is_dark { 0.16 } else { 0.12 })
                } else {
                    solid(p.bg_base)
                })
                .hover(move |s| {
                    if selected {
                        s
                    } else {
                        s.bg(solid(p.bg_hover))
                    }
                })
                .when_some(drag_payload, |el, payload| {
                    el.on_drag(payload, move |drag, cursor_offset, _, cx| {
                        cx.new(|_| DragBadgeView {
                            label: drag.label.clone(),
                            palette: p,
                            cursor_offset,
                        })
                    })
                })
                .when_some(archive_drag_payload, |el, payload| {
                    el.on_drag(payload, move |drag, cursor_offset, _, cx| {
                        cx.new(|_| DragBadgeView {
                            label: drag.label.clone(),
                            palette: p,
                            cursor_offset,
                        })
                    })
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                        this.handle_entry_left_click(side, vis_idx, ev, window, cx);
                    }),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                        window.prevent_default();
                        this.handle_entry_right_click(side, vis_idx, ev, cx);
                    }),
                )
                .child(render_row_entry_icon(
                    sys_icon,
                    icon,
                    icon_hex,
                    15.0,
                    false,
                    is_hidden,
                    is_cut,
                    p,
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_xs()
                        .text_color(if is_cut {
                            solid(p.text_muted)
                        } else {
                            solid(p.text_primary)
                        })
                        .child(name),
                )
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(solid(p.text_muted))
                        .child(size),
                );

            div()
                .w_full()
                .h(px(28.))
                .pl(px(6.))
                .pr(px(LIST_MARQUEE_GUTTER_PX))
                .bg(solid(p.bg_base))
                .child(row)
                .into_any_element()
        })
        .collect()
}

fn build_drag_payload(
    ws: &Workspace,
    side: PaneSide,
    entry_path: Option<&PathBuf>,
    entry_name: &str,
) -> Option<DraggedEntries> {
    let path = entry_path?;
    let selected = ws.selected_local_paths(side);
    let paths = if selected.iter().any(|p| p == path) {
        selected
    } else {
        vec![path.clone()]
    };
    let label: SharedString = if paths.len() == 1 {
        entry_name.to_string().into()
    } else {
        format!("{} items", paths.len()).into()
    };
    Some(DraggedEntries { paths, label })
}

fn build_archive_drag_payload(
    ws: &Workspace,
    side: PaneSide,
    entry: &texplorer_core::FileEntry,
) -> Option<DraggedArchiveEntries> {
    let Location::Archive {
        archive_path,
        inner_path,
    } = &entry.location
    else {
        return None;
    };
    let selected = ws.selected_archive_inner_paths(side, archive_path);
    let inner_paths = if selected.iter().any(|p| p == inner_path) {
        selected
    } else {
        vec![inner_path.clone()]
    };
    let label: SharedString = if inner_paths.len() == 1 {
        format!("Extract {}", entry.name).into()
    } else {
        format!("Extract {} items", inner_paths.len()).into()
    };
    Some(DraggedArchiveEntries {
        archive_path: archive_path.clone(),
        inner_paths,
        label,
    })
}

/// Wraps a fixed-width Details header cell with a draggable left-edge column resize handle
/// (double-click any divider to reset default widths).
#[allow(clippy::too_many_arguments)]
fn resizable_header_column(
    side: PaneSide,
    divider: ColumnDivider,
    active: bool,
    width_px: f32,
    header_cell: impl IntoElement,
    p: Palette,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let div_id = match (side, divider) {
        (PaneSide::Left, ColumnDivider::NameModified) => "col-div-l-nm",
        (PaneSide::Left, ColumnDivider::ModifiedType) => "col-div-l-mt",
        (PaneSide::Left, ColumnDivider::TypeSize) => "col-div-l-ts",
        (PaneSide::Right, ColumnDivider::NameModified) => "col-div-r-nm",
        (PaneSide::Right, ColumnDivider::ModifiedType) => "col-div-r-mt",
        (PaneSide::Right, ColumnDivider::TypeSize) => "col-div-r-ts",
    };

    h_flex()
        .relative()
        .w(px(width_px))
        .h_full()
        .flex_shrink_0()
        .pl_2()
        .items_center()
        .child(header_cell)
        .child(
            div()
                .id(div_id)
                .absolute()
                .left(px(-4.))
                .top_0()
                .bottom_0()
                .w(px(9.))
                .flex()
                .items_center()
                .justify_center()
                .cursor_col_resize()
                .hover(move |s| s.bg(soft_tint(p.accent_primary, 0.2)))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                        cx.stop_propagation();
                        window.prevent_default();
                        this.begin_column_resize(divider, ev, cx);
                    }),
                )
                .child(
                    div()
                        .w(if active { px(2.) } else { px(1.) })
                        .h(px(16.))
                        .rounded(px(1.))
                        .bg(if active {
                            solid(p.accent_primary)
                        } else {
                            solid(p.border_subtle)
                        }),
                ),
        )
}

#[allow(clippy::too_many_arguments)]
fn sort_header_cell(
    side: PaneSide,
    label: &'static str,
    column: SortColumn,
    sort: SortSpec,
    width_px: Option<f32>,
    align_right: bool,
    p: Palette,
    cx: &mut Context<Workspace>,
) -> impl IntoElement + use<> {
    let is_active = sort.column == column;
    let descending = sort.descending;
    let id = match (side, column) {
        (PaneSide::Left, SortColumn::Name) => "hdr-l-name",
        (PaneSide::Left, SortColumn::Modified) => "hdr-l-mod",
        (PaneSide::Left, SortColumn::Type) => "hdr-l-type",
        (PaneSide::Left, SortColumn::Size) => "hdr-l-size",
        (PaneSide::Right, SortColumn::Name) => "hdr-r-name",
        (PaneSide::Right, SortColumn::Modified) => "hdr-r-mod",
        (PaneSide::Right, SortColumn::Type) => "hdr-r-type",
        (PaneSide::Right, SortColumn::Size) => "hdr-r-size",
    };

    h_flex()
        .id(id)
        .h_full()
        .gap_1()
        .items_center()
        .overflow_hidden()
        .cursor_pointer()
        .when(align_right, |el| el.justify_end())
        .map(|el| match width_px {
            Some(w) => el.w(px(w)).flex_shrink_0(),
            None => el.flex_1().min_w_0(),
        })
        .text_xs()
        .font_medium()
        .text_color(if is_active {
            solid(p.text_primary)
        } else {
            solid(p.text_secondary)
        })
        .hover(move |s| s.text_color(solid(p.text_primary)))
        .on_click(cx.listener(move |this, _, _, cx| {
            this.toggle_sort_column(side, column, cx);
        }))
        .child(div().min_w_0().truncate().child(label))
        .when(is_active, |el| {
            el.child(
                Icon::new(if descending {
                    LucideIcon::ChevronDown
                } else {
                    LucideIcon::ChevronUp
                })
                .xsmall(),
            )
        })
}

fn home_section_header(title: &'static str, p: Palette) -> impl IntoElement {
    div()
        .text_xs()
        .font_semibold()
        .text_color(solid(p.text_secondary))
        .child(title)
}

fn render_empty_state(
    loading: bool,
    error: Option<String>,
    filtered: bool,
    p: Palette,
) -> impl IntoElement {
    let (headline, helper): (SharedString, SharedString) = if loading {
        ("Loading folder".into(), "Reading directory entries...".into())
    } else if let Some(err) = error {
        ("Unable to open folder".into(), err.into())
    } else if filtered {
        (
            "No matching items".into(),
            "Clear the filter box to see all items in this folder.".into(),
        )
    } else {
        (
            "This folder is empty".into(),
            "Create a new folder with Ctrl+Shift+N or drop items here.".into(),
        )
    };

    v_flex()
        .flex_1()
        .size_full()
        .items_center()
        .justify_center()
        .gap_1()
        .px_6()
        .child(
            div()
                .text_sm()
                .font_semibold()
                .text_color(solid(p.text_primary))
                .child(headline),
        )
        .child(
            div()
                .text_xs()
                .text_color(solid(p.text_muted))
                .child(helper),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn details_layout_preserves_name_space_and_hides_secondary_columns_when_narrow() {
        // Wide single pane shows all 4 columns at full preferred widths.
        let wide = compute_details_column_layout(1000.0, 164.0, 144.0, 106.0);
        assert!(wide.show_modified && wide.show_type && wide.show_size);
        assert_eq!(wide.mod_w, 164.0);
        assert_eq!(wide.type_w, 144.0);
        assert_eq!(wide.size_w, 106.0);

        // Medium split pane compresses secondary columns while preserving >= 160px for Name.
        let mid = compute_details_column_layout(500.0, 164.0, 144.0, 106.0);
        assert!(mid.show_modified && mid.show_type && mid.show_size);
        let name_space_mid = 500.0 - DETAILS_ROW_CHROME_PX - mid.gutter_w - mid.mod_w - mid.type_w - mid.size_w;
        assert!(name_space_mid >= DETAILS_MIN_NAME_PX);

        // Narrow split pane + Inspector/Sidebar hides Type first so Name stays legible.
        let narrow = compute_details_column_layout(420.0, 164.0, 144.0, 106.0);
        assert!(narrow.show_modified && !narrow.show_type && narrow.show_size);
        let name_space_narrow =
            420.0 - DETAILS_ROW_CHROME_PX - narrow.gutter_w - narrow.mod_w - narrow.size_w;
        assert!(name_space_narrow >= DETAILS_MIN_NAME_PX);

        // Very narrow pane hides Date Modified too, keeping Name + Size.
        let very_narrow = compute_details_column_layout(310.0, 164.0, 144.0, 106.0);
        assert!(!very_narrow.show_modified && !very_narrow.show_type && very_narrow.show_size);

        // Ultra-narrow pane renders Name only.
        let tiny = compute_details_column_layout(220.0, 164.0, 144.0, 106.0);
        assert!(!tiny.show_modified && !tiny.show_type && !tiny.show_size);
    }
}
