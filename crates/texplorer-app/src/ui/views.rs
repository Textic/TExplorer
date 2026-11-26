use std::ops::Range;

use gpui_kit::{
    Context, MouseButton, MouseDownEvent, SharedString,
    assets::IconName as LucideIcon,
    component::{Icon, Sizable as _, Size, StyledExt as _, h_flex, scroll::ScrollableElement as _, v_flex},
    div, prelude::*, px, relative, uniform_list,
};
use texplorer_core::{EntryFlags, Location, PaneSide, SortColumn, SortSpec};

use crate::theme::{
    BG_BASE, BG_ELEVATED, BG_SURFACE, BORDER, BORDER_SUBTLE, RADIUS_CONTROL, RADIUS_ROW,
    TEXT_MUTED, TEXT_PRIMARY, TEXT_SECONDARY, drive_kind_icon, file_kind_visual, format_size,
    format_timestamp, known_folder_icon, soft_tint, solid, usage_bar_color,
};
use crate::ui::context_menu::ContextMenuTarget;
use crate::ui::workspace::Workspace;

/// Renders the "Home / This PC" overview dashboard with Quick Access cards,
/// Pinned folders, and storage drive cards with real usage bars.
pub fn render_home_view(
    ws: &Workspace,
    side: PaneSide,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let accent = ws.settings.accent;
    let unit = ws.settings.size_unit;
    let visible_quick_access: Vec<_> = ws
        .known_folders
        .iter()
        .filter(|f| ws.settings.is_quick_access_visible(f.kind))
        .cloned()
        .collect();

    v_flex()
        .id(match side {
            PaneSide::Left => "home-view-left",
            PaneSide::Right => "home-view-right",
        })
        .size_full()
        .bg(solid(BG_BASE))
        .overflow_y_scrollbar()
        .p_5()
        .gap_6()
        // Quick Access grid (respects user-hidden folders; right-click to remove)
        .when(!visible_quick_access.is_empty(), |el| {
            el.child(
                v_flex()
                    .gap_2p5()
                    .child(home_section_header("Quick Access"))
                    .child(
                        h_flex()
                            .flex_wrap()
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
                                        .w(px(210.))
                                        .p_2p5()
                                        .gap_2p5()
                                        .items_center()
                                        .rounded(px(RADIUS_ROW))
                                        .cursor_pointer()
                                        .bg(solid(BG_SURFACE))
                                        .border_1()
                                        .border_color(solid(BORDER_SUBTLE))
                                        .hover(|s| {
                                            s.bg(solid(BG_ELEVATED)).border_color(solid(BORDER))
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
                                                    this.open_new_tab(
                                                        mid_target.clone(),
                                                        window,
                                                        cx,
                                                    );
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
                                                .bg(soft_tint(accent.primary_hex(), 0.12))
                                                .child(
                                                    Icon::new(icon)
                                                        .small()
                                                        .text_color(solid(accent.primary_hex())),
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
                                                        .text_color(solid(TEXT_PRIMARY))
                                                        .child(name),
                                                )
                                                .child(
                                                    div()
                                                        .truncate()
                                                        .text_size(px(11.))
                                                        .text_color(solid(TEXT_MUTED))
                                                        .child(sub),
                                                ),
                                        )
                                },
                            )),
                    ),
            )
        })
        // Devices & Drives grid
        .child(
            v_flex()
                .gap_2p5()
                .child(home_section_header("Devices and Drives"))
                .child(
                    h_flex()
                        .flex_wrap()
                        .gap_3()
                        .children(ws.volumes.iter().enumerate().map(|(i, vol)| {
                            let target = Location::local(&vol.root_path);
                            let mid_target = target.clone();
                            let vol_ctx = vol.clone();
                            let frac = vol.used_fraction();
                            let bar_hex = usage_bar_color(frac, accent);
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
                                .w(px(268.))
                                .p_3()
                                .gap_2()
                                .rounded(px(RADIUS_ROW))
                                .cursor_pointer()
                                .bg(solid(BG_SURFACE))
                                .border_1()
                                .border_color(solid(BORDER_SUBTLE))
                                .hover(|s| s.bg(solid(BG_ELEVATED)).border_color(solid(BORDER)))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                                        this.navigate_pane(side, target.clone(), window, cx);
                                    }),
                                )
                                .on_mouse_down(
                                    MouseButton::Middle,
                                    cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                                        this.open_new_tab(mid_target.clone(), window, cx);
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
                                                        .text_color(solid(accent.primary_hex())),
                                                )
                                                .child(
                                                    div()
                                                        .flex_1()
                                                        .min_w_0()
                                                        .truncate()
                                                        .text_xs()
                                                        .font_semibold()
                                                        .text_color(solid(TEXT_PRIMARY))
                                                        .child(title),
                                                ),
                                        )
                                        .when(!vol.file_system.is_empty(), |el| {
                                            el.child(
                                                div()
                                                    .px_1p5()
                                                    .py_0p5()
                                                    .rounded(px(3.))
                                                    .bg(solid(BG_BASE))
                                                    .border_1()
                                                    .border_color(solid(BORDER_SUBTLE))
                                                    .text_size(px(10.))
                                                    .text_color(solid(TEXT_SECONDARY))
                                                    .child(fs_label),
                                            )
                                        }),
                                )
                                .child(
                                    div()
                                        .w_full()
                                        .h(px(6.))
                                        .rounded(px(3.))
                                        .bg(solid(BG_BASE))
                                        .overflow_hidden()
                                        .child(
                                            div()
                                                .h_full()
                                                .w(relative(frac))
                                                .bg(solid(bar_hex)),
                                        ),
                                )
                                .child(
                                    h_flex()
                                        .items_center()
                                        .justify_between()
                                        .child(
                                            div()
                                                .text_size(px(11.))
                                                .text_color(solid(TEXT_MUTED))
                                                .child(space_label),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(11.))
                                                .font_medium()
                                                .text_color(solid(TEXT_SECONDARY))
                                                .child(pct_label),
                                        ),
                                )
                        })),
                ),
        )
}

/// Renders the virtualized tabular Details view (`28px` uniform rows + sortable headers).
pub fn render_details_view(
    ws: &Workspace,
    side: PaneSide,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let Some(tab) = ws.active_tab() else {
        return div().size_full().into_any_element();
    };
    let model = tab.model.pane(side);
    let pane_ui = tab.pane_ui(side);
    let row_count = model.filtered_indices.len();
    let sort = model.sort;
    let is_loading = model.loading;
    let error = model.error.clone();
    let list_id = match side {
        PaneSide::Left => "details-list-left",
        PaneSide::Right => "details-list-right",
    };

    v_flex()
        .size_full()
        .bg(solid(BG_BASE))
        .overflow_hidden()
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                if !window.default_prevented() {
                    this.open_pane_bg_context_menu(side, ev.position, cx);
                }
            }),
        )
        // Elevated 30px sortable column header bar
        .child(
            h_flex()
                .w_full()
                .h(px(30.))
                .px_2p5()
                .flex_shrink_0()
                .items_center()
                .bg(solid(BG_ELEVATED))
                .border_b_1()
                .border_color(solid(BORDER_SUBTLE))
                .child(sort_header_cell(
                    side,
                    "Name",
                    SortColumn::Name,
                    sort,
                    None,
                    false,
                    cx,
                ))
                .child(sort_header_cell(
                    side,
                    "Date Modified",
                    SortColumn::Modified,
                    sort,
                    Some(142.),
                    false,
                    cx,
                ))
                .child(sort_header_cell(
                    side,
                    "Type",
                    SortColumn::Type,
                    sort,
                    Some(126.),
                    false,
                    cx,
                ))
                .child(sort_header_cell(
                    side,
                    "Size",
                    SortColumn::Size,
                    sort,
                    Some(92.),
                    true,
                    cx,
                )),
        )
        .child(if row_count == 0 {
            render_empty_state(is_loading, error, !model.quick_filter.trim().is_empty())
                .into_any_element()
        } else {
            uniform_list(
                list_id,
                row_count,
                cx.processor(move |this, range: Range<usize>, _window, cx| {
                    render_details_rows(this, side, range, cx)
                }),
            )
            .track_scroll(&pane_ui.scroll_handle)
            .flex_1()
            .w_full()
            .into_any_element()
        })
        .into_any_element()
}

fn render_details_rows(
    ws: &mut Workspace,
    side: PaneSide,
    range: Range<usize>,
    cx: &mut Context<Workspace>,
) -> Vec<gpui_kit::AnyElement> {
    let Some(tab) = ws.active_tab() else {
        return Vec::new();
    };
    let model = tab.model.pane(side);
    let accent = ws.settings.accent;
    let unit = ws.settings.size_unit;

    range
        .filter_map(|vis_idx| {
            let entry = model.visible_entry(vis_idx)?;
            let selected = model.selection.contains(vis_idx);
            let is_cursor = model.selection.cursor == Some(vis_idx);
            let (icon, icon_hex) = file_kind_visual(entry.kind, accent);
            let is_hidden = entry.is_hidden();
            let name: SharedString = entry.name.clone().into();
            let modified: SharedString = format_timestamp(entry.modified_secs).into();
            let type_label: SharedString = entry.type_label.clone().into();
            let size_label: SharedString = match entry.size {
                Some(bytes) => format_size(bytes, unit).into(),
                None => "—".into(),
            };
            let is_symlink = entry.flags.contains(EntryFlags::IS_SYMLINK);
            let row_bg = if selected {
                soft_tint(accent.primary_hex(), 0.16)
            } else if vis_idx % 2 == 1 {
                solid(BG_SURFACE)
            } else {
                solid(BG_BASE)
            };

            Some(
                h_flex()
                    .id((
                        match side {
                            PaneSide::Left => "det-row-l",
                            PaneSide::Right => "det-row-r",
                        },
                        vis_idx,
                    ))
                    .w_full()
                    .h(px(28.))
                    .px_2p5()
                    .items_center()
                    .cursor_pointer()
                    .bg(row_bg)
                    .border_b_1()
                    .border_color(if is_cursor && selected {
                        solid(accent.primary_hex())
                    } else {
                        gpui_kit::transparent_black()
                    })
                    .hover(move |s| {
                        if selected {
                            s
                        } else {
                            s.bg(solid(BG_ELEVATED))
                        }
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
                            this.handle_entry_right_click(side, vis_idx, ev.position, cx);
                        }),
                    )
                    // Name column
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .pr_2()
                            .gap_2()
                            .items_center()
                            .child(
                                Icon::new(icon).small().text_color(if is_hidden {
                                    solid(TEXT_MUTED)
                                } else {
                                    solid(icon_hex)
                                }),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_xs()
                                    .text_color(if is_hidden {
                                        solid(TEXT_SECONDARY)
                                    } else {
                                        solid(TEXT_PRIMARY)
                                    })
                                    .child(name),
                            )
                            .when(is_symlink, |el| {
                                el.child(
                                    Icon::new(LucideIcon::Link)
                                        .xsmall()
                                        .text_color(solid(TEXT_MUTED)),
                                )
                            }),
                    )
                    // Modified column
                    .child(
                        div()
                            .w(px(142.))
                            .flex_shrink_0()
                            .truncate()
                            .text_xs()
                            .text_color(solid(TEXT_SECONDARY))
                            .child(modified),
                    )
                    // Type column
                    .child(
                        div()
                            .w(px(126.))
                            .flex_shrink_0()
                            .truncate()
                            .text_xs()
                            .text_color(solid(TEXT_MUTED))
                            .child(type_label),
                    )
                    // Size column
                    .child(
                        div()
                            .w(px(92.))
                            .flex_shrink_0()
                            .text_right()
                            .truncate()
                            .text_xs()
                            .text_color(solid(TEXT_SECONDARY))
                            .child(size_label),
                    )
                    .into_any_element(),
            )
        })
        .collect()
}

/// Renders a virtualized Grid of flat Obsidian Dark file tiles (6 tiles per row,
/// virtualized row-by-row via `uniform_list` for smooth 120 FPS scrolling).
pub fn render_grid_view(
    ws: &Workspace,
    side: PaneSide,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let Some(tab) = ws.active_tab() else {
        return div().size_full().into_any_element();
    };
    let model = tab.model.pane(side);
    let pane_ui = tab.pane_ui(side);
    let total = model.filtered_indices.len();
    let is_loading = model.loading;
    let error = model.error.clone();
    let cols = if tab.model.split_enabled { 4usize } else { 6usize };
    let row_count = total.div_ceil(cols);
    let list_id = match side {
        PaneSide::Left => "grid-list-left",
        PaneSide::Right => "grid-list-right",
    };

    v_flex()
        .size_full()
        .bg(solid(BG_BASE))
        .overflow_hidden()
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                if !window.default_prevented() {
                    this.open_pane_bg_context_menu(side, ev.position, cx);
                }
            }),
        )
        .child(if total == 0 {
            render_empty_state(is_loading, error, !model.quick_filter.trim().is_empty())
                .into_any_element()
        } else {
            uniform_list(
                list_id,
                row_count,
                cx.processor(move |this, range: Range<usize>, _window, cx| {
                    render_grid_rows(this, side, cols, range, cx)
                }),
            )
            .track_scroll(&pane_ui.scroll_handle)
            .flex_1()
            .w_full()
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
    let Some(tab) = ws.active_tab() else {
        return Vec::new();
    };
    let model = tab.model.pane(side);
    let accent = ws.settings.accent;
    let unit = ws.settings.size_unit;
    let total = model.filtered_indices.len();

    range
        .map(|row_idx| {
            let start = row_idx * cols;
            let end = (start + cols).min(total);

            h_flex()
                .w_full()
                .h(px(114.))
                .px_3()
                .py_1p5()
                .gap_2()
                .items_center()
                .children((start..end).filter_map(|vis_idx| {
                    let entry = model.visible_entry(vis_idx)?;
                    let selected = model.selection.contains(vis_idx);
                    let (icon, icon_hex) = file_kind_visual(entry.kind, accent);
                    let name: SharedString = entry.name.clone().into();
                    let sub: SharedString = if entry.is_dir() {
                        "Folder".into()
                    } else if let Some(bytes) = entry.size {
                        format_size(bytes, unit).into()
                    } else {
                        entry.type_label.clone().into()
                    };

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
                            .bg(if selected {
                                soft_tint(accent.primary_hex(), 0.16)
                            } else {
                                solid(BG_SURFACE)
                            })
                            .border_1()
                            .border_color(if selected {
                                solid(accent.primary_hex())
                            } else {
                                solid(BORDER_SUBTLE)
                            })
                            .hover(move |s| {
                                if selected {
                                    s
                                } else {
                                    s.bg(solid(BG_ELEVATED)).border_color(solid(BORDER))
                                }
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
                                    this.handle_entry_right_click(side, vis_idx, ev.position, cx);
                                }),
                            )
                            .child(
                                div()
                                    .w(px(36.))
                                    .h(px(36.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(RADIUS_CONTROL))
                                    .bg(soft_tint(icon_hex, 0.14))
                                    .child(
                                        Icon::new(icon)
                                            .with_size(Size::Medium)
                                            .text_color(solid(icon_hex)),
                                    ),
                            )
                            .child(
                                div()
                                    .w_full()
                                    .truncate()
                                    .text_center()
                                    .text_xs()
                                    .font_medium()
                                    .text_color(solid(TEXT_PRIMARY))
                                    .child(name),
                            )
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(solid(TEXT_MUTED))
                                    .child(sub),
                            ),
                    )
                }))
                // Pad incomplete trailing row so tiles stay uniform width
                .children((end..start + cols).map(|_| div().flex_1().min_w_0()))
                .into_any_element()
        })
        .collect()
}

/// Renders a high-density Compact virtualized list (`24px` rows) for rapid keyboard browsing.
pub fn render_compact_view(
    ws: &Workspace,
    side: PaneSide,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let Some(tab) = ws.active_tab() else {
        return div().size_full().into_any_element();
    };
    let model = tab.model.pane(side);
    let pane_ui = tab.pane_ui(side);
    let row_count = model.filtered_indices.len();
    let is_loading = model.loading;
    let error = model.error.clone();
    let list_id = match side {
        PaneSide::Left => "compact-list-left",
        PaneSide::Right => "compact-list-right",
    };

    v_flex()
        .size_full()
        .bg(solid(BG_BASE))
        .overflow_hidden()
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                if !window.default_prevented() {
                    this.open_pane_bg_context_menu(side, ev.position, cx);
                }
            }),
        )
        .child(if row_count == 0 {
            render_empty_state(is_loading, error, !model.quick_filter.trim().is_empty())
                .into_any_element()
        } else {
            uniform_list(
                list_id,
                row_count,
                cx.processor(move |this, range: Range<usize>, _window, cx| {
                    render_compact_rows(this, side, range, cx)
                }),
            )
            .track_scroll(&pane_ui.scroll_handle)
            .flex_1()
            .w_full()
            .into_any_element()
        })
        .into_any_element()
}

fn render_compact_rows(
    ws: &mut Workspace,
    side: PaneSide,
    range: Range<usize>,
    cx: &mut Context<Workspace>,
) -> Vec<gpui_kit::AnyElement> {
    let Some(tab) = ws.active_tab() else {
        return Vec::new();
    };
    let model = tab.model.pane(side);
    let accent = ws.settings.accent;
    let unit = ws.settings.size_unit;

    range
        .filter_map(|vis_idx| {
            let entry = model.visible_entry(vis_idx)?;
            let selected = model.selection.contains(vis_idx);
            let (icon, icon_hex) = file_kind_visual(entry.kind, accent);
            let name: SharedString = entry.name.clone().into();
            let size: SharedString = entry
                .size
                .map(|b| format_size(b, unit))
                .unwrap_or_default()
                .into();

            Some(
                h_flex()
                    .id((
                        match side {
                            PaneSide::Left => "cmp-row-l",
                            PaneSide::Right => "cmp-row-r",
                        },
                        vis_idx,
                    ))
                    .w_full()
                    .h(px(24.))
                    .px_2p5()
                    .gap_2()
                    .items_center()
                    .cursor_pointer()
                    .bg(if selected {
                        soft_tint(accent.primary_hex(), 0.16)
                    } else {
                        solid(BG_BASE)
                    })
                    .hover(move |s| {
                        if selected {
                            s
                        } else {
                            s.bg(solid(BG_ELEVATED))
                        }
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
                            this.handle_entry_right_click(side, vis_idx, ev.position, cx);
                        }),
                    )
                    .child(Icon::new(icon).xsmall().text_color(solid(icon_hex)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_xs()
                            .text_color(solid(TEXT_PRIMARY))
                            .child(name),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(solid(TEXT_MUTED))
                            .child(size),
                    )
                    .into_any_element(),
            )
        })
        .collect()
}

fn sort_header_cell(
    side: PaneSide,
    label: &'static str,
    column: SortColumn,
    sort: SortSpec,
    width_px: Option<f32>,
    align_right: bool,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
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
        .cursor_pointer()
        .when(align_right, |el| el.justify_end())
        .map(|el| match width_px {
            Some(w) => el.w(px(w)).flex_shrink_0(),
            None => el.flex_1().min_w_0(),
        })
        .text_xs()
        .font_medium()
        .text_color(if is_active {
            solid(TEXT_PRIMARY)
        } else {
            solid(TEXT_SECONDARY)
        })
        .hover(|s| s.text_color(solid(TEXT_PRIMARY)))
        .on_click(cx.listener(move |this, _, _, cx| {
            this.toggle_sort_column(side, column, cx);
        }))
        .child(label)
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

fn home_section_header(title: &'static str) -> impl IntoElement {
    div()
        .text_xs()
        .font_semibold()
        .text_color(solid(TEXT_SECONDARY))
        .child(title)
}

fn render_empty_state(loading: bool, error: Option<String>, filtered: bool) -> impl IntoElement {
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
                .text_color(solid(TEXT_PRIMARY))
                .child(headline),
        )
        .child(
            div()
                .text_xs()
                .text_color(solid(TEXT_MUTED))
                .child(helper),
        )
}
